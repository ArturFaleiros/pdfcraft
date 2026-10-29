//! Word (WordprocessingML, ECMA-376 Part 1 §17) to PDF. The body's paragraphs and tables flow
//! onto pages sized by each section; styles, direct formatting, list numbering, tab stops,
//! images, headers and footers (with page-number fields) and footnotes are carried over.
//! Floating objects are placed in the flow where they are anchored, and text boxes' text joins
//! the flow after their paragraph.

use std::collections::HashMap;
use std::sync::Arc;

use pdfcraft_cos::{Document, ObjRef};
use roxmltree::Node;

use super::canvas::{Canvas, n};
use super::flow::{
    self, Align, Field, Floating, Inline, Laid, LineRule, Para, ParaBorders, Pic, Place, Rule, ShapePart, TabKind, TabStop, Wrap, fin,
};
use super::xml::{self, Rels, attr, attr_f64, child, children, flag, is, rel_attr};
use super::zip::Zip;
use super::{LinkTarget, Notes, RunStyle, family_of, hex_color};

/// Deepest nesting of content controls, tables and text boxes followed.
const MAX_DEPTH: usize = 24;

/// Longest numbering-level text kept, in characters: every paragraph the level numbers copies
/// it, so a hostile level would otherwise be multiplied by the paragraph count.
const MAX_LVL_TEXT: usize = 64;

fn twips(v: f64) -> f64 {
    v / 20.0
}

// ── styles ──────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Default, Debug)]
struct RPr {
    font: Option<String>,
    /// `Some(true)` for the theme's major (heading) font, `Some(false)` for the minor one.
    theme_major: Option<bool>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    color: Option<Option<[f64; 3]>>,
    size: Option<f64>,
    highlight: Option<Option<[f64; 3]>>,
    vert: Option<i8>,
    caps: Option<bool>,
    hidden: Option<bool>,
    style: Option<String>,
}

impl RPr {
    fn merge(&mut self, o: &RPr) {
        macro_rules! take {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f.clone(); } )* };
        }
        take!(font, theme_major, bold, italic, underline, strike, color, size, highlight, vert, caps, hidden, style);
        if o.font.is_some() {
            self.theme_major = o.theme_major;
        }
    }

    fn parse(n: Node<'_, '_>) -> RPr {
        let mut r = RPr::default();
        for c in n.children().filter(Node::is_element) {
            let val = attr(c, "val");
            match c.tag_name().name() {
                "rStyle" => r.style = val.map(str::to_owned),
                "rFonts" => {
                    if let Some(f) = attr(c, "ascii").or_else(|| attr(c, "hAnsi")) {
                        r.font = Some(f.to_owned());
                        r.theme_major = None;
                    } else if let Some(t) = attr(c, "asciiTheme").or_else(|| attr(c, "hAnsiTheme")) {
                        r.font = Some(String::new());
                        r.theme_major = Some(t.starts_with("major"));
                    }
                }
                "b" => r.bold = Some(flag(val)),
                "i" => r.italic = Some(flag(val)),
                "u" => r.underline = Some(val.is_some_and(|v| v != "none")),
                "strike" | "dstrike" => r.strike = Some(flag(val)),
                "color" => r.color = val.map(|v| if v == "auto" { None } else { hex_color(v) }),
                "sz" => r.size = attr_f64(c, "val").map(|v| v / 2.0).filter(|v| *v > 0.0),
                "highlight" => r.highlight = val.map(highlight_color),
                "shd" => {
                    if let Some(f) = attr(c, "fill").filter(|f| *f != "auto").and_then(hex_color) {
                        r.highlight = Some(Some(f));
                    }
                }
                "vertAlign" => {
                    r.vert = Some(match val {
                        Some("superscript") => 1,
                        Some("subscript") => -1,
                        _ => 0,
                    })
                }
                "caps" | "smallCaps" => r.caps = Some(flag(val)),
                "vanish" | "specVanish" => r.hidden = Some(flag(val)),
                _ => {}
            }
        }
        r
    }

    fn style(&self, theme: &Theme) -> RunStyle {
        let font = match (&self.font, self.theme_major) {
            (_, Some(true)) => theme.major.as_str(),
            (_, Some(false)) => theme.minor.as_str(),
            (Some(f), None) => f.as_str(),
            (None, None) => theme.minor.as_str(),
        };
        RunStyle {
            fam: family_of(font),
            bold: self.bold.unwrap_or(false),
            italic: self.italic.unwrap_or(false),
            size: fin(self.size.unwrap_or(10.0), 10.0).clamp(1.0, 400.0),
            color: self.color.flatten().unwrap_or([0.0; 3]),
            underline: self.underline.unwrap_or(false),
            strike: self.strike.unwrap_or(false),
            highlight: self.highlight.flatten(),
            vert: self.vert.unwrap_or(0),
            caps: self.caps.unwrap_or(false),
            hidden: self.hidden.unwrap_or(false),
            link: None,
        }
    }
}

fn highlight_color(v: &str) -> Option<[f64; 3]> {
    Some(match v {
        "yellow" => [1.0, 1.0, 0.0],
        "green" => [0.0, 1.0, 0.0],
        "cyan" => [0.0, 1.0, 1.0],
        "magenta" => [1.0, 0.0, 1.0],
        "blue" => [0.0, 0.0, 1.0],
        "red" => [1.0, 0.0, 0.0],
        "darkBlue" => [0.0, 0.0, 0.5],
        "darkCyan" => [0.0, 0.5, 0.5],
        "darkGreen" => [0.0, 0.5, 0.0],
        "darkMagenta" => [0.5, 0.0, 0.5],
        "darkRed" => [0.5, 0.0, 0.0],
        "darkYellow" => [0.5, 0.5, 0.0],
        "darkGray" => [0.5, 0.5, 0.5],
        "lightGray" => [0.75, 0.75, 0.75],
        "black" => [0.0, 0.0, 0.0],
        "white" => [1.0, 1.0, 1.0],
        _ => return None,
    })
}

#[derive(Clone, Default, Debug)]
struct PPr {
    style: Option<String>,
    align: Option<Align>,
    left: Option<f64>,
    right: Option<f64>,
    first: Option<f64>,
    before: Option<f64>,
    after: Option<f64>,
    line: Option<LineRule>,
    num: Option<(String, usize)>,
    /// Tab stops set (`Some`) or cleared (`None`) at a position, in document order.
    tabs: Vec<(f64, Option<TabStop>)>,
    page_break_before: Option<bool>,
    shade: Option<Option<[f64; 3]>>,
    keep_next: Option<bool>,
    contextual: Option<bool>,
    /// A right-to-left paragraph (`w:bidi`): aligned to the right unless it says otherwise.
    bidi: Option<bool>,
    /// Paragraph borders by side (top, left, bottom, right, between); `Some(None)` = none.
    borders: [Option<Option<Rule>>; 5],
    /// The outline level (`w:outlineLvl`, 0–8; 9 is body text).
    outline: Option<u8>,
    widow: Option<bool>,
    keep_lines: Option<bool>,
    no_line_numbers: Option<bool>,
    /// A drop cap frame: in the margin, lines spanned, distance from the text.
    drop_cap: Option<(bool, u8, f64)>,
}

impl PPr {
    fn merge(&mut self, o: &PPr) {
        macro_rules! take {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f.clone(); } )* };
        }
        take!(
            style,
            align,
            left,
            right,
            first,
            before,
            after,
            line,
            num,
            page_break_before,
            shade,
            keep_next,
            contextual,
            bidi,
            outline,
            widow,
            keep_lines,
            no_line_numbers,
            drop_cap
        );
        for (mine, theirs) in self.borders.iter_mut().zip(o.borders) {
            if theirs.is_some() {
                *mine = theirs;
            }
        }
        for (pos, stop) in &o.tabs {
            self.tabs.retain(|(p, _)| (p - pos).abs() > 0.5);
            if stop.is_some() {
                self.tabs.push((*pos, *stop));
            }
        }
    }

    fn parse(n: Node<'_, '_>) -> PPr {
        let mut p = PPr::default();
        for c in n.children().filter(Node::is_element) {
            let val = attr(c, "val");
            match c.tag_name().name() {
                "pStyle" => p.style = val.map(str::to_owned),
                "outlineLvl" => p.outline = val.and_then(|v| v.parse::<u8>().ok()).map(|v| v.min(9)),
                "jc" => {
                    p.align = Some(match val {
                        Some("center") => Align::Center,
                        Some("right" | "end") => Align::Right,
                        Some("both" | "distribute" | "lowKashida" | "mediumKashida" | "highKashida" | "thaiDistribute") => Align::Justify,
                        _ => Align::Left,
                    })
                }
                "ind" => {
                    if let Some(v) = attr_f64(c, "left").or_else(|| attr_f64(c, "start")) {
                        p.left = Some(twips(v));
                    }
                    if let Some(v) = attr_f64(c, "right").or_else(|| attr_f64(c, "end")) {
                        p.right = Some(twips(v));
                    }
                    if let Some(v) = attr_f64(c, "hanging") {
                        p.first = Some(-twips(v));
                    } else if let Some(v) = attr_f64(c, "firstLine") {
                        p.first = Some(twips(v));
                    }
                }
                "spacing" => {
                    if let Some(v) = attr_f64(c, "before") {
                        p.before = Some(twips(v));
                    }
                    if let Some(v) = attr_f64(c, "after") {
                        p.after = Some(twips(v));
                    }
                    if flag(attr(c, "beforeAutospacing")) && attr(c, "beforeAutospacing").is_some() {
                        p.before = Some(14.0);
                    }
                    if flag(attr(c, "afterAutospacing")) && attr(c, "afterAutospacing").is_some() {
                        p.after = Some(14.0);
                    }
                    if let Some(v) = attr_f64(c, "line") {
                        p.line = Some(match attr(c, "lineRule") {
                            Some("exact") => LineRule::Exact(twips(v.abs())),
                            Some("atLeast") => LineRule::AtLeast(twips(v)),
                            _ => LineRule::Auto(v / 240.0),
                        });
                    }
                }
                "numPr" => {
                    let id = child(c, "numId").and_then(|n| attr(n, "val")).unwrap_or("0").to_owned();
                    let lvl = child(c, "ilvl").and_then(|n| attr_f64(n, "val")).unwrap_or(0.0).clamp(0.0, 8.0) as usize;
                    p.num = Some((id, lvl));
                }
                "tabs" => {
                    for t in children(c, "tab") {
                        if let Some(pos) = attr_f64(t, "pos").map(twips).filter(|p| p.is_finite()) {
                            p.tabs.push((pos, tab_stop(t, pos)));
                        }
                    }
                }
                "pageBreakBefore" => p.page_break_before = Some(flag(val)),
                "keepNext" => p.keep_next = Some(flag(val)),
                "keepLines" => p.keep_lines = Some(flag(val)),
                "widowControl" => p.widow = Some(flag(val)),
                "suppressLineNumbers" => p.no_line_numbers = Some(flag(val)),
                "framePr" => {
                    p.drop_cap = match attr(c, "dropCap") {
                        Some(kind @ ("drop" | "margin")) => Some((
                            kind == "margin",
                            attr_f64(c, "lines").map_or(3, |v| fin(v, 3.0).clamp(1.0, 10.0) as u8),
                            attr_f64(c, "hSpace").map_or(0.0, |v| fin(twips(v), 0.0).clamp(0.0, 72.0)),
                        )),
                        _ => None,
                    }
                }
                "contextualSpacing" => p.contextual = Some(flag(val)),
                "bidi" => p.bidi = Some(flag(val)),
                "shd" => p.shade = Some(attr(c, "fill").filter(|f| *f != "auto").and_then(hex_color)),
                "pBdr" => {
                    for (slot, name) in p.borders.iter_mut().zip(["top", "left", "bottom", "right", "between"]) {
                        let side = child(c, name).or_else(|| match name {
                            "left" => child(c, "start"),
                            "right" => child(c, "end"),
                            _ => None,
                        });
                        if let Some(side) = side {
                            *slot = Some(rule(side));
                        }
                    }
                }
                _ => {}
            }
        }
        p
    }

    fn para_borders(&self) -> Option<ParaBorders> {
        let [top, left, bottom, right, between] = self.borders.map(Option::flatten);
        let b = ParaBorders { top, bottom, left, right, between };
        (b != ParaBorders::default()).then_some(b)
    }
}

/// A paragraph border side (`w:top`, `w:left`… in `w:pBdr`); `None` when it says none.
fn rule(n: Node<'_, '_>) -> Option<Rule> {
    let val = attr(n, "val").unwrap_or("single");
    if matches!(val, "nil" | "none") {
        return None;
    }
    // Sizes are eighths of a point, spaces whole points.
    let width = attr_f64(n, "sz").map_or(0.5, |s| fin(s / 8.0, 0.5).clamp(0.25, 6.0));
    let space = attr_f64(n, "space").map_or(0.0, |s| fin(s, 0.0).clamp(0.0, 31.0));
    let color = attr(n, "color").filter(|c| *c != "auto").and_then(hex_color).unwrap_or([0.0; 3]);
    let double = val.starts_with("double") || val.starts_with("thinThick") || val.starts_with("thickThin") || val == "triple";
    // A double border's size is each line's: the pair and the gap between take three.
    Some(Rule { width: if double { width * 3.0 } else { width }, color, space, double, dash: flow::LineStyle::of(val) })
}

/// A `w:tab` stop at `pos` points; `None` when it clears an inherited stop. Bar tabs draw a
/// line rather than stopping text, so they are not stops.
fn tab_stop(t: Node<'_, '_>, pos: f64) -> Option<TabStop> {
    let kind = match attr(t, "val") {
        Some("clear" | "bar") => return None,
        Some("center") => TabKind::Center,
        Some("right" | "end") => TabKind::Right,
        Some("decimal") => TabKind::Decimal,
        _ => TabKind::Left,
    };
    let leader = match attr(t, "leader") {
        Some("dot") => Some('.'),
        Some("hyphen") => Some('-'),
        Some("underscore" | "heavy") => Some('_'),
        Some("middleDot") => Some('\u{b7}'),
        _ => None,
    };
    Some(TabStop { pos, kind, leader })
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Border {
    width: f64,
    color: [f64; 3],
    dash: flow::LineStyle,
}

/// Table or cell borders; `None` = not specified here, `Some(None)` = explicitly none.
#[derive(Clone, Copy, Default, Debug)]
struct Borders {
    top: Option<Option<Border>>,
    bottom: Option<Option<Border>>,
    left: Option<Option<Border>>,
    right: Option<Option<Border>>,
    inside_h: Option<Option<Border>>,
    inside_v: Option<Option<Border>>,
}

impl Borders {
    fn parse(n: Node<'_, '_>) -> Borders {
        let side = |names: &[&str]| -> Option<Option<Border>> {
            let c = names.iter().find_map(|name| child(n, name))?;
            let val = attr(c, "val").unwrap_or("single");
            if matches!(val, "nil" | "none") {
                return Some(None);
            }
            let width = attr_f64(c, "sz").map_or(0.5, |s| (s / 8.0).clamp(0.25, 6.0));
            let color = attr(c, "color").filter(|c| *c != "auto").and_then(hex_color).unwrap_or([0.0; 3]);
            Some(Some(Border { width, color, dash: flow::LineStyle::of(val) }))
        };
        Borders {
            top: side(&["top"]),
            bottom: side(&["bottom"]),
            left: side(&["left", "start"]),
            right: side(&["right", "end"]),
            inside_h: side(&["insideH"]),
            inside_v: side(&["insideV"]),
        }
    }

    fn merge(&mut self, o: &Borders) {
        macro_rules! take {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f; } )* };
        }
        take!(top, bottom, left, right, inside_h, inside_v);
    }
}

/// Formatting a table style gives the whole table or one part of it (`w:tblStylePr`): the
/// header row, banded rows, the first column…
#[derive(Clone, Default, Debug)]
struct Cond {
    p: PPr,
    r: RPr,
    shade: Option<[f64; 3]>,
    borders: Option<Borders>,
}

impl Cond {
    fn parse(n: Node<'_, '_>) -> Cond {
        Cond {
            p: child(n, "pPr").map(PPr::parse).unwrap_or_default(),
            r: child(n, "rPr").map(RPr::parse).unwrap_or_default(),
            shade: xml::path(n, &["tcPr", "shd"]).and_then(|s| attr(s, "fill")).filter(|f| *f != "auto").and_then(hex_color),
            borders: xml::path(n, &["tcPr", "tcBorders"]).map(Borders::parse),
        }
    }

    fn merge(&mut self, o: &Cond) {
        self.p.merge(&o.p);
        self.r.merge(&o.r);
        if o.shade.is_some() {
            self.shade = o.shade;
        }
        if let Some(ob) = &o.borders {
            self.borders.get_or_insert_with(Borders::default).merge(ob);
        }
    }
}

#[derive(Clone, Default, Debug)]
struct StyleDef {
    based_on: Option<String>,
    p: PPr,
    r: RPr,
    borders: Option<Borders>,
    /// A table style's cell shading for the whole table, and its parts by `w:type`.
    table: Option<Cond>,
    parts: Vec<(String, Cond)>,
}

struct Theme {
    major: String,
    minor: String,
}

struct Styles {
    p_default: PPr,
    r_default: RPr,
    by_id: HashMap<String, StyleDef>,
    default_para: Option<String>,
    default_table: Option<String>,
    /// The character style named "Hyperlink".
    hyperlink: Option<String>,
    /// Style ids by lower-case name, for style references (`STYLEREF "Heading 1"`).
    by_name: HashMap<String, String>,
}

impl Styles {
    /// A style and the styles it is based on, root first (cycles and deep chains are cut).
    fn chain(&self, id: &str) -> Vec<&StyleDef> {
        let mut out = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        let mut cur = Some(id);
        while let Some(i) = cur {
            if seen.contains(&i) || seen.len() > 16 {
                break;
            }
            seen.push(i);
            let Some(s) = self.by_id.get(i) else { break };
            out.push(s);
            cur = s.based_on.as_deref();
        }
        out.reverse();
        out
    }

    /// A paragraph style's properties over the document defaults and, in a table cell, the
    /// table style's (`layer`). As in Word, the table style comes after the default paragraph
    /// style (Normal) but before any other paragraph style.
    fn para_style(&self, id: Option<&str>, layer: Option<&(PPr, RPr)>) -> (PPr, RPr) {
        let mut p = self.p_default.clone();
        let mut r = self.r_default.clone();
        let id = id.or(self.default_para.as_deref());
        let normal = id.is_none() || id == self.default_para.as_deref();
        let apply_style = |p: &mut PPr, r: &mut RPr| {
            if let Some(id) = id {
                for s in self.chain(id) {
                    p.merge(&s.p);
                    r.merge(&s.r);
                }
            }
        };
        if normal {
            apply_style(&mut p, &mut r);
        }
        if let Some((lp, lr)) = layer {
            p.merge(lp);
            r.merge(lr);
        }
        if !normal {
            apply_style(&mut p, &mut r);
        }
        (p, r)
    }

    fn run_style(&self, id: &str) -> RPr {
        let mut r = RPr::default();
        for s in self.chain(id) {
            r.merge(&s.r);
        }
        r
    }

    /// A table style's formatting for the whole table and for each of its parts, through its
    /// `basedOn` chain.
    fn table_parts(&self, id: Option<&str>) -> (Cond, Vec<(String, Cond)>) {
        let mut whole = Cond::default();
        let mut parts: Vec<(String, Cond)> = Vec::new();
        if let Some(id) = id.or(self.default_table.as_deref()) {
            for s in self.chain(id) {
                whole.merge(&Cond { p: s.p.clone(), r: s.r.clone(), ..s.table.clone().unwrap_or_default() });
                for (ty, c) in &s.parts {
                    match parts.iter_mut().find(|(t, _)| t == ty) {
                        Some((_, mine)) => mine.merge(c),
                        None => parts.push((ty.clone(), c.clone())),
                    }
                }
            }
        }
        (whole, parts)
    }

    fn table_borders(&self, id: Option<&str>) -> Borders {
        let mut b = Borders::default();
        if let Some(id) = id.or(self.default_table.as_deref()) {
            for s in self.chain(id) {
                if let Some(sb) = &s.borders {
                    b.merge(sb);
                }
            }
        }
        b
    }
}

fn load_theme(zip: &Zip<'_>, rels: &Rels) -> Theme {
    let mut t = Theme { major: "Calibri Light".into(), minor: "Calibri".into() };
    let Some(part) = rels.of_type("theme") else { return t };
    let Ok(Some(text)) = zip.text(part) else { return t };
    let Ok(d) = xml::parse(&text) else { return t };
    for (name, slot) in [("majorFont", &mut t.major), ("minorFont", &mut t.minor)] {
        if let Some(face) =
            d.descendants().find(|n| is(*n, name)).and_then(|f| child(f, "latin")).and_then(|l| attr(l, "typeface")).filter(|f| !f.is_empty())
        {
            *slot = face.to_owned();
        }
    }
    t
}

fn load_styles(zip: &Zip<'_>, rels: &Rels) -> Result<Styles, String> {
    let mut s = Styles {
        p_default: PPr::default(),
        r_default: RPr::default(),
        by_id: HashMap::new(),
        default_para: None,
        default_table: None,
        hyperlink: None,
        by_name: HashMap::new(),
    };
    let Some(part) = rels.of_type("styles") else { return Ok(s) };
    let Some(text) = zip.text(part)? else { return Ok(s) };
    let d = xml::parse(&text).map_err(|e| format!("{part}: {e}"))?;
    let root = d.root_element();
    if let Some(defaults) = child(root, "docDefaults") {
        if let Some(r) = xml::path(defaults, &["rPrDefault", "rPr"]) {
            s.r_default = RPr::parse(r);
        }
        if let Some(p) = xml::path(defaults, &["pPrDefault", "pPr"]) {
            s.p_default = PPr::parse(p);
        }
    }
    for st in children(root, "style") {
        let Some(id) = attr(st, "styleId") else { continue };
        let ty = attr(st, "type").unwrap_or("paragraph");
        let def = StyleDef {
            based_on: child(st, "basedOn").and_then(|b| attr(b, "val")).map(str::to_owned),
            p: child(st, "pPr").map(PPr::parse).unwrap_or_default(),
            r: child(st, "rPr").map(RPr::parse).unwrap_or_default(),
            borders: xml::path(st, &["tblPr", "tblBorders"]).map(Borders::parse),
            table: (ty == "table").then(|| Cond { p: PPr::default(), r: RPr::default(), ..Cond::parse(st) }),
            // A hostile style can't multiply the work: Word defines thirteen parts.
            parts: children(st, "tblStylePr").take(16).filter_map(|c| Some((attr(c, "type")?.to_owned(), Cond::parse(c)))).collect(),
        };
        if ty == "character" && child(st, "name").and_then(|n| attr(n, "val")).is_some_and(|n| n.eq_ignore_ascii_case("hyperlink")) {
            s.hyperlink = Some(id.to_owned());
        }
        if flag(attr(st, "default")) && attr(st, "default").is_some() {
            match ty {
                "paragraph" => s.default_para = Some(id.to_owned()),
                "table" => s.default_table = Some(id.to_owned()),
                _ => {}
            }
        }
        if let Some(name) = child(st, "name").and_then(|n| attr(n, "val")) {
            s.by_name.insert(name.to_lowercase(), id.to_owned());
        }
        s.by_id.insert(id.to_owned(), def);
    }
    Ok(s)
}

// ── numbering ───────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Default, Debug)]
struct Level {
    format: String,
    text: String,
    start: i64,
    p: PPr,
    r: RPr,
}

/// A list instance: its nine levels, its start overrides and its abstract list's id (instances
/// of one abstract list share their counters, as in Word).
type NumDef = (Vec<Level>, Vec<(usize, i64)>, String);

/// A table cell laid out: its paragraphs, and its lines and gaps in order (height, paragraph and
/// line, or `None` for spacing).
type CellLines = (Vec<Laid>, Vec<(f64, Option<(usize, usize)>)>);

#[derive(Default)]
struct Numbering {
    /// numId → its definition.
    nums: HashMap<String, NumDef>,
    /// Counters by abstract list.
    counters: HashMap<String, [i64; 9]>,
    /// Instances already used: a start override applies when its instance is first used.
    started: std::collections::HashSet<String>,
}

fn load_numbering(zip: &Zip<'_>, rels: &Rels) -> Result<Numbering, String> {
    let mut out = Numbering::default();
    let Some(part) = rels.of_type("numbering") else { return Ok(out) };
    let Some(text) = zip.text(part)? else { return Ok(out) };
    let d = xml::parse(&text).map_err(|e| format!("{part}: {e}"))?;
    let root = d.root_element();
    let mut abstracts: HashMap<String, Vec<Level>> = HashMap::new();
    for a in children(root, "abstractNum") {
        let Some(id) = attr(a, "abstractNumId") else { continue };
        let mut levels = vec![Level::default(); 9];
        for l in children(a, "lvl") {
            let i = attr_f64(l, "ilvl").unwrap_or(0.0).clamp(0.0, 8.0) as usize;
            let Some(slot) = levels.get_mut(i) else { continue };
            *slot = Level {
                format: child(l, "numFmt").and_then(|n| attr(n, "val")).unwrap_or("decimal").to_owned(),
                text: child(l, "lvlText").and_then(|n| attr(n, "val")).unwrap_or_default().chars().take(MAX_LVL_TEXT).collect(),
                start: child(l, "start").and_then(|n| attr_f64(n, "val")).unwrap_or(1.0).clamp(-1e9, 1e9) as i64,
                p: child(l, "pPr").map(PPr::parse).unwrap_or_default(),
                r: child(l, "rPr").map(RPr::parse).unwrap_or_default(),
            };
        }
        abstracts.insert(id.to_owned(), levels);
    }
    for n in children(root, "num") {
        let Some(id) = attr(n, "numId") else { continue };
        let Some(abs) = child(n, "abstractNumId").and_then(|a| attr(a, "val")) else { continue };
        let Some(levels) = abstracts.get(abs) else { continue };
        let overrides = children(n, "lvlOverride")
            .filter_map(|o| {
                let lvl = attr_f64(o, "ilvl")?.clamp(0.0, 8.0) as usize;
                let start = child(o, "startOverride").and_then(|s| attr_f64(s, "val"))?.clamp(-1e9, 1e9) as i64;
                Some((lvl, start))
            })
            .collect();
        out.nums.insert(id.to_owned(), (levels.clone(), overrides, abs.to_owned()));
    }
    Ok(out)
}

fn roman(mut n: i64) -> String {
    if !(1..=3999).contains(&n) {
        return n.to_string();
    }
    let table = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut s = String::new();
    for (v, r) in table {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

fn letters(n: i64) -> String {
    if n < 1 {
        return n.to_string();
    }
    // Word repeats the letter: a … z, aa … zz.
    let n = n.min(780);
    let c = char::from(b'a' + ((n - 1) % 26) as u8);
    std::iter::repeat_n(c, ((n - 1) / 26 + 1) as usize).collect()
}

fn format_number(n: i64, format: &str) -> String {
    match format {
        "lowerLetter" => letters(n),
        "upperLetter" => letters(n).to_uppercase(),
        "lowerRoman" => roman(n),
        "upperRoman" => roman(n).to_uppercase(),
        "decimalZero" if (0..10).contains(&n) => format!("0{n}"),
        "none" => String::new(),
        _ => n.to_string(),
    }
}

impl Numbering {
    /// The label of the next item at `lvl` of list `id`, its paragraph and run properties.
    fn next(&mut self, id: &str, lvl: usize) -> Option<(String, PPr, RPr)> {
        let (levels, overrides, abs) = self.nums.get(id)?;
        let level = levels.get(lvl)?.clone();
        let starts: Vec<i64> = (0..9)
            .map(|i| overrides.iter().find(|(l, _)| *l == i).map(|(_, s)| *s).or_else(|| levels.get(i).map(|l| l.start)).unwrap_or(1))
            .collect();
        let first_use = self.started.insert(id.to_owned());
        let counters = self.counters.entry(abs.clone()).or_insert([i64::MIN; 9]);
        // An instance with a start override restarts those levels the first time it is used.
        if first_use {
            for (l, _) in overrides {
                if let Some(slot) = counters.get_mut(*l) {
                    *slot = i64::MIN;
                }
            }
        }
        let slot = counters.get_mut(lvl)?;
        *slot = if *slot == i64::MIN { starts.get(lvl).copied().unwrap_or(1) } else { slot.saturating_add(1) };
        for deeper in counters.iter_mut().skip(lvl + 1) {
            *deeper = i64::MIN;
        }
        let label = if level.format == "bullet" {
            level.text.clone()
        } else {
            let mut t = level.text.clone();
            for i in (0..9).rev() {
                let key = format!("%{}", i + 1);
                if t.contains(&key) {
                    let v = counters.get(i).copied().filter(|v| *v != i64::MIN).or_else(|| starts.get(i).copied()).unwrap_or(1);
                    let fmt = levels.get(i).map_or("decimal", |l| l.format.as_str());
                    t = t.replace(&key, &format_number(v, fmt));
                }
            }
            t
        };
        Some((label, level.p, level.r))
    }
}

// ── document model ──────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Cell {
    span: usize,
    paras: Vec<Para>,
    shade: Option<[f64; 3]>,
    borders: Borders,
    /// `Some(true)` starts a vertical merge, `Some(false)` continues one.
    vmerge: Option<bool>,
    /// Where the content sits in a taller cell: 0 top, 0.5 centre, 1 bottom.
    valign: f64,
    /// Text turned a quarter (`w:textDirection`): 1 reading upwards (`btLr`), -1 downwards (`tbRl`).
    turn: i8,
}

#[derive(Clone, Debug)]
struct Row {
    cells: Vec<Cell>,
    /// Minimum height, or the exact height when the bool is true.
    height: Option<(f64, bool)>,
    /// A header row, repeated at the top of each page the table goes on to (`w:tblHeader`).
    header: bool,
}

#[derive(Clone, Debug)]
struct Table {
    cols: Vec<f64>,
    rows: Vec<Row>,
    borders: Borders,
    indent: f64,
    center: bool,
    pad: (f64, f64),
    /// A floating table placed against the page or its margins (`w:tblpPr`), drawn there
    /// without moving the text.
    float: Option<Float>,
}

/// Where a floating table goes: its top as a fraction of the room left (0 top, 0.5 centre,
/// 1 bottom) or an offset in points, inside the margins or the whole page.
#[derive(Clone, Copy, Debug)]
struct Float {
    y: FloatY,
    page: bool,
    /// Placed from where the text has reached (`vertAnchor="text"`), the text after it
    /// wrapping beside it.
    text: bool,
    /// Its horizontal place, against the page (`true`) or the text column.
    x: Option<(FloatY, bool)>,
    /// The distance the text keeps at its left, right and below.
    gaps: [f64; 3],
}

#[derive(Clone, Copy, Debug)]
enum FloatY {
    Align(f64),
    Offset(f64),
}

#[derive(Clone, Debug)]
enum Block {
    Para(Para),
    Table(Table),
    PageBreak,
    /// A column break: the next column, or the next page after the last column.
    ColumnBreak,
}

#[derive(Clone, Debug)]
struct Section {
    w: f64,
    h: f64,
    margins: [f64; 4],
    header_dist: f64,
    footer_dist: f64,
    continuous: bool,
    header: Option<String>,
    footer: Option<String>,
    first_header: Option<String>,
    first_footer: Option<String>,
    title_page: bool,
    /// Text columns: each one's left edge from the text area's and its width; empty for one.
    cols: Vec<(f64, f64)>,
    /// A line between columns.
    col_sep: bool,
    /// Page numbering (`w:pgNumType`): where the section restarts it, and its format.
    num_start: Option<i64>,
    num_fmt: Option<String>,
    /// Page borders (`w:pgBorders`).
    page_borders: Option<PageBorders>,
    /// Line numbering (`w:lnNumType`): every how many lines, the first number, when it
    /// restarts (0 each page, 1 each section, 2 never) and its distance from the text.
    line_numbers: Option<(i64, i64, u8, f64)>,
}

/// A section's page borders: top, left, bottom, right; measured from the page edge or from the
/// text; on every page (0), the first page (1) or all but the first (2).
#[derive(Clone, Copy, Debug)]
struct PageBorders {
    sides: [Option<Rule>; 4],
    from_page: bool,
    display: u8,
}

impl Default for Section {
    fn default() -> Self {
        Section {
            w: 612.0,
            h: 792.0,
            margins: [72.0; 4],
            header_dist: 36.0,
            footer_dist: 36.0,
            continuous: false,
            header: None,
            footer: None,
            first_header: None,
            first_footer: None,
            title_page: false,
            cols: Vec::new(),
            col_sep: false,
            num_start: None,
            num_fmt: None,
            page_borders: None,
            line_numbers: None,
        }
    }
}

fn parse_section(n: Node<'_, '_>, prev: &Section) -> Section {
    let mut s = Section { continuous: false, title_page: false, first_header: None, first_footer: None, ..prev.clone() };
    if let Some(sz) = child(n, "pgSz") {
        let (w, h) = (attr_f64(sz, "w").map(twips), attr_f64(sz, "h").map(twips));
        if let (Some(w), Some(h)) = (w, h) {
            s.w = w;
            s.h = h;
            if attr(sz, "orient") == Some("landscape") && s.w < s.h {
                std::mem::swap(&mut s.w, &mut s.h);
            }
        }
    }
    if let Some(m) = child(n, "pgMar") {
        let get = |k: &str, d: f64| attr_f64(m, k).map_or(d, |v| twips(v.abs()));
        s.margins = [get("top", 72.0), get("right", 72.0), get("bottom", 72.0), get("left", 72.0)];
        s.header_dist = get("header", 36.0);
        s.footer_dist = get("footer", 36.0);
    }
    s.w = fin(s.w, 612.0).clamp(72.0, 14_400.0);
    s.h = fin(s.h, 792.0).clamp(72.0, 14_400.0);
    // Margins may not leave less than an inch of text area.
    let [t, r, b, l] = s.margins.map(|m| fin(m, 72.0).clamp(0.0, 7_000.0));
    let (l, r) = if l + r > s.w - 72.0 { (36.0f64.min(l), 36.0f64.min(r)) } else { (l, r) };
    let (t, b) = if t + b > s.h - 72.0 { (36.0f64.min(t), 36.0f64.min(b)) } else { (t, b) };
    s.margins = [t, r, b, l];
    s.continuous = child(n, "type").and_then(|t| attr(t, "val")) == Some("continuous");
    // Columns (`w:cols`): equal ones from their number and spacing, or each one's own width.
    s.cols = Vec::new();
    s.col_sep = false;
    if let Some(c) = child(n, "cols") {
        let text_w = (s.w - l - r).max(36.0);
        let space = attr_f64(c, "space").map_or(36.0, |v| fin(twips(v), 36.0).clamp(0.0, text_w));
        let own: Vec<(f64, f64)> = children(c, "col")
            .take(45)
            .map(|col| {
                (
                    attr_f64(col, "w").map_or(0.0, |v| fin(twips(v), 0.0).max(0.0)),
                    attr_f64(col, "space").map_or(space, |v| fin(twips(v), 0.0).max(0.0)),
                )
            })
            .collect();
        let equal = attr(c, "equalWidth").is_none_or(|v| flag(Some(v))) || own.is_empty();
        let num = attr_f64(c, "num").map_or(1, |v| fin(v, 1.0).clamp(1.0, 45.0) as usize);
        let mut x = 0.0;
        if !equal {
            for (w, gap) in &own {
                s.cols.push((x, w.min(text_w)));
                x += w + gap;
            }
        } else if num > 1 {
            let w = ((text_w - space * (num - 1) as f64) / num as f64).max(18.0);
            for i in 0..num {
                s.cols.push((i as f64 * (w + space), w));
            }
        }
        if s.cols.len() < 2 {
            s.cols.clear();
        }
        s.col_sep = flag(attr(c, "sep")) && attr(c, "sep").is_some();
    }
    s.title_page = child(n, "titlePg").is_some_and(|t| flag(attr(t, "val")));
    let num = child(n, "pgNumType");
    s.num_start = num.and_then(|p| attr_f64(p, "start")).filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 1e6) as i64);
    s.num_fmt = num.and_then(|p| attr(p, "fmt")).map(str::to_owned);
    s.page_borders = child(n, "pgBorders").map(|b| PageBorders {
        sides: ["top", "left", "bottom", "right"].map(|k| child(b, k).and_then(rule)),
        from_page: attr(b, "offsetFrom") == Some("page"),
        display: match attr(b, "display") {
            Some("firstPage") => 1,
            Some("notFirstPage") => 2,
            _ => 0,
        },
    });
    s.line_numbers = child(n, "lnNumType").filter(|l| attr_f64(*l, "countBy").is_some_and(|v| v >= 1.0)).map(|l| {
        (
            attr_f64(l, "countBy").map_or(1, |v| fin(v, 1.0).clamp(1.0, 100.0) as i64),
            // Word saves the first number less one.
            attr_f64(l, "start").map_or(1, |v| fin(v, 0.0).clamp(0.0, 1e6) as i64 + 1),
            match attr(l, "restart") {
                Some("newSection") => 1,
                Some("continuous") => 2,
                _ => 0,
            },
            attr_f64(l, "distance").map_or(18.0, |v| fin(twips(v), 18.0).clamp(0.0, 144.0)),
        )
    });
    for (tag, first, default) in [("headerReference", &mut s.first_header, &mut s.header), ("footerReference", &mut s.first_footer, &mut s.footer)] {
        for r in children(n, tag) {
            let Some(id) = rel_attr(r, "id") else { continue };
            match attr(r, "type") {
                Some("first") => *first = Some(id.to_owned()),
                Some("even") => {}
                _ => *default = Some(id.to_owned()),
            }
        }
    }
    s
}

// ── reading ─────────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
enum FieldState {
    #[default]
    None,
    Instr(String),
    /// Showing the cached result; `true` when the field was replaced by a live page field.
    Result(bool),
}

struct Cx<'z, 'd> {
    zip: &'z Zip<'z>,
    doc: &'d mut Document,
    notes: &'d mut Notes,
    styles: Styles,
    numbering: Numbering,
    theme: Theme,
    images: HashMap<String, Option<ObjRef>>,
    /// Footnote and endnote references in order: (kind, id, label).
    note_refs: Vec<(&'static str, String, String)>,
    /// How many footnote and endnote references `note_refs` holds, for their labels.
    footnotes: usize,
    endnotes: usize,
    /// Complex fields open at this point of the document, across paragraphs: their instruction
    /// and whether their result is being read.
    fields: Vec<(String, bool)>,
    /// Hyperlinks open around the run being read.
    in_link: usize,
    /// The links open at this point (a hyperlink, a cross-reference field's result), innermost
    /// last; their runs carry the link.
    links: Vec<Option<Arc<LinkTarget>>>,
    /// For each open complex field, whether its result opened a link.
    field_links: Vec<bool>,
    /// While a table cell is read: its table style's paragraph and run properties.
    cell_layer: Option<(PPr, RPr)>,
    /// The theme's accent colours, for charts, and all its colours (lt1, dk1, lt2, dk2,
    /// accent1–6…), for shapes.
    accents: Vec<[f64; 3]>,
    scheme: Vec<[f64; 3]>,
}

impl Cx<'_, '_> {
    fn para_style(&self, id: Option<&str>) -> (PPr, RPr) {
        self.styles.para_style(id, self.cell_layer.as_ref())
    }

    /// Inside a table of contents' (or of figures') field result, where Word prints links
    /// without the Hyperlink style.
    fn in_toc(&self) -> bool {
        self.fields.iter().any(|(instr, result)| *result && instr.split_whitespace().next().is_some_and(|w| w.eq_ignore_ascii_case("TOC")))
    }

    fn image(&mut self, rels: &Rels, id: &str) -> Option<ObjRef> {
        if rels.is_external(id) {
            self.notes.add("linked (not embedded) images, drawn as empty boxes");
            return None;
        }
        let part = rels.target(id)?.to_owned();
        if let Some(cached) = self.images.get(&part) {
            return *cached;
        }
        let loaded = match self.zip.read(&part) {
            Ok(Some(bytes)) => {
                crate::image_xobject(self.doc, &part, &bytes).ok().map(|(r, _)| r).or_else(|| super::emf::form(self.doc, &bytes, self.notes))
            }
            _ => None,
        };
        if loaded.is_none() {
            self.notes.add("images in a format the importer can't draw (SVG…), drawn as empty boxes");
        }
        self.images.insert(part, loaded);
        loaded
    }

    fn para_props(&mut self, ppr: Option<Node<'_, '_>>) -> (PPr, RPr, Option<(String, RunStyle)>) {
        let direct = ppr.map(PPr::parse).unwrap_or_default();
        let (mut p, mut mark) = self.para_style(direct.style.as_deref());
        // Numbering's indents sit between the style's and the paragraph's own.
        let num = direct.num.clone().or_else(|| p.num.clone());
        let mut label = None;
        if let Some((id, lvl)) = num
            && id != "0"
            && let Some((text, lp, lr)) = self.numbering.next(&id, lvl)
        {
            p.merge(&PPr { num: None, style: None, ..lp });
            let mut lstyle = mark.clone();
            lstyle.merge(&lr);
            if !text.is_empty() {
                let style = lstyle.style(&self.theme);
                label = Some((self.notes.drawable(&text), RunStyle { underline: false, strike: false, highlight: None, ..style }));
            }
        }
        p.merge(&direct);
        if let Some(r) = ppr.and_then(|n| child(n, "rPr")) {
            mark.merge(&RPr::parse(r));
        }
        (p, mark, label)
    }

    fn to_para(&self, p: &PPr, mark: &RPr) -> Para {
        Para {
            align: match p.align {
                None | Some(Align::Left) if p.bidi == Some(true) => Align::Right,
                a => a.unwrap_or(Align::Left),
            },
            left: p.left.unwrap_or(0.0),
            right: p.right.unwrap_or(0.0),
            first: p.first.unwrap_or(0.0),
            before: p.before.unwrap_or(0.0),
            after: p.after.unwrap_or(0.0),
            line: p.line.unwrap_or(LineRule::Auto(1.0)),
            tabs: p.tabs.iter().filter_map(|(_, t)| *t).collect(),
            base: mark.style(&self.theme),
            page_break_before: p.page_break_before.unwrap_or(false),
            shade: p.shade.flatten(),
            borders: p.para_borders().map(Box::new),
            keep_next: p.keep_next.unwrap_or(false),
            contextual: p.contextual.unwrap_or(false).then(|| p.style.clone().or_else(|| self.styles.default_para.clone()).unwrap_or_default()),
            style: p.style.as_deref().or(self.styles.default_para.as_deref()).map(Arc::from),
            rtl: p.bidi == Some(true),
            outline: p.outline.filter(|l| *l < 9),
            // Word lays paragraphs out with widow and orphan control unless they turn it off.
            widow: p.widow.unwrap_or(true),
            keep_lines: p.keep_lines.unwrap_or(false),
            no_line_numbers: p.no_line_numbers.unwrap_or(false),
            drop_cap: p.drop_cap,
            ..Para::default()
        }
    }

    /// Read a paragraph. Page breaks inside it split it into several blocks; text boxes'
    /// paragraphs follow it.
    fn paragraph(&mut self, n: Node<'_, '_>, rels: &Rels, depth: usize, out: &mut Vec<Block>) {
        let (p, mark, label) = self.para_props(child(n, "pPr"));
        let mut para = self.to_para(&p, &mark);
        para.label = label;
        let mut extra = Vec::new();
        let mut state = FieldState::None;
        let mut pieces: Vec<Block> = Vec::new();
        // Runs start from the paragraph style's run properties; the paragraph mark's own
        // formatting only styles the mark (and an empty paragraph's height).
        let (_, base) = self.para_style(p.style.as_deref());
        self.inline(n, rels, &base, &mut para, &mut pieces, &mut extra, &mut state, depth);
        // A paragraph ending with a page break keeps its mark on the page before (as Word 2013
        // and later show it): its empty remainder takes no line on the new page.
        let ends_in_break = matches!(pieces.last(), Some(Block::PageBreak | Block::ColumnBreak));
        if !(ends_in_break && para.items.is_empty() && para.label.is_none() && para.floats.is_empty()) {
            pieces.push(Block::Para(para));
        }
        out.extend(pieces);
        out.extend(extra);
    }

    #[allow(clippy::too_many_arguments)]
    fn inline(
        &mut self,
        n: Node<'_, '_>,
        rels: &Rels,
        base: &RPr,
        para: &mut Para,
        out: &mut Vec<Block>,
        extra: &mut Vec<Block>,
        state: &mut FieldState,
        depth: usize,
    ) {
        if depth > MAX_DEPTH {
            self.notes.add("content beyond the nesting limit, left out");
            return;
        }
        for c in n.children().filter(Node::is_element) {
            match c.tag_name().name() {
                "r" => self.run(c, rels, base, para, out, extra, state, depth),
                "hyperlink" => {
                    // An address (an external relationship) or a bookmark in the document.
                    let target = rel_attr(c, "id")
                        .and_then(|id| rels.external(id))
                        .map(|u| LinkTarget::Uri(u.to_owned()))
                        .or_else(|| attr(c, "anchor").map(|a| LinkTarget::Anchor(a.to_owned())));
                    self.in_link = self.in_link.saturating_add(1);
                    self.links.push(target.map(Arc::new));
                    self.inline(c, rels, base, para, out, extra, state, depth + 1);
                    self.links.pop();
                    self.in_link = self.in_link.saturating_sub(1);
                }
                "bookmarkStart" => {
                    if let Some(name) = attr(c, "name").filter(|n| *n != "_GoBack" && n.len() <= 256)
                        && para.anchors.len() < 256
                    {
                        para.anchors.push(name.to_owned());
                    }
                }
                "smartTag" | "ins" | "customXml" | "dir" | "bdo" | "moveTo" => self.inline(c, rels, base, para, out, extra, state, depth + 1),
                "sdt" => {
                    if let Some(content) = child(c, "sdtContent") {
                        self.inline(content, rels, base, para, out, extra, state, depth + 1);
                    }
                }
                "fldSimple" => {
                    let instr = attr(c, "instr").unwrap_or_default();
                    if let Some(field) = self.live_field(instr) {
                        para.items.push(Inline::Field(field, self.run_props(None, base)));
                    } else if let Some(now) = date_field(instr) {
                        let style = child(c, "r").and_then(|r| child(r, "rPr"));
                        let text = self.notes.drawable(&now);
                        para.items.push(Inline::Text(text, self.run_props(style, base)));
                    } else {
                        self.inline(c, rels, base, para, out, extra, state, depth + 1);
                    }
                }
                "oMath" | "oMathPara" => {
                    let style = self.run_props(None, base);
                    // A display equation (`m:oMathPara`) is centred unless it says otherwise, each
                    // of its equations on a line of its own.
                    let maths: Vec<Node<'_, '_>> = if is(c, "oMathPara") { children(c, "oMath").collect() } else { vec![c] };
                    if is(c, "oMathPara") {
                        para.align = match xml::path(c, &["oMathParaPr", "jc"]).and_then(|j| attr(j, "val")) {
                            Some("left") => Align::Left,
                            Some("right") => Align::Right,
                            _ => Align::Center,
                        };
                    }
                    for (i, m) in maths.into_iter().enumerate() {
                        if !super::math::has_content(m) {
                            continue;
                        }
                        if i > 0 {
                            para.items.push(Inline::Break);
                        }
                        let b = super::math::layout(m, style.size, style.color, self.notes);
                        para.items.push(Inline::Math(std::sync::Arc::new(b)));
                    }
                }
                _ => {}
            }
        }
    }

    fn run_props(&self, rpr: Option<Node<'_, '_>>, mark: &RPr) -> RunStyle {
        // The paragraph style's run properties, then the character style's, then direct ones.
        let mut r = mark.clone();
        r.style = None;
        let direct = rpr.map(RPr::parse).unwrap_or_default();
        if let Some(id) = &direct.style {
            let plain_link = self.in_link > 0 && self.styles.hyperlink.as_ref() == Some(id) && self.in_toc();
            if !plain_link {
                r.merge(&self.styles.run_style(id));
            }
        }
        r.merge(&direct);
        RunStyle { link: self.links.iter().rev().flatten().next().cloned(), ..r.style(&self.theme) }
    }

    #[allow(clippy::too_many_arguments)]
    fn run(
        &mut self,
        r: Node<'_, '_>,
        rels: &Rels,
        base: &RPr,
        para: &mut Para,
        out: &mut Vec<Block>,
        extra: &mut Vec<Block>,
        state: &mut FieldState,
        depth: usize,
    ) {
        let style = self.run_props(child(r, "rPr"), base);
        for c in r.children().filter(Node::is_element) {
            let name = c.tag_name().name();
            match (name, &mut *state) {
                ("fldChar", _) => match attr(c, "fldCharType") {
                    Some("begin") => {
                        if self.fields.len() < MAX_DEPTH {
                            self.fields.push((String::new(), false));
                            self.field_links.push(false);
                        }
                        *state = FieldState::Instr(String::new());
                    }
                    Some("separate") => {
                        if let Some((_, result)) = self.fields.last_mut() {
                            *result = true;
                        }
                        let instr = if let FieldState::Instr(i) = &*state { i.clone() } else { String::new() };
                        // A cross-reference with `\h` links its result to the bookmark it names.
                        if let Some(target) = ref_link(&instr)
                            && let Some(open) = self.field_links.last_mut()
                        {
                            *open = true;
                            self.links.push(Some(Arc::new(target)));
                        }
                        let field = self.live_field(&instr);
                        let live = field.is_some();
                        if let Some(field) = field {
                            para.items.push(Inline::Field(field, style.clone()));
                        }
                        // Word updates the date and time when it prints: today replaces the
                        // saved result.
                        let now = if live { None } else { date_field(&instr) };
                        if let Some(now) = &now {
                            let text = self.notes.drawable(now);
                            para.items.push(Inline::Text(text, style.clone()));
                        }
                        *state = FieldState::Result(live || now.is_some());
                    }
                    Some("end") => {
                        if self.fields.pop().is_some() && self.field_links.pop() == Some(true) {
                            self.links.pop();
                        }
                        // A field with no cached result: only page fields can still be shown.
                        if let FieldState::Instr(i) = &*state {
                            let i = i.clone();
                            if let Some(field) = self.live_field(&i) {
                                para.items.push(Inline::Field(field, style.clone()));
                            } else if let Some(now) = date_field(&i) {
                                let text = self.notes.drawable(&now);
                                para.items.push(Inline::Text(text, style.clone()));
                            }
                        }
                        *state = FieldState::None;
                    }
                    _ => {}
                },
                ("instrText", FieldState::Instr(i)) if i.len() < 4096 => {
                    let t = c.text().unwrap_or_default();
                    i.push_str(t);
                    if let Some((instr, false)) = self.fields.last_mut()
                        && instr.len() < 4096
                    {
                        instr.push_str(t);
                    }
                }
                (_, FieldState::Instr(_)) | (_, FieldState::Result(true)) => {}
                ("t", _) => {
                    let text = self.notes.drawable(c.text().unwrap_or_default());
                    para.items.push(Inline::Text(text, style.clone()));
                }
                ("tab" | "ptab", _) => para.items.push(Inline::Tab),
                ("br" | "cr", _) => match attr(c, "type") {
                    Some("page") => {
                        // Content after a page break starts the next page.
                        let rest = Para { items: Vec::new(), before: 0.0, label: None, notes: Vec::new(), floats: Vec::new(), ..para.clone() };
                        let done = std::mem::replace(para, rest);
                        out.push(Block::Para(Para { after: 0.0, ..done }));
                        out.push(Block::PageBreak);
                    }
                    Some("column") => {
                        let rest = Para { items: Vec::new(), before: 0.0, label: None, notes: Vec::new(), floats: Vec::new(), ..para.clone() };
                        let done = std::mem::replace(para, rest);
                        out.push(Block::Para(Para { after: 0.0, ..done }));
                        out.push(Block::ColumnBreak);
                    }
                    _ => para.items.push(Inline::Break),
                },
                ("noBreakHyphen", _) => para.items.push(Inline::Text("-".into(), style.clone())),
                ("sym", _) => {
                    let ch = attr(c, "char").and_then(|h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32).unwrap_or('•');
                    let text = self.notes.drawable(&ch.to_string());
                    para.items.push(Inline::Text(text, style.clone()));
                }
                ("drawing", _) => self.drawing(c, rels, para, extra, depth),
                // Word 2010 and later wrap shapes and text boxes in markup compatibility: a
                // `Choice` with DrawingML and a `Fallback` with VML. One of them is read.
                ("AlternateContent", _) if depth < MAX_DEPTH => {
                    if let Some(pick) = child(c, "Choice").or_else(|| child(c, "Fallback")) {
                        self.run(pick, rels, base, para, out, extra, state, depth + 1);
                    }
                }
                ("pict" | "object", _) => self.vml(c, rels, para, extra, depth),
                ("footnoteReference" | "endnoteReference", _) => {
                    let kind = if name == "footnoteReference" { "footnote" } else { "endnote" };
                    // A running count per kind: recounting the earlier references made each one cost
                    // as much as all before it.
                    let count = if kind == "footnote" { &mut self.footnotes } else { &mut self.endnotes };
                    let number = count.saturating_add(1);
                    let label = if kind == "endnote" { roman(i64::try_from(number).unwrap_or(1)) } else { number.to_string() };
                    if let Some(id) = attr(c, "id") {
                        self.note_refs.push((kind, id.to_owned(), label.clone()));
                        *count = number;
                        if kind == "footnote" {
                            para.notes.push((self.note_refs.len() - 1, para.items.len()));
                        }
                    }
                    para.items.push(Inline::Text(label, RunStyle { vert: 1, ..style.clone() }));
                }
                _ => {}
            }
        }
    }

    /// A DrawingML object: a picture, a text box (its text joins the flow), or a placeholder.
    fn drawing(&mut self, d: Node<'_, '_>, rels: &Rels, para: &mut Para, extra: &mut Vec<Block>, depth: usize) {
        let Some(holder) = d.children().find(|c| is(*c, "inline") || is(*c, "anchor")) else { return };
        let extent = child(holder, "extent");
        let w = extent.and_then(|e| attr_f64(e, "cx")).map_or(72.0, |v| v / 12_700.0);
        let h = extent.and_then(|e| attr_f64(e, "cy")).map_or(72.0, |v| v / 12_700.0);
        let Some(data) = holder.descendants().find(|n| is(*n, "graphicData")) else { return };
        let uri = attr(data, "uri").unwrap_or_default();
        // An anchored picture or chart floats where its anchor says, not in the line.
        let float = |pic: Pic| -> Option<Floating> {
            if !is(holder, "anchor") {
                return None;
            }
            let place = |name: &str| -> Option<(Place, bool)> {
                let p = child(holder, name)?;
                let page = matches!(attr(p, "relativeFrom"), Some("page"));
                let rel_para = matches!(attr(p, "relativeFrom"), Some("paragraph" | "line"));
                if let Some(a) = child(p, "align").and_then(|a| a.text()) {
                    let k = match a.trim() {
                        "center" => 0.5,
                        "right" | "bottom" | "outside" => 1.0,
                        _ => 0.0,
                    };
                    return Some((Place::Align(k, page), rel_para));
                }
                let off = child(p, "posOffset")
                    .and_then(|o| o.text())
                    .and_then(|t| t.trim().parse::<f64>().ok())
                    .map_or(0.0, |v| fin(v / 12_700.0, 0.0).clamp(-14_400.0, 14_400.0));
                Some((Place::Offset(off, page), rel_para))
            };
            let (x, _) = place("positionH").unwrap_or((Place::Align(0.0, false), false));
            let (y, rel_para) = place("positionV").unwrap_or((Place::Offset(0.0, false), true));
            let wrap = if holder.children().any(|c| is(c, "wrapSquare") || is(c, "wrapTight") || is(c, "wrapThrough")) {
                Wrap::Square
            } else if holder.children().any(|c| is(c, "wrapTopAndBottom")) {
                Wrap::TopBottom
            } else {
                Wrap::None
            };
            let gap = attr_f64(holder, "distL").map_or(9.0, |v| fin(v / 12_700.0, 9.0).clamp(0.0, 200.0));
            let gap_below = attr_f64(holder, "distB").map_or(0.0, |v| fin(v / 12_700.0, 0.0).clamp(0.0, 200.0));
            let (y, y_offset) = match (rel_para, y) {
                (true, Place::Offset(o, _)) => (None, o),
                (true, Place::Align(..)) => (None, 0.0),
                (false, p) => (Some(p), 0.0),
            };
            Some(Floating { pic, w, h, x, y, y_offset, wrap, gap, gap_below })
        };
        if let Some(blip) = data.descendants().find(|n| is(*n, "blip")) {
            let r = rel_attr(blip, "embed").and_then(|id| self.image(rels, id));
            if r.is_none() && rel_attr(blip, "link").is_some() {
                self.notes.add("linked (not embedded) images, drawn as empty boxes");
            }
            // A crop (`a:srcRect`, thousandths of a percent of each side) keeps part of the image.
            let crop = data
                .descendants()
                .find(|n| is(*n, "srcRect"))
                .map(|s| ["l", "t", "r", "b"].map(|k| attr_f64(s, k).map_or(0.0, |v| fin(v / 100_000.0, 0.0).clamp(-1.0, 0.99))));
            let crop = crop.filter(|c| c.iter().any(|v| v.abs() > 1e-4) && c[0] + c[2] < 0.99 && c[1] + c[3] < 0.99);
            match (r, crop) {
                (Some(r), Some(crop)) => match float(Pic::Cropped(r, crop)) {
                    Some(f) => para.floats.push(f),
                    None => para.items.push(Inline::Cropped(r, crop, w, h)),
                },
                _ => match float(Pic::Image(r)) {
                    Some(f) => para.floats.push(f),
                    None => para.items.push(Inline::Image(r, w, h)),
                },
            }
            return;
        }
        // Shapes, text boxes, groups and drawing canvases are drawn where they are, their text
        // inside them.
        let parts = self.shape_parts(data, rels, (w, h), depth);
        if !parts.is_empty() {
            let parts = std::sync::Arc::new(parts);
            match float(Pic::Shapes(parts.clone())) {
                Some(f) => para.floats.push(f),
                None => para.items.push(Inline::Shapes(parts, w, h)),
            }
            return;
        }
        let boxes = outermost_boxes(data);
        if !boxes.is_empty() {
            self.notes.add("text boxes and shapes, their text placed in the flow");
            for b in boxes {
                self.blocks(b, rels, depth + 1, extra);
            }
            return;
        }
        if uri.ends_with("/chart") || uri.contains("chart") {
            let part = data.descendants().find(|n| is(*n, "chart")).and_then(|c| rel_attr(c, "id")).and_then(|id| rels.target(id)).map(str::to_owned);
            let accents = self.accents.clone();
            if let Some(c) = part.and_then(|p| super::chart::read(self.zip, &p, &accents, self.notes)) {
                let c = std::sync::Arc::new(c);
                match float(Pic::Chart(c.clone())) {
                    Some(f) => para.floats.push(f),
                    None => para.items.push(Inline::Chart(c, w, h)),
                }
                return;
            }
            self.notes.add("charts, drawn as empty boxes");
        } else if uri.contains("diagram") {
            let scheme = self.scheme.clone();
            if let Some(shapes) = super::xlsx_drawing::diagram(self.zip, rels, data, (w * 12_700.0, h * 12_700.0), &scheme, self.notes) {
                let parts: Vec<ShapePart> = shapes
                    .into_iter()
                    .map(|(rect, g)| ShapePart {
                        rect,
                        geom: Some(g),
                        image: None,
                        paras: Vec::new(),
                        inset: [0.0; 4],
                        anchor: 0.0,
                        rot: 0.0,
                        own_text: true,
                        vert: 0,
                    })
                    .collect();
                let parts = std::sync::Arc::new(parts);
                match float(Pic::Shapes(parts.clone())) {
                    Some(f) => para.floats.push(f),
                    None => para.items.push(Inline::Shapes(parts, w, h)),
                }
                return;
            }
            self.notes.add("SmartArt diagrams without a saved drawing, drawn as empty boxes");
        } else if uri.contains("wordprocessingShape") || uri.contains("wordprocessingGroup") || uri.contains("wordprocessingCanvas") {
            // A shape without text or picture: decoration the importer doesn't draw.
            self.notes.add("drawing shapes left out");
            return;
        } else {
            self.notes.add("embedded objects, drawn as empty boxes");
        }
        para.items.push(Inline::Image(None, w, h));
    }

    /// The shapes of a DrawingML shape, group or canvas (`wps:wsp`, `wpg:wgp`, `wpc:wpc`) in a
    /// drawing `size` points big, each placed by its fraction of the drawing.
    fn shape_parts(&mut self, data: Node<'_, '_>, rels: &Rels, size: (f64, f64), depth: usize) -> Vec<ShapePart> {
        let mut out = Vec::new();
        let Some(top) = data.children().find(|c| is(*c, "wsp") || is(*c, "wgp") || is(*c, "wpc")) else { return out };
        let (w_emu, h_emu) = (size.0 * 12_700.0, size.1 * 12_700.0);
        if w_emu <= 0.0 || h_emu <= 0.0 {
            return out;
        }
        if is(top, "wsp") {
            self.shape_part(top, rels, [0.0, 0.0, 1.0, 1.0], depth, &mut out);
            return out;
        }
        // A group maps its children's coordinates (`chOff`, `chExt`) onto its own box; a canvas
        // places them in EMU from its corner.
        let frame = if is(top, "wgp") { group_frame(child(top, "grpSpPr")) } else { None };
        let map = frame.unwrap_or((0.0, 0.0, w_emu, h_emu));
        self.group_parts(top, rels, ([0.0, 0.0, 1.0, 1.0], map), depth, &mut out);
        out
    }

    /// The parts of a group's children: `into` is the group's box (fractions of the drawing)
    /// and the child coordinates that fill it (offset and extent).
    fn group_parts(
        &mut self,
        g: Node<'_, '_>,
        rels: &Rels,
        (into, (ox, oy, cw, ch)): ([f64; 4], (f64, f64, f64, f64)),
        depth: usize,
        out: &mut Vec<ShapePart>,
    ) {
        if depth > MAX_DEPTH || out.len() > 2_000 {
            return;
        }
        let place = |n: Node<'_, '_>| -> Option<[f64; 4]> {
            let x = n.descendants().find(|d| is(*d, "xfrm"))?;
            let off = child(x, "off")?;
            let ext = child(x, "ext")?;
            let v = |e: Node<'_, '_>, k: &str| attr_f64(e, k).filter(|v| v.is_finite()).unwrap_or(0.0);
            let (cw, ch) = (cw.max(1.0), ch.max(1.0));
            let fx = (v(off, "x") - ox) / cw;
            let fy = (v(off, "y") - oy) / ch;
            Some([into[0] + fx * into[2], into[1] + fy * into[3], v(ext, "cx") / cw * into[2], v(ext, "cy") / ch * into[3]])
        };
        for c in g.children().filter(Node::is_element) {
            match c.tag_name().name() {
                "wsp" => {
                    if let Some(r) = place(c) {
                        self.shape_part(c, rels, r, depth, out);
                    }
                }
                "pic" => {
                    if let Some(r) = place(c) {
                        let img = c.descendants().find(|n| is(*n, "blip")).and_then(|b| rel_attr(b, "embed")).and_then(|id| self.image(rels, id));
                        out.push(ShapePart {
                            rect: r,
                            geom: None,
                            image: Some(img),
                            paras: Vec::new(),
                            inset: [0.0; 4],
                            anchor: 0.0,
                            rot: 0.0,
                            own_text: false,
                            vert: 0,
                        });
                    }
                }
                "grpSp" => {
                    if let (Some(r), Some(f)) = (place(child(c, "grpSpPr").unwrap_or(c)), group_frame(child(c, "grpSpPr"))) {
                        self.group_parts(c, rels, (r, f), depth + 1, out);
                    }
                }
                _ => {}
            }
        }
    }

    /// One `wps:wsp`: its geometry and the paragraphs of its text box.
    fn shape_part(&mut self, sp: Node<'_, '_>, rels: &Rels, rect: [f64; 4], depth: usize, out: &mut Vec<ShapePart>) {
        let scheme = self.scheme.clone();
        let geom = super::xlsx_drawing::shape(sp, &scheme, self.notes);
        let mut blocks = Vec::new();
        if let Some(content) = sp.descendants().find(|n| is(*n, "txbxContent")) {
            self.blocks(content, rels, depth + 1, &mut blocks);
        }
        let paras: Vec<Para> = blocks
            .into_iter()
            .flat_map(|b| match b {
                Block::Para(p) => vec![p],
                Block::Table(t) => t.rows.into_iter().flat_map(|r| r.cells.into_iter().flat_map(|c| c.paras)).collect(),
                Block::PageBreak | Block::ColumnBreak => Vec::new(),
            })
            .collect();
        let body = child(sp, "bodyPr");
        let ins = |k: &str, d: f64| body.and_then(|b| attr_f64(b, k)).map_or(d, |v| fin(v / 12_700.0, d).clamp(0.0, 200.0));
        let anchor = match body.and_then(|b| attr(b, "anchor")) {
            Some("ctr") => 0.5,
            Some("b") => 1.0,
            _ => 0.0,
        };
        let rot = xml::path(sp, &["spPr", "xfrm"]).and_then(|x| attr_f64(x, "rot")).map_or(0.0, |r| fin(r / 60_000.0, 0.0) % 360.0);
        out.push(ShapePart {
            rect,
            geom: Some(geom),
            image: None,
            paras,
            inset: [ins("lIns", 7.2), ins("tIns", 3.6), ins("rIns", 7.2), ins("bIns", 3.6)],
            anchor,
            rot,
            own_text: false,
            vert: match body.and_then(|b| attr(b, "vert")) {
                Some("vert" | "eaVert" | "wordArtVertRtl") => -1,
                Some("vert270") => 1,
                _ => 0,
            },
        });
    }

    /// Legacy VML content (`w:pict`, `w:object`): an image preview or a text box.
    fn vml(&mut self, v: Node<'_, '_>, rels: &Rels, para: &mut Para, extra: &mut Vec<Block>, depth: usize) {
        if let Some(img) = v.descendants().find(|n| is(*n, "imagedata")) {
            let size = v.descendants().find(|n| attr(*n, "style").is_some()).and_then(|n| attr(n, "style")).map(vml_size).unwrap_or((72.0, 72.0));
            let r = rel_attr(img, "id").and_then(|id| self.image(rels, id));
            para.items.push(Inline::Image(r, size.0, size.1));
            return;
        }
        let boxes = outermost_boxes(v);
        for b in boxes {
            // The box's shape: a rectangle, rounded rectangle or oval with a `v:textbox`.
            let Some(shape) = b
                .ancestors()
                .skip(1)
                .take_while(|a| *a != v.parent().unwrap_or(v))
                .find(|a| ["shape", "rect", "roundrect", "oval"].iter().any(|k| is(*a, k)))
            else {
                self.notes.add("text boxes and shapes, their text placed in the flow");
                self.blocks(b, rels, depth + 1, extra);
                continue;
            };
            let style = attr(shape, "style").unwrap_or_default();
            let (w, h) = vml_size(style);
            let prop =
                |k: &str| style.split(';').find_map(|kv| kv.split_once(':').filter(|(key, _)| key.trim() == k).map(|(_, v)| v.trim().to_owned()));
            let pt = |k: &str| prop(k).and_then(|v| vml_length(&v));
            let fill = match (attr(shape, "filled"), attr(shape, "fillcolor")) {
                (Some("f" | "false"), _) => None,
                (_, Some(c)) => vml_color(c),
                _ => Some([1.0; 3]),
            };
            let line = match attr(shape, "stroked") {
                Some("f" | "false") => None,
                _ => Some((
                    attr(shape, "strokeweight").and_then(vml_length).unwrap_or(0.75),
                    attr(shape, "strokecolor").and_then(vml_color).unwrap_or([0.0; 3]),
                )),
            };
            let prst = match shape.tag_name().name() {
                "roundrect" => "roundRect",
                "oval" => "ellipse",
                _ => "rect",
            };
            let mut blocks = Vec::new();
            self.blocks(b, rels, depth + 1, &mut blocks);
            let paras: Vec<Para> = blocks
                .into_iter()
                .flat_map(|b| match b {
                    Block::Para(p) => vec![p],
                    Block::Table(t) => t.rows.into_iter().flat_map(|r| r.cells.into_iter().flat_map(|c| c.paras)).collect(),
                    Block::PageBreak | Block::ColumnBreak => Vec::new(),
                })
                .collect();
            let inset = child(shape, "textbox").and_then(|t| attr(t, "inset")).map_or([7.2, 3.6, 7.2, 3.6], |s| {
                let v: Vec<Option<f64>> = s.split(',').map(vml_length).collect();
                let d = [7.2, 3.6, 7.2, 3.6];
                [0, 1, 2, 3].map(|i| v.get(i).copied().flatten().unwrap_or(d[i]))
            });
            let part = ShapePart {
                rect: [0.0, 0.0, 1.0, 1.0],
                geom: Some(super::xlsx_drawing::Shape::plain(prst, fill, line)),
                image: None,
                paras,
                inset,
                anchor: 0.0,
                rot: prop("rotation")
                    .and_then(|r| r.trim_end_matches("deg").trim().parse::<f64>().ok())
                    .filter(|r| r.is_finite())
                    .map_or(0.0, |r| r % 360.0),
                own_text: false,
                vert: 0,
            };
            let parts = Arc::new(vec![part]);
            if prop("position").as_deref() == Some("absolute") {
                let wrap = match v.descendants().find(|n| is(*n, "wrap")).and_then(|n| attr(n, "type")) {
                    Some("topAndBottom") => Wrap::TopBottom,
                    Some("square" | "tight" | "through") => Wrap::Square,
                    _ => Wrap::None,
                };
                para.floats.push(Floating {
                    pic: Pic::Shapes(parts),
                    w,
                    h,
                    x: Place::Offset(pt("margin-left").or_else(|| pt("left")).unwrap_or(0.0), false),
                    y: None,
                    y_offset: pt("margin-top").or_else(|| pt("top")).unwrap_or(0.0),
                    wrap,
                    gap: 9.0,
                    gap_below: 0.0,
                });
            } else {
                para.items.push(Inline::Shapes(parts, w, h));
            }
        }
    }

    fn table(&mut self, t: Node<'_, '_>, rels: &Rels, depth: usize) -> Table {
        let tpr = child(t, "tblPr");
        let style_id = tpr.and_then(|p| child(p, "tblStyle")).and_then(|s| attr(s, "val"));
        let mut borders = self.styles.table_borders(style_id);
        if let Some(b) = tpr.and_then(|p| child(p, "tblBorders")) {
            borders.merge(&Borders::parse(b));
        }
        let (whole, parts) = self.styles.table_parts(style_id);
        let look = Look::parse(tpr.and_then(|p| child(p, "tblLook")));
        let band =
            tpr.and_then(|p| child(p, "tblStyleRowBandSize")).and_then(|b| attr_f64(b, "val")).map_or(1, |v| fin(v, 1.0).clamp(1.0, 100.0) as usize);
        let cols: Vec<f64> = xml::path(t, &["tblGrid"])
            .map(|g| children(g, "gridCol").map(|c| attr_f64(c, "w").map_or(0.0, |w| twips(w).max(0.0))).collect())
            .unwrap_or_default();
        let indent = tpr.and_then(|p| child(p, "tblInd")).and_then(|i| attr_f64(i, "w")).map_or(0.0, twips);
        let center = tpr.and_then(|p| child(p, "jc")).and_then(|j| attr(j, "val")) == Some("center");
        let mar = tpr.and_then(|p| child(p, "tblCellMar"));
        let side = |names: [&str; 2]| mar.and_then(|m| names.iter().find_map(|n| child(m, n))).and_then(|s| attr_f64(s, "w")).map(twips);
        let pad = (side(["left", "start"]).unwrap_or(5.4), side(["right", "end"]).unwrap_or(5.4));
        let float = tpr.and_then(|p| child(p, "tblpPr")).and_then(float_of);
        let ncols = cols.len().max(1);
        let row_nodes: Vec<Node<'_, '_>> = children(t, "tr").collect();
        let nrows = row_nodes.len();
        let saved_layer = self.cell_layer.take();
        let mut rows = Vec::new();
        for (ri, tr) in row_nodes.into_iter().enumerate() {
            let height = xml::path(tr, &["trPr", "trHeight"]).and_then(|h| Some((twips(attr_f64(h, "val")?), attr(h, "hRule") == Some("exact"))));
            let mut cells = Vec::new();
            let mut g = 0usize;
            for tc in tr.children().filter(|c| is(*c, "tc") || is(*c, "sdt")) {
                let tc = if is(tc, "sdt") {
                    match xml::path(tc, &["sdtContent", "tc"]) {
                        Some(c) => c,
                        None => continue,
                    }
                } else {
                    tc
                };
                let pr = child(tc, "tcPr");
                let span = pr.and_then(|p| child(p, "gridSpan")).and_then(|g| attr_f64(g, "val")).map_or(1, |v| v.clamp(1.0, 64.0) as usize);
                let vmerge = pr.and_then(|p| child(p, "vMerge")).map(|v| attr(v, "val") == Some("restart"));
                let valign = match pr.and_then(|p| child(p, "vAlign")).and_then(|v| attr(v, "val")) {
                    Some("center") => 0.5,
                    Some("bottom") => 1.0,
                    _ => 0.0,
                };
                // The table style's formatting for where this cell is, under the cell's own.
                let cond = cell_cond(&whole, &parts, look, band, (ri, nrows), (g, g.saturating_add(span), ncols));
                g = g.saturating_add(span);
                let shade = match pr.and_then(|p| child(p, "shd")) {
                    Some(s) => attr(s, "fill").filter(|f| *f != "auto").and_then(hex_color),
                    None => cond.shade,
                };
                let mut cb = cond.borders.unwrap_or_default();
                if let Some(own) = pr.and_then(|p| child(p, "tcBorders")) {
                    cb.merge(&Borders::parse(own));
                }
                self.cell_layer = Some((cond.p, cond.r));
                let mut blocks = Vec::new();
                self.blocks(tc, rels, depth + 1, &mut blocks);
                self.cell_layer = None;
                let paras = blocks
                    .into_iter()
                    .flat_map(|b| match b {
                        Block::Para(p) => vec![p],
                        Block::Table(t) => {
                            self.notes.add("nested tables, their text placed in the cell");
                            t.rows.into_iter().flat_map(|r| r.cells.into_iter().flat_map(|c| c.paras)).collect()
                        }
                        Block::PageBreak | Block::ColumnBreak => Vec::new(),
                    })
                    .collect();
                let turn = match pr.and_then(|p| child(p, "textDirection")).and_then(|v| attr(v, "val")) {
                    Some("btLr") => 1,
                    Some("tbRl" | "tbRlV") => -1,
                    _ => 0,
                };
                cells.push(Cell { span, paras, shade, borders: cb, vmerge, valign, turn });
            }
            let header = xml::path(tr, &["trPr", "tblHeader"]).is_some_and(|h| attr(h, "val").is_none_or(|v| flag(Some(v))));
            rows.push(Row { cells, height, header });
        }
        self.cell_layer = saved_layer;
        Table { cols, rows, borders, indent, center, pad, float }
    }

    /// Read block content (a body, a cell, a header, a text box).
    fn blocks(&mut self, n: Node<'_, '_>, rels: &Rels, depth: usize, out: &mut Vec<Block>) {
        if depth > MAX_DEPTH {
            self.notes.add("content beyond the nesting limit, left out");
            return;
        }
        for c in n.children().filter(Node::is_element) {
            match c.tag_name().name() {
                "p" => self.paragraph(c, rels, depth, out),
                "tbl" => {
                    let t = self.table(c, rels, depth);
                    out.push(Block::Table(t));
                }
                "sdt" => {
                    if let Some(content) = child(c, "sdtContent") {
                        self.blocks(content, rels, depth + 1, out);
                    }
                }
                "customXml" | "ins" | "moveTo" => self.blocks(c, rels, depth + 1, out),
                _ => {}
            }
        }
        attach_drop_caps(out);
    }

    /// The paragraphs of a header or footer part.
    fn header(&mut self, rels: &Rels, id: Option<&str>) -> Vec<Block> {
        let Some(part) = id.and_then(|id| rels.target(id)).map(str::to_owned) else { return Vec::new() };
        let Ok(Some(text)) = self.zip.text(&part) else { return Vec::new() };
        let Ok(d) = xml::parse(&text) else { return Vec::new() };
        let Ok(hrels) = Rels::load(self.zip, &part) else { return Vec::new() };
        self.fields.clear();
        let mut out = Vec::new();
        self.blocks(d.root_element(), &hrels, 0, &mut out);
        out
    }

    /// Footnote or endnote paragraphs, keyed by id.
    fn note_bodies(&mut self, rels: &Rels, ty: &str, tag: &str) -> HashMap<String, Vec<Block>> {
        let mut out = HashMap::new();
        let Some(part) = rels.of_type(ty).map(str::to_owned) else { return out };
        let Ok(Some(text)) = self.zip.text(&part) else { return out };
        let Ok(d) = xml::parse(&text) else { return out };
        let Ok(nrels) = Rels::load(self.zip, &part) else { return out };
        self.fields.clear();
        for n in children(d.root_element(), tag) {
            if let Some(id) = attr(n, "id") {
                let mut blocks = Vec::new();
                self.blocks(n, &nrels, 0, &mut blocks);
                out.insert(id.to_owned(), blocks);
            }
        }
        out
    }
}

/// Which parts of its table style a table shows (`w:tblLook`).
#[derive(Clone, Copy, Debug)]
struct Look {
    first_row: bool,
    last_row: bool,
    first_col: bool,
    last_col: bool,
    h_band: bool,
    v_band: bool,
}

impl Look {
    fn parse(n: Option<Node<'_, '_>>) -> Look {
        // Word's default look: header row, first column and banded rows.
        let bits = n.and_then(|n| attr(n, "val")).and_then(|v| u32::from_str_radix(v, 16).ok()).unwrap_or(0x04A0);
        let on = |name: &str, bit: u32| n.and_then(|n| attr(n, name)).map_or(bits & bit != 0, |v| flag(Some(v)));
        Look {
            first_row: on("firstRow", 0x20),
            last_row: on("lastRow", 0x40),
            first_col: on("firstColumn", 0x80),
            last_col: on("lastColumn", 0x100),
            h_band: !on("noHBand", 0x200),
            v_band: !on("noVBand", 0x400),
        }
    }
}

/// A table style's formatting for the cell at row `ri` of `nrows` spanning grid columns
/// `g0..g1` of `ncols`: the whole table's, then the bands', the first and last columns', the
/// header and total rows' and the corners', each over the one before (ECMA-376 §17.7.6).
fn cell_cond(
    whole: &Cond,
    parts: &[(String, Cond)],
    look: Look,
    band: usize,
    (ri, nrows): (usize, usize),
    (g0, g1, ncols): (usize, usize, usize),
) -> Cond {
    let mut c = whole.clone();
    let first_row = look.first_row && ri == 0;
    let last_row = look.last_row && nrows > 1 && ri + 1 == nrows;
    let first_col = look.first_col && g0 == 0;
    let last_col = look.last_col && g1 >= ncols;
    let body_row = ri.saturating_sub(usize::from(look.first_row)) / band.max(1);
    let body_col = g0.saturating_sub(usize::from(look.first_col));
    let mut apply = |ty: &str| {
        if let Some((_, p)) = parts.iter().find(|(t, _)| t == ty) {
            c.merge(p);
        }
    };
    if look.v_band && !first_col && !last_col {
        apply(if body_col.is_multiple_of(2) { "band1Vert" } else { "band2Vert" });
    }
    if look.h_band && !first_row && !last_row {
        apply(if body_row.is_multiple_of(2) { "band1Horz" } else { "band2Horz" });
    }
    if first_col {
        apply("firstCol");
    }
    if last_col {
        apply("lastCol");
    }
    if first_row {
        apply("firstRow");
    }
    if last_row {
        apply("lastRow");
    }
    match (first_row, last_row, first_col, last_col) {
        (true, _, true, _) => apply("nwCell"),
        (true, _, _, true) => apply("neCell"),
        (_, true, true, _) => apply("swCell"),
        (_, true, _, true) => apply("seCell"),
        _ => {}
    }
    c
}

/// Where a floating table is placed, when that is against the page or its margins. A table
/// floating in the text (`vertAnchor="text"`) stays in the flow.
fn float_of(n: Node<'_, '_>) -> Option<Float> {
    let (page, text) = match attr(n, "vertAnchor") {
        Some("page") => (true, false),
        Some("margin") => (false, false),
        // An aligned table without an anchor is placed against the margins.
        None if attr(n, "tblpYSpec").is_some() => (false, false),
        _ => (false, true),
    };
    let x = match attr(n, "tblpXSpec") {
        Some("left" | "inside") => Some(FloatY::Align(0.0)),
        Some("center") => Some(FloatY::Align(0.5)),
        Some("right" | "outside") => Some(FloatY::Align(1.0)),
        _ => attr_f64(n, "tblpX").map(|v| FloatY::Offset(fin(twips(v), 0.0).clamp(-14_400.0, 14_400.0))),
    }
    .map(|x| (x, attr(n, "horzAnchor") == Some("page")));
    let gap = |k: &str| attr_f64(n, k).map_or(0.0, |v| fin(twips(v), 0.0).clamp(0.0, 720.0));
    let gaps = [gap("leftFromText"), gap("rightFromText"), gap("bottomFromText")];
    let y = match attr(n, "tblpYSpec") {
        Some("top" | "inside") => FloatY::Align(0.0),
        Some("center") => FloatY::Align(0.5),
        Some("bottom" | "outside") => FloatY::Align(1.0),
        _ => FloatY::Offset(fin(attr_f64(n, "tblpY").map_or(0.0, twips), 0.0).clamp(-14_400.0, 14_400.0)),
    };
    Some(Float { y, page, text, x, gaps })
}

/// A group's box in its parent's coordinates' terms: the child offset and extent its children
/// are placed in (`a:chOff`, `a:chExt`).
fn group_frame(gpr: Option<Node<'_, '_>>) -> Option<(f64, f64, f64, f64)> {
    let x = child(gpr?, "xfrm")?;
    let v = |n: &str, k: &str| child(x, n).and_then(|e| attr_f64(e, k)).filter(|v| v.is_finite());
    Some((v("chOff", "x")?, v("chOff", "y")?, v("chExt", "cx")?.max(1.0), v("chExt", "cy")?.max(1.0)))
}

/// The text boxes in `n` that aren't inside another of its text boxes: an inner box is read with
/// the box holding it, so collecting it here too would place it once per level of nesting.
fn outermost_boxes<'a, 'i>(n: Node<'a, 'i>) -> Vec<Node<'a, 'i>> {
    n.descendants().filter(|b| is(*b, "txbxContent") && !b.ancestors().skip(1).take_while(|a| *a != n).any(|a| is(a, "txbxContent"))).collect()
}

/// A VML length (`12pt`, `0.5in`, `1cm`, `20px`, or EMU without a unit) in points.
fn vml_length(v: &str) -> Option<f64> {
    let v = v.trim();
    let at = v.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(v.len());
    let x: f64 = v.get(..at)?.trim().parse().ok().filter(|x: &f64| x.is_finite())?;
    let pt = match v.get(at..).unwrap_or("") {
        "pt" => x,
        "in" => x * 72.0,
        "cm" => x * 72.0 / 2.54,
        "mm" => x * 72.0 / 25.4,
        "px" => x * 0.75,
        "" => x / 12_700.0,
        _ => return None,
    };
    Some(pt.clamp(-14_400.0, 14_400.0))
}

/// A VML colour: `#RRGGBB`, `#RGB` or a few names (with an optional ` [index]`).
fn vml_color(v: &str) -> Option<[f64; 3]> {
    let v = v.split_whitespace().next()?.trim();
    if let Some(hex) = v.strip_prefix('#') {
        return match hex.len() {
            3 => hex_color(&hex.chars().flat_map(|c| [c, c]).collect::<String>()),
            _ => hex_color(hex),
        };
    }
    Some(match v.to_ascii_lowercase().as_str() {
        "black" => [0.0; 3],
        "white" => [1.0; 3],
        "red" => [1.0, 0.0, 0.0],
        "green" => [0.0, 0.5, 0.0],
        "blue" => [0.0, 0.0, 1.0],
        "yellow" => [1.0, 1.0, 0.0],
        "gray" | "grey" => [0.5; 3],
        _ => return None,
    })
}

fn vml_size(style: &str) -> (f64, f64) {
    let get = |key: &str| {
        style.split(';').find_map(|kv| {
            let (k, v) = kv.split_once(':')?;
            if k.trim() != key {
                return None;
            }
            let v = v.trim();
            let (num, unit) = v.find(|c: char| c.is_ascii_alphabetic()).map_or((v, "px"), |i| (v.get(..i).unwrap_or(v), v.get(i..).unwrap_or("")));
            let x: f64 = num.trim().parse().ok().filter(|x: &f64| x.is_finite())?;
            Some(match unit {
                "pt" => x,
                "in" => x * 72.0,
                "cm" => x * 72.0 / 2.54,
                "mm" => x * 72.0 / 25.4,
                _ => x * 0.75,
            })
        })
    };
    (get("width").unwrap_or(72.0).clamp(1.0, 14_400.0), get("height").unwrap_or(72.0).clamp(1.0, 14_400.0))
}

impl Cx<'_, '_> {
    /// The field shown live a field instruction names, by its first word: page numbers and
    /// counts, and style references. `PAGEREF` (a table of contents' or cross-reference's page
    /// number) and `PAGES…` look alike but are not page fields, and keep their cached result.
    fn live_field(&self, instr: &str) -> Option<Field> {
        let mut words = instr.split_whitespace();
        let name = words.next()?;
        if name.eq_ignore_ascii_case("PAGE") {
            // A numbering format switch: `\* roman`, `\* ALPHABETIC`…
            let fmt = instr.split_once("\\*").and_then(|(_, rest)| rest.split_whitespace().next()).and_then(|f| match f {
                "roman" => Some("lowerRoman"),
                "ROMAN" | "Roman" => Some("upperRoman"),
                "alphabetic" => Some("lowerLetter"),
                "ALPHABETIC" | "Alphabetic" => Some("upperLetter"),
                "Arabic" | "arabic" => Some("decimal"),
                _ => None,
            });
            Some(Field::Page(fmt))
        } else if name.eq_ignore_ascii_case("NUMPAGES") {
            Some(Field::Pages)
        } else if name.eq_ignore_ascii_case("SECTIONPAGES") {
            Some(Field::SectionPages)
        } else if name.eq_ignore_ascii_case("STYLEREF") {
            // The style by name (quoted when it has spaces) or id.
            let rest = instr.trim_start().get(name.len()..)?.trim_start();
            let style = match rest.strip_prefix('"') {
                Some(q) => q.split('"').next()?,
                None => rest.split_whitespace().next()?,
            };
            let id = self
                .styles
                .by_name
                .get(&style.to_lowercase())
                .cloned()
                .or_else(|| self.styles.by_id.contains_key(style).then(|| style.to_owned()))?;
            Some(Field::StyleRef(Arc::from(id)))
        } else {
            None
        }
    }
}

/// A section's page borders on a page: each side at its distance from the page edge, or out from
/// the text area.
fn draw_page_borders(page: &mut super::canvas::Page, sec: &Section, b: &PageBorders) {
    let [top, left, bottom, right] = b.sides;
    let space = |r: Option<Rule>| r.map_or(0.0, |r| r.space + r.width / 2.0);
    let (x1, x2, y1, y2) = if b.from_page {
        (space(left), sec.w - space(right), space(bottom), sec.h - space(top))
    } else {
        let [mt, mr, mb, ml] = sec.margins;
        (ml - space(left), sec.w - mr + space(right), mb - space(bottom), sec.h - mt + space(top))
    };
    for (r, (ax, ay, bx, by)) in [(top, (x1, y2, x2, y2)), (bottom, (x1, y1, x2, y1)), (left, (x1, y1, x1, y2)), (right, (x2, y1, x2, y2))] {
        let Some(r) = r else { continue };
        if r.double {
            let w = r.width / 3.0;
            let (dx, dy) = if ay == by { (0.0, w) } else { (w, 0.0) };
            page.line(ax - dx, ay - dy, bx - dx, by - dy, w, r.color);
            page.line(ax + dx, ay + dy, bx + dx, by + dy, w, r.color);
        } else {
            flow::stroke(page, ax, ay, bx, by, r.width, r.color, r.dash);
        }
    }
}

/// The width a laid-out line's text reaches.
fn line_ink(l: &Laid, line: &flow::Line) -> f64 {
    line.items
        .iter()
        .map(|(x, it)| match it {
            flow::Placed::Text(t, si) => x + l.styles.get(*si).map_or(0.0, |s| s.width(t)),
            flow::Placed::Image(_, w, _) => x + w,
        })
        .fold(0.0, f64::max)
}

/// Turn each drop cap paragraph into a picture floating at the start of the paragraph after it,
/// as tall as the lines it spans, with that paragraph's text wrapping beside it.
fn attach_drop_caps(blocks: &mut Vec<Block>) {
    let mut i = 0usize;
    while i + 1 < blocks.len() {
        let cap = match (&blocks[i], &blocks[i + 1]) {
            (Block::Para(p), Block::Para(_)) => p.drop_cap,
            _ => None,
        };
        let Some((margin, lines, gap)) = cap else {
            i += 1;
            continue;
        };
        let Block::Para(mut cap_para) = blocks.remove(i) else { continue };
        cap_para.drop_cap = None;
        cap_para.before = 0.0;
        cap_para.after = 0.0;
        cap_para.left = 0.0;
        cap_para.first = 0.0;
        let laid = flow::layout(&cap_para, 1000.0, (1, 1));
        let w = laid
            .lines
            .iter()
            .flat_map(|l| l.items.iter())
            .map(|(x, it)| match it {
                flow::Placed::Text(t, si) => x + laid.styles.get(*si).map_or(0.0, |s| s.width(t)),
                flow::Placed::Image(_, w, _) => x + w,
            })
            .fold(0.0, f64::max)
            .max(4.0)
            + 1.0;
        if let Some(Block::Para(next)) = blocks.get_mut(i) {
            let line = flow::layout(next, 400.0, (1, 1)).lines.first().map_or(next.base.size * 1.2, |l| l.height);
            // As tall as the lines it spans: the letter may reach a little below, as in Word.
            let h = line * f64::from(lines) - 1.0;
            let part = ShapePart {
                rect: [0.0, 0.0, 1.0, 1.0],
                geom: None,
                image: None,
                paras: vec![cap_para],
                inset: [0.0; 4],
                anchor: 0.0,
                rot: 0.0,
                own_text: false,
                vert: 0,
            };
            next.floats.insert(
                0,
                Floating {
                    pic: Pic::Shapes(Arc::new(vec![part])),
                    w,
                    h,
                    x: Place::Offset(if margin { -w - gap } else { 0.0 }, false),
                    y: None,
                    y_offset: 0.0,
                    wrap: Wrap::Square,
                    gap: gap.max(1.0),
                    gap_below: 0.0,
                },
            );
        }
        i += 1;
    }
}

/// The bookmark a `REF` or `PAGEREF` field with `\h` links to.
fn ref_link(instr: &str) -> Option<LinkTarget> {
    let mut words = instr.split_whitespace();
    let name = words.next()?;
    if !(name.eq_ignore_ascii_case("REF") || name.eq_ignore_ascii_case("PAGEREF")) || !instr.contains("\\h") {
        return None;
    }
    let mark = words.next().filter(|w| !w.starts_with('\\'))?;
    Some(LinkTarget::Anchor(mark.trim_matches('"').to_owned()))
}

/// Today's date or time for the fields Word updates when it prints (`DATE`, `TIME`,
/// `PRINTDATE`), in the field's date format (`\@ "d MMMM yyyy"`) or Word's US default.
fn date_field(instr: &str) -> Option<String> {
    let name = instr.split_whitespace().next()?.to_ascii_uppercase();
    let default = match name.as_str() {
        "DATE" => "M/d/yyyy",
        "TIME" => "h:mm AM/PM",
        "PRINTDATE" => "M/d/yyyy h:mm:ss AM/PM",
        _ => return None,
    };
    let picture = instr.split_once("\\@").map(|(_, rest)| {
        let rest = rest.trim_start();
        match rest.strip_prefix('"') {
            Some(q) => q.split('"').next().unwrap_or_default(),
            None => rest.split_whitespace().next().unwrap_or_default(),
        }
    });
    Some(date_picture(&chrono::Local::now().naive_local(), picture.filter(|p| !p.is_empty()).unwrap_or(default)))
}

/// Format a date and time with a Word date picture: `d dd ddd dddd`, `M MM MMM MMMM`, `yy yyyy`,
/// `h hh H HH`, `m mm`, `s ss`, `AM/PM` (or `am/pm`), and literal text in single quotes.
fn date_picture(t: &chrono::NaiveDateTime, picture: &str) -> String {
    use chrono::{Datelike, Timelike};
    const MONTHS: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    const DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
    let chars: Vec<char> = picture.chars().take(256).collect();
    let twelve = picture.to_ascii_lowercase().contains("am/pm");
    let mut out = String::new();
    let mut i = 0usize;
    while let Some(&c) = chars.get(i) {
        let mut n = 1usize;
        while chars.get(i + n) == Some(&c) {
            n += 1;
        }
        let starts = |w: &str| chars.get(i..i + w.len()).is_some_and(|s| s.iter().copied().eq(w.chars()));
        match c {
            '\'' => {
                let lit: String = chars[i + 1..].iter().take_while(|q| **q != '\'').collect();
                i += lit.chars().count() + 2;
                out.push_str(&lit);
                continue;
            }
            'd' => out.push_str(&match n {
                1 => t.day().to_string(),
                2 => format!("{:02}", t.day()),
                3 => DAYS[t.weekday().num_days_from_monday() as usize][..3].to_owned(),
                _ => DAYS[t.weekday().num_days_from_monday() as usize].to_owned(),
            }),
            'M' => out.push_str(&match n {
                1 => t.month().to_string(),
                2 => format!("{:02}", t.month()),
                3 => MONTHS[t.month0() as usize][..3].to_owned(),
                _ => MONTHS[t.month0() as usize].to_owned(),
            }),
            'y' => out.push_str(&if n <= 2 { format!("{:02}", t.year().rem_euclid(100)) } else { t.year().to_string() }),
            'h' | 'H' => {
                let h = if c == 'h' || twelve { (t.hour() + 11) % 12 + 1 } else { t.hour() };
                out.push_str(&if n >= 2 { format!("{h:02}") } else { h.to_string() });
            }
            'm' => out.push_str(&if n >= 2 { format!("{:02}", t.minute()) } else { t.minute().to_string() }),
            's' => out.push_str(&if n >= 2 { format!("{:02}", t.second()) } else { t.second().to_string() }),
            'A' | 'a' if starts("AM/PM") || starts("am/pm") => {
                let pm = t.hour() >= 12;
                out.push_str(match (c == 'A', pm) {
                    (true, true) => "PM",
                    (true, false) => "AM",
                    (false, true) => "pm",
                    (false, false) => "am",
                });
                i += 5;
                continue;
            }
            _ => {
                for _ in 0..n {
                    out.push(c);
                }
            }
        }
        i += n;
    }
    out
}

/// A copy of header or footer blocks with the fields that change from page to page resolved to
/// text by `value` (fields it leaves are laid out as usual).
fn resolve_fields(blocks: &[&Block], value: &dyn Fn(&Field) -> Option<String>) -> Vec<Block> {
    let para = |p: &Para| resolve_para(p, value);
    blocks
        .iter()
        .map(|b| match b {
            Block::Para(p) => Block::Para(para(p)),
            Block::Table(t) => {
                let mut t = t.clone();
                for row in &mut t.rows {
                    for cell in &mut row.cells {
                        cell.paras = cell.paras.iter().map(para).collect();
                    }
                }
                Block::Table(t)
            }
            other => (*other).clone(),
        })
        .collect()
}

/// A copy of a paragraph with its fields resolved to text by `value` (see [`resolve_fields`]).
fn resolve_para(p: &Para, value: &dyn Fn(&Field) -> Option<String>) -> Para {
    Para {
        items: p
            .items
            .iter()
            .map(|it| match it {
                Inline::Field(f, s) => value(f).map_or_else(|| it.clone(), |t| Inline::Text(t, s.clone())),
                _ => it.clone(),
            })
            .collect(),
        ..p.clone()
    }
}

/// The text a style reference shows on page `page`: the first paragraph in the style on the
/// page, else the last one before it, else the first after it.
fn style_ref(heads: &[(usize, Arc<str>, String)], page: usize, style: &str) -> String {
    let of_style = || heads.iter().filter(|(_, s, _)| &**s == style);
    of_style()
        .find(|(p, _, _)| *p == page)
        .or_else(|| of_style().rfind(|(p, _, _)| *p < page))
        .or_else(|| of_style().find(|(p, _, _)| *p > page))
        .map(|(_, _, t)| t.clone())
        .unwrap_or_default()
}

// ── layout ──────────────────────────────────────────────────────────────────────────────────

struct Flow<'c> {
    canvas: &'c mut Canvas,
    sec: Section,
    /// Index of the page being filled, and the PDF y of the next line's top.
    page: usize,
    y: f64,
    at_top: bool,
    /// The section in force on each page.
    page_sections: Vec<usize>,
    section_index: usize,
    /// The previous paragraph's contextual-spacing style, and its space after (already added).
    prev_contextual: Option<(String, f64)>,
    /// The page number and count page fields show.
    fields: (usize, usize),
    /// The page was not started by the document or a section: space before the first paragraph
    /// on it is dropped, as Word does.
    soft_top: bool,
    /// Drawing in place on one page (a header, a footer, a floating table, a measurement):
    /// nothing starts a new page.
    fixed: bool,
    /// Paragraphs with a style placed so far (page, style id, text), for style references.
    heads: Vec<(usize, Arc<str>, String)>,
    /// Line numbering: the next number, and the page and section it counts on.
    line_no: (i64, usize, usize),
    /// The body's page numbering and counts, which its page fields show (`None` in headers,
    /// footers and notes, whose fields are resolved for each page before they are laid out).
    counts: Option<std::rc::Rc<PageCounts>>,
    /// The previous paragraph's borders and indents, when it may share a box with this one.
    prev_borders: Option<(ParaBorders, f64, f64)>,
    /// The column being filled, where this page's columns start, the lowest point they reach,
    /// and a balanced bottom for them (a section ending at a continuous break).
    col: usize,
    col_top: f64,
    col_low: f64,
    col_limit: Option<f64>,
    /// Footnote bodies by reference, the height of this page's footnote area, and the footnotes
    /// placed on each page (page, footnote).
    foot: std::rc::Rc<Vec<Vec<Block>>>,
    foot_h: f64,
    page_notes: Vec<(usize, usize)>,
    placed: std::collections::HashSet<usize>,
    /// A picture text wraps beside on this page: down to `bottom`, the text keeps clear of
    /// `left..right` (PDF x).
    wrap: Option<(f64, f64, f64)>,
}

/// Space above a page's footnotes, for their separator line.
const FOOT_SEP: f64 = 12.0;

impl<'c> Flow<'c> {
    /// A flow that draws on page `page` from PDF `y` down, without starting pages.
    fn fixed(canvas: &'c mut Canvas, sec: Section, page: usize, y: f64, fields: (usize, usize)) -> Flow<'c> {
        // Headers, footers and measurements span the whole text width.
        let sec = Section { cols: Vec::new(), ..sec };
        Flow {
            canvas,
            sec,
            page,
            y,
            at_top: false,
            page_sections: Vec::new(),
            section_index: 0,
            prev_contextual: None,
            fields,
            soft_top: false,
            fixed: true,
            heads: Vec::new(),
            prev_borders: None,
            col: 0,
            col_top: y,
            col_low: y,
            col_limit: None,
            foot: std::rc::Rc::new(Vec::new()),
            foot_h: 0.0,
            page_notes: Vec::new(),
            placed: std::collections::HashSet::new(),
            wrap: None,
            counts: None,
            line_no: (0, usize::MAX, usize::MAX),
        }
    }
}

/// The height `blocks` take when laid out in `sec`'s text width, drawn on a scratch page.
fn measure(sec: &Section, fields: (usize, usize), blocks: &[&Block]) -> f64 {
    let mut canvas = Canvas::default();
    let Ok(page) = canvas.new_page(sec.w, sec.h) else { return 0.0 };
    let mut fl = Flow::fixed(&mut canvas, sec.clone(), page, sec.h, fields);
    for (i, b) in blocks.iter().enumerate() {
        if fl.block(b, blocks.get(i + 1).copied()).is_err() {
            break;
        }
    }
    fin(sec.h - fl.y, 0.0).max(0.0)
}

impl Flow<'_> {
    fn top(&self) -> f64 {
        self.sec.h - self.sec.margins[0]
    }

    fn bottom(&self) -> f64 {
        // Content drawn in place may run past the margin rather than split.
        if self.fixed {
            -1.0e7
        } else {
            let foot = self.sec.margins[2] + self.foot_h;
            self.col_limit.map_or(foot, |l| l.max(foot))
        }
    }

    fn block(&mut self, b: &Block, next: Option<&Block>) -> Result<(), String> {
        match b {
            Block::Para(p) => self.para(p, next),
            Block::Table(t) => self.table(t),
            // Space before is dropped at the top of a page a manual break started, as at the top
            // of any page but the document's and a section's first.
            Block::PageBreak => {
                self.new_page()?;
                self.soft_top = true;
                Ok(())
            }
            Block::ColumnBreak => self.next_column(true),
        }
    }

    fn x0(&self) -> f64 {
        self.sec.margins[3] + self.sec.cols.get(self.col).map_or(0.0, |c| c.0)
    }

    fn width(&self) -> f64 {
        self.sec.cols.get(self.col).map_or(self.sec.w - self.sec.margins[1] - self.sec.margins[3], |c| c.1).max(36.0)
    }

    /// Start a page at a break (page, section, page break before).
    fn new_page(&mut self) -> Result<(), String> {
        self.soft_top = false;
        self.prev_borders = None;
        if self.fixed {
            // Carry on where we are; the next item is placed even if it doesn't fit.
            self.at_top = true;
            return Ok(());
        }
        self.close_columns();
        self.page = self.canvas.new_page(self.sec.w, self.sec.h)?;
        self.page_sections.push(self.section_index);
        self.y = self.top();
        self.at_top = true;
        self.col_top = self.y;
        self.col_low = self.y;
        self.col_limit = None;
        self.foot_h = 0.0;
        self.wrap = None;
        Ok(())
    }

    /// The text width and its left shift for a line at the current y, beside a wrapped picture.
    fn text_span(&self) -> (f64, f64) {
        let (x0, w) = (self.x0(), self.width());
        match self.wrap {
            Some((bottom, l, r)) if self.y > bottom + 0.5 => {
                // The wider side of the picture.
                let (left_w, right_w) = ((l - x0).max(0.0), (x0 + w - r).max(0.0));
                if left_w >= right_w { (0.0, left_w.max(36.0)) } else { (r - x0, right_w.max(36.0)) }
            }
            _ => (0.0, w),
        }
    }

    /// Draw a paragraph's floating pictures and make room for them: none for pictures in front
    /// of or behind the text, their height for top-and-bottom ones, a side for square ones.
    fn floats(&mut self, p: &Para) -> Result<(), String> {
        for f in &p.floats {
            if f.wrap == Wrap::TopBottom && f.y.is_none() && self.y - f.h < self.bottom() && !self.at_top {
                self.overflow()?;
            }
            let (x0, w) = (self.x0(), self.width());
            let x = match f.x {
                Place::Align(k, true) => (self.sec.w - f.w) * k,
                Place::Align(k, false) => x0 + (w - f.w) * k,
                Place::Offset(o, true) => o,
                Place::Offset(o, false) => x0 + o,
            };
            let top = match f.y {
                None => self.y - f.y_offset,
                Some(Place::Align(k, true)) => self.sec.h - (self.sec.h - f.h) * k,
                Some(Place::Align(k, false)) => self.top() - (self.top() - self.sec.margins[2] - f.h) * k,
                Some(Place::Offset(o, true)) => self.sec.h - o,
                Some(Place::Offset(o, false)) => self.top() - o,
            };
            if let Some(page) = self.canvas.page(self.page) {
                flow::draw_pic(page, &f.pic, x, top - f.h, f.w, f.h);
            }
            match f.wrap {
                Wrap::None => {}
                Wrap::TopBottom => {
                    if f.y.is_none() {
                        self.y = self.y.min(top - f.h - f.gap_below);
                        self.at_top = false;
                    }
                }
                Wrap::Square => self.wrap = Some((top - f.h - f.gap_below, x - f.gap, x + f.w + f.gap)),
            }
        }
        Ok(())
    }

    /// Room this page needs for footnotes `notes` not placed yet (with the separator for the
    /// first ones).
    fn notes_height(&self, notes: &[usize]) -> f64 {
        let fresh: Vec<&Block> = notes.iter().filter(|i| !self.placed.contains(*i)).filter_map(|i| self.foot.get(*i)).flatten().collect();
        if fresh.is_empty() {
            return 0.0;
        }
        measure(&self.sec, self.fields, &fresh) + if self.foot_h > 0.0 { 0.0 } else { FOOT_SEP }
    }

    /// Put footnotes `notes` at the foot of this page.
    fn place_notes(&mut self, notes: &[usize]) {
        if self.fixed {
            return;
        }
        self.foot_h += self.notes_height(notes);
        for i in notes {
            // Notes without a body (a missing id) take no room and need no record.
            if self.foot.get(*i).is_some_and(|b| !b.is_empty()) && self.placed.insert(*i) {
                self.page_notes.push((self.page, *i));
            }
        }
    }

    /// Start a page because this one is full (the next column first, if there is one).
    fn overflow(&mut self) -> Result<(), String> {
        self.next_column(true)
    }

    /// Move to the next column, or to a new page after the last one. `soft` when the column is
    /// full rather than broken.
    fn next_column(&mut self, soft: bool) -> Result<(), String> {
        if !self.fixed && self.col + 1 < self.sec.cols.len() {
            self.col_low = self.col_low.min(self.y);
            self.col += 1;
            self.y = self.col_top;
            self.at_top = true;
            self.soft_top = soft;
            self.prev_borders = None;
            self.prev_contextual = None;
            return Ok(());
        }
        self.new_page()?;
        self.soft_top = soft;
        Ok(())
    }

    /// End this page's columns: draw the lines between the columns used, and go on below the
    /// lowest of them in a single column.
    fn close_columns(&mut self) {
        let low = self.col_low.min(self.y);
        if self.sec.col_sep && self.col > 0 {
            let x0 = self.sec.margins[3];
            let (top, col) = (self.col_top, self.col);
            let lines: Vec<f64> = self
                .sec
                .cols
                .windows(2)
                .take(col)
                .filter_map(|w| {
                    let (a, b) = (w.first()?, w.get(1)?);
                    Some(x0 + (a.0 + a.1 + b.0) / 2.0)
                })
                .collect();
            if let Some(page) = self.canvas.page(self.page) {
                for x in lines {
                    page.line(x, top, x, low, 0.5, [0.0; 3]);
                }
            }
        }
        self.y = low;
        self.col = 0;
        self.col_top = low;
        self.col_low = low;
        self.col_limit = None;
    }

    /// Whether `blocks` fit on this page in this section's columns when the columns end at
    /// `limit`, laid out on a scratch page.
    fn fits(&self, blocks: &[Block], limit: f64) -> bool {
        let mut canvas = Canvas::default();
        let Ok(page) = canvas.new_page(self.sec.w, self.sec.h) else { return false };
        let mut fl = Flow {
            canvas: &mut canvas,
            sec: self.sec.clone(),
            page,
            y: self.y,
            at_top: self.at_top,
            page_sections: vec![0],
            section_index: 0,
            prev_contextual: None,
            fields: self.fields,
            soft_top: self.soft_top,
            fixed: false,
            heads: Vec::new(),
            prev_borders: None,
            col: 0,
            col_top: self.y,
            col_low: self.y,
            col_limit: Some(limit),
            foot: self.foot.clone(),
            foot_h: self.foot_h,
            page_notes: self.page_notes.clone(),
            placed: self.placed.clone(),
            wrap: self.wrap,
            counts: self.counts.clone(),
            line_no: (0, usize::MAX, usize::MAX),
        };
        for (i, b) in blocks.iter().enumerate() {
            if fl.block(b, blocks.get(i + 1)).is_err() || fl.canvas.pages.len() > 1 {
                return false;
            }
        }
        fl.canvas.pages.len() == 1
    }

    /// Balance a section's columns, as Word does before a continuous section break: the
    /// shortest column height that still fits the section on this page, so the first columns
    /// are the fullest. Nothing changes when the section doesn't fit on the page.
    fn balance(&mut self, blocks: &[Block]) {
        let full = self.col_top - self.sec.margins[2];
        if full <= 0.0 || !self.fits(blocks, self.sec.margins[2]) {
            return;
        }
        let (mut lo, mut hi) = (0.0, full);
        for _ in 0..14 {
            let mid = (lo + hi) / 2.0;
            if self.fits(blocks, self.col_top - mid) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        self.col_limit = Some(self.col_top - hi - 0.01);
    }

    /// The left and right edges of a paragraph's box: its indents, pushed out by its borders.
    fn box_x(&self, laid: &Laid, b: Option<ParaBorders>) -> (f64, f64) {
        let out = |r: Option<Rule>| r.map_or(0.0, |r| r.space + r.width);
        let x0 = self.x0();
        (x0 + laid.left - out(b.and_then(|b| b.left)), x0 + laid.right + out(b.and_then(|b| b.right)))
    }

    /// A band `h` tall of a paragraph's box from the current y: its shading and side borders.
    fn band(&mut self, laid: &Laid, b: Option<ParaBorders>, h: f64) {
        if h <= 0.0 {
            return;
        }
        let (x1, x2) = self.box_x(laid, b);
        let y = self.y;
        let Some(page) = self.canvas.page(self.page) else { return };
        if let Some(c) = laid.shade {
            page.fill_rect(x1, y - h, x2 - x1, h, c);
        }
        for (r, x) in [(b.and_then(|b| b.left), x1), (b.and_then(|b| b.right), x2)] {
            let Some(r) = r else { continue };
            let x = if x == x1 { x + r.width / 2.0 } else { x - r.width / 2.0 };
            if r.double {
                // Two lines with a gap as wide as each between them.
                let w = r.width / 3.0;
                page.line(x - w, y, x - w, y - h, w, r.color);
                page.line(x + w, y, x + w, y - h, w, r.color);
            } else {
                flow::stroke(page, x, y, x, y - h, r.width, r.color, r.dash);
            }
        }
    }

    /// A horizontal border of a paragraph's box, taking `r.space + r.width` of height: above
    /// the text (`top`) the rule comes first, below it the space does.
    fn rule(&mut self, laid: &Laid, b: Option<ParaBorders>, r: Rule, top: bool) {
        let h = r.space + r.width;
        let r = if r.double { Rule { width: r.width / 3.0, ..r } } else { r };
        self.band(laid, b, h);
        let (x1, x2) = self.box_x(laid, b);
        let y = if top { self.y - r.width / 2.0 } else { self.y - r.space - r.width / 2.0 };
        if let Some(page) = self.canvas.page(self.page) {
            if r.double {
                page.line(x1, y + r.width, x2, y + r.width, r.width, r.color);
                page.line(x1, y - r.width, x2, y - r.width, r.width, r.color);
            } else {
                flow::stroke(page, x1, y, x2, y, r.width, r.color, r.dash);
            }
        }
        self.y -= h;
    }

    /// Count a line for line numbering and print its number in the margin when it is due.
    fn line_number(&mut self, (every, first, restart, distance): (i64, i64, u8, f64), base: &RunStyle, baseline: f64) {
        let (n, page, section) = self.line_no;
        let fresh = match restart {
            0 => page != self.page,
            1 => section != self.section_index,
            _ => page == usize::MAX,
        };
        let n = if fresh { first } else { n };
        self.line_no = (n.saturating_add(1), self.page, self.section_index);
        if every <= 0 || n % every != 0 {
            return;
        }
        let style = RunStyle { bold: false, italic: false, underline: false, strike: false, highlight: None, vert: 0, link: None, ..base.clone() };
        let text = n.to_string();
        let x = self.x0() - distance - style.width(&text);
        if let Some(page) = self.canvas.page(self.page) {
            page.text(x, baseline, &style, &text);
        }
    }

    /// A table's column widths (fitted to the text width), its width, and its left edge from the
    /// column's: indented, centred, or where a floating table says.
    fn table_geometry(&self, t: &Table) -> (Vec<f64>, f64, f64) {
        let area = self.width();
        let ncols = t.cols.len().max(t.rows.iter().map(|r| r.cells.iter().map(|c| c.span).sum::<usize>()).max().unwrap_or(1)).max(1);
        let mut cols = t.cols.clone();
        cols.resize(ncols, 0.0);
        let total: f64 = cols.iter().sum();
        if total <= 1.0 {
            cols = vec![area / ncols as f64; ncols];
        } else if total > area - t.indent.max(0.0) {
            let k = (area - t.indent.max(0.0)).max(36.0) / total;
            cols.iter_mut().for_each(|c| *c *= k);
        }
        let table_w: f64 = cols.iter().sum();
        // From the column's left edge: a row moved to the next column moves with it.
        let x_shift = match t.float.and_then(|f| f.x) {
            Some((FloatY::Align(k), true)) => (self.sec.w - table_w) * k - self.x0(),
            Some((FloatY::Align(k), false)) => (area - table_w) * k,
            Some((FloatY::Offset(o), true)) => o - self.x0(),
            Some((FloatY::Offset(o), false)) => o,
            None if t.center => ((area - table_w) / 2.0).max(0.0),
            None => t.indent.clamp(-self.x0(), area),
        };
        (cols, table_w, x_shift.clamp(-self.x0(), self.sec.w))
    }

    /// A body paragraph with its fields resolved for the current page and section.
    fn resolve_body_fields(&self, p: &Para, counts: &PageCounts) -> Para {
        let numbers = page_numbers(&self.page_sections, |si| counts.starts.get(si).copied().flatten());
        let number = numbers.get(self.page).or(numbers.last()).copied().unwrap_or(1);
        let si = self.section_index;
        resolve_para(p, &|f| {
            Some(match f {
                Field::Page(fmt) => format_number(number, fmt.or(counts.fmts.get(si).and_then(|f| f.as_deref())).unwrap_or("decimal")),
                Field::Pages => counts.total.to_string(),
                Field::SectionPages => counts.section_pages.get(si).copied().unwrap_or(1).to_string(),
                Field::StyleRef(style) => style_ref(&self.heads, self.page, style),
            })
        })
    }

    /// Height of a block's first line (with its space before), for keep-with-next.
    fn first_line(&self, b: Option<&Block>) -> f64 {
        match b {
            Some(Block::Para(p)) => {
                let l = flow::layout(p, self.width(), self.fields);
                l.before + l.lines.first().map_or(0.0, |l| l.height)
            }
            Some(Block::Table(_)) => 20.0,
            _ => 0.0,
        }
    }

    fn para(&mut self, p: &Para, next: Option<&Block>) -> Result<(), String> {
        // The body's fields show the page the paragraph starts on.
        let resolved;
        let p = match &self.counts {
            Some(counts) if p.items.iter().any(|i| matches!(i, Inline::Field(..))) => {
                resolved = self.resolve_body_fields(p, counts);
                &resolved
            }
            _ => p,
        };
        if p.page_break_before && !self.at_top {
            self.new_page()?;
            self.soft_top = true;
        }
        if !p.floats.is_empty() {
            self.floats(p)?;
        }
        // Beside a picture text wraps around, lines are narrower.
        let (mut shift, span) = self.text_span();
        let laid = flow::layout(p, span, self.fields);
        // Contextual spacing: no space between paragraphs of the same style.
        let mut before = laid.before;
        if let (Some(style), Some((prev, prev_after))) = (&p.contextual, &self.prev_contextual)
            && style == prev
        {
            // The previous paragraph's space after was left on the page it ended; a paragraph
            // starting a new page has none to take back.
            if !self.at_top {
                self.y += prev_after;
            }
            before = 0.0;
        }
        self.prev_contextual = None;
        // Consecutive paragraphs with the same borders and indents share one box.
        let bd = p.borders.as_deref().copied();
        let grouped_prev = bd.is_some() && self.prev_borders == bd.map(|b| (b, laid.left, laid.right));
        let grouped_next =
            bd.is_some() && matches!(next, Some(Block::Para(n)) if n.borders.as_deref().copied() == bd && n.left == p.left && n.right == p.right);
        let top_rule = bd.and_then(|b| if grouped_prev { b.between } else { b.top });
        let bottom_rule = bd.and_then(|b| b.bottom).filter(|_| !grouped_next);
        let rules = top_rule.map_or(0.0, |r| r.space + r.width) + bottom_rule.map_or(0.0, |r| r.space + r.width);
        // Keep with next: move to a new page rather than strand this paragraph at the bottom.
        if p.keep_next && !self.at_top {
            let need = before + rules + laid.lines.iter().map(|l| l.height).sum::<f64>() + laid.after + self.first_line(next);
            if self.y - need < self.bottom() && need <= self.top() - self.bottom() {
                self.overflow()?;
            }
        }
        // Keep lines together: the whole paragraph moves to the next page rather than split.
        if p.keep_lines && !self.at_top && !self.fixed {
            let need = before + rules + laid.lines.iter().map(|l| l.height).sum::<f64>();
            if self.y - need < self.bottom() - 0.5 && need <= self.top() - self.bottom() {
                self.overflow()?;
            }
        }
        // Space before is kept only at the top of the document's and a section's first page.
        if !self.at_top || !self.soft_top {
            if grouped_prev && !self.at_top {
                self.band(&laid, bd, before);
            }
            self.y -= before;
        }
        if let Some(r) = top_rule {
            let first = laid.lines.first().map_or(0.0, |l| l.height);
            if self.y - r.space - r.width - first < self.bottom() - 0.5 && !self.at_top {
                self.overflow()?;
            }
            self.rule(&laid, bd, r, true);
        }
        let mut laid = laid;
        let mut laid_w = span;
        let mut li = 0usize;
        // Footnotes go at the foot of the page their reference's line is placed on; a line moves
        // to the next page when it and its notes don't fit. `base` is the item the current
        // layout starts from.
        let mut pending: Vec<(usize, usize)> = if self.fixed { Vec::new() } else { p.notes.clone() };
        let mut base = 0usize;
        let mut noted = false;
        let note = |fl: &mut Self| {
            if fl.fixed {
                return;
            }
            let text = || -> String {
                p.items.iter().filter_map(|it| if let Inline::Text(t, s) = it { (!s.hidden).then_some(t.as_str()) } else { None }).collect()
            };
            if let Some(style) = p.style.clone().filter(|_| fl.heads.len() < 100_000) {
                fl.heads.push((fl.page, style, text().trim().to_owned()));
            }
            // Bookmarks and headings are places to go: their page and the top of their first line.
            let top = fl.y + 2.0;
            for name in &p.anchors {
                fl.canvas.anchors.entry(name.clone()).or_insert((fl.page, top));
            }
            if let Some(level) = p.outline {
                fl.canvas.outline.push((level, text(), fl.page, top));
            }
        };
        while let Some(h) = laid.lines.get(li).map(|l| l.height) {
            let end = laid.lines.get(li + 1).and_then(|l| l.src).map_or(usize::MAX, |(item, _)| base.saturating_add(item));
            let here: Vec<usize> = pending.iter().filter(|(_, item)| *item < end).map(|(n, _)| *n).collect();
            let need = if here.is_empty() { 0.0 } else { self.notes_height(&here) };
            // Widow and orphan control: a first line doesn't stay alone at the foot of a page, nor
            // a last line alone at the top of the next (a three-line paragraph moves whole).
            if p.widow && !self.at_top && !self.fixed {
                let n = laid.lines.len();
                let hs = |k: usize| laid.lines.get(k).map_or(0.0, |l| l.height);
                let room = self.y - self.bottom() - need + 0.5;
                let orphan = li == 0 && n >= 2 && h <= room && h + hs(1) > room;
                let three = li == 0 && n == 3 && h + hs(1) <= room && h + hs(1) + hs(2) > room;
                let widow = li >= 1 && n >= 4 && li + 2 == n && h <= room && h + hs(li + 1) > room;
                if orphan || three || widow {
                    self.overflow()?;
                }
            }
            if self.y - h < self.bottom() + need - 0.5 && !self.at_top {
                self.overflow()?;
            }
            if !noted {
                note(self);
                noted = true;
            }
            // The rest of the paragraph goes on where the text is another width (the next
            // column, or below a picture text wrapped beside): break it again.
            let (sh, w) = self.text_span();
            if li > 0
                && (w - laid_w).abs() > 0.5
                && let Some(src) = laid.lines.get(li).and_then(|l| l.src)
            {
                laid = flow::layout(&flow::rest(p, src), w, self.fields);
                laid_w = w;
                shift = sh;
                li = 0;
                base = base.saturating_add(src.0);
                continue;
            }
            if !here.is_empty() {
                self.place_notes(&here);
                pending.retain(|(_, item)| *item >= end);
            }
            self.band(&laid, bd, h);
            // Read after any overflow: the line may have moved to the next column.
            let (x0, y) = (self.x0() + shift, self.y);
            if let Some(line) = laid.lines.get(li)
                && let Some(page) = self.canvas.page(self.page)
            {
                flow::draw_line(page, &laid, line, x0, y);
            }
            if let (Some(numbering), false, false) = (self.sec.line_numbers, self.fixed, p.no_line_numbers) {
                self.line_number(numbering, &p.base, y - laid.lines.get(li).map_or(h, |l| l.baseline));
            }
            self.y -= h;
            self.at_top = false;
            li += 1;
        }
        if !noted {
            note(self);
        }
        if let Some(r) = bottom_rule {
            self.rule(&laid, bd, r, false);
        }
        if grouped_next {
            self.band(&laid, bd, laid.after);
        }
        self.y -= laid.after;
        self.prev_contextual = p.contextual.clone().map(|s| (s, laid.after));
        self.prev_borders = bd.map(|b| (b, laid.left, laid.right));
        Ok(())
    }

    fn table(&mut self, t: &Table) -> Result<(), String> {
        // Fields in the cells show the page the table starts on.
        let resolved;
        let t = match &self.counts {
            Some(counts)
                if t.rows.iter().any(|r| r.cells.iter().any(|c| c.paras.iter().any(|p| p.items.iter().any(|i| matches!(i, Inline::Field(..)))))) =>
            {
                let counts = counts.clone();
                let mut copy = t.clone();
                for row in &mut copy.rows {
                    for cell in &mut row.cells {
                        cell.paras = cell.paras.iter().map(|p| self.resolve_body_fields(p, &counts)).collect();
                    }
                }
                resolved = copy;
                &resolved
            }
            _ => t,
        };
        self.prev_contextual = None;
        self.prev_borders = None;
        // Tables don't wrap around pictures here: they start below one.
        if let Some((bottom, _, _)) = self.wrap.take()
            && self.y > bottom
            && !self.fixed
        {
            self.y = bottom;
        }
        // A table floating against the page or its margins is drawn there, and the text goes on
        // where it was.
        if let Some(f) = t.float
            && !self.fixed
        {
            let h = measure(&self.sec, self.fields, &[&Block::Table(Table { float: None, ..t.clone() })]);
            let (top, bottom) = if f.page { (self.sec.h, 0.0) } else { (self.top(), self.sec.margins[2]) };
            let y_top = match (f.text, f.y) {
                // From where the text has reached; on the next page when it doesn't fit here.
                (true, y) => {
                    let o = if let FloatY::Offset(o) = y { o } else { 0.0 };
                    if self.y - o - h < self.bottom() && !self.at_top && h <= self.top() - self.bottom() {
                        self.overflow()?;
                    }
                    self.y - o
                }
                (false, FloatY::Align(k)) => top - (top - bottom - h).max(0.0) * k,
                (false, FloatY::Offset(o)) => top - o,
            };
            let saved = (self.y, self.at_top, self.soft_top);
            self.fixed = true;
            self.y = y_top;
            self.at_top = false;
            let drawn = self.table(t);
            self.fixed = false;
            (self.y, self.at_top, self.soft_top) = saved;
            // The text after a table floating in it wraps beside the table.
            if f.text {
                let (_, table_w, x_shift) = self.table_geometry(t);
                let x1 = self.x0() + x_shift;
                self.wrap = Some((y_top - h - f.gaps[2], x1 - f.gaps[0], x1 + table_w + f.gaps[1]));
            }
            return drawn;
        }
        let (cols, table_w, x_shift) = self.table_geometry(t);
        let ncols = cols.len();
        let edges: Vec<f64> = std::iter::once(0.0)
            .chain(cols.iter().scan(0.0, |acc, c| {
                *acc += c;
                Some(*acc)
            }))
            .collect();

        // Grid position of each cell, for vertical merges.
        let grid: Vec<Vec<usize>> = t
            .rows
            .iter()
            .map(|r| {
                r.cells
                    .iter()
                    .scan(0usize, |g, c| {
                        let at = *g;
                        *g = g.saturating_add(c.span);
                        Some(at)
                    })
                    .collect()
            })
            .collect();

        for (ri, row) in t.rows.iter().enumerate() {
            // Lay out each cell's paragraphs into (height, paragraph, line) items.
            let mut cells: Vec<CellLines> = Vec::new();
            for (ci, cell) in row.cells.iter().enumerate() {
                let g = grid.get(ri).and_then(|r| r.get(ci)).copied().unwrap_or(0);
                let x1 = edges.get(g).copied().unwrap_or(0.0);
                let x2 = edges.get(g.saturating_add(cell.span).min(ncols)).copied().unwrap_or(table_w);
                let inner = (x2 - x1 - t.pad.0 - t.pad.1).max(4.0);
                // Turned text runs along the cell's height: as long as an exact row allows, else
                // on one line, and the row grows to its length.
                let along = match (cell.turn, row.height) {
                    (0, _) => inner,
                    (_, Some((h, true))) => (fin(h, 0.0) - 4.0).max(4.0),
                    _ => 2_000.0,
                };
                let laids: Vec<Laid> =
                    if cell.vmerge == Some(false) { Vec::new() } else { cell.paras.iter().map(|p| flow::layout(p, along, self.fields)).collect() };
                let mut items = Vec::new();
                if cell.turn != 0 {
                    let length = laids.iter().flat_map(|l| l.lines.iter().map(move |line| line_ink(l, line))).fold(0.0, f64::max);
                    items.push((length + 4.0, None));
                    cells.push((laids, items));
                    continue;
                }
                for (pi, l) in laids.iter().enumerate() {
                    let same = |a: usize, b: usize| matches!((cell.paras.get(a).and_then(|p| p.contextual.as_ref()), cell.paras.get(b).and_then(|p| p.contextual.as_ref())), (Some(x), Some(y)) if x == y);
                    if pi > 0 && l.before > 0.0 && !same(pi - 1, pi) {
                        items.push((l.before, None));
                    }
                    for (li, line) in l.lines.iter().enumerate() {
                        items.push((line.height, Some((pi, li))));
                    }
                    if pi + 1 < laids.len() && l.after > 0.0 && !same(pi, pi + 1) {
                        items.push((l.after, None));
                    }
                }
                cells.push((laids, items));
            }
            // Word's cells have no top or bottom margin by default: a row is as tall as its tallest
            // cell's lines.
            let content = |items: &[(f64, Option<(usize, usize)>)]| items.iter().map(|(h, _)| *h).sum::<f64>();
            let mut row_h = cells.iter().map(|(_, items)| content(items)).fold(0.0, f64::max);
            if let Some((h, exact)) = row.height {
                let h = fin(h, 0.0).clamp(0.0, 10_000.0);
                row_h = if exact { h } else { row_h.max(h) };
            }
            let full = self.top() - self.bottom();
            if self.y - row_h < self.bottom() && !self.at_top && row_h <= full {
                self.overflow()?;
                // The header rows again at the top of the new page.
                let header: Vec<Row> = t.rows.iter().take_while(|r| r.header).map(|r| Row { header: false, ..r.clone() }).collect();
                if !header.is_empty() && ri >= header.len() {
                    self.table(&Table { rows: header, float: None, ..t.clone() })?;
                }
            }
            // Footnotes referenced in the row's cells go at the foot of this page.
            let row_notes: Vec<usize> = row.cells.iter().flat_map(|c| c.paras.iter().flat_map(|p| p.notes.iter().map(|(n, _)| *n))).collect();
            if !row_notes.is_empty() {
                self.place_notes(&row_notes);
            }
            // Draw the row, continuing it on new pages while it doesn't fit.
            let mut cursors = vec![0usize; cells.len()];
            let mut first_part = true;
            loop {
                let avail = self.y - self.bottom();
                let remaining_h = row_h.min(
                    cells
                        .iter()
                        .zip(&cursors)
                        .map(|((_, items), c)| content(items.get(*c..).unwrap_or_default()))
                        .fold(if first_part { row_h } else { 0.0 }, f64::max),
                );
                let part_h = if remaining_h <= avail || self.at_top && avail <= 0.0 { remaining_h } else { avail.max(0.0) };
                let y_top = self.y;
                // On a fresh page each cell places at least one item even if it is taller than
                // the page (it is cut at the bottom), so a row always moves on.
                let fresh = self.at_top;
                // A row's grid positions strictly increase (every span is at least 1), so the
                // cell below is found by binary search rather than a scan per cell.
                let next_vmerge = |g: usize| {
                    let below = ri.checked_add(1);
                    let (Some(nr), Some(ng)) = (below.and_then(|b| t.rows.get(b)), below.and_then(|b| grid.get(b))) else { return false };
                    ng.binary_search(&g).ok().and_then(|i| nr.cells.get(i)).is_some_and(|c| c.vmerge == Some(false))
                };
                let mut done = true;
                for (ci, cell) in row.cells.iter().enumerate() {
                    let g = grid.get(ri).and_then(|r| r.get(ci)).copied().unwrap_or(0);
                    let x_table = self.x0() + x_shift;
                    let x1 = x_table + edges.get(g).copied().unwrap_or(0.0);
                    let x2 = x_table + edges.get(g.saturating_add(cell.span).min(ncols)).copied().unwrap_or(table_w);
                    let page = match self.canvas.page(self.page) {
                        Some(p) => p,
                        None => break,
                    };
                    if let Some(c) = cell.shade {
                        page.fill_rect(x1, y_top - part_h, x2 - x1, part_h, c);
                    }
                    let Some((laids, items)) = cells.get(ci) else { continue };
                    let Some(cursor) = cursors.get_mut(ci) else { continue };
                    if cell.turn != 0 {
                        // Turned text: its lines laid side by side across the cell, from its bottom
                        // (reading upwards) or its top (downwards).
                        if first_part {
                            let thick: f64 = laids.iter().map(Laid::height).sum();
                            let (bx, by, bw, bh) =
                                (x1 + t.pad.0, y_top - part_h + 2.0, (x2 - x1 - t.pad.0 - t.pad.1).max(1.0), (part_h - 4.0).max(1.0));
                            page.save();
                            page.clip_rect(x1, y_top - part_h, x2 - x1, part_h);
                            flow::turn(page, cell.turn, bx, by, bw, bh);
                            let mut yy = bw - (bw - thick).max(0.0) * cell.valign;
                            for l in laids {
                                yy -= flow::draw(page, l, 0.0, yy);
                            }
                            page.restore();
                        }
                        *cursor = items.len();
                    }
                    let mut y = y_top;
                    // A cell whose row fits here is aligned in it (top, centre or bottom).
                    if first_part && part_h >= remaining_h - 0.01 && cell.valign > 0.0 {
                        y -= (part_h - content(items)).max(0.0) * cell.valign;
                    }
                    let mut placed = 0usize;
                    while let Some((h, what)) = items.get(*cursor) {
                        let fits = y - h >= y_top - part_h - 0.01;
                        let forced = fresh && placed == 0;
                        if !(fits || forced) {
                            break;
                        }
                        placed += 1;
                        if let Some((pi, li)) = what
                            && let Some(l) = laids.get(*pi)
                            && let Some(line) = l.lines.get(*li)
                        {
                            // A paragraph's own shading and borders, around its lines.
                            let (bx1, bx2) = (x1 + t.pad.0 + l.left, x1 + t.pad.0 + l.right);
                            if let Some(c) = l.shade {
                                page.fill_rect(bx1, y - h, bx2 - bx1, *h, c);
                            }
                            if let Some(b) = cell.paras.get(*pi).and_then(|p| p.borders.as_deref()) {
                                let last = *li + 1 == l.lines.len();
                                let sides = [
                                    (b.top.filter(|_| *li == 0), (bx1, y, bx2, y)),
                                    (b.bottom.filter(|_| last), (bx1, y - h, bx2, y - h)),
                                    (b.left, (bx1, y, bx1, y - h)),
                                    (b.right, (bx2, y, bx2, y - h)),
                                ];
                                for (r, (ax, ay, bx, by)) in sides {
                                    if let Some(r) = r {
                                        flow::stroke(page, ax, ay, bx, by, r.width, r.color, r.dash);
                                    }
                                }
                            }
                            flow::draw_line(page, l, line, x1 + t.pad.0, y);
                        }
                        y -= h;
                        *cursor += 1;
                    }
                    if *cursor < items.len() {
                        done = false;
                    }
                    // Borders: the cell's own, else the table's outer or inside ones.
                    let first_col = g == 0;
                    let last_col = g.saturating_add(cell.span) >= ncols;
                    let first_row = ri == 0;
                    let last_row = ri + 1 == t.rows.len();
                    let pick = |own: Option<Option<Border>>, outer: Option<Option<Border>>, inside: Option<Option<Border>>, is_outer: bool| {
                        own.unwrap_or(if is_outer { outer.flatten() } else { inside.flatten() })
                    };
                    let continues_below = next_vmerge(g);
                    let top = if cell.vmerge == Some(false) || !first_part {
                        None
                    } else {
                        pick(cell.borders.top, t.borders.top, t.borders.inside_h, first_row)
                    };
                    let bottom =
                        if continues_below || !done { None } else { pick(cell.borders.bottom, t.borders.bottom, t.borders.inside_h, last_row) };
                    let left = pick(cell.borders.left, t.borders.left, t.borders.inside_v, first_col);
                    let right = pick(cell.borders.right, t.borders.right, t.borders.inside_v, last_col);
                    let yb = y_top - part_h;
                    for (b, (ax, ay, bx, by)) in
                        [(top, (x1, y_top, x2, y_top)), (bottom, (x1, yb, x2, yb)), (left, (x1, y_top, x1, yb)), (right, (x2, y_top, x2, yb))]
                    {
                        if let Some(b) = b {
                            flow::stroke(page, ax, ay, bx, by, b.width, b.color, b.dash);
                        }
                    }
                }
                self.y = y_top - part_h;
                self.at_top = false;
                if done || part_h >= remaining_h {
                    break;
                }
                self.overflow()?;
                first_part = false;
            }
        }
        // Word leaves no gap after a table; the next paragraph's spacing applies.
        Ok(())
    }
}

/// What the body's page fields show: where each section restarts its page numbers and their
/// format, and the page count and each section's pages (from a first layout, else 1).
#[derive(Clone)]
struct PageCounts {
    starts: Vec<Option<i64>>,
    fmts: Vec<Option<String>>,
    total: usize,
    section_pages: Vec<usize>,
}

/// Page numbers for pages in sections `page_sections`: a section that says where to start
/// restarts them on its first page; others count on.
fn page_numbers(page_sections: &[usize], start: impl Fn(usize) -> Option<i64>) -> Vec<i64> {
    let mut out = Vec::with_capacity(page_sections.len());
    let mut n = 0i64;
    for (pi, si) in page_sections.iter().enumerate() {
        let first = page_sections.iter().position(|s| s == si) == Some(pi);
        match start(*si) {
            Some(s) if first => n = s,
            _ => n = n.saturating_add(1),
        }
        out.push(n);
    }
    out
}

/// Whether a block shows a page count (`NUMPAGES`, `SECTIONPAGES`).
fn counts_pages(b: &Block) -> bool {
    let para = |p: &Para| p.items.iter().any(|i| matches!(i, Inline::Field(Field::Pages | Field::SectionPages, _)));
    match b {
        Block::Para(p) => para(p),
        Block::Table(t) => t.rows.iter().any(|r| r.cells.iter().any(|c| c.paras.iter().any(para))),
        _ => false,
    }
}

/// Lay the body's sections out on `canvas`; returns each page's section, the footnotes placed on
/// each page, and the styled paragraphs placed (for style references).
#[allow(clippy::type_complexity)]
fn lay_body(
    canvas: &mut Canvas,
    sections: &[(Section, Vec<Block>)],
    foot: &std::rc::Rc<Vec<Vec<Block>>>,
    counts: std::rc::Rc<PageCounts>,
) -> Result<(Vec<usize>, Vec<(usize, usize)>, Vec<(usize, Arc<str>, String)>), String> {
    let first = sections.first().map(|(s, _)| s.clone()).unwrap_or_default();
    let mut fl = Flow {
        canvas,
        sec: first,
        page: 0,
        y: 0.0,
        at_top: true,
        page_sections: Vec::new(),
        section_index: 0,
        prev_contextual: None,
        fields: (1, 1),
        soft_top: false,
        fixed: false,
        heads: Vec::new(),
        prev_borders: None,
        col: 0,
        col_top: 0.0,
        col_low: 0.0,
        col_limit: None,
        foot: foot.clone(),
        foot_h: 0.0,
        page_notes: Vec::new(),
        placed: std::collections::HashSet::new(),
        wrap: None,
        counts: Some(counts),
        line_no: (0, usize::MAX, usize::MAX),
    };
    fl.new_page()?;
    for (si, (sec, blocks)) in sections.iter().enumerate() {
        if si > 0 {
            let same_size = (fl.sec.w - sec.w).abs() < 0.5 && (fl.sec.h - sec.h).abs() < 0.5;
            // The previous section's columns end; this one starts below the lowest of them.
            fl.close_columns();
            fl.sec = sec.clone();
            fl.section_index = si;
            if !(sec.continuous && same_size) || fl.at_top {
                if !fl.at_top {
                    fl.new_page()?;
                    // Word keeps space before at the top of the document's first page only.
                    fl.soft_top = true;
                } else {
                    // The empty page takes this section's size and starts at its top margin.
                    if let Some(s) = fl.page_sections.last_mut() {
                        *s = si;
                    }
                    fl.canvas.resize(fl.page, fl.sec.w, fl.sec.h);
                    fl.y = fl.top();
                }
            }
        }
        // Columns ending at a continuous section break are balanced.
        let next_continuous = sections.get(si + 1).is_some_and(|(n, _)| n.continuous && (n.w - sec.w).abs() < 0.5 && (n.h - sec.h).abs() < 0.5);
        if sec.cols.len() > 1 && next_continuous {
            fl.balance(blocks);
        }
        for (bi, b) in blocks.iter().enumerate() {
            // A manual page break at the end of a paragraph leaves its empty remainder on the
            // new page; when a "page break before" paragraph follows, Word starts one page, not
            // two, so the remainder is left out.
            let after_break = bi.checked_sub(1).and_then(|i| blocks.get(i)).is_some_and(|p| matches!(p, Block::PageBreak));
            let breaks_next = matches!(blocks.get(bi + 1), Some(Block::Para(n)) if n.page_break_before);
            if after_break && breaks_next && matches!(b, Block::Para(p) if p.items.is_empty() && p.label.is_none()) {
                continue;
            }
            fl.block(b, blocks.get(bi + 1))?;
        }
    }
    Ok((fl.page_sections, fl.page_notes, fl.heads))
}

pub(crate) fn convert(zip: &Zip<'_>, main: &str, doc: &mut Document, notes: &mut Notes) -> Result<usize, String> {
    let rels = Rels::load(zip, main)?;
    let text = zip.text(main)?.ok_or_else(|| format!("{main} is missing"))?;
    let xd = xml::parse(&text).map_err(|e| format!("{main}: {e}"))?;
    let body = child(xd.root_element(), "body").ok_or("the document has no body")?;
    // The page colour (`w:background`), printed under everything as Word saves it to PDF.
    let page_color = child(xd.root_element(), "background").and_then(|b| attr(b, "color")).filter(|c| *c != "auto").and_then(hex_color);
    let theme = load_theme(zip, &rels);
    let styles = load_styles(zip, &rels)?;
    let numbering = load_numbering(zip, &rels)?;
    let mut cx = Cx {
        zip,
        doc,
        notes,
        styles,
        numbering,
        theme,
        images: HashMap::new(),
        note_refs: Vec::new(),
        footnotes: 0,
        endnotes: 0,
        fields: Vec::new(),
        in_link: 0,
        links: Vec::new(),
        field_links: Vec::new(),
        cell_layer: None,
        accents: super::xlsx::theme_colors(zip, &rels).get(4..10).map(<[[f64; 3]]>::to_vec).unwrap_or_default(),
        scheme: super::xlsx::theme_colors(zip, &rels),
    };

    // Split the body into sections: a paragraph carrying a sectPr ends one.
    let final_sect = child(body, "sectPr");
    let mut sections: Vec<(Section, Vec<Block>)> = Vec::new();
    let mut current: Vec<Block> = Vec::new();
    let mut prev = Section::default();
    for c in body.children().filter(Node::is_element) {
        match c.tag_name().name() {
            "p" => {
                cx.paragraph(c, &rels, 0, &mut current);
                if let Some(s) = xml::path(c, &["pPr", "sectPr"]) {
                    let sec = parse_section(s, &prev);
                    prev = sec.clone();
                    sections.push((sec, std::mem::take(&mut current)));
                }
            }
            "tbl" => {
                let t = cx.table(c, &rels, 0);
                current.push(Block::Table(t));
            }
            "sdt" | "customXml" | "ins" | "moveTo" => {
                let inner = if is(c, "sdt") { child(c, "sdtContent") } else { Some(c) };
                if let Some(inner) = inner {
                    cx.blocks(inner, &rels, 1, &mut current);
                }
            }
            _ => {}
        }
    }
    let last = final_sect.map(|s| parse_section(s, &prev)).unwrap_or(prev);
    sections.push((last, current));
    for (_, blocks) in &mut sections {
        attach_drop_caps(blocks);
    }
    // The empty paragraph holding a continuous section break takes no room.
    for i in 1..sections.len() {
        if sections.get(i).is_some_and(|(s, _)| s.continuous)
            && let Some((_, blocks)) = sections.get_mut(i - 1)
            && matches!(blocks.last(), Some(Block::Para(p)) if p.items.is_empty() && p.label.is_none())
        {
            blocks.pop();
        }
    }

    // Footnotes go at the foot of the page of their reference, endnotes after the body, each
    // with its mark.
    let mut foot_blocks: Vec<Vec<Block>> = Vec::new();
    if !cx.note_refs.is_empty() {
        let refs = std::mem::take(&mut cx.note_refs);
        let foot = cx.note_bodies(&rels, "footnotes", "footnote");
        let end = cx.note_bodies(&rels, "endnotes", "endnote");
        let marked = |body: &[Block], label: &str| -> Vec<Block> {
            body.iter()
                .enumerate()
                .map(|(i, b)| match b {
                    Block::Para(p) if i == 0 => {
                        let mut p = p.clone();
                        let st = p.items.iter().find_map(|it| if let Inline::Text(_, s) = it { Some(s.clone()) } else { None }).unwrap_or_default();
                        p.items.insert(0, Inline::Text(format!("{label} "), RunStyle { vert: 1, ..st }));
                        Block::Para(p)
                    }
                    other => other.clone(),
                })
                .collect()
        };
        // Each footnote once, however often it is referenced.
        let mut seen = std::collections::HashSet::new();
        foot_blocks = refs
            .iter()
            .map(|(kind, id, label)| match foot.get(id) {
                Some(body) if *kind == "footnote" && seen.insert(id.clone()) => marked(body, label),
                _ => Vec::new(),
            })
            .collect();
        let refs: Vec<(&'static str, String, String)> = refs.into_iter().filter(|(kind, _, _)| *kind == "endnote").collect();
        if !refs.is_empty() {
            cx.notes.add("endnotes, placed at the end of the document");
        }
        if !refs.is_empty()
            && let Some((_, blocks)) = sections.last_mut()
        {
            let rule = Para {
                items: vec![Inline::Text("______________".into(), RunStyle { color: [0.4; 3], ..RunStyle::default() })],
                before: 12.0,
                after: 6.0,
                ..Para::default()
            };
            blocks.push(Block::Para(rule));
            // Each note once, however often it is referenced: a long note referenced again and
            // again would otherwise be copied once per reference.
            let mut placed = std::collections::HashSet::new();
            for (kind, id, label) in refs {
                let body = if kind == "footnote" { foot.get(&id) } else { end.get(&id) };
                let Some(body) = body else { continue };
                if !placed.insert((kind, id)) {
                    continue;
                }
                blocks.extend(marked(body, &label));
            }
        }
    }

    // Lay the body out. Page counts in the body's own text (`NUMPAGES`, `SECTIONPAGES`) need
    // the pages laid out first: a first layout on a scratch canvas counts them.
    let foot = std::rc::Rc::new(foot_blocks);
    let mut counts = PageCounts {
        starts: sections.iter().map(|(s, _)| s.num_start).collect(),
        fmts: sections.iter().map(|(s, _)| s.num_fmt.clone()).collect(),
        total: 1,
        section_pages: vec![1; sections.len()],
    };
    if sections.iter().any(|(_, blocks)| blocks.iter().any(counts_pages)) {
        let mut scratch = Canvas::default();
        let (pages, _, _) = lay_body(&mut scratch, &sections, &foot, std::rc::Rc::new(counts.clone()))?;
        counts.total = pages.len().max(1);
        counts.section_pages = (0..sections.len()).map(|si| pages.iter().filter(|s| **s == si).count().max(1)).collect();
    }
    let mut canvas = Canvas::default();
    let (page_sections, page_notes, heads) = lay_body(&mut canvas, &sections, &foot, std::rc::Rc::new(counts))?;

    // Footnotes at the foot of their pages: a short rule, then the notes down to the margin.
    let mut by_page: Vec<(usize, Vec<usize>)> = Vec::new();
    for (pg, n) in &page_notes {
        match by_page.iter_mut().find(|(p, _)| p == pg) {
            Some((_, v)) => v.push(*n),
            None => by_page.push((*pg, vec![*n])),
        }
    }
    let total = canvas.pages.len();
    for (pi, notes) in by_page {
        let Some((sec, _)) = page_sections.get(pi).and_then(|s| sections.get(*s)) else { continue };
        let blocks: Vec<&Block> = notes.iter().filter_map(|i| foot.get(*i)).flatten().collect();
        let h = measure(sec, (pi + 1, total), &blocks);
        let top = sec.margins[2] + h + FOOT_SEP;
        if let Some(page) = canvas.page(pi) {
            page.line(sec.margins[3], top - FOOT_SEP / 2.0, sec.margins[3] + 144.0, top - FOOT_SEP / 2.0, 0.5, [0.0; 3]);
        }
        let mut fl = Flow::fixed(&mut canvas, sec.clone(), pi, top - FOOT_SEP, (pi + 1, total));
        for (i, b) in blocks.iter().enumerate() {
            fl.block(b, blocks.get(i + 1).copied())?;
        }
    }

    // Headers and footers, now that the page count is known.
    let mut cache: HashMap<Option<String>, Vec<Block>> = HashMap::new();
    let mut section_first_page: Vec<Option<usize>> = vec![None; sections.len()];
    for (pi, si) in page_sections.iter().enumerate() {
        if let Some(slot) = section_first_page.get_mut(*si)
            && slot.is_none()
        {
            *slot = Some(pi);
        }
    }
    let numbers = page_numbers(&page_sections, |si| sections.get(si).and_then(|(s, _)| s.num_start));
    for (pi, si) in page_sections.iter().enumerate() {
        let Some((sec, _)) = sections.get(*si) else { continue };
        let first_of_section = section_first_page.get(*si).copied().flatten() == Some(pi);
        let number = numbers.get(pi).copied().unwrap_or(pi as i64 + 1);
        let section_pages = page_sections.iter().filter(|s| *s == si).count();
        let use_first = sec.title_page && first_of_section;
        if let Some(page) = canvas.page(pi) {
            if let Some(c) = page_color {
                page.underlay(&format!("q {} {} {} rg 0 0 {} {} re f Q\n", n(c[0]), n(c[1]), n(c[2]), n(page.w), n(page.h)));
            }
            if let Some(b) = sec.page_borders.filter(|b| match b.display {
                1 => first_of_section,
                2 => !first_of_section,
                _ => true,
            }) {
                draw_page_borders(page, sec, &b);
            }
        }
        for footer in [false, true] {
            let id = match (footer, use_first) {
                (false, true) => sec.first_header.clone(),
                (false, false) => sec.header.clone(),
                (true, true) => sec.first_footer.clone(),
                (true, false) => sec.footer.clone(),
            };
            if id.is_none() {
                continue;
            }
            if !cache.contains_key(&id) {
                let blocks = cx.header(&rels, id.as_deref());
                cache.insert(id.clone(), blocks);
            }
            let Some(blocks) = cache.get(&id) else { continue };
            // Headers and footers are laid out like the body (tables as tables), in place: a
            // header from its distance below the page's top, a footer up to its distance above
            // the bottom.
            let refs: Vec<&Block> = blocks.iter().filter(|b| !matches!(b, Block::PageBreak | Block::ColumnBreak)).collect();
            let resolved = resolve_fields(&refs, &|f| match f {
                Field::Page(fmt) => Some(format_number(number, fmt.or(sec.num_fmt.as_deref()).unwrap_or("decimal"))),
                Field::SectionPages => Some(section_pages.to_string()),
                Field::StyleRef(style) => Some(style_ref(&heads, pi, style)),
                Field::Pages => None,
            });
            let refs: Vec<&Block> = resolved.iter().collect();
            let fields = (pi + 1, total);
            let start = if footer { sec.footer_dist + measure(sec, fields, &refs) } else { sec.h - sec.header_dist };
            let mut fl = Flow::fixed(&mut canvas, sec.clone(), pi, start, fields);
            for (i, b) in refs.iter().enumerate() {
                fl.block(b, refs.get(i + 1).copied())?;
            }
        }
    }
    canvas.finish(cx.doc).map_err(|e| e.to_string())
}
