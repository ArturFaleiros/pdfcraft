//! Excel (SpreadsheetML, ECMA-376 Part 1 §18) to PDF. Each visible sheet is printed the way its
//! page setup asks: its print areas (or used range, pictures included) at its scale or fitted,
//! columns and rows at their widths and heights, pages ordered down then across (or across then
//! down) with manual breaks, print titles repeated, centred if asked, gridlines only when the
//! sheet asks for them, and headers and footers ([`super::xlsx_hf`]). Cell values are formatted
//! with their number formats (dates, currency, percentages…), and fonts, fills, borders,
//! alignment, wrapping, merged cells and conditional formatting ([`super::xlsx_cf`]) are kept;
//! pictures and shapes are drawn at their anchors ([`super::xlsx_drawing`]). Formulas show their
//! cached results.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use chrono::{Datelike, Timelike};
use pdfcraft_cos::Document;
use roxmltree::Node;

use super::canvas::{Canvas, Page};
use super::flow::fin;
use super::xlsx_cf::{self as cf, Dxf, Look};
use super::xlsx_drawing::{self as drawing, Anchor, Drawn};
use super::xlsx_hf::{self as hf, HeaderFooter, Pictures};
use super::xml::{self, Rels, attr, attr_f64, child, children, flag, is, rel_attr};
use super::zip::Zip;
use super::{LinkTarget, Notes, RunStyle, family_of, hex_color};

/// Most cells read from one sheet.
const MAX_CELLS: usize = 2_000_000;
/// Most merged ranges read from one sheet.
const MAX_MERGES: usize = 20_000;
/// Most cell text a sheet keeps, in bytes.
const MAX_TEXT_BYTES: usize = 64 << 20;
/// Most merge comparisons a sheet's pages make when finding the merge each cell is in.
const MAX_MERGE_WORK: usize = 100_000_000;
const MAX_COL: u32 = 16_384;
/// Most columns a sheet's `<col>` ranges name between them: four times the sheet's width, so
/// overlapping ranges in a valid file still apply in full but repeated ones can't multiply the work.
const MAX_COL_WORK: u32 = MAX_COL * 4;
const MAX_ROW: u32 = 1_048_576;
/// Longest number format read, in characters: Excel's own limit. Every cell re-reads its format,
/// so this also bounds the padding and placeholders a format can ask for.
const MAX_FORMAT_CHARS: usize = 255;

// ── styles ──────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
struct Font {
    name: String,
    size: f64,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    color: Option<[f64; 3]>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Edge {
    width: f64,
    color: [f64; 3],
    /// The dash pattern of dotted and dashed styles (`[on off …]` in points).
    dash: Option<&'static str>,
    /// Two thin lines.
    double: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct Edges {
    left: Option<Edge>,
    right: Option<Edge>,
    top: Option<Edge>,
    bottom: Option<Edge>,
    /// A diagonal line, rising (`diagonalUp`) and or falling (`diagonalDown`).
    diagonal: Option<(Edge, bool, bool)>,
}

#[derive(Clone, Debug, Default)]
struct Xf {
    num_fmt: u32,
    font: usize,
    fill: usize,
    border: usize,
    h_align: Option<String>,
    v_align: Option<String>,
    wrap: bool,
    indent: f64,
    /// `textRotation`: 1–90 degrees counter-clockwise, 91–180 clockwise (less 90), 255 stacked.
    rotation: u16,
    /// Shrink to fit: text too wide for its cell is set smaller.
    shrink: bool,
}

#[derive(Default)]
struct Styles {
    fonts: Vec<Font>,
    fills: Vec<Option<[f64; 3]>>,
    borders: Vec<Edges>,
    xfs: Vec<Xf>,
    formats: HashMap<u32, String>,
    /// Conditional formatting's differential formats.
    dxfs: Vec<Dxf>,
    /// The workbook's own table styles by name: the differential format of each part (whole
    /// table, header row, total row, first and second row stripes, first and last column).
    table_styles: HashMap<String, [Option<usize>; 7]>,
}

/// The default palette behind `indexed` colours.
const INDEXED: [u32; 64] = [
    0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0xFF00FF, 0x00FFFF, 0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00,
    0xFF00FF, 0x00FFFF, 0x800000, 0x008000, 0x000080, 0x808000, 0x800080, 0x008080, 0xC0C0C0, 0x808080, 0x9999FF, 0x993366, 0xFFFFCC, 0xCCFFFF,
    0x660066, 0xFF8080, 0x0066CC, 0xCCCCFF, 0x000080, 0xFF00FF, 0xFFFF00, 0x00FFFF, 0x800080, 0x800000, 0x008080, 0x0000FF, 0x00CCFF, 0xCCFFFF,
    0xCCFFCC, 0xFFFF99, 0x99CCFF, 0xFF99CC, 0xCC99FF, 0xFFCC99, 0x3366FF, 0x33CCCC, 0x99CC00, 0xFFCC00, 0xFF9900, 0xFF6600, 0x666699, 0x969696,
    0x003366, 0x339966, 0x003300, 0x333300, 0x993300, 0x993366, 0x333399, 0x333333,
];

fn rgb24(v: u32) -> [f64; 3] {
    [((v >> 16) & 0xFF) as f64 / 255.0, ((v >> 8) & 0xFF) as f64 / 255.0, (v & 0xFF) as f64 / 255.0]
}

/// A number format's colour tag: one of its eight colour names, or `ColorN` (1–56), the indexed
/// palette from its ninth entry.
fn format_color(tag: &str) -> Option<[f64; 3]> {
    let lower = tag.trim().to_ascii_lowercase();
    let v = match lower.as_str() {
        "black" => 0x000000,
        "white" => 0xFFFFFF,
        "red" => 0xFF0000,
        "green" => 0x00FF00,
        "blue" => 0x0000FF,
        "yellow" => 0xFFFF00,
        "magenta" => 0xFF00FF,
        "cyan" => 0x00FFFF,
        _ => {
            let k: usize = lower.strip_prefix("color")?.trim().parse().ok()?;
            if !(1..=56).contains(&k) {
                return None;
            }
            *INDEXED.get(k + 7)?
        }
    };
    Some(rgb24(v))
}

fn color(n: Node<'_, '_>, theme: &[[f64; 3]]) -> Option<[f64; 3]> {
    let mut c = if let Some(rgb) = attr(n, "rgb") {
        hex_color(rgb)?
    } else if let Some(t) = attr_f64(n, "theme") {
        *theme.get(t.clamp(0.0, 64.0) as usize)?
    } else {
        let i = attr_f64(n, "indexed")?;
        let i = i.clamp(0.0, 100.0) as usize;
        if i == 64 {
            // System foreground.
            [0.0; 3]
        } else {
            rgb24(*INDEXED.get(i)?)
        }
    };
    if let Some(t) = attr_f64(n, "tint").filter(|t| t.is_finite()) {
        let t = t.clamp(-1.0, 1.0);
        c = c.map(|v| if t < 0.0 { v * (1.0 + t) } else { v + (1.0 - v) * t });
    }
    Some(c)
}

pub(super) fn theme_colors(zip: &Zip<'_>, rels: &Rels) -> Vec<[f64; 3]> {
    // Excel's order: lt1, dk1, lt2, dk2, accent1–6, hlink, folHlink.
    let mut out = vec![[1.0; 3], [0.0; 3], [0.93, 0.93, 0.88], [0.27, 0.33, 0.42]];
    let Some(part) = rels.of_type("theme") else { return out };
    let Ok(Some(text)) = zip.text(part) else { return out };
    let Ok(d) = xml::parse(&text) else { return out };
    let Some(scheme) = d.descendants().find(|n| is(*n, "clrScheme")) else { return out };
    let get = |name: &str| -> Option<[f64; 3]> {
        let e = child(scheme, name)?.children().find(Node::is_element)?;
        match e.tag_name().name() {
            "srgbClr" => hex_color(attr(e, "val")?),
            "sysClr" => attr(e, "lastClr").and_then(hex_color),
            _ => None,
        }
    };
    out = ["lt1", "dk1", "lt2", "dk2", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6", "hlink", "folHlink"]
        .iter()
        .enumerate()
        .map(|(i, n)| get(n).unwrap_or(if i == 0 { [1.0; 3] } else { [0.0; 3] }))
        .collect();
    out
}

fn load_styles(zip: &Zip<'_>, rels: &Rels, theme: &[[f64; 3]], notes: &mut Notes) -> Result<Styles, String> {
    let mut s = Styles::default();
    let Some(part) = rels.of_type("styles") else { return Ok(s) };
    let Some(text) = zip.text(part)? else { return Ok(s) };
    let d = xml::parse(&text).map_err(|e| format!("{part}: {e}"))?;
    let root = d.root_element();
    if let Some(f) = child(root, "numFmts") {
        for n in children(f, "numFmt") {
            if let (Some(id), Some(code)) = (attr_f64(n, "numFmtId"), attr(n, "formatCode")) {
                let code = if code.chars().nth(MAX_FORMAT_CHARS).is_some() {
                    notes.add("number formats longer than Excel allows, shown as General");
                    "General"
                } else {
                    code
                };
                s.formats.insert(id.clamp(0.0, 1e6) as u32, code.to_owned());
            }
        }
    }
    if let Some(f) = child(root, "fonts") {
        for n in children(f, "font") {
            let val = |k: &str| child(n, k).map(|c| flag(attr(c, "val")));
            s.fonts.push(Font {
                name: child(n, "name").and_then(|c| attr(c, "val")).unwrap_or("Calibri").to_owned(),
                size: child(n, "sz").and_then(|c| attr_f64(c, "val")).unwrap_or(11.0).clamp(1.0, 400.0),
                bold: val("b").unwrap_or(false),
                italic: val("i").unwrap_or(false),
                underline: child(n, "u").is_some_and(|c| attr(c, "val") != Some("none")),
                strike: val("strike").unwrap_or(false),
                color: child(n, "color").and_then(|c| color(c, theme)),
            });
        }
    }
    if let Some(f) = child(root, "fills") {
        for n in children(f, "fill") {
            let fill = child(n, "patternFill").and_then(|p| {
                let ty = attr(p, "patternType").unwrap_or("none");
                let fg = child(p, "fgColor").and_then(|c| color(c, theme));
                let bg = child(p, "bgColor").and_then(|c| color(c, theme));
                // A pattern prints as its dots or hatching over the background: drawn as the mix
                // of the two in the share the pattern covers.
                let share = match ty {
                    "none" => return None,
                    "solid" => return fg.or(bg),
                    "gray0625" => 0.0625,
                    "gray125" => 0.125,
                    "lightGray" | "lightHorizontal" | "lightVertical" | "lightDown" | "lightUp" | "lightGrid" | "lightTrellis" => 0.25,
                    "mediumGray" | "darkHorizontal" | "darkVertical" | "darkDown" | "darkUp" | "darkGrid" => 0.5,
                    "darkGray" | "darkTrellis" => 0.75,
                    _ => return fg.or(bg),
                };
                let (fg, bg) = (fg.unwrap_or([0.0; 3]), bg.unwrap_or([1.0; 3]));
                Some([0, 1, 2].map(|i| bg[i] + (fg[i] - bg[i]) * share))
            });
            let fill =
                fill.or_else(|| child(n, "gradientFill").and_then(|g| g.descendants().find(|n| is(*n, "color"))).and_then(|c| color(c, theme)));
            s.fills.push(fill);
        }
    }
    if let Some(f) = child(root, "borders") {
        for n in children(f, "border") {
            let edge = |names: [&str; 2]| -> Option<Edge> {
                let e = names.iter().find_map(|k| child(n, k))?;
                let width = match attr(e, "style")? {
                    "none" => return None,
                    "thin" | "hair" | "dotted" | "dashed" | "dashDot" | "dashDotDot" => 0.5,
                    "medium" | "mediumDashed" | "mediumDashDot" | "mediumDashDotDot" | "slantDashDot" => 1.0,
                    "thick" | "double" => 1.5,
                    _ => 0.5,
                };
                let dash = match attr(e, "style")? {
                    "hair" => Some("[0.5 0.5]"),
                    "dotted" => Some("[1 1]"),
                    "dashed" | "mediumDashed" => Some("[3 1.5]"),
                    "dashDot" | "mediumDashDot" | "slantDashDot" => Some("[3 1.5 1 1.5]"),
                    "dashDotDot" | "mediumDashDotDot" => Some("[3 1.5 1 1.5 1 1.5]"),
                    _ => None,
                };
                let double = attr(e, "style") == Some("double");
                Some(Edge {
                    width: if double { 0.5 } else { width },
                    color: child(e, "color").and_then(|c| color(c, theme)).unwrap_or([0.0; 3]),
                    dash,
                    double,
                })
            };
            let up = flag(attr(n, "diagonalUp")) && attr(n, "diagonalUp").is_some();
            let down = flag(attr(n, "diagonalDown")) && attr(n, "diagonalDown").is_some();
            s.borders.push(Edges {
                left: edge(["left", "start"]),
                right: edge(["right", "end"]),
                top: edge(["top", "top"]),
                bottom: edge(["bottom", "bottom"]),
                diagonal: edge(["diagonal", "diagonal"]).filter(|_| up || down).map(|e| (e, up, down)),
            });
        }
    }
    s.dxfs = cf::dxfs(root, &|n| color(n, theme));
    for t in child(root, "tableStyles").into_iter().flat_map(|t| children(t, "tableStyle")).take(1_000) {
        let Some(name) = attr(t, "name") else { continue };
        let mut parts = [None; 7];
        for e in children(t, "tableStyleElement") {
            let slot = match attr(e, "type") {
                Some("wholeTable") => 0,
                Some("headerRow") => 1,
                Some("totalRow") => 2,
                Some("firstRowStripe") => 3,
                Some("secondRowStripe") => 4,
                Some("firstColumn") => 5,
                Some("lastColumn") => 6,
                _ => continue,
            };
            parts[slot] = attr_f64(e, "dxfId").filter(|v| v.is_finite() && *v >= 0.0).map(|v| v.min(1e6) as usize);
        }
        s.table_styles.insert(name.to_owned(), parts);
    }
    if let Some(f) = child(root, "cellXfs") {
        for n in children(f, "xf") {
            let al = child(n, "alignment");
            let idx = |k: &str| attr_f64(n, k).map_or(0, |v| v.clamp(0.0, 1e6) as usize);
            s.xfs.push(Xf {
                num_fmt: attr_f64(n, "numFmtId").map_or(0, |v| v.clamp(0.0, 1e6) as u32),
                font: idx("fontId"),
                fill: idx("fillId"),
                border: idx("borderId"),
                h_align: al.and_then(|a| attr(a, "horizontal")).map(str::to_owned),
                v_align: al.and_then(|a| attr(a, "vertical")).map(str::to_owned),
                wrap: al.is_some_and(|a| flag(attr(a, "wrapText")) && attr(a, "wrapText").is_some()),
                indent: al.and_then(|a| attr_f64(a, "indent")).unwrap_or(0.0).clamp(0.0, 250.0),
                rotation: al.and_then(|a| attr_f64(a, "textRotation")).map_or(0, |v| v.clamp(0.0, 255.0) as u16).min(255),
                shrink: al.is_some_and(|a| flag(attr(a, "shrinkToFit")) && attr(a, "shrinkToFit").is_some()),
            });
        }
    }
    Ok(s)
}

// ── number formats ──────────────────────────────────────────────────────────────────────────

fn builtin_format(id: u32) -> &'static str {
    match id {
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        5 => "\"$\"#,##0_);(\"$\"#,##0)",
        6 => "\"$\"#,##0_);[Red](\"$\"#,##0)",
        7 => "\"$\"#,##0.00_);(\"$\"#,##0.00)",
        8 => "\"$\"#,##0.00_);[Red](\"$\"#,##0.00)",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        // The regional short date; written unambiguously.
        14 => "yyyy-mm-dd",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "yyyy-mm-dd h:mm",
        37 => "#,##0 ;(#,##0)",
        38 => "#,##0 ;[Red](#,##0)",
        39 => "#,##0.00;(#,##0.00)",
        40 => "#,##0.00;[Red](#,##0.00)",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mm:ss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => "General",
    }
}

/// Locales that write a decimal comma (by Windows LCID), from a `[$-xxxx]` tag.
fn comma_decimal(lcid: u32) -> bool {
    matches!(
        lcid & 0x3FF,
        0x16 | 0x07
            | 0x0C
            | 0x0A
            | 0x10
            | 0x13
            | 0x15
            | 0x19
            | 0x1F
            | 0x05
            | 0x0E
            | 0x1B
            | 0x24
            | 0x1A
            | 0x06
            | 0x0B
            | 0x1D
            | 0x14
            | 0x02
            | 0x22
            | 0x26
            | 0x27
            | 0x25
            | 0x18
            | 0x08
    )
}

fn portuguese(lcid: u32) -> bool {
    lcid & 0x3FF == 0x16
}

fn general(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    if v.fract() == 0.0 && v.abs() < 1e11 {
        return format!("{v:.0}");
    }
    let a = v.abs();
    if !(1e-9..1e11).contains(&a) {
        let s = format!("{v:.5E}");
        // Rust writes 1.23457E15; Excel writes 1.23457E+15.
        let (m, e) = s.split_once('E').unwrap_or((&s, "0"));
        let m = m.trim_end_matches('0').trim_end_matches('.');
        let e: i32 = e.parse().unwrap_or(0);
        return format!("{m}E{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
    }
    // Ten significant digits, trailing zeros dropped.
    let digits = (9 - a.log10().floor() as i32).clamp(0, 10) as usize;
    let s = format!("{v:.digits$}");
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
}

fn group(int: &str, sep: char) -> String {
    let mut out = String::new();
    let n = int.len();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (n - i).is_multiple_of(3) {
            out.push(sep);
        }
        out.push(c);
    }
    out
}

/// Split a format code into its `;` sections, outside quotes and brackets.
fn sections(code: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let (mut quoted, mut bracket, mut escape) = (false, false, false);
    for c in code.chars() {
        if let Some(cur) = out.last_mut() {
            if escape {
                cur.push(c);
                escape = false;
                continue;
            }
            match c {
                '\\' => {
                    escape = true;
                    cur.push(c);
                }
                '"' => {
                    quoted = !quoted;
                    cur.push(c);
                }
                '[' if !quoted => {
                    bracket = true;
                    cur.push(c);
                }
                ']' if !quoted => {
                    bracket = false;
                    cur.push(c);
                }
                ';' if !quoted && !bracket => out.push(String::new()),
                c => cur.push(c),
            }
        }
        if out.len() > 4 {
            break;
        }
    }
    out
}

/// A parsed format section: literal text and placeholders in order.
#[derive(Debug, Default)]
struct Section {
    prefix: String,
    suffix: String,
    int_zeros: usize,
    int_any: bool,
    dec_zeros: usize,
    dec_hash: usize,
    grouping: bool,
    percent: usize,
    scale: i32,
    sci: Option<usize>,
    text_at: bool,
    date: bool,
    tokens: Vec<String>,
    color: Option<[f64; 3]>,
    lcid: u32,
}

fn parse_section(sec: &str) -> Section {
    let mut s = Section::default();
    let mut lit = String::new();
    let mut seen_digit = false;
    let mut in_dec = false;
    let chars: Vec<char> = sec.chars().collect();
    let lower: Vec<char> = chars.iter().map(char::to_ascii_lowercase).collect();
    let starts_at = |at: usize, word: &str| word.chars().enumerate().all(|(k, w)| lower.get(at + k) == Some(&w));
    // A date/time format has date or time letters outside quotes, escapes and brackets.
    {
        let (mut q, mut b, mut esc) = (false, false, false);
        for c in &lower {
            match c {
                _ if esc => esc = false,
                '\\' if !q => esc = true,
                '"' => q = !q,
                '[' if !q => b = true,
                ']' if !q => b = false,
                'y' | 'd' | 'h' | 's' | 'm' if !q && !b => s.date = true,
                _ => {}
            }
        }
        if sec.to_ascii_lowercase().contains("general") {
            s.date = false;
        }
    }
    let mut i = 0usize;
    let flush = |s: &mut Section, lit: &mut String, seen: bool| {
        if seen {
            s.suffix.push_str(lit);
        } else {
            s.prefix.push_str(lit);
        }
        lit.clear();
    };
    while let Some(&c) = chars.get(i) {
        i += 1;
        match c {
            '"' => {
                while let Some(&q) = chars.get(i) {
                    i += 1;
                    if q == '"' {
                        break;
                    }
                    lit.push(q);
                }
                if s.date {
                    s.tokens.push(format!("\"{lit}"));
                    lit.clear();
                }
            }
            '\\' => {
                if let Some(&q) = chars.get(i) {
                    i += 1;
                    if s.date {
                        s.tokens.push(format!("\"{q}"));
                    } else {
                        lit.push(q);
                    }
                }
            }
            '_' => {
                // Space as wide as the next character.
                i += 1;
                if s.date {
                    s.tokens.push("\" ".into());
                } else {
                    lit.push(' ');
                }
            }
            '*' => i += 1,
            '[' => {
                let mut tag = String::new();
                while let Some(&q) = chars.get(i) {
                    i += 1;
                    if q == ']' {
                        break;
                    }
                    tag.push(q);
                }
                let lower = tag.to_ascii_lowercase();
                if let Some(c) = format_color(&tag) {
                    s.color = Some(c);
                } else if let Some(rest) = tag.strip_prefix('$') {
                    let (sym, lcid) = rest.split_once('-').unwrap_or((rest, ""));
                    s.lcid = u32::from_str_radix(lcid.trim(), 16).unwrap_or(0);
                    if s.date {
                        if !sym.is_empty() {
                            s.tokens.push(format!("\"{sym}"));
                        }
                    } else {
                        lit.push_str(sym);
                    }
                } else if s.date && matches!(lower.as_str(), "h" | "hh" | "m" | "mm" | "s" | "ss") {
                    s.tokens.push(format!("[{lower}]"));
                }
            }
            '@' => {
                s.text_at = true;
                flush(&mut s, &mut lit, seen_digit);
                seen_digit = true;
            }
            _ if s.date => {
                let lc = c.to_ascii_lowercase();
                if matches!(lc, 'y' | 'm' | 'd' | 'h' | 's') {
                    let mut tok = lc.to_string();
                    while chars.get(i).is_some_and(|n| n.to_ascii_lowercase() == lc) {
                        tok.push(lc);
                        i += 1;
                    }
                    s.tokens.push(tok);
                } else if starts_at(i - 1, "am/pm") {
                    s.tokens.push("ampm".into());
                    i += 4;
                } else if starts_at(i - 1, "a/p") {
                    s.tokens.push("ap".into());
                    i += 2;
                } else if c == '0' {
                    let mut tok = "0".to_string();
                    while chars.get(i) == Some(&'0') {
                        tok.push('0');
                        i += 1;
                    }
                    s.tokens.push(tok);
                } else {
                    s.tokens.push(format!("\"{c}"));
                }
            }
            '0' | '#' | '?' => {
                if !seen_digit {
                    flush(&mut s, &mut lit, false);
                }
                seen_digit = true;
                if s.sci.is_some() {
                    s.sci = s.sci.map(|n| n + 1);
                } else if in_dec {
                    if c == '0' {
                        s.dec_zeros += 1 + s.dec_hash;
                        s.dec_hash = 0;
                    } else {
                        s.dec_hash += 1;
                    }
                } else {
                    s.int_any = true;
                    if c == '0' {
                        s.int_zeros += 1;
                    }
                }
                // Trailing commas after the last digit scale by thousands.
                s.scale = 0;
            }
            '.' if seen_digit || chars.get(i).is_some_and(|n| matches!(n, '0' | '#' | '?')) => {
                if !seen_digit {
                    flush(&mut s, &mut lit, false);
                    seen_digit = true;
                }
                in_dec = true;
            }
            ',' if seen_digit && s.sci.is_none() => {
                if chars.get(i).is_some_and(|n| matches!(n, '0' | '#' | '?')) {
                    s.grouping = true;
                } else {
                    s.scale += 1;
                }
            }
            'E' | 'e' if seen_digit && chars.get(i).is_some_and(|n| matches!(n, '+' | '-')) => {
                i += 1;
                s.sci = Some(0);
            }
            '%' => {
                s.percent += 1;
                lit.push('%');
            }
            c => lit.push(c),
        }
    }
    flush(&mut s, &mut lit, seen_digit);
    s
}

const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const MESES: [&str; 12] =
    ["janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro"];
const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const DIAS: [&str; 7] = ["domingo", "segunda-feira", "terça-feira", "quarta-feira", "quinta-feira", "sexta-feira", "sábado"];

fn date(serial: f64, sec: &Section, date1904: bool) -> Option<String> {
    if !serial.is_finite() || !(-1.0..3_000_000.0).contains(&serial) {
        return None;
    }
    let day = serial.floor() as i64;
    let mut secs = ((serial - serial.floor()) * 86_400.0).round() as i64;
    let day = day + secs / 86_400;
    secs %= 86_400;
    // Days from 1 January of year 1 (chrono's day 1). The 1904 system's day 0 is 1 January 1904;
    // the 1900 system's day 1 is 1 January 1900, and it counts a 29 February 1900 that never was
    // (serial 60).
    let ce_days = if date1904 {
        day + 695_056
    } else if day >= 60 {
        day + 693_594
    } else {
        day + 693_595
    };
    let date = chrono::NaiveDate::from_num_days_from_ce_opt(i32::try_from(ce_days).ok()?)?;
    let (y, m, d) = (i64::from(date.year()), date.month(), date.day());
    let weekday = date.weekday().num_days_from_sunday() as usize;
    let (h, mi, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    let ampm = sec.tokens.iter().any(|t| t == "ampm" || t == "ap");
    let pt = portuguese(sec.lcid);
    let mut out = String::new();
    let toks = &sec.tokens;
    for (i, t) in toks.iter().enumerate() {
        // `m` means minutes after an hour or before a second.
        let minute = t.starts_with('m')
            && t.len() <= 2
            && (toks.get(..i).and_then(|p| p.iter().rev().find(|x| !x.starts_with('"'))).is_some_and(|p| p.starts_with('h') || p.starts_with("[h"))
                || toks.get(i + 1..).and_then(|n| n.iter().find(|x| !x.starts_with('"'))).is_some_and(|n| n.starts_with('s')));
        let month_name = |full: bool| {
            let names = if pt { &MESES } else { &MONTHS };
            let name = names.get(m.saturating_sub(1) as usize).copied().unwrap_or("");
            if full { name.to_string() } else { name.chars().take(3).collect() }
        };
        match t.as_str() {
            "yy" => out.push_str(&format!("{:02}", y.rem_euclid(100))),
            t if t.starts_with('y') => out.push_str(&format!("{y:04}")),
            "m" if minute => out.push_str(&mi.to_string()),
            "mm" if minute => out.push_str(&format!("{mi:02}")),
            "m" => out.push_str(&m.to_string()),
            "mm" => out.push_str(&format!("{m:02}")),
            "mmm" => out.push_str(&month_name(false)),
            "mmmmm" => out.push_str(&month_name(false).chars().take(1).collect::<String>()),
            t if t.starts_with("mmmm") => out.push_str(&month_name(true)),
            "d" => out.push_str(&d.to_string()),
            "dd" => out.push_str(&format!("{d:02}")),
            "ddd" | "dddd" => {
                let names = if pt { &DIAS } else { &DAYS };
                let name = names.get(weekday).copied().unwrap_or("");
                if t.len() == 3 {
                    out.push_str(&name.chars().take(3).collect::<String>());
                } else {
                    out.push_str(name);
                }
            }
            "h" | "hh" => {
                let hv = if ampm { (h + 11) % 12 + 1 } else { h };
                out.push_str(&if t.len() == 2 { format!("{hv:02}") } else { hv.to_string() });
            }
            "[h]" | "[hh]" => out.push_str(&(day * 24 + h).to_string()),
            "[m]" | "[mm]" => out.push_str(&((day * 24 + h) * 60 + mi).to_string()),
            "s" => out.push_str(&s.to_string()),
            "ss" => out.push_str(&format!("{s:02}")),
            "ampm" => out.push_str(if h < 12 { "AM" } else { "PM" }),
            "ap" => out.push_str(if h < 12 { "A" } else { "P" }),
            t if t.starts_with('"') => out.push_str(t.get(1..).unwrap_or("")),
            _ => {}
        }
    }
    Some(out)
}

/// Format a number with a format code. Returns the text and its section's colour, if any.
fn format_number(v: f64, code: &str, date1904: bool) -> (String, Option<[f64; 3]>) {
    if code.eq_ignore_ascii_case("general") || code.is_empty() {
        return (general(v), None);
    }
    let secs = sections(code);
    let (sec_text, negative_section) = match secs.first().and_then(|s| section_condition(s)) {
        // Conditional sections (`[>=100]0;[<0]-0.0;0.00`): the first whose condition holds, else
        // the last; the number keeps its sign.
        Some(first) => {
            if first.holds(v) {
                (secs.first(), false)
            } else {
                match secs.get(1).and_then(|s| section_condition(s)) {
                    Some(second) if second.holds(v) => (secs.get(1), false),
                    Some(_) => (secs.get(2).or(secs.get(1)), false),
                    None => (secs.get(1).or(secs.first()), false),
                }
            }
        }
        None => match (secs.len(), v) {
            (_, v) if v > 0.0 => (secs.first(), false),
            (1, _) => (secs.first(), false),
            (_, v) if v < 0.0 => (secs.get(1), true),
            (2, _) => (secs.first(), false),
            _ => (secs.get(2).or(secs.first()), false),
        },
    };
    let sec = parse_section(sec_text.map_or("General", String::as_str));
    if !sec.date
        && let Some(f) = sec_text.and_then(|s| fraction(if negative_section { v.abs() } else { v }, s))
    {
        return (f, sec.color);
    }
    if sec.date {
        return (date(v, &sec, date1904).unwrap_or_else(|| general(v)), sec.color);
    }
    if sec_text.is_some_and(|s| s.to_ascii_lowercase().contains("general")) {
        let g = general(if negative_section { v.abs() } else { v });
        return (format!("{}{g}{}", sec.prefix.replace("General", ""), sec.suffix), sec.color);
    }
    let (dec_sep, group_sep) = if comma_decimal(sec.lcid) { (',', '.') } else { ('.', ',') };
    let mut x = if negative_section { v.abs() } else { v };
    for _ in 0..sec.percent.min(3) {
        x *= 100.0;
    }
    for _ in 0..sec.scale.clamp(0, 5) {
        x /= 1000.0;
    }
    let decimals = (sec.dec_zeros + sec.dec_hash).min(15);
    // Excel rounds halves away from zero; Rust's formatting rounds them to even.
    if sec.sci.is_none() {
        let k = 10f64.powi(decimals as i32);
        let r = (x * k).round() / k;
        if r.is_finite() {
            x = r;
        }
    }
    let body = if let Some(exp_digits) = sec.sci {
        let s = format!("{:.*E}", decimals, x.abs());
        let (m, e) = s.split_once('E').unwrap_or((&s, "0"));
        let e: i32 = e.parse().unwrap_or(0);
        format!(
            "{}{}E{}{:0w$}",
            if x < 0.0 { "-" } else { "" },
            m.replace('.', &dec_sep.to_string()),
            if e < 0 { '-' } else { '+' },
            e.abs(),
            w = exp_digits.max(1)
        )
    } else {
        let s = format!("{:.*}", decimals, x.abs());
        let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
        let mut int = int.trim_start_matches('0').to_string();
        if int.len() < sec.int_zeros {
            int.insert_str(0, &"0".repeat(sec.int_zeros - int.len()));
        }
        if sec.grouping {
            int = group(&int, group_sep);
        }
        let mut frac = frac.to_string();
        while frac.len() > sec.dec_zeros && frac.ends_with('0') {
            frac.pop();
        }
        let neg = x < 0.0 && (int.chars().any(|c| c.is_ascii_digit() && c != '0') || frac.chars().any(|c| c != '0'));
        let mut b = String::new();
        if neg {
            b.push('-');
        }
        b.push_str(&int);
        if !frac.is_empty() {
            b.push(dec_sep);
            b.push_str(&frac);
        } else if decimals > 0 && sec.dec_zeros > 0 {
            b.push(dec_sep);
        }
        if !sec.int_any && sec.int_zeros == 0 && decimals == 0 && !sec.text_at {
            // A format of only literal text (e.g. `"-"` for zero).
            String::new()
        } else {
            b
        }
    };
    (format!("{}{body}{}", sec.prefix, sec.suffix), sec.color)
}

/// A number format section's condition (`[>100]`, `[<=0]`, `[<>5]`).
#[derive(Clone, Copy)]
struct Condition {
    op: &'static str,
    value: f64,
}

impl Condition {
    fn holds(self, v: f64) -> bool {
        match self.op {
            "<" => v < self.value,
            "<=" => v <= self.value,
            ">" => v > self.value,
            ">=" => v >= self.value,
            "<>" => v != self.value,
            _ => v == self.value,
        }
    }
}

fn section_condition(sec: &str) -> Option<Condition> {
    let (mut quoted, mut i) = (false, 0usize);
    let chars: Vec<char> = sec.chars().collect();
    while let Some(&c) = chars.get(i) {
        match c {
            '"' => quoted = !quoted,
            '[' if !quoted => {
                let tag: String = chars[i + 1..].iter().take_while(|c| **c != ']').collect();
                let op = ["<=", ">=", "<>", "<", ">", "="].into_iter().find(|op| tag.starts_with(op));
                if let Some(op) = op
                    && let Ok(value) = tag[op.len()..].trim().parse::<f64>()
                {
                    return Some(Condition { op, value });
                }
                i += tag.chars().count() + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// A fraction format (`# ?/?`, `# ??/??`, `?/8`): the whole part when the section has one, then
/// the nearest fraction with at most as many denominator digits as it asks (or its fixed
/// denominator), padded where it uses `?`.
fn fraction(v: f64, sec: &str) -> Option<String> {
    // The section without its quoted text and bracketed tags.
    let mut plain = String::new();
    let (mut quoted, mut tag) = (false, false);
    for c in sec.chars() {
        match c {
            '"' => quoted = !quoted,
            '[' if !quoted => tag = true,
            ']' if tag => tag = false,
            _ if quoted || tag => {}
            _ => plain.push(c),
        }
    }
    let (left, right) = plain.split_once('/')?;
    let num_len = left.chars().rev().take_while(|c| matches!(c, '#' | '?' | '0')).count();
    let den: String = right.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '#' | '?' | '0')).collect();
    if num_len == 0 || den.is_empty() {
        return None;
    }
    let whole = left[..left.len() - num_len].trim_end().chars().any(|c| matches!(c, '#' | '?' | '0'));
    if !v.is_finite() || v.abs() > 1e15 {
        return None;
    }
    let a = v.abs();
    let (mut int, frac) = if whole { (a.trunc(), a.fract()) } else { (0.0, a) };
    let (mut n, d) = match den.parse::<u64>() {
        Ok(fixed) if fixed > 0 => ((frac * fixed as f64).round() as u64, fixed),
        _ => {
            let max = 10u64.pow(den.len().min(4) as u32) - 1;
            let mut best = (frac.round() as u64, 1u64, f64::INFINITY);
            for d in 1..=max.max(1) {
                let n = (frac * d as f64).round();
                let err = (frac - n / d as f64).abs();
                if err < best.2 - 1e-12 {
                    best = (n as u64, d, err);
                }
            }
            (best.0, best.1)
        }
    };
    if whole && n >= d {
        int += (n / d) as f64;
        n %= d;
    }
    let sign = if v < 0.0 { "-" } else { "" };
    let pad = |s: String, len: usize, right: bool| if right { format!("{s:<len$}") } else { format!("{s:>len$}") };
    if n == 0 && whole {
        // A whole number: the fraction's room is left blank.
        return Some(format!("{sign}{int:.0}{}", " ".repeat(num_len + den.len() + 2)));
    }
    let f = format!("{}/{}", pad(n.to_string(), num_len, false), pad(d.to_string(), den.len(), true));
    Some(match (whole, int > 0.0) {
        (true, true) => format!("{sign}{int:.0} {f}"),
        (true, false) => format!("{sign}{f}"),
        (false, _) => format!("{sign}{}", f.trim_start()),
    })
}

/// The shortest General form of `v` whose width fits `room`: fewer decimals, then scientific
/// notation with fewer digits, as Excel shows General numbers in narrow columns.
fn general_fit(v: f64, room: f64, width: &dyn Fn(&str) -> f64) -> Option<String> {
    let full = general(v);
    if width(&full) <= room {
        return Some(full);
    }
    for digits in (0..10usize).rev() {
        let s = format!("{v:.digits$}");
        let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
        if (v.abs() < 1e11 || digits == 0) && width(&s) <= room && s.trim_start_matches('-') != "0" {
            return Some(s);
        }
    }
    for digits in (0..6usize).rev() {
        let s = format!("{v:.digits$E}");
        let (m, e) = s.split_once('E').unwrap_or((&s, "0"));
        let e: i32 = e.parse().unwrap_or(0);
        let s = format!("{m}E{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
        if width(&s) <= room {
            return Some(s);
        }
    }
    None
}

/// Format text with a format's text section (its fourth, or an only section holding `@`): the
/// literals around `@` and the section's colour. Other formats show text as it is.
fn format_text(t: &str, code: &str) -> (String, Option<[f64; 3]>) {
    let secs = sections(code);
    let sec = match secs.len() {
        4.. => secs.get(3),
        1 if secs.first().is_some_and(|s| s.contains('@')) => secs.first(),
        _ => None,
    };
    let Some(sec) = sec else { return (t.to_owned(), None) };
    let (mut out, mut colour) = (String::new(), None);
    let mut chars = sec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => out.extend(chars.by_ref().take_while(|q| *q != '"')),
            '\\' => out.extend(chars.next()),
            '_' => {
                chars.next();
                out.push(' ');
            }
            '*' => {
                chars.next();
            }
            '[' => {
                let tag: String = chars.by_ref().take_while(|q| *q != ']').collect();
                colour = format_color(&tag).or(colour);
            }
            '@' => out.push_str(t),
            _ => out.push(c),
        }
        if out.len() > MAX_TEXT_BYTES {
            break;
        }
    }
    (out, colour)
}

/// A `t="d"` cell's ISO 8601 date, time or both as a date serial.
fn iso_serial(s: &str, date1904: bool) -> Option<f64> {
    let s = s.trim();
    let (day, time) = match s.split_once('T') {
        Some((d, t)) => (Some(d), Some(t)),
        None if s.contains(':') => (None, Some(s)),
        None => (Some(s), None),
    };
    let mut serial = 0.0;
    if let Some(day) = day {
        let date = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
        let base = if date1904 { chrono::NaiveDate::from_ymd_opt(1904, 1, 1)? } else { chrono::NaiveDate::from_ymd_opt(1899, 12, 30)? };
        serial = (date - base).num_days() as f64;
        // The 1900 system counts a 29 February 1900 that never was: earlier days are one less.
        if !date1904 && serial < 61.0 {
            serial -= 1.0;
        }
    }
    if let Some(time) = time {
        let time = time.trim_end_matches('Z');
        let time = time.find(['+', '-']).map_or(time, |i| &time[..i]);
        let t = chrono::NaiveTime::parse_from_str(time, "%H:%M:%S%.f").or_else(|_| chrono::NaiveTime::parse_from_str(time, "%H:%M")).ok()?;
        serial += f64::from(t.num_seconds_from_midnight()) / 86_400.0 + f64::from(t.nanosecond()) / 86_400e9;
    }
    serial.is_finite().then_some(serial)
}

// ── sheets ──────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Cell {
    text: String,
    /// The saved number, for conditional formatting.
    value: Option<f64>,
    numeric: bool,
    boolean: bool,
    xf: usize,
    /// The number format's colour, over the font's.
    color: Option<[f64; 3]>,
    /// Rich text: the runs of a shared or inline string with fonts of their own.
    runs: Option<Arc<Vec<Rich>>>,
    /// The cell's hyperlink.
    link: Option<Arc<LinkTarget>>,
    /// A number in the General format, which is shortened to fit a narrow column.
    general: bool,
}

/// A run of rich text in a cell and, when it has run properties, its own font.
#[derive(Clone, Debug)]
struct Rich {
    text: String,
    look: Option<RichLook>,
}

/// A rich text run's properties (`rPr`): what it leaves out comes from the cell's font for the
/// name, size and colour, and is off for the rest.
#[derive(Clone, Debug, Default)]
struct RichLook {
    name: Option<String>,
    size: Option<f64>,
    color: Option<[f64; 3]>,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    vert: i8,
}

/// A shared string: its text and, when its runs carry fonts, the runs.
struct Shared {
    text: String,
    runs: Option<Arc<Vec<Rich>>>,
}

/// A string item's (`si`, `is`) text and rich runs. Phonetic runs (rPh) are reading aids, not text.
fn string_item(si: Node<'_, '_>, theme: &[[f64; 3]]) -> Shared {
    let text = si.descendants().filter(|n| is(*n, "t") && !n.ancestors().any(|a| is(a, "rPh"))).filter_map(|t| t.text()).collect();
    let runs: Vec<Rich> = children(si, "r")
        .map(|r| Rich {
            text: xml::text_of(r, "t"),
            look: child(r, "rPr").map(|p| {
                let on = |k: &str| child(p, k).is_some_and(|c| flag(attr(c, "val")));
                RichLook {
                    name: child(p, "rFont").and_then(|c| attr(c, "val")).map(str::to_owned),
                    size: child(p, "sz").and_then(|c| attr_f64(c, "val")).filter(|v| v.is_finite()).map(|v| v.clamp(1.0, 400.0)),
                    color: child(p, "color").and_then(|c| color(c, theme)),
                    bold: on("b"),
                    italic: on("i"),
                    underline: child(p, "u").is_some_and(|c| attr(c, "val") != Some("none")),
                    strike: on("strike"),
                    vert: match child(p, "vertAlign").and_then(|c| attr(c, "val")) {
                        Some("superscript") => 1,
                        Some("subscript") => -1,
                        _ => 0,
                    },
                }
            }),
        })
        .collect();
    let runs = runs.iter().any(|r| r.look.is_some()).then(|| Arc::new(runs));
    Shared { text, runs }
}

/// The year and month of a date serial (1900 date system).
pub(super) fn year_month(serial: f64) -> Option<(i64, u32)> {
    if !serial.is_finite() || !(1.0..3_000_000.0).contains(&serial) {
        return None;
    }
    let day = serial.floor() as i64;
    let ce_days = if day >= 60 { day + 693_594 } else { day + 693_595 };
    let date = chrono::NaiveDate::from_num_days_from_ce_opt(i32::try_from(ce_days).ok()?)?;
    Some((i64::from(date.year()), date.month()))
}

pub(super) fn col_index(r: &str) -> Option<(u32, u32)> {
    let letters: String = r.chars().take_while(char::is_ascii_alphabetic).collect();
    let digits = r.get(letters.len()..)?;
    let mut col = 0u32;
    for c in letters.chars() {
        col = col.checked_mul(26)?.checked_add(u32::from(c.to_ascii_uppercase() as u8 - b'A' + 1))?;
    }
    let row: u32 = digits.parse().ok()?;
    ((1..=MAX_COL).contains(&col) && (1..=MAX_ROW).contains(&row)).then_some((row, col))
}

/// Points for an Excel column width in characters, `mdw` being the maximum digit width in
/// pixels (Calibri 11: 7 px in Excel for Windows, 8 px in Excel for Mac).
fn col_points(chars: f64, mdw: f64) -> f64 {
    let px = (chars * mdw + 5.0).floor().max(0.0);
    px * 0.75
}

/// A stored column width (`<col width>`, `defaultColWidth`) in points. Stored widths already
/// include the 5 px of padding (ECMA-376 Part 1, §18.3.1.13): pixels are
/// `trunc((256·width + trunc(128/mdw)) / 256 · mdw)`.
fn stored_col_points(width: f64, mdw: f64) -> f64 {
    let px = ((256.0 * width + (128.0 / mdw).trunc()) / 256.0 * mdw).trunc().max(0.0);
    px * 0.75
}

struct Sheet {
    cells: BTreeMap<(u32, u32), Cell>,
    widths: HashMap<u32, f64>,
    hidden_cols: std::collections::HashSet<u32>,
    heights: HashMap<u32, (f64, bool)>,
    hidden_rows: std::collections::HashSet<u32>,
    default_w: f64,
    default_h: f64,
    merges: Vec<(u32, u32, u32, u32)>,
    gridlines: bool,
    landscape: bool,
    /// Print scale: `pageSetup@scale`, or fit to a number of pages wide and tall.
    scale: f64,
    fit: Option<(u32, u32)>,
    paper: (f64, f64),
    margins: [f64; 4],
    /// Distances of the header from the page's top and of the footer from its bottom.
    hf_margins: (f64, f64),
    hf: HeaderFooter,
    hf_pictures: Pictures,
    h_center: bool,
    v_center: bool,
    /// Row numbers and column letters printed with the cells (`printOptions@headings`).
    headings: bool,
    /// Pages printed across, then down (`pageOrder="overThenDown"`).
    over_then_down: bool,
    /// Manual page breaks: a new page starts after these rows and columns.
    row_breaks: BTreeSet<u32>,
    col_breaks: BTreeSet<u32>,
    /// Print areas (first row, first column, last row, last column), each printed on its own
    /// pages, and the rows and columns printed on every page.
    print_areas: Vec<(u32, u32, u32, u32)>,
    title_rows: Option<(u32, u32)>,
    title_cols: Option<(u32, u32)>,
    drawings: Vec<Drawn>,
    /// How conditional formatting changes cells.
    looks: HashMap<(u32, u32), Look>,
    /// Comments printed after the sheet (`cellComments="atEnd"`): cell reference and text.
    end_comments: Vec<(String, String)>,
    /// The sheet's own first page number (`pageSetup useFirstPageNumber firstPageNumber`).
    first_number: Option<usize>,
    /// Sparklines: the cell each is drawn in, its kind (0 line, 1 column, 2 win/loss), colour and
    /// values.
    sparklines: Vec<Sparkline>,
    /// Formatted tables' (Insert ▸ Table) look in their cells: fill, font colour, bold, and
    /// borders, under the cells' own formatting.
    table_looks: HashMap<(u32, u32), (TableCell, Edges)>,
}

/// What a sheet is read with besides its part.
struct Book<'a> {
    shared: &'a [Shared],
    styles: &'a Styles,
    theme: &'a [[f64; 3]],
    date1904: bool,
    /// Today as a date serial, for time-period rules.
    today: f64,
    /// The maximum digit width column widths count in, in pixels: Excel for Mac's (8) for a
    /// workbook it saved, else Excel for Windows' (7). Column widths are stored in digits, so
    /// the same file prints wider on a Mac.
    mdw: f64,
}

fn read_sheet(zip: &Zip<'_>, part: &str, book: &Book<'_>, doc: &mut Document, notes: &mut Notes) -> Result<Sheet, String> {
    let (shared, styles, date1904) = (book.shared, book.styles, book.date1904);
    let text = zip.text(part)?.ok_or_else(|| format!("{part} is missing"))?;
    let d = xml::parse(&text).map_err(|e| format!("{part}: {e}"))?;
    let root = d.root_element();
    let fmt = child(root, "sheetFormatPr");
    let base = fmt.and_then(|f| attr_f64(f, "baseColWidth")).unwrap_or(8.0);
    let mdw = book.mdw;
    let default_w = fmt.and_then(|f| attr_f64(f, "defaultColWidth")).map_or(col_points(base + 0.43, mdw), |w| stored_col_points(w, mdw));
    let default_h = fmt.and_then(|f| attr_f64(f, "defaultRowHeight")).unwrap_or(15.0).clamp(1.0, 409.0);
    let mut sh = Sheet {
        cells: BTreeMap::new(),
        widths: HashMap::new(),
        hidden_cols: Default::default(),
        heights: HashMap::new(),
        hidden_rows: Default::default(),
        default_w: fin(default_w, 48.0).clamp(1.0, 2000.0),
        default_h,
        merges: Vec::new(),
        gridlines: child(root, "printOptions").is_some_and(|p| flag(attr(p, "gridLines")) && attr(p, "gridLines").is_some()),
        landscape: child(root, "pageSetup").and_then(|p| attr(p, "orientation")) == Some("landscape"),
        scale: child(root, "pageSetup").and_then(|p| attr_f64(p, "scale")).map_or(1.0, |s| (s / 100.0).clamp(0.1, 4.0)),
        fit: xml::path(root, &["sheetPr", "pageSetUpPr"]).filter(|p| flag(attr(*p, "fitToPage")) && attr(*p, "fitToPage").is_some()).map(|_| {
            let setup = child(root, "pageSetup");
            let n = |k: &str| setup.and_then(|p| attr_f64(p, k)).map_or(1, |v| v.clamp(0.0, 10_000.0) as u32);
            (n("fitToWidth"), n("fitToHeight"))
        }),
        paper: match child(root, "pageSetup").and_then(|p| attr_f64(p, "paperSize")).map(|v| v as i64) {
            Some(9) => (595.28, 841.89),
            Some(5) => (612.0, 1008.0),
            Some(8) => (841.89, 1190.55),
            Some(11) => (419.53, 595.28),
            _ => (612.0, 792.0),
        },
        margins: [54.0, 50.4, 54.0, 50.4],
        hf_margins: (21.6, 21.6),
        hf: HeaderFooter::default(),
        hf_pictures: Pictures::new(),
        h_center: child(root, "printOptions").is_some_and(|p| flag(attr(p, "horizontalCentered")) && attr(p, "horizontalCentered").is_some()),
        headings: child(root, "printOptions").is_some_and(|p| flag(attr(p, "headings")) && attr(p, "headings").is_some()),
        v_center: child(root, "printOptions").is_some_and(|p| flag(attr(p, "verticalCentered")) && attr(p, "verticalCentered").is_some()),
        over_then_down: child(root, "pageSetup").and_then(|p| attr(p, "pageOrder")) == Some("overThenDown"),
        row_breaks: BTreeSet::new(),
        col_breaks: BTreeSet::new(),
        print_areas: Vec::new(),
        title_rows: None,
        title_cols: None,
        drawings: Vec::new(),
        looks: HashMap::new(),
        end_comments: Vec::new(),
        table_looks: HashMap::new(),
        sparklines: Vec::new(),
        first_number: child(root, "pageSetup")
            .filter(|p| flag(attr(*p, "useFirstPageNumber")) && attr(*p, "useFirstPageNumber").is_some())
            .and_then(|p| attr_f64(p, "firstPageNumber"))
            .filter(|v| v.is_finite())
            .map(|v| v.clamp(0.0, 1e6) as usize),
    };
    if let Some(m) = child(root, "pageMargins") {
        let g = |k: &str, d: f64| attr_f64(m, k).map_or(d, |v| (v * 72.0).clamp(0.0, 300.0));
        sh.margins = [g("top", 54.0), g("right", 50.4), g("bottom", 54.0), g("left", 50.4)];
        sh.hf_margins = (g("header", 21.6), g("footer", 21.6));
    }
    if let Some(h) = child(root, "headerFooter") {
        let text = |k: &str| child(h, k).and_then(|c| c.text()).map(str::to_owned);
        sh.hf = HeaderFooter {
            odd_header: text("oddHeader"),
            odd_footer: text("oddFooter"),
            even_header: text("evenHeader"),
            even_footer: text("evenFooter"),
            first_header: text("firstHeader"),
            first_footer: text("firstFooter"),
            different_first: flag(attr(h, "differentFirst")) && attr(h, "differentFirst").is_some(),
            different_odd_even: flag(attr(h, "differentOddEven")) && attr(h, "differentOddEven").is_some(),
        };
    }
    // Manual breaks: `id` is the last row or column before the break (fit-to-page ignores them).
    for (tag, set) in [("rowBreaks", &mut sh.row_breaks), ("colBreaks", &mut sh.col_breaks)] {
        for b in child(root, tag).into_iter().flat_map(|b| children(b, "brk")).take(1_026) {
            if let Some(id) = attr_f64(b, "id").filter(|v| v.is_finite() && *v >= 1.0) {
                set.insert(id.min(f64::from(MAX_ROW)) as u32);
            }
        }
    }
    let rels = Rels::load(zip, part).ok();
    match child(root, "pageSetup").and_then(|p| attr(p, "cellComments")) {
        Some("atEnd") => {
            if let Some(target) = rels.as_ref().and_then(|r| r.of_type("comments")) {
                sh.end_comments = comments(zip, target);
            }
        }
        Some("asDisplayed") if rels.as_ref().is_some_and(|r| r.of_type("comments").is_some()) => {
            notes.add("comments printed as displayed on the sheet, left out");
        }
        _ => {}
    }
    if let Some(target) = child(root, "drawing").and_then(|d| rel_attr(d, "id")).and_then(|id| rels.as_ref()?.target(id)) {
        sh.drawings = drawing::read(zip, target, doc, book.theme, notes);
    }
    if let Some(target) = child(root, "legacyDrawingHF").and_then(|d| rel_attr(d, "id")).and_then(|id| rels.as_ref()?.target(id)) {
        sh.hf_pictures = hf_pictures(zip, target, doc, notes);
    }
    if let Some(cols) = child(root, "cols") {
        let mut budget = MAX_COL_WORK;
        for c in children(cols, "col") {
            let (Some(min), Some(max)) = (attr_f64(c, "min"), attr_f64(c, "max")) else { continue };
            let (min, max) = (min.clamp(1.0, f64::from(MAX_COL)) as u32, max.clamp(1.0, f64::from(MAX_COL)) as u32);
            let span = max.saturating_sub(min).saturating_add(1);
            let Some(left) = budget.checked_sub(span) else {
                notes.add("column ranges naming more than four times a sheet's columns, ignored");
                break;
            };
            budget = left;
            let hidden = flag(attr(c, "hidden")) && attr(c, "hidden").is_some();
            let w = attr_f64(c, "width").map(|w| stored_col_points(w, mdw));
            for col in min..=max {
                if hidden {
                    sh.hidden_cols.insert(col);
                } else if let Some(w) = w {
                    sh.widths.insert(col, w.clamp(0.0, 2000.0));
                }
            }
        }
    }
    let mut count = 0usize;
    let mut next_row = 1u32;
    let mut uncached: Vec<((u32, u32), String, usize)> = Vec::new();
    let mut text_bytes = 0usize;
    if let Some(data) = child(root, "sheetData") {
        for row in children(data, "row") {
            let r = attr_f64(row, "r").map_or(next_row, |v| v.clamp(1.0, f64::from(MAX_ROW)) as u32);
            next_row = r.saturating_add(1);
            if flag(attr(row, "hidden")) && attr(row, "hidden").is_some() {
                sh.hidden_rows.insert(r);
                continue;
            }
            if let Some(h) = attr_f64(row, "ht") {
                sh.heights.insert(r, (h.clamp(0.0, 409.0), flag(attr(row, "customHeight")) && attr(row, "customHeight").is_some()));
            }
            let mut next_col = 1u32;
            for c in children(row, "c") {
                count += 1;
                if count > MAX_CELLS {
                    notes.add("cells beyond the first two million of a sheet, left out");
                    break;
                }
                let (rr, cc) = attr(c, "r").and_then(col_index).unwrap_or((r, next_col));
                next_col = cc.saturating_add(1);
                let xf = attr_f64(c, "s").map_or(0, |v| v.clamp(0.0, 1e6) as usize);
                let ty = attr(c, "t").unwrap_or("n");
                let v = child(c, "v").and_then(|v| v.text()).unwrap_or_default();
                if child(c, "f").is_some() && child(c, "v").is_none() && ty != "inlineStr" {
                    // Calculated below, once the sheet's values are known.
                    match child(c, "f").and_then(|f| f.text()).filter(|t| !t.trim().is_empty()) {
                        Some(f) if uncached.len() < 10_000 => uncached.push(((rr, cc), f.to_owned(), xf)),
                        _ => notes.add("formulas without a saved result, left empty"),
                    }
                }
                let mut cell = Cell {
                    text: String::new(),
                    value: None,
                    numeric: false,
                    boolean: false,
                    xf,
                    color: None,
                    runs: None,
                    link: None,
                    general: false,
                };
                let code = || {
                    let id = styles.xfs.get(xf).map_or(0, |x| x.num_fmt);
                    styles.formats.get(&id).map_or_else(|| builtin_format(id), String::as_str)
                };
                match ty {
                    "s" | "inlineStr" | "str" => {
                        let item = match ty {
                            "s" => v.trim().parse::<usize>().ok().and_then(|i| shared.get(i)).map(|s| (s.text.clone(), s.runs.clone())),
                            "inlineStr" => child(c, "is").map(|i| string_item(i, book.theme)).map(|s| (s.text, s.runs)),
                            _ => Some((v.to_owned(), None)),
                        };
                        let (text, runs) = item.unwrap_or_default();
                        let (text, colour) = format_text(&text, code());
                        // A text section's literals change the text: the runs no longer match.
                        cell.runs = runs.filter(|r| r.iter().map(|r| r.text.len()).sum::<usize>() == text.len());
                        cell.text = text;
                        cell.color = colour;
                    }
                    "d" => {
                        if let Some(x) = iso_serial(v, date1904) {
                            let code = match code() {
                                "General" | "general" | "" => "yyyy-mm-dd",
                                c => c,
                            };
                            let (t, colour) = format_number(x, code, date1904);
                            cell.text = t;
                            cell.color = colour;
                            cell.numeric = true;
                            cell.value = Some(x);
                        } else {
                            cell.text = v.to_owned();
                        }
                    }
                    "b" => {
                        cell.text = if v.trim() == "1" { "TRUE".into() } else { "FALSE".into() };
                        cell.boolean = true;
                    }
                    "e" => {
                        cell.text = v.to_owned();
                        cell.boolean = true;
                    }
                    _ => {
                        if let Ok(x) = v.trim().parse::<f64>()
                            && x.is_finite()
                        {
                            let (t, colour) = format_number(x, code(), date1904);
                            cell.text = t;
                            cell.color = colour;
                            cell.numeric = true;
                            cell.value = Some(x);
                            cell.general = code().eq_ignore_ascii_case("general");
                        }
                    }
                }
                // A shared string is copied into every cell that uses it, so one long string used
                // by many cells could exhaust memory: past the cap, cell text is left out.
                text_bytes = text_bytes.saturating_add(cell.text.len());
                if text_bytes > MAX_TEXT_BYTES {
                    notes.add("cell text beyond 64 MiB in a sheet, left out");
                    cell.text.clear();
                }
                let styled = styles.xfs.get(xf).is_some_and(|x| x.fill > 0 || x.border > 0);
                if !cell.text.is_empty() || styled {
                    sh.cells.insert((rr, cc), cell);
                }
            }
        }
    }
    // Formulas saved without a result (written by tools that don't calculate): the ones the
    // evaluator knows are calculated from the saved values.
    // A formula reading another one's result sees it from the next pass on.
    for pass in 0..3 {
        if uncached.is_empty() {
            break;
        }
        let last = pass == 2;
        let mut results = Vec::with_capacity(uncached.len());
        {
            let values: cf::Cells<'_> = sh.cells.iter().map(|(k, c)| (*k, cf::Value { number: c.value, text: &c.text })).collect();
            for (at, f, xf) in &uncached {
                let value = super::xlsx_formula::parse(f, *at).map(|p| p.eval(&values, *at, book.today));
                results.push((*at, *xf, value));
            }
        }
        for (at, xf, value) in results {
            let id = styles.xfs.get(xf).map_or(0, |x| x.num_fmt);
            let code = styles.formats.get(&id).map_or_else(|| builtin_format(id), String::as_str);
            let mut cell =
                Cell { text: String::new(), value: None, numeric: false, boolean: false, xf, color: None, runs: None, link: None, general: false };
            match value {
                Some(super::xlsx_formula::Value::Num(x)) if x.is_finite() => {
                    let (t, colour) = format_number(x, code, date1904);
                    cell = Cell { text: t, value: Some(x), numeric: true, color: colour, general: code.eq_ignore_ascii_case("general"), ..cell };
                }
                Some(super::xlsx_formula::Value::Text(t)) => cell.text = t.chars().take(32_767).collect(),
                Some(super::xlsx_formula::Value::Bool(b)) => {
                    cell = Cell { text: if b { "TRUE".into() } else { "FALSE".into() }, boolean: true, ..cell }
                }
                Some(super::xlsx_formula::Value::Empty) => cell = Cell { text: "0".into(), value: Some(0.0), numeric: true, ..cell },
                _ => {
                    if last {
                        notes.add("formulas without a saved result, left empty");
                    }
                    continue;
                }
            }
            sh.cells.insert(at, cell);
        }
    }
    // Sparklines (an Excel 2010 extension): each one's values from a range of this sheet.
    for group in root.descendants().filter(|n| is(*n, "sparklineGroup")).take(1_000) {
        let kind = match attr(group, "type") {
            Some("column") => 1,
            Some("stacked") => 2,
            _ => 0,
        };
        let colour = child(group, "colorSeries").and_then(|c| color(c, book.theme)).unwrap_or([0.21, 0.37, 0.57]);
        for sl in group.descendants().filter(|n| is(*n, "sparkline")).take(10_000) {
            let (Some(range), Some(at)) = (child(sl, "f").and_then(|f| f.text()), child(sl, "sqref").and_then(|f| f.text()).and_then(col_index))
            else {
                continue;
            };
            let range = range.rsplit('!').next().unwrap_or(range).replace('$', "");
            let (a, b) = range.split_once(':').unwrap_or((&range, &range));
            let (Some((r1, c1)), Some((r2, c2))) = (col_index(a), col_index(b)) else { continue };
            let cols = c1.min(c2)..=c1.max(c2);
            let values: Vec<f64> = sh
                .cells
                .range((r1.min(r2), c1.min(c2))..=(r1.max(r2), c1.max(c2)))
                .filter(|((_, c), _)| cols.contains(c))
                .filter_map(|(_, cell)| cell.value)
                .take(1_000)
                .collect();
            if !values.is_empty() && sh.sparklines.len() < 10_000 {
                sh.sparklines.push((at, kind, colour, values));
            }
        }
    }
    // Hyperlinks: an address (an external relationship) or a place in the workbook (a sheet).
    let mut link_cells = 0usize;
    for h in child(root, "hyperlinks").into_iter().flat_map(|h| children(h, "hyperlink")).take(10_000) {
        let target = rel_attr(h, "id")
            .and_then(|id| rels.as_ref()?.external(id))
            .map(|u| LinkTarget::Uri(u.to_owned()))
            .or_else(|| attr(h, "location").and_then(sheet_anchor).map(LinkTarget::Anchor));
        let (Some(target), Some(range)) = (target, attr(h, "ref")) else { continue };
        let target = Arc::new(target);
        let (a, b) = range.split_once(':').unwrap_or((range, range));
        let (Some((r1, c1)), Some((r2, c2))) = (col_index(a), col_index(b)) else { continue };
        let cols = c1.min(c2)..=c1.max(c2);
        // Keys run row by row: the range holds the rows' other columns too.
        for (_, cell) in sh.cells.range_mut((r1.min(r2), c1.min(c2))..=(r1.max(r2), c1.max(c2))).filter(|((_, c), _)| cols.contains(c)) {
            link_cells += 1;
            if link_cells > 100_000 {
                break;
            }
            cell.link.get_or_insert_with(|| target.clone());
        }
    }
    if let Some(m) = child(root, "mergeCells") {
        for mc in children(m, "mergeCell") {
            if sh.merges.len() >= MAX_MERGES {
                notes.add("merged ranges beyond the first 20 000 of a sheet, drawn unmerged");
                break;
            }
            let Some((a, b)) = attr(mc, "ref").and_then(|r| r.split_once(':')) else { continue };
            if let (Some((r1, c1)), Some((r2, c2))) = (col_index(a), col_index(b)) {
                sh.merges.push((r1.min(r2), c1.min(c2), r1.max(r2), c1.max(c2)));
            }
        }
    }
    if child(root, "conditionalFormatting").is_some() {
        let values: cf::Cells<'_> = sh.cells.iter().map(|(k, c)| (*k, cf::Value { number: c.value, text: &c.text })).collect();
        sh.looks = cf::evaluate(root, &values, &styles.dxfs, &|n| color(n, book.theme), book.today, notes);
    }
    for id in child(root, "tableParts").into_iter().flat_map(|t| children(t, "tablePart")).take(256).filter_map(|t| rel_attr(t, "id")) {
        if let Some(target) = rels.as_ref().and_then(|r| r.target(id)) {
            read_table(zip, target, styles, book.theme, &mut sh, notes);
        }
    }
    Ok(sh)
}

/// A sparkline: the cell it is drawn in, its kind (0 line, 1 column, 2 win/loss), colour and values.
type Sparkline = ((u32, u32), u8, [f64; 3], Vec<f64>);

/// A formatted table's look in one cell.
#[derive(Clone, Copy, Debug, Default)]
struct TableCell {
    fill: Option<[f64; 3]>,
    color: Option<[f64; 3]>,
    bold: bool,
}

/// One part of a table style: its fill, font colour and boldness, and borders: around the part
/// and between its rows and columns.
#[derive(Clone, Copy, Debug, Default)]
struct TablePart {
    fill: Option<[f64; 3]>,
    color: Option<[f64; 3]>,
    bold: bool,
    outline: Option<Edge>,
    inner_h: Option<Edge>,
    inner_v: Option<Edge>,
}

/// A table style: the whole table, header row, total row, row stripes and first and last columns.
struct TableStyle {
    parts: [TablePart; 7],
}

fn thin(color: [f64; 3]) -> Option<Edge> {
    Some(Edge { width: 0.5, color, dash: None, double: false })
}

/// Excel's built-in table styles (`TableStyleLight1`… `TableStyleMedium28`, `TableStyleDark11`),
/// which workbooks name but don't store: their families' look in the theme's colours.
fn builtin_table_style(name: &str, theme: &[[f64; 3]]) -> Option<TableStyle> {
    let (kind, n) =
        ["Light", "Medium", "Dark"].iter().find_map(|k| Some((*k, name.strip_prefix("TableStyle")?.strip_prefix(k)?.parse::<usize>().ok()?)))?;
    if n == 0 || n > 28 {
        return None;
    }
    let (group, k) = ((n - 1) / 7, (n - 1) % 7);
    let black = theme.get(1).copied().unwrap_or([0.0; 3]);
    let accent = if k == 0 { black } else { theme.get(3 + k).copied().unwrap_or(black) };
    let tint = |t: f64| accent.map(|v| v + (1.0 - v) * t);
    let shade = |t: f64| accent.map(|v| v * t);
    let white = Some([1.0; 3]);
    let p = |fill: Option<[f64; 3]>, color: Option<[f64; 3]>, bold: bool| TablePart { fill, color, bold, ..TablePart::default() };
    let mut parts = [TablePart::default(); 7];
    match (kind, group) {
        ("Light", 0) => {
            let text = if k == 0 { None } else { Some(shade(0.75)) };
            parts[0] = TablePart { color: text, outline: thin(accent), ..TablePart::default() };
            parts[1] = TablePart { bold: true, outline: thin(accent), ..p(None, text, true) };
            parts[3] = p(Some(tint(0.8)), None, false);
        }
        ("Light", 1) => {
            parts[0] = TablePart { outline: thin(accent), inner_h: thin(accent), ..TablePart::default() };
            parts[1] = p(Some(accent), white, true);
        }
        ("Light", _) => {
            parts[0] = TablePart { outline: thin(accent), inner_h: thin(accent), inner_v: thin(accent), ..TablePart::default() };
            parts[1] = p(None, None, true);
            parts[3] = p(Some(tint(0.8)), None, false);
        }
        ("Medium", 0) => {
            parts[0] = TablePart { outline: thin(tint(0.4)), inner_h: thin(tint(0.4)), ..TablePart::default() };
            parts[1] = p(Some(accent), white, true);
            parts[3] = p(Some(tint(0.8)), None, false);
        }
        ("Medium", 1) => {
            parts[0] = TablePart { inner_h: thin([1.0; 3]), inner_v: thin([1.0; 3]), ..p(Some(tint(0.8)), None, false) };
            parts[1] = p(Some(accent), white, true);
            parts[3] = p(Some(tint(0.6)), None, false);
        }
        ("Medium", 2) => {
            parts[0] = TablePart { outline: thin(black), ..TablePart::default() };
            parts[1] = p(Some(black), white, true);
            parts[3] = p(Some(tint(0.8)), None, false);
        }
        ("Medium", _) => {
            parts[0] = TablePart { outline: thin(tint(0.4)), inner_h: thin(tint(0.4)), inner_v: thin(tint(0.4)), ..TablePart::default() };
            parts[1] = p(Some(tint(0.6)), None, true);
            parts[3] = p(Some(tint(0.8)), None, false);
        }
        ("Dark", 0) => {
            parts[0] = p(Some(shade(0.5)), white, false);
            parts[1] = p(Some(black), white, true);
            parts[3] = p(Some(accent), white, false);
        }
        _ => {
            parts[0] = p(Some(tint(0.6)), None, false);
            parts[1] = p(Some(black), white, true);
            parts[3] = p(Some(tint(0.4)), None, false);
        }
    }
    parts[2] = TablePart { bold: true, outline: thin(accent), ..parts[0] };
    parts[5].bold = true;
    parts[6].bold = true;
    Some(TableStyle { parts })
}

/// Read a formatted table (`xl/tables/table1.xml`) and give its cells the look of its style:
/// the workbook's own style of that name, else Excel's built-in one.
fn read_table(zip: &Zip<'_>, part: &str, styles: &Styles, theme: &[[f64; 3]], sh: &mut Sheet, notes: &mut Notes) {
    let Ok(Some(text)) = zip.text(part) else { return };
    let Ok(d) = xml::parse(&text) else { return };
    let t = d.root_element();
    let Some((a, b)) = attr(t, "ref").and_then(|r| r.split_once(':')) else { return };
    let (Some((r1, c1)), Some((r2, c2))) = (col_index(a), col_index(b)) else { return };
    let (r1, c1, r2, c2) = (r1.min(r2), c1.min(c2), r1.max(r2), c1.max(c2));
    if u64::from(r2 - r1 + 1) * u64::from(c2 - c1 + 1) > 200_000 {
        notes.add("formatted tables over 200 000 cells, drawn without their style");
        return;
    }
    let info = child(t, "tableStyleInfo");
    let Some(name) = info.and_then(|i| attr(i, "name")) else { return };
    let show = |k: &str| info.and_then(|i| attr(i, k)).is_some_and(|v| flag(Some(v)));
    let style = match styles.table_styles.get(name) {
        Some(ids) => {
            let mut parts = [TablePart::default(); 7];
            for (slot, id) in ids.iter().enumerate() {
                if let Some(dxf) = id.and_then(|i| styles.dxfs.get(i)) {
                    parts[slot] = TablePart { fill: dxf.fill, color: dxf.color, bold: dxf.bold.unwrap_or(false), ..TablePart::default() };
                }
            }
            TableStyle { parts }
        }
        None => match builtin_table_style(name, theme) {
            Some(s) => s,
            None => return,
        },
    };
    let header_rows = attr_f64(t, "headerRowCount").map_or(1, |v| v.clamp(0.0, 1.0) as u32);
    let total_rows = attr_f64(t, "totalsRowCount").map_or(0, |v| v.clamp(0.0, 1.0) as u32);
    let [whole, header, total, stripe1, stripe2, first_col, last_col] = style.parts;
    for r in r1..=r2 {
        for c in c1..=c2 {
            let part = if r < r1 + header_rows {
                header
            } else if total_rows > 0 && r == r2 {
                total
            } else if show("showRowStripes") && (r - r1 - header_rows).is_multiple_of(2) {
                TablePart { fill: stripe1.fill.or(whole.fill), color: stripe1.color.or(whole.color), bold: stripe1.bold, ..whole }
            } else if show("showRowStripes") {
                TablePart { fill: stripe2.fill.or(whole.fill), color: stripe2.color.or(whole.color), bold: stripe2.bold, ..whole }
            } else {
                whole
            };
            let body = r >= r1 + header_rows && !(total_rows > 0 && r == r2);
            let bold = part.bold
                || (body && ((c == c1 && show("showFirstColumn") && first_col.bold) || (c == c2 && show("showLastColumn") && last_col.bold)));
            let look = TableCell { fill: part.fill.or(whole.fill), color: part.color.or(whole.color), bold };
            // Borders: the table's outline at its edges, its inner lines between cells, and a
            // header's or total row's own lines.
            let edge = |outer: bool, inner: Option<Edge>| if outer { whole.outline } else { inner };
            let mut edges = Edges {
                top: edge(r == r1, whole.inner_h),
                bottom: edge(r == r2, whole.inner_h),
                left: edge(c == c1, whole.inner_v),
                right: edge(c == c2, whole.inner_v),
                diagonal: None,
            };
            if r < r1 + header_rows {
                edges.top = header.outline.or(edges.top);
                edges.bottom = header.outline.or(edges.bottom);
            } else if total_rows > 0 && r == r2 {
                edges.top = total.outline.or(edges.top);
            }
            sh.table_looks.insert((r, c), (look, edges));
            sh.cells.entry((r, c)).or_insert_with(|| Cell {
                text: String::new(),
                value: None,
                numeric: false,
                boolean: false,
                xf: 0,
                color: None,
                runs: None,
                link: None,
                general: false,
            });
        }
    }
}

/// The header and footer pictures of a `legacyDrawingHF` VML part, by shape id.
fn hf_pictures(zip: &Zip<'_>, part: &str, doc: &mut Document, notes: &mut Notes) -> Pictures {
    let mut out = Pictures::new();
    let Ok(Some(text)) = zip.text(part) else { return out };
    let Ok(d) = xml::parse(&text) else {
        notes.add("header and footer pictures that can't be read, left out");
        return out;
    };
    let rels = Rels::load(zip, part).ok();
    for shape in d.descendants().filter(|n| is(*n, "shape")).take(64) {
        let Some(id) = attr(shape, "id") else { continue };
        let Some(img) = shape.children().find(|c| is(*c, "imagedata")) else { continue };
        let (w, h) = hf::vml_size(attr(shape, "style").unwrap_or_default());
        // `o:relid` is in the VML office namespace, not the relationships one.
        let rid = img.attributes().find(|a| a.name() == "relid").map(|a| a.value()).or_else(|| rel_attr(img, "id"));
        let target = rid.and_then(|r| rels.as_ref()?.target(r)).map(str::to_owned);
        let r = target.and_then(|t| {
            let bytes = zip.read(&t).ok()??;
            crate::image_xobject(doc, &t, &bytes).ok().map(|(r, _)| r).or_else(|| super::emf::form(doc, &bytes, notes))
        });
        out.insert(id.to_owned(), (r, w, h));
    }
    out
}

fn shared_strings(zip: &Zip<'_>, rels: &Rels, theme: &[[f64; 3]]) -> Result<Vec<Shared>, String> {
    let Some(part) = rels.of_type("sharedStrings") else { return Ok(Vec::new()) };
    let Some(text) = zip.text(part)? else { return Ok(Vec::new()) };
    let d = xml::parse(&text).map_err(|e| format!("{part}: {e}"))?;
    Ok(children(d.root_element(), "si").map(|si| string_item(si, theme)).collect())
}

/// The anchor of a workbook place a hyperlink names (`Sheet2!A1`, `'My sheet'!B2`): its sheet.
fn sheet_anchor(location: &str) -> Option<String> {
    let (sheet, _) = location.rsplit_once('!')?;
    let sheet = sheet.trim().trim_matches('\'').replace("''", "'");
    (!sheet.is_empty()).then(|| format!("sheet:{}", sheet.to_lowercase()))
}

/// A comments part's comments in order: the cell each is on and its text (Excel saves the
/// author's name as the text's first line).
fn comments(zip: &Zip<'_>, part: &str) -> Vec<(String, String)> {
    let Ok(Some(text)) = zip.text(part) else { return Vec::new() };
    let Ok(d) = xml::parse(&text) else { return Vec::new() };
    let Some(list) = child(d.root_element(), "commentList") else { return Vec::new() };
    children(list, "comment")
        .take(65_536)
        .filter_map(|c| {
            let at = attr(c, "ref")?.to_owned();
            let text = child(c, "text").map(|t| string_item(t, &[]).text).unwrap_or_default();
            Some((at, text))
        })
        .collect()
}

/// Print a sheet's comments on pages after it, as Excel does for "At end of sheet": each as
/// `Cell:` and `Comment:` lines, a blank line between, in the workbook's default font.
fn draw_comments(
    canvas: &mut Canvas,
    sh: &Sheet,
    styles: &Styles,
    notes: &mut Notes,
    jobs: &mut Vec<HfJob>,
    sheet_index: usize,
) -> Result<(), String> {
    if sh.end_comments.is_empty() {
        return Ok(());
    }
    let style = styles.fonts.first().map_or_else(RunStyle::default, |f| RunStyle {
        fam: family_of(&f.name),
        font: super::faces::face_of(&f.name),
        size: f.size,
        ..RunStyle::default()
    });
    let (pw, ph) = if sh.landscape { (sh.paper.1, sh.paper.0) } else { sh.paper };
    let [mt, mr, mb, ml] = sh.margins;
    let area_w = (pw - ml - mr).max(72.0);
    let line_h = {
        let (a, d, gap) = style.line_metrics();
        style.size * (a + d + gap)
    };
    let mut lines: Vec<String> = Vec::new();
    for (at, text) in &sh.end_comments {
        lines.push(format!("Cell: {at}"));
        for (i, para) in text.split('\n').enumerate() {
            let para = notes.drawable(if i == 0 { para } else { para.trim_end_matches('\r') });
            let para = if i == 0 { format!("Comment: {para}") } else { para };
            lines.extend(pdfcraft_fonts::wrap_with(&para, style.size, area_w, |t, size| RunStyle { size, ..style.clone() }.width(t)));
        }
        lines.push(String::new());
    }
    let mut page: Option<usize> = None;
    let mut y = 0.0;
    for line in lines {
        if page.is_none() || y - line_h < mb {
            let pi = canvas.new_page(pw, ph)?;
            jobs.push(HfJob { page: pi, sheet: sheet_index, first: false });
            page = Some(pi);
            y = ph - mt;
        }
        y -= line_h;
        if let Some(p) = page.and_then(|i| canvas.page(i)) {
            p.text(ml, y + line_h - style.size, &style, &line);
        }
    }
    Ok(())
}

/// A sheet's page waiting for its header and footer (drawn once the page count is known).
struct HfJob {
    page: usize,
    sheet: usize,
    first: bool,
}

/// What a sheet's headers and footers need after its pages are drawn.
struct HfSheet {
    name: String,
    hf: HeaderFooter,
    pictures: Pictures,
    margins: [f64; 4],
    hf_margins: (f64, f64),
    /// The sheet's own first page number (`useFirstPageNumber`), else numbered through the
    /// workbook.
    first_number: Option<usize>,
}

/// A sheet measured for printing: row heights (grown for wrapped text), wrapped lines, and
/// running offsets so a column's or row's position is two lookups.
struct Grid<'a> {
    sh: &'a Sheet,
    styles: &'a Styles,
    heights: Vec<f64>,
    wrapped: HashMap<(u32, u32), Vec<String>>,
    texts: HashMap<(u32, u32), String>,
    /// Rich text cells' drawable runs with their styles (line breaks kept as `\n`).
    rich: HashMap<(u32, u32), Vec<(String, RunStyle)>>,
    /// `col_at[c]` is the width of the columns before `c`, `row_at[r]` the height of the rows
    /// before `r`; both go one past the last, for the far edge.
    col_at: Vec<f64>,
    row_at: Vec<f64>,
}

impl Grid<'_> {
    fn width(&self, c: u32) -> f64 {
        if self.sh.hidden_cols.contains(&c) { 0.0 } else { self.sh.widths.get(&c).copied().unwrap_or(self.sh.default_w) }
    }

    fn row_h(&self, r: u32) -> f64 {
        self.heights.get(r as usize).copied().unwrap_or(0.0)
    }

    fn col_x(&self, c: u32) -> f64 {
        self.col_at.get(c as usize).or(self.col_at.last()).copied().unwrap_or(0.0)
    }

    fn row_y(&self, r: u32) -> f64 {
        self.row_at.get(r as usize).or(self.row_at.last()).copied().unwrap_or(0.0)
    }

    fn style_of(&self, cell: &Cell) -> RunStyle {
        let f = self.styles.xfs.get(cell.xf).and_then(|x| self.styles.fonts.get(x.font)).cloned().unwrap_or(Font {
            name: "Calibri".into(),
            size: 11.0,
            ..Font::default()
        });
        RunStyle {
            fam: family_of(&f.name),
            font: super::faces::face_of(&f.name),
            bold: f.bold,
            italic: f.italic,
            size: f.size,
            color: cell.color.or(f.color).unwrap_or([0.0; 3]),
            link: cell.link.clone(),
            underline: f.underline,
            strike: f.strike,
            ..RunStyle::default()
        }
    }

    /// A rich run's style: the cell's, with the run's own font over it.
    fn rich_style(&self, cell: &Cell, look: Option<&RichLook>) -> RunStyle {
        let base = self.style_of(cell);
        let Some(l) = look else { return base };
        let name = l.name.as_deref();
        RunStyle {
            fam: name.map_or(base.fam, family_of),
            font: name.map_or(base.font, super::faces::face_of),
            size: l.size.unwrap_or(base.size),
            color: cell.color.or(l.color).unwrap_or(base.color),
            bold: l.bold,
            italic: l.italic,
            underline: l.underline,
            strike: l.strike,
            vert: l.vert,
            ..base
        }
    }

    /// The style a cell's rows are fitted to: its own, or its largest rich run's.
    fn tallest(&self, at: (u32, u32), cell: &Cell) -> RunStyle {
        self.rich
            .get(&at)
            .and_then(|segs| segs.iter().map(|(_, st)| st).filter(|st| st.vert == 0).max_by(|a, b| a.size.total_cmp(&b.size)))
            .cloned()
            .unwrap_or_else(|| self.style_of(cell))
    }

    /// A drawing's rectangle on the sheet (x, y from the top-left of A1, width, height).
    fn rect_of(&self, a: &Anchor) -> (f64, f64, f64, f64) {
        let at = |c: &drawing::Corner| (self.col_x(c.col.saturating_add(1)) + c.dx, self.row_y(c.row.saturating_add(1)) + c.dy);
        match a {
            Anchor::Cells(f, t) => {
                let (x1, y1) = at(f);
                let (x2, y2) = at(t);
                (x1, y1, (x2 - x1).max(0.0), (y2 - y1).max(0.0))
            }
            Anchor::Sized(f, w, h) => {
                let (x, y) = at(f);
                (x, y, *w, *h)
            }
            Anchor::Absolute(x, y, w, h) => (*x, *y, *w, *h),
        }
    }
}

/// The last row and column a drawing reaches, from default sizes (before wrapped rows grow).
fn drawing_extent(sh: &Sheet, a: &Anchor) -> (u32, u32) {
    let width = |c: u32| if sh.hidden_cols.contains(&c) { 0.0 } else { sh.widths.get(&c).copied().unwrap_or(sh.default_w) };
    let height = |r: u32| if sh.hidden_rows.contains(&r) { 0.0 } else { sh.heights.get(&r).map_or(sh.default_h, |(h, _)| *h) };
    // Walk from a cell until `len` points are covered (bounded by the sheet's size).
    let walk = |start: u32, len: f64, size: &dyn Fn(u32) -> f64, max: u32| {
        let (mut i, mut left) = (start, len);
        while i < max && left > size(i) && size(i) >= 0.0 {
            left -= size(i);
            i += 1;
            if i - start > 100_000 {
                break;
            }
        }
        i
    };
    match a {
        Anchor::Cells(_, t) => (t.row.saturating_add(1).min(MAX_ROW), t.col.saturating_add(1).min(MAX_COL)),
        Anchor::Sized(f, w, h) => {
            (walk(f.row.saturating_add(1), f.dy + h, &height, MAX_ROW), walk(f.col.saturating_add(1), f.dx + w, &width, MAX_COL))
        }
        Anchor::Absolute(x, y, w, h) => (walk(1, y + h, &height, MAX_ROW), walk(1, x + w, &width, MAX_COL)),
    }
}

/// Split `from..=to` into page bands: a band ends when the next item doesn't fit in what
/// `avail` leaves for a band starting there, or after a manual break.
fn bands(from: u32, to: u32, size: &dyn Fn(u32) -> f64, avail: &dyn Fn(u32) -> f64, breaks: &BTreeSet<u32>) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let (mut start, mut used) = (from, 0.0);
    for i in from..=to {
        let s = size(i);
        let manual = i > start && breaks.contains(&(i - 1));
        if manual || (used + s > avail(start) && used > 0.0) {
            out.push((start, i - 1));
            start = i;
            used = 0.0;
        }
        used += s;
    }
    out.push((start, to));
    out
}

fn draw_sheet(canvas: &mut Canvas, sh: &Sheet, styles: &Styles, notes: &mut Notes, jobs: &mut Vec<HfJob>, sheet_index: usize) -> Result<(), String> {
    if sh.cells.is_empty() && sh.drawings.is_empty() && sh.sparklines.is_empty() {
        return Ok(());
    }
    let mut max_row = sh.cells.keys().map(|(r, _)| *r).max().unwrap_or(1);
    let mut max_col = sh.cells.keys().map(|(_, c)| *c).max().unwrap_or(1);
    // Pictures and shapes are printed with the cells: the used range reaches them.
    for d in &sh.drawings {
        let (r, c) = drawing_extent(sh, &d.anchor);
        max_row = max_row.max(r);
        max_col = max_col.max(c);
    }
    for ((r, c), ..) in &sh.sparklines {
        max_row = max_row.max(*r);
        max_col = max_col.max(*c);
    }
    let (pw, ph) = if sh.landscape { (sh.paper.1, sh.paper.0) } else { sh.paper };
    let [mt, mr, mb, ml] = sh.margins;
    let area_w = (pw - ml - mr).max(72.0);
    let area_h = (ph - mt - mb).max(72.0);
    let mut g = Grid {
        sh,
        styles,
        heights: vec![0.0; max_row as usize + 1],
        wrapped: HashMap::new(),
        texts: HashMap::new(),
        rich: HashMap::new(),
        col_at: Vec::new(),
        row_at: Vec::new(),
    };
    for ((r, c), cell) in &sh.cells {
        // Line breaks are split on below (or become spaces), and `drawable` drops control
        // characters, so each line is made drawable on its own.
        let lines: Vec<String> = cell.text.split('\n').map(|line| notes.drawable(line)).collect();
        g.texts.insert((*r, *c), lines.join("\n"));
        if let Some(runs) = &cell.runs {
            let segs: Vec<(String, RunStyle)> = runs
                .iter()
                .map(|run| {
                    (run.text.split('\n').map(|line| notes.drawable(line)).collect::<Vec<_>>().join("\n"), g.rich_style(cell, run.look.as_ref()))
                })
                .filter(|(t, _)| !t.is_empty())
                .collect();
            g.rich.insert((*r, *c), segs);
        }
    }
    for r in 1..=max_row {
        let h = if sh.hidden_rows.contains(&r) { 0.0 } else { sh.heights.get(&r).map_or(sh.default_h, |(h, _)| *h) };
        if let Some(slot) = g.heights.get_mut(r as usize) {
            *slot = h;
        }
    }
    // Rows without a custom height are fitted again when printing, as Excel does: the tallest of
    // their cells' fonts (one line, or each wrapped line), in whole pixels, plus 2 px. A merged
    // range wraps across its whole width, and a row holding wrapped merged text keeps its height.
    let merge_ends: HashMap<(u32, u32), u32> = sh.merges.iter().map(|&(r1, c1, _, c2)| ((r1, c1), c2)).collect();
    let line_px = |st: &RunStyle| {
        let (a, d, gap) = st.line_metrics();
        ((st.size * 4.0 / 3.0).round() * (a + d + gap)).round()
    };
    let default_px = line_px(&styles.fonts.first().map_or_else(RunStyle::default, |f| RunStyle {
        fam: family_of(&f.name),
        font: super::faces::face_of(&f.name),
        size: f.size,
        ..RunStyle::default()
    })) + 2.0;
    // Row → (fitted pixels, keep the saved height).
    let mut fit: HashMap<u32, (f64, bool)> = HashMap::new();
    for ((r, c), cell) in &sh.cells {
        let Some(xf) = styles.xfs.get(cell.xf) else { continue };
        let style = g.tallest((*r, *c), cell);
        let merged_to = merge_ends.get(&(*r, *c)).copied();
        let mut lines_n = 1usize;
        // Rotated text needs the height of its slanted line; stacked text one line a character.
        if xf.rotation != 0 && !cell.text.is_empty() {
            let text = g.texts.get(&(*r, *c)).cloned().unwrap_or_default().replace('\n', " ");
            let px = if xf.rotation == 255 {
                text.chars().count() as f64 * line_px(&style) + 2.0
            } else {
                let a = rotation_degrees(xf.rotation).to_radians();
                ((style.width(&text) * a.sin().abs() + style.size * 1.2 * a.cos().abs()) * 4.0 / 3.0).round() + 4.0
            };
            let e = fit.entry(*r).or_insert((0.0, false));
            e.0 = e.0.max(px);
            continue;
        }
        if xf.wrap && !cell.text.is_empty() {
            let w = (merged_to.map_or_else(|| g.width(*c), |c2| (*c..=c2.min(c.saturating_add(16_384))).map(|cc| g.width(cc)).sum()) - 4.0).max(4.0);
            let text = g.texts.get(&(*r, *c)).cloned().unwrap_or_default();
            let lines: Vec<String> = text
                .split('\n')
                .flat_map(|p| pdfcraft_fonts::wrap_with(p, style.size, w, |s, size| RunStyle { size, ..style.clone() }.width(s)))
                .collect();
            lines_n = if merged_to.is_some() { 1 } else { lines.len().max(1) };
            if merged_to.is_some() {
                fit.entry(*r).or_insert((0.0, false)).1 = true;
            }
            g.wrapped.insert((*r, *c), lines);
        }
        let px = lines_n as f64 * line_px(&style) + 2.0;
        let e = fit.entry(*r).or_insert((0.0, false));
        e.0 = e.0.max(px);
    }
    for (r, (px, keep)) in fit {
        if keep || sh.heights.get(&r).is_some_and(|(_, custom)| *custom) {
            continue;
        }
        if let Some(slot) = g.heights.get_mut(r as usize)
            && *slot > 0.0
        {
            *slot = px.max(default_px) * 0.75;
        }
    }
    // Excel prints a row of `px` screen pixels as round(px · 247/255 · 6.25) printer dots of a
    // 600 dpi grid, a little shorter than its nominal height (a 15 pt row prints 14.52 pt).
    for h in g.heights.iter_mut() {
        if *h > 0.0 {
            let px = (*h * 4.0 / 3.0).round();
            *h = (px * 247.0 / 255.0 * 6.25).round() / 600.0 * 72.0;
        }
    }
    let running = |n: u32, size: &dyn Fn(u32) -> f64| -> Vec<f64> {
        let mut at = vec![0.0; 2];
        for i in 1..=n {
            let last = at.last().copied().unwrap_or(0.0);
            at.push(last + size(i));
        }
        at
    };
    g.col_at = running(max_col, &|c| g.width(c));
    g.row_at = running(max_row, &|r| g.row_h(r));
    let span_w = |a: u32, b: u32| g.col_x(b.saturating_add(1)) - g.col_x(a);
    let span_h = |a: u32, b: u32| g.row_y(b.saturating_add(1)) - g.row_y(a);
    // Each print area on its own pages; without one, the used range.
    let areas: Vec<(u32, u32, u32, u32)> = if sh.print_areas.is_empty() {
        vec![(1, 1, max_row, max_col)]
    } else {
        sh.print_areas
            .iter()
            .map(|&(r1, c1, r2, c2)| (r1.min(max_row), c1.min(max_col), r2.min(max_row).max(r1.min(max_row)), c2.min(max_col).max(c1.min(max_col))))
            .collect()
    };
    let title_rows = sh.title_rows.map(|(a, b)| (a.min(max_row), b.min(max_row)));
    let title_cols = sh.title_cols.map(|(a, b)| (a.min(max_col), b.min(max_col)));
    let no_breaks = BTreeSet::new();
    // Printed headings: a column of row numbers as wide as the longest one, and a row of column
    // letters as tall as a default row, in the sheet's default font.
    let head_font = styles.fonts.first().map_or_else(RunStyle::default, |f| RunStyle {
        fam: family_of(&f.name),
        font: super::faces::face_of(&f.name),
        size: f.size,
        ..RunStyle::default()
    });
    let head_w = if sh.headings { head_font.width(&max_row.to_string()) + 8.0 } else { 0.0 };
    let head_h = if sh.headings { sh.default_h } else { 0.0 };
    let mut first = true;
    let merge_work = std::cell::Cell::new(MAX_MERGE_WORK);
    let merge_cut = std::cell::Cell::new(false);
    for (ar1, ac1, ar2, ac2) in areas {
        // Excel's print scale: fitting only ever shrinks; 0 pages means "as many as needed".
        let k = match sh.fit {
            Some((fw, fh)) => {
                let total_w = span_w(ac1, ac2);
                let total_h = span_h(ar1, ar2);
                let kw = if fw > 0 && total_w > 0.0 { area_w * f64::from(fw) / total_w } else { f64::INFINITY };
                let kh = if fh > 0 && total_h > 0.0 { area_h * f64::from(fh) / total_h } else { f64::INFINITY };
                kw.min(kh).clamp(0.1, 1.0)
            }
            None => sh.scale,
        };
        // Title rows and columns repeat on the pages after them, taking their room there.
        let title_h = title_rows.map_or(0.0, |(a, b)| span_h(a, b));
        let title_w = title_cols.map_or(0.0, |(a, b)| span_w(a, b));
        let avail_w = |start: u32| area_w / k - head_w - if title_cols.is_some_and(|(_, b)| start > b) { title_w } else { 0.0 };
        let avail_h = |start: u32| area_h / k - head_h - if title_rows.is_some_and(|(_, b)| start > b) { title_h } else { 0.0 };
        let (rb, cb) = if sh.fit.is_some() { (&no_breaks, &no_breaks) } else { (&sh.row_breaks, &sh.col_breaks) };
        let col_bands = bands(ac1, ac2, &|c| g.width(c), &avail_w, cb);
        let row_bands = bands(ar1, ar2, &|r| g.row_h(r), &avail_h, rb);
        let band_of = |bands: &[(u32, u32)], i: u32| bands.partition_point(|(_, end)| *end < i);
        // Only the pages something is printed on: a sparse sheet has billions of empty ones.
        let mut pages: BTreeSet<(usize, usize)> = BTreeSet::new();
        if sh.print_areas.is_empty() {
            pages.extend(sh.cells.keys().map(|(r, c)| (band_of(&col_bands, *c), band_of(&row_bands, *r))));
            for d in &sh.drawings {
                let (x, y, w, h) = g.rect_of(&d.anchor);
                let col_of = |x: f64| g.col_at.partition_point(|v| *v <= x).saturating_sub(1).max(1) as u32;
                let row_of = |y: f64| g.row_at.partition_point(|v| *v <= y).saturating_sub(1).max(1) as u32;
                let (c1, c2) = (band_of(&col_bands, col_of(x)), band_of(&col_bands, col_of(x + w)));
                let (r1, r2) = (band_of(&row_bands, row_of(y)), band_of(&row_bands, row_of(y + h)));
                for ci in c1..=c2.min(c1 + 64) {
                    for ri in r1..=r2.min(r1 + 64) {
                        pages.insert((ci, ri));
                    }
                }
            }
            pages.retain(|(ci, ri)| *ci < col_bands.len() && *ri < row_bands.len());
        } else {
            pages.extend((0..col_bands.len()).flat_map(|ci| (0..row_bands.len()).map(move |ri| (ci, ri))));
        }
        let mut order: Vec<(usize, usize)> = pages.into_iter().collect();
        if sh.over_then_down {
            order.sort_by_key(|(ci, ri)| (*ri, *ci));
        }
        for (ci, ri) in order {
            let (Some(&(c0, c1)), Some(&(r0, r1))) = (col_bands.get(ci), row_bands.get(ri)) else { continue };
            let row_segs: Vec<(u32, u32)> = title_rows.filter(|(_, b)| r0 > *b).into_iter().chain(std::iter::once((r0, r1))).collect();
            let col_segs: Vec<(u32, u32)> = title_cols.filter(|(_, b)| c0 > *b).into_iter().chain(std::iter::once((c0, c1))).collect();
            let content_w: f64 = head_w + col_segs.iter().map(|(a, b)| span_w(*a, *b)).sum::<f64>();
            let content_h: f64 = head_h + row_segs.iter().map(|(a, b)| span_h(*a, *b)).sum::<f64>();
            let cx = if sh.h_center { ((area_w - content_w * k) / 2.0).max(0.0) } else { 0.0 };
            let cy = if sh.v_center { ((area_h - content_h * k) / 2.0).max(0.0) } else { 0.0 };
            let pi = canvas.new_page(pw, ph)?;
            jobs.push(HfJob { page: pi, sheet: sheet_index, first });
            first = false;
            let Some(page) = canvas.page(pi) else { continue };
            // Scale about the printable area's top-left corner, moved to centre the content.
            page.save();
            if (k - 1.0).abs() > 1e-6 || cx > 0.0 || cy > 0.0 {
                page.raw(&format!(
                    "{} 0 0 {} {} {} cm\n",
                    super::canvas::n(k),
                    super::canvas::n(k),
                    super::canvas::n(ml * (1.0 - k) + cx),
                    super::canvas::n((ph - mt) * (1.0 - k) - cy)
                ));
            }
            if sh.headings {
                draw_headings(page, &g, &head_font, (&row_segs, &col_segs), (ml, ph - mt), (head_w, head_h));
            }
            let mut oy = ph - mt - head_h;
            for rs in &row_segs {
                let mut ox = ml + head_w;
                for cs in &col_segs {
                    draw_block(page, &g, *rs, *cs, (ox, oy), &merge_work, &merge_cut);
                    // Pictures and shapes in the page's own cells, clipped to them.
                    if *rs == (r0, r1) && *cs == (c0, c1) {
                        let (bw, bh) = (span_w(c0, c1), span_h(r0, r1));
                        page.save();
                        page.clip_rect(ox, oy - bh, bw, bh);
                        for d in &sh.drawings {
                            let (x, y, w, h) = g.rect_of(&d.anchor);
                            let (px, py) = (ox + x - g.col_x(c0), oy - (y - g.row_y(r0)));
                            if px + w < ox || px > ox + bw || py < oy - bh || py - h > oy {
                                continue;
                            }
                            drawing::draw(page, d, px, py - h, w, h);
                        }
                        page.restore();
                    }
                    ox += span_w(cs.0, cs.1);
                }
                oy -= span_h(rs.0, rs.1);
            }
            page.restore();
        }
    }
    if merge_cut.get() {
        notes.add("merged ranges on very dense pages, drawn unmerged");
    }
    Ok(())
}

/// A run of rows or columns, first and last.
type Span = (u32, u32);

/// A column's letters (1 is A, 27 is AA).
fn col_letters(mut c: u32) -> String {
    let mut out = Vec::new();
    while c > 0 && out.len() < 4 {
        let r = (c - 1) % 26;
        out.push(char::from(b'A' + r as u8));
        c = (c - 1) / 26;
    }
    out.iter().rev().collect()
}

/// Printed headings for a page whose printable area starts at `(x, y)` (top left): the corner,
/// the column letters above the columns and the row numbers left of the rows, each centred in a
/// cell with a grey outline.
fn draw_headings(page: &mut Page, g: &Grid<'_>, font: &RunStyle, (rows, cols): (&[Span], &[Span]), (x, y): (f64, f64), (hw, hh): (f64, f64)) {
    const LINE: [f64; 3] = [0.6, 0.6, 0.6];
    let cell = |page: &mut Page, cx: f64, cy: f64, w: f64, h: f64, text: &str| {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        page.stroke_rect(cx, cy - h, w, h, 0.25, LINE);
        if !text.is_empty() {
            let tw = font.width(text);
            page.text(cx + (w - tw) / 2.0, cy - h / 2.0 - font.size * 0.3, font, text);
        }
    };
    cell(page, x, y, hw, hh, "");
    let mut cx = x + hw;
    for &(a, b) in cols {
        for c in a..=b.min(a.saturating_add(16_384)) {
            let w = g.width(c);
            cell(page, cx, y, w, hh, &col_letters(c));
            cx += w;
        }
    }
    let mut cy = y - hh;
    for &(a, b) in rows {
        for r in a..=b.min(a.saturating_add(1_048_576)) {
            let h = g.row_h(r);
            cell(page, x, cy, hw, h, &r.to_string());
            cy -= h;
        }
    }
}

/// Draw the cells of rows `r0..=r1` and columns `c0..=c1` with their top-left corner at PDF
/// `(ox, oy)`: fills, then text, then borders.
fn draw_block(
    page: &mut Page,
    g: &Grid<'_>,
    (r0, r1): (u32, u32),
    (c0, c1): (u32, u32),
    (ox, oy): (f64, f64),
    merge_work: &std::cell::Cell<usize>,
    merge_cut: &std::cell::Cell<bool>,
) {
    let (sh, styles) = (g.sh, g.styles);
    // Only the merges that touch this block are searched for each of its cells. Past the
    // sheet's budget of comparisons the rest of its cells are drawn unmerged (and reported).
    let merges: Vec<(u32, u32, u32, u32)> =
        sh.merges.iter().copied().filter(|(mr1, mc1, mr2, mc2)| *mr1 <= r1 && *mr2 >= r0 && *mc1 <= c1 && *mc2 >= c0).collect();
    let merged_at = |r: u32, c: u32| {
        let Some(left) = merge_work.get().checked_sub(merges.len()) else {
            merge_cut.set(true);
            return None;
        };
        merge_work.set(left);
        merges.iter().find(|(r1, c1, r2, c2)| (*r1..=*r2).contains(&r) && (*c1..=*c2).contains(&c)).copied()
    };
    let x_of = |c: u32| ox + g.col_x(c) - g.col_x(c0);
    let y_of = |r: u32| oy - (g.row_y(r) - g.row_y(r0));
    if sh.gridlines {
        let (xa, xb) = (x_of(c0), x_of(c1 + 1));
        let (ya, yb) = (y_of(r0), y_of(r1 + 1));
        // A hidden column or row puts its gridline where the next one goes: draw it once.
        let mut last = f64::NAN;
        for c in c0..=c1 + 1 {
            let x = x_of(c);
            if (x - last).abs() > 0.01 || c == c0 {
                page.line(x, ya, x, yb, 0.25, [0.75; 3]);
                last = x;
            }
        }
        last = f64::NAN;
        for r in r0..=r1 + 1 {
            let y = y_of(r);
            if (y - last).abs() > 0.01 || r == r0 {
                page.line(xa, y, xb, y, 0.25, [0.75; 3]);
                last = y;
            }
        }
    }
    // The block's cells, row by occupied row and only within its columns, already in (row,
    // col) order: a band of hidden rows can be a million rows tall.
    let next_row = |from: u32| (from <= r1).then(|| sh.cells.range((from, 0)..=(r1, u32::MAX)).next()).flatten().map(|((r, _), _)| *r);
    let mut cells_here: Vec<(&(u32, u32), &Cell)> = Vec::new();
    let mut row = next_row(r0);
    while let Some(r) = row {
        cells_here.extend(sh.cells.range((r, c0)..=(r, c1)));
        row = r.checked_add(1).and_then(next_row);
    }
    let rect = |r: u32, c: u32| -> (f64, f64, f64, f64) {
        let (r2, c2) = match merged_at(r, c) {
            Some((mr1, mc1, mr2, mc2)) if mr1 == r && mc1 == c => (mr2.min(r1), mc2.min(c1)),
            _ => (r, c),
        };
        let x = x_of(c);
        let w = x_of(c2 + 1) - x;
        let top = y_of(r);
        let h = top - y_of(r2 + 1);
        (x, top - h, w, h)
    };
    let covered = |r: u32, c: u32| merged_at(r, c).is_some_and(|(mr1, mc1, _, _)| (mr1, mc1) != (r, c));
    for ((r, c), cell) in &cells_here {
        if covered(*r, *c) {
            continue;
        }
        let Some(xf) = styles.xfs.get(cell.xf) else { continue };
        let look = sh.looks.get(&(*r, *c));
        let fill = look
            .and_then(|l| l.fill)
            .or_else(|| styles.fills.get(xf.fill).copied().flatten())
            .or_else(|| sh.table_looks.get(&(*r, *c)).and_then(|(t, _)| t.fill));
        if let Some(f) = fill {
            let (x, y, w, h) = rect(*r, *c);
            page.fill_rect(x, y, w, h, f);
        }
        // A data bar from the cell's left edge, between a tenth and nine tenths of the cell as
        // its value goes from the lowest to the highest, fading from its colour to white.
        if let Some((share, col)) = look.and_then(|l| l.bar) {
            let (x, y, w, h) = rect(*r, *c);
            let len = ((w - 3.0) * (0.1 + 0.8 * share)).max(0.0);
            const STRIPS: u32 = 12;
            for i in 0..STRIPS {
                let t = f64::from(i) / f64::from(STRIPS);
                let c = col.map(|v| v + (1.0 - v) * t * 0.85);
                page.fill_rect(x + 1.5 + len * t, y + 1.5, len / f64::from(STRIPS) + 0.05, (h - 3.0).max(0.0), c);
            }
        }
    }
    for ((r, c), kind, colour, values) in sh.sparklines.iter().filter(|((r, c), ..)| (r0..=r1).contains(r) && (c0..=c1).contains(c)) {
        let (x, y, w, h) = rect(*r, *c);
        draw_sparkline(page, (x + 2.0, y + 2.0, (w - 4.0).max(1.0), (h - 4.0).max(1.0)), *kind, *colour, values);
    }
    for ((r, c), cell) in &cells_here {
        if covered(*r, *c) || cell.text.is_empty() {
            continue;
        }
        let xf = styles.xfs.get(cell.xf).cloned().unwrap_or_default();
        let mut style = g.style_of(cell);
        // A formatted table's font colour and bold, where the cell's own font doesn't set them.
        if let Some((t, _)) = sh.table_looks.get(&(*r, *c)) {
            let own_color = cell.color.is_some() || styles.fonts.get(xf.font).is_some_and(|f| f.color.is_some());
            if let (Some(col), false) = (t.color, own_color) {
                style.color = col;
            }
            style.bold |= t.bold;
        }
        let (mut x, y, mut w, h) = rect(*r, *c);
        if w <= 0.0 || h <= 0.0 {
            continue;
        }
        if let Some(l) = sh.looks.get(&(*r, *c)) {
            style.color = l.color.unwrap_or(style.color);
            style.bold = l.bold.unwrap_or(style.bold);
            style.italic = l.italic.unwrap_or(style.italic);
            style.underline = l.underline.unwrap_or(style.underline);
            style.strike = l.strike.unwrap_or(style.strike);
            // An icon sits at the cell's left; the value moves over to make room.
            if let Some(icon) = l.icon {
                let size = (h - 3.0).clamp(4.0, 12.0);
                cf::draw_icon(page, icon, x + 2.0, y + (h - size) / 2.0, size);
                x += size + 2.0;
                w -= size + 2.0;
            }
            if l.hide_value {
                continue;
            }
        }
        let text = g.texts.get(&(*r, *c)).cloned().unwrap_or_default();
        // A rich cell's runs, with the conditional format's font changes over each.
        let segs: Option<Vec<(String, RunStyle)>> = g.rich.get(&(*r, *c)).map(|segs| {
            segs.iter()
                .map(|(t, st)| {
                    let mut st = st.clone();
                    if let Some(l) = sh.looks.get(&(*r, *c)) {
                        st.color = l.color.unwrap_or(st.color);
                        st.bold = l.bold.unwrap_or(st.bold);
                        st.italic = l.italic.unwrap_or(st.italic);
                        st.underline = l.underline.unwrap_or(st.underline);
                        st.strike = l.strike.unwrap_or(st.strike);
                    }
                    (t.clone(), st)
                })
                .collect()
        });
        if let Some(segs) = &segs {
            style = segs.iter().map(|(_, st)| st).filter(|st| st.vert == 0).max_by(|a, b| a.size.total_cmp(&b.size)).cloned().unwrap_or(style);
        }
        if xf.rotation != 0 {
            let one: Vec<(String, RunStyle)> =
                segs.unwrap_or_else(|| vec![(text.clone(), style.clone())]).into_iter().map(|(t, st)| (t.replace('\n', " "), st)).collect();
            draw_turned(page, (x, y, w, h), &xf, cell, &one, &style);
            continue;
        }
        let lines = g.wrapped.get(&(*r, *c)).cloned().unwrap_or_else(|| vec![text.replace('\n', " ")]);
        // Shrink to fit: a line too wide for the cell is set small enough to fit.
        if xf.shrink && !xf.wrap && segs.is_none() {
            let tw = lines.iter().map(|l| style.width(l)).fold(0.0, f64::max);
            if tw > w - 4.0 && tw > 0.0 {
                style.size = (style.size * (w - 4.0) / tw).max(1.0);
            }
        }
        // Each line's runs: the rich runs cut where the line's characters fall, else the cell's style.
        let line_segs: Vec<Vec<(String, RunStyle)>> = match &segs {
            Some(segs) => split_rich(segs, &lines),
            None => lines.iter().map(|l| vec![(l.clone(), style.clone())]).collect(),
        };
        let line_h = if lines.len() > 1 {
            let (a, d, gap) = style.line_metrics();
            style.size * (a + d + gap)
        } else {
            style.size * 1.2
        };
        let block = lines.len() as f64 * line_h;
        let base_top = match xf.v_align.as_deref() {
            Some("top") => y + h - 1.0,
            Some("center") => y + h / 2.0 + block / 2.0,
            _ => y + block + 1.0,
        };
        // "general" is the default, written out by LibreOffice and others.
        let align = xf.h_align.clone().filter(|a| a != "general" && !a.is_empty()).unwrap_or_else(|| {
            if cell.numeric {
                "right".into()
            } else if cell.boolean {
                "center".into()
            } else {
                "left".into()
            }
        });
        // Text may run into empty neighbours unless it wraps or is a number, as in Excel.
        let mut clip_w = w;
        if !xf.wrap && !cell.numeric && align == "left" {
            let tw = lines.first().map_or(0.0, |l| style.width(l));
            let mut cc = c + 1;
            while clip_w < tw + 4.0 && cc <= c1 && sh.cells.get(&(*r, cc)).is_none_or(|n| n.text.is_empty()) && !covered(*r, cc) {
                clip_w += g.width(cc);
                cc += 1;
            }
        }
        page.save();
        page.clip_rect(x, y, clip_w, h);
        for (i, l) in lines.iter().enumerate() {
            let pieces = line_segs.get(i).cloned().unwrap_or_else(|| vec![(l.clone(), style.clone())]);
            let tw: f64 = pieces.iter().map(|(t, st)| st.width(t)).sum();
            let indent = xf.indent * 9.0;
            let lx = match align.as_str() {
                "right" => x + w - 2.0 - tw - indent,
                "center" | "centerContinuous" => x + (w - tw) / 2.0,
                _ => x + 2.0 + indent,
            };
            let ly = base_top - (i as f64 + 1.0) * line_h + style.size * 0.25;
            // Fill repeats the text across the cell; justified lines (all but the last) and
            // distributed ones (every line) spread their words to the cell's width.
            let last = i + 1 == lines.len();
            let spread = match align.as_str() {
                "justify" => !last,
                "distributed" => true,
                _ => false,
            };
            if line_segs.get(i).is_some_and(|p| p.len() == 1) && !cell.numeric && (align == "fill" || spread) {
                let room = w - 4.0 - indent;
                if align == "fill" {
                    let times = if tw > 0.0 { ((room / tw).floor() as usize).clamp(1, 1_000) } else { 1 };
                    page.text(x + 2.0 + indent, ly, &style, &l.repeat(times));
                    continue;
                }
                let words: Vec<&str> = l.split(' ').filter(|w| !w.is_empty()).collect();
                if words.len() > 1 {
                    let ink: f64 = words.iter().map(|w| style.width(w)).sum();
                    let gap = ((room - ink) / (words.len() - 1) as f64).max(style.width(" "));
                    let mut px = x + 2.0 + indent;
                    for word in words {
                        page.text(px, ly, &style, word);
                        px += style.width(word) + gap;
                    }
                    continue;
                }
                if align == "distributed" {
                    page.text(x + (w - tw) / 2.0, ly, &style, l);
                    continue;
                }
            }
            let fitted =
                cell.value.filter(|_| cell.general && numeric_overflow(cell, tw, w)).and_then(|v| general_fit(v, w - 4.0, &|s| style.width(s)));
            if let Some(short) = fitted {
                page.text(x + w - 2.0 - style.width(&short), ly, &style, &short);
            } else if numeric_overflow(cell, tw, w) {
                // Excel shows ### when a number doesn't fit its column.
                let hashes = "#".repeat(((w - 4.0) / style.width("#")).floor().clamp(1.0, 255.0) as usize);
                page.text(x + 2.0, ly, &style, &hashes);
            } else {
                let mut px = lx;
                for (t, st) in &pieces {
                    page.text(px, ly, st, t);
                    px += st.width(t);
                }
            }
        }
        page.restore();
    }
    for ((r, c), _) in &cells_here {
        if covered(*r, *c) {
            continue;
        }
        // A cell's borders: its own, else its formatted table's.
        let borders_of = |r: u32, c: u32| -> Edges {
            let own = sh
                .cells
                .get(&(r, c))
                .and_then(|cell| styles.xfs.get(cell.xf))
                .and_then(|xf| styles.borders.get(xf.border))
                .copied()
                .unwrap_or_default();
            let table = sh.table_looks.get(&(r, c)).map(|(_, e)| *e).unwrap_or_default();
            Edges {
                left: own.left.or(table.left),
                right: own.right.or(table.right),
                top: own.top.or(table.top),
                bottom: own.bottom.or(table.bottom),
                diagonal: own.diagonal,
            }
        };
        let mut b = borders_of(*r, *c);
        // A merged range's bottom and right edges are its last row's and column's.
        if let Some((_, _, mr2, mc2)) = merged_at(*r, *c).filter(|(mr1, mc1, ..)| (*mr1, *mc1) == (*r, *c)) {
            if mr2 > *r {
                b.bottom = borders_of(mr2, *c).bottom.or(b.bottom);
            }
            if mc2 > *c {
                b.right = borders_of(*r, mc2).right.or(b.right);
            }
        }
        if b.top.is_none() && b.bottom.is_none() && b.left.is_none() && b.right.is_none() && b.diagonal.is_none() {
            continue;
        }
        let (x, y, w, h) = rect(*r, *c);
        let mut edge = |e: Option<Edge>, x1: f64, y1: f64, x2: f64, y2: f64| {
            let Some(e) = e else { return };
            if e.double {
                // Two thin lines, one each side of the edge.
                let (dx, dy) = if (y1 - y2).abs() < 0.01 { (0.0, 0.75) } else { (0.75, 0.0) };
                page.line(x1 - dx, y1 - dy, x2 - dx, y2 - dy, e.width, e.color);
                page.line(x1 + dx, y1 + dy, x2 + dx, y2 + dy, e.width, e.color);
                return;
            }
            match e.dash {
                Some(d) => page.dashed_line(x1, y1, x2, y2, e.width, e.color, d),
                None => page.line(x1, y1, x2, y2, e.width, e.color),
            }
        };
        edge(b.top, x, y + h, x + w, y + h);
        edge(b.bottom, x, y, x + w, y);
        edge(b.left, x, y, x, y + h);
        edge(b.right, x + w, y, x + w, y + h);
        if let Some((e, up, down)) = b.diagonal {
            if up {
                edge(Some(e), x, y, x + w, y + h);
            }
            if down {
                edge(Some(e), x, y + h, x + w, y);
            }
        }
    }
}

/// A sparkline in its cell: a line through the values, a column for each, or win/loss marks
/// (up for positive, down for negative, all the same height).
fn draw_sparkline(page: &mut Page, (x, y, w, h): (f64, f64, f64, f64), kind: u8, colour: [f64; 3], values: &[f64]) {
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min).min(if kind == 1 { 0.0 } else { f64::INFINITY });
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max).max(if kind == 1 { 0.0 } else { f64::NEG_INFINITY });
    let span = (hi - lo).max(1e-9);
    let yv = |v: f64| y + (v - lo) / span * h;
    let n = values.len();
    let step = w / n.max(1) as f64;
    match kind {
        0 => {
            let gap = if n > 1 { w / (n - 1) as f64 } else { 0.0 };
            let path: String = values
                .iter()
                .enumerate()
                .map(|(i, v)| format!("{} {} {} ", super::canvas::n(x + gap * i as f64), super::canvas::n(yv(*v)), if i == 0 { "m" } else { "l" }))
                .collect();
            page.raw(&format!(
                "q {} {} {} RG 0.75 w 1 j {path}S Q\n",
                super::canvas::n(colour[0]),
                super::canvas::n(colour[1]),
                super::canvas::n(colour[2])
            ));
        }
        1 => {
            for (i, v) in values.iter().enumerate() {
                let (a, z) = (yv(0.0f64.clamp(lo, hi)), yv(*v));
                page.fill_rect(x + step * i as f64 + step * 0.1, a.min(z), step * 0.8, (z - a).abs().max(0.3), colour);
            }
        }
        _ => {
            for (i, v) in values.iter().enumerate() {
                let half = h / 2.0;
                let (by, bh) = if *v >= 0.0 { (y + half, half * 0.9) } else { (y + half * 0.1, half * 0.9) };
                page.fill_rect(x + step * i as f64 + step * 0.1, by, step * 0.8, bh, if *v >= 0.0 { colour } else { [0.75, 0.0, 0.0] });
            }
        }
    }
}

/// Degrees counter-clockwise for a `textRotation` (91–180 turn clockwise).
fn rotation_degrees(rot: u16) -> f64 {
    if rot <= 90 { f64::from(rot) } else { -f64::from(rot.saturating_sub(90).min(90)) }
}

/// Cut a rich cell's runs to its wrapped lines: each line's characters are found in the runs in
/// order (the spaces and breaks wrapping dropped are skipped). A line that can't be matched is
/// drawn whole in the first run's style.
fn split_rich(segs: &[(String, RunStyle)], lines: &[String]) -> Vec<Vec<(String, RunStyle)>> {
    let chars: Vec<(char, usize)> = segs.iter().enumerate().flat_map(|(i, (t, _))| t.chars().map(move |c| (c, i))).collect();
    let first = segs.first().map(|(_, st)| st.clone()).unwrap_or_default();
    let mut at = 0usize;
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        let want: Vec<char> = line.chars().collect();
        while at < chars.len() && want.first().is_some_and(|w| chars[at].0 != *w) && matches!(chars[at].0, ' ' | '\n') {
            at += 1;
        }
        let got = chars.get(at..at + want.len());
        let matched = got.is_some_and(|g| g.iter().zip(&want).all(|((c, _), w)| c == w || (*c == '\n' && *w == ' ')));
        let Some(got) = got.filter(|_| matched) else {
            out.push(vec![(line.clone(), first.clone())]);
            continue;
        };
        // Pieces of the line by the run each character came from.
        let mut pieces: Vec<(String, usize)> = Vec::new();
        for (c, i) in got {
            let c = if *c == '\n' { ' ' } else { *c };
            match pieces.last_mut() {
                Some((t, run)) if run == i => t.push(c),
                _ => pieces.push((c.to_string(), *i)),
            }
        }
        at += want.len();
        out.push(pieces.into_iter().map(|(t, i)| (t, segs.get(i).map(|(_, st)| st.clone()).unwrap_or_default())).collect());
    }
    out
}

/// Draw a cell's rotated (or, at 255, stacked) text, placed in the cell by its alignment.
fn draw_turned(page: &mut Page, (x, y, w, h): (f64, f64, f64, f64), xf: &Xf, cell: &Cell, segs: &[(String, RunStyle)], style: &RunStyle) {
    let h_align = xf.h_align.as_deref().filter(|a| *a != "general" && !a.is_empty());
    let v_align = xf.v_align.as_deref();
    page.save();
    page.clip_rect(x, y, w, h);
    if xf.rotation == 255 {
        // One character a line, top to bottom, each centred unless the cell says otherwise.
        let chars: Vec<(char, &RunStyle)> = segs.iter().flat_map(|(t, st)| t.chars().map(move |c| (c, st))).collect();
        let line_h = style.size * 1.2;
        let block = chars.len() as f64 * line_h;
        let top = match v_align {
            Some("top") => y + h - 1.0,
            Some("center") => y + (h + block) / 2.0,
            _ => y + block + 1.0,
        };
        for (i, (c, st)) in chars.iter().enumerate() {
            let s = c.to_string();
            let cw = st.width(&s);
            let lx = match h_align {
                Some("left") => x + 2.0,
                Some("right") => x + w - 2.0 - cw,
                _ => x + (w - cw) / 2.0,
            };
            page.text(lx, top - (i as f64 + 1.0) * line_h + style.size * 0.25, st, &s);
        }
        page.restore();
        return;
    }
    let a = rotation_degrees(xf.rotation).to_radians();
    let (sin, cos) = a.sin_cos();
    let tw: f64 = segs.iter().map(|(t, st)| st.width(t)).sum();
    // The text's box about its baseline origin, turned: where its corners land.
    let (down, up) = (style.size * 0.25, style.size * 0.95);
    let corners = [(0.0, -down), (tw, -down), (0.0, up), (tw, up)].map(|(px, py)| (px * cos - py * sin, px * sin + py * cos));
    let (min_x, max_x) = corners.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (px, _)| (lo.min(*px), hi.max(*px)));
    let (min_y, max_y) = corners.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (_, py)| (lo.min(*py), hi.max(*py)));
    let (bw, bh) = (max_x - min_x, max_y - min_y);
    let bx = match h_align.unwrap_or(if cell.numeric { "right" } else { "left" }) {
        "right" => x + w - 2.0 - bw,
        "center" | "centerContinuous" => x + (w - bw) / 2.0,
        _ => x + 2.0,
    };
    let by = match v_align {
        Some("top") => y + h - 1.0 - bh,
        Some("center") => y + (h - bh) / 2.0,
        _ => y + 1.0,
    };
    page.raw(&format!(
        "{} {} {} {} {} {} cm\n",
        super::canvas::n(cos),
        super::canvas::n(sin),
        super::canvas::n(-sin),
        super::canvas::n(cos),
        super::canvas::n(bx - min_x),
        super::canvas::n(by - min_y)
    ));
    let mut px = 0.0;
    for (t, st) in segs {
        page.text(px, 0.0, st, t);
        px += st.width(t);
    }
    page.restore();
}

fn numeric_overflow(cell: &Cell, text_w: f64, w: f64) -> bool {
    cell.numeric && text_w > w - 2.0 && w > 6.0
}

/// Print a chart sheet: its chart filling the page inside the margins (landscape unless the
/// sheet says otherwise). Returns what its header and footer need.
#[allow(clippy::too_many_arguments)]
fn chart_sheet(
    zip: &Zip<'_>,
    part: &str,
    book: &Book<'_>,
    doc: &mut Document,
    canvas: &mut Canvas,
    notes: &mut Notes,
    jobs: &mut Vec<HfJob>,
    index: usize,
) -> Option<HfSheet> {
    let text = zip.text(part).ok()??;
    let d = xml::parse(&text).ok()?;
    let root = d.root_element();
    let setup = child(root, "pageSetup");
    let paper = match setup.and_then(|p| attr_f64(p, "paperSize")).map(|v| v as i64) {
        Some(9) => (595.28, 841.89),
        Some(5) => (612.0, 1008.0),
        Some(8) => (841.89, 1190.55),
        Some(11) => (419.53, 595.28),
        _ => (612.0, 792.0),
    };
    let (pw, ph) = if setup.and_then(|p| attr(p, "orientation")) == Some("portrait") { paper } else { (paper.1, paper.0) };
    let m = child(root, "pageMargins");
    let g = |k: &str, dflt: f64| m.and_then(|m| attr_f64(m, k)).map_or(dflt, |v| (v * 72.0).clamp(0.0, 300.0));
    let margins = [g("top", 54.0), g("right", 50.4), g("bottom", 54.0), g("left", 50.4)];
    let rels = Rels::load(zip, part).ok()?;
    let drawn = child(root, "drawing")
        .and_then(|dr| rel_attr(dr, "id"))
        .and_then(|id| rels.target(id))
        .map(|t| drawing::read(zip, t, doc, book.theme, notes))
        .unwrap_or_default();
    let pi = canvas.new_page(pw, ph).ok()?;
    let page = canvas.page(pi)?;
    let [mt, mr, mb, ml] = margins;
    for dr in &drawn {
        drawing::draw(page, dr, ml, mb, (pw - ml - mr).max(10.0), (ph - mt - mb).max(10.0));
    }
    jobs.push(HfJob { page: pi, sheet: index, first: true });
    let hf = child(root, "headerFooter").map(|h| {
        let text = |k: &str| child(h, k).and_then(|c| c.text()).map(str::to_owned);
        HeaderFooter {
            odd_header: text("oddHeader"),
            odd_footer: text("oddFooter"),
            even_header: text("evenHeader"),
            even_footer: text("evenFooter"),
            first_header: text("firstHeader"),
            first_footer: text("firstFooter"),
            different_first: flag(attr(h, "differentFirst")) && attr(h, "differentFirst").is_some(),
            different_odd_even: flag(attr(h, "differentOddEven")) && attr(h, "differentOddEven").is_some(),
        }
    });
    Some(HfSheet {
        name: String::new(),
        hf: hf.unwrap_or_default(),
        pictures: Pictures::new(),
        margins,
        hf_margins: (g("header", 21.6), g("footer", 21.6)),
        first_number: None,
    })
}

/// A defined name's ranges (`'Sheet 1'!$A$1:$C$9,'Sheet 1'!$3:$3`) as (first row, first
/// column, last row, last column); whole rows or columns run to the sheet's edge.
fn name_ranges(v: &str) -> Vec<(u32, u32, u32, u32)> {
    let mut parts = Vec::new();
    let (mut cur, mut quoted) = (String::new(), false);
    for c in v.chars().take(4_096) {
        match c {
            '\'' => {
                quoted = !quoted;
                cur.push(c);
            }
            ',' if !quoted => parts.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    parts.push(cur);
    parts
        .iter()
        .filter_map(|p| {
            let r = p.rsplit_once('!').map_or(p.as_str(), |(_, r)| r).replace('$', "");
            let (a, b) = r.split_once(':').unwrap_or((r.as_str(), r.as_str()));
            if let (Some((r1, c1)), Some((r2, c2))) = (col_index(a), col_index(b)) {
                return Some((r1.min(r2), c1.min(c2), r1.max(r2), c1.max(c2)));
            }
            if let (Ok(r1), Ok(r2)) = (a.parse::<u32>(), b.parse::<u32>()) {
                let (r1, r2) = (r1.clamp(1, MAX_ROW), r2.clamp(1, MAX_ROW));
                return Some((r1.min(r2), 1, r1.max(r2), MAX_COL));
            }
            let (Some((_, c1)), Some((_, c2))) = (col_index(&format!("{a}1")), col_index(&format!("{b}1"))) else { return None };
            Some((1, c1.min(c2), MAX_ROW, c1.max(c2)))
        })
        .take(64)
        .collect()
}

/// The short date and time formats Excel's `&D` and `&T` print in for a locale name
/// (`LC_ALL`/`LC_TIME`/`LANG`, e.g. `fr_FR.UTF-8`): US English (`10/28/26`, `7:33 PM`) unless
/// the locale writes day first or year first.
fn date_formats(locale: &str) -> (&'static str, &'static str) {
    let l = locale.to_ascii_lowercase().replace('-', "_");
    let lang = l.split(['_', '.', '@']).next().unwrap_or("");
    match lang {
        "" | "c" | "posix" => ("%-m/%-d/%y", "%-I:%M %p"),
        "en" if l.starts_with("en_us") || l == "en" => ("%-m/%-d/%y", "%-I:%M %p"),
        "en" => ("%d/%m/%Y", "%H:%M"),
        "de" | "ru" | "pl" | "cs" | "sk" | "fi" | "nb" | "no" | "da" | "tr" | "uk" | "ro" | "hu" => ("%d.%m.%Y", "%H:%M"),
        "nl" => ("%d-%m-%Y", "%H:%M"),
        "ja" | "zh" | "ko" => ("%Y/%m/%d", "%H:%M"),
        "sv" | "lt" => ("%Y-%m-%d", "%H:%M"),
        _ => ("%d/%m/%Y", "%H:%M"),
    }
}

/// Now as headers print it (`&D`, `&T`), in the computer's time zone and short date format where
/// they can be read (UTC and US English in the browser), and today as a date serial.
fn now(date1904: bool) -> (String, String, f64) {
    let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let utc = i64::try_from(secs).ok().and_then(|s| chrono::DateTime::from_timestamp(s, 0)).unwrap_or_default();
    #[cfg(not(target_arch = "wasm32"))]
    let (local, locale) = (
        utc.with_timezone(&chrono::Local).naive_local(),
        ["LC_ALL", "LC_TIME", "LANG"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty())).unwrap_or_default(),
    );
    #[cfg(target_arch = "wasm32")]
    let (local, locale) = (utc.naive_utc(), String::new());
    let (date_fmt, time_fmt) = date_formats(&locale);
    // Serial 25 569 is 1970-01-01 in the 1900 system; the 1904 system counts 1 462 days fewer.
    let local_secs = local.and_utc().timestamp() as f64;
    let serial = local_secs / 86_400.0 + 25_569.0 - if date1904 { 1_462.0 } else { 0.0 };
    (local.format(date_fmt).to_string(), local.format(time_fmt).to_string(), serial)
}

pub(crate) fn convert(zip: &Zip<'_>, main: &str, name: &str, doc: &mut Document, notes: &mut Notes) -> Result<usize, String> {
    let rels = Rels::load(zip, main)?;
    let text = zip.text(main)?.ok_or_else(|| format!("{main} is missing"))?;
    let wb = xml::parse(&text).map_err(|e| format!("{main}: {e}"))?;
    let root = wb.root_element();
    let date1904 = child(root, "workbookPr").is_some_and(|p| flag(attr(p, "date1904")) && attr(p, "date1904").is_some());
    let theme = theme_colors(zip, &rels);
    let styles = load_styles(zip, &rels, &theme, notes)?;
    let shared = shared_strings(zip, &rels, &theme)?;
    let (date, time, today) = now(date1904);
    let mac = zip.text("docProps/app.xml").ok().flatten().is_some_and(|t| t.contains("Macintosh"));
    let book = Book { shared: &shared, styles: &styles, theme: &theme, date1904, today, mdw: if mac { 8.0 } else { 7.0 } };
    // Print areas and titles are names local to a sheet, by its position in the workbook.
    let mut areas: HashMap<usize, Vec<(u32, u32, u32, u32)>> = HashMap::new();
    let mut titles: HashMap<usize, Vec<(u32, u32, u32, u32)>> = HashMap::new();
    for d in child(root, "definedNames").into_iter().flat_map(|d| children(d, "definedName")) {
        let Some(sheet) = attr_f64(d, "localSheetId").filter(|v| v.is_finite() && *v >= 0.0).map(|v| v.min(1e6) as usize) else { continue };
        let value = d.text().unwrap_or_default();
        match attr(d, "name") {
            Some("_xlnm.Print_Area") => {
                areas.insert(sheet, name_ranges(value));
            }
            Some("_xlnm.Print_Titles") => {
                titles.insert(sheet, name_ranges(value));
            }
            _ => {}
        }
    }
    let mut canvas = Canvas::default();
    let mut jobs: Vec<HfJob> = Vec::new();
    let mut hf_sheets: Vec<HfSheet> = Vec::new();
    for (index, s) in child(root, "sheets").into_iter().flat_map(|s| children(s, "sheet")).enumerate() {
        if attr(s, "state").is_some_and(|v| v != "visible") {
            notes.add("hidden sheets, left out");
            continue;
        }
        let Some(id) = rel_attr(s, "id") else { continue };
        let Some(part) = rels.target(id) else { continue };
        // Chart sheets print their chart on a page of its own; dialog sheets and macro sheets
        // (xlMacrosheet) are relationships of their own types and are not printed.
        if rels.type_of(id) == Some("chartsheet") {
            if let Some(hf) = chart_sheet(zip, part, &book, doc, &mut canvas, notes, &mut jobs, hf_sheets.len()) {
                hf_sheets.push(HfSheet { name: attr(s, "name").unwrap_or_default().to_owned(), ..hf });
            }
            continue;
        }
        if rels.type_of(id) != Some("worksheet") {
            notes.add("dialog and macro sheets, left out");
            continue;
        }
        let mut sheet = read_sheet(zip, part, &book, doc, notes)?;
        sheet.print_areas = areas.remove(&index).unwrap_or_default();
        for (r1, c1, r2, c2) in titles.remove(&index).unwrap_or_default() {
            if c1 == 1 && c2 == MAX_COL {
                sheet.title_rows = Some((r1, r2));
            } else if r1 == 1 && r2 == MAX_ROW {
                sheet.title_cols = Some((c1, c2));
            }
        }
        draw_sheet(&mut canvas, &sheet, &styles, notes, &mut jobs, hf_sheets.len())?;
        // Links to the sheet go to the top of its first page.
        if let Some(first) = jobs.iter().filter(|j| j.sheet == hf_sheets.len()).map(|j| j.page).min() {
            let top = canvas.page(first).map_or(792.0, |p| p.h);
            canvas.anchors.entry(format!("sheet:{}", attr(s, "name").unwrap_or_default().to_lowercase())).or_insert((first, top));
        }
        draw_comments(&mut canvas, &sheet, &styles, notes, &mut jobs, hf_sheets.len())?;
        hf_sheets.push(HfSheet {
            name: attr(s, "name").unwrap_or_default().to_owned(),
            hf: sheet.hf,
            pictures: sheet.hf_pictures,
            margins: sheet.margins,
            hf_margins: sheet.hf_margins,
            first_number: sheet.first_number,
        });
    }
    // Headers and footers, now that the page count is known: pages are numbered through the
    // workbook, as Excel prints a whole workbook.
    let total = canvas.pages.len();
    let base = styles.fonts.first().map_or_else(RunStyle::default, |f| RunStyle {
        fam: family_of(&f.name),
        font: super::faces::face_of(&f.name),
        size: f.size,
        ..RunStyle::default()
    });
    let first_pages: HashMap<usize, usize> = jobs.iter().fold(HashMap::new(), |mut m, j| {
        let e = m.entry(j.sheet).or_insert(j.page);
        *e = (*e).min(j.page);
        m
    });
    for job in jobs {
        let Some(sh) = hf_sheets.get(job.sheet) else { continue };
        let number = match (sh.first_number, first_pages.get(&job.sheet)) {
            (Some(first), Some(start)) => first.saturating_add(job.page - start),
            _ => job.page + 1,
        };
        let (header, footer, suffix) = sh.hf.pick(job.first, number);
        let Some(page) = canvas.page(job.page) else { continue };
        let frame = hf::Frame { w: page.w, h: page.h, left: sh.margins[3], right: sh.margins[1], header: sh.hf_margins.0, footer: sh.hf_margins.1 };
        let vars = hf::Vars { page: number, pages: total, date: &date, time: &time, file: name, sheet: &sh.name };
        for (text, top) in [(header, true), (footer, false)] {
            if let Some(text) = text {
                hf::draw(page, text, top, &frame, &base, &vars, &sh.pictures, suffix, notes);
            }
        }
    }
    notes.fonts = canvas.embedded_faces();
    canvas.finish(doc).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractions_conditions_and_narrow_general_numbers() {
        let f = |v: f64, code: &str| format_number(v, code, false).0;
        assert_eq!(f(5.25, "# ?/?"), "5 1/4");
        assert_eq!(f(0.3333, "# ??/??"), " 1/3 ");
        assert_eq!(f(1.5, "?/?"), "3/2");
        assert_eq!(f(2.375, "# ?/8"), "2 3/8");
        assert_eq!(f(-0.5, "# ?/?"), "-1/2");
        assert_eq!(f(4.0, "# ?/?").trim_end(), "4");
        assert_eq!(f(150.0, "[>=100]0;0.00"), "150");
        assert_eq!(f(50.0, "[>=100]0;0.00"), "50.00");
        assert_eq!(f(-3.0, "[>0]0;[<0]0.0;\"zero\""), "-3.0");
        assert_eq!(f(0.0, "[>0]0;[<0]0.0;\"zero\""), "zero");
        let w = |s: &str| s.len() as f64 * 6.0;
        assert_eq!(general_fit(1.23456789, 29.0, &w).as_deref(), Some("1.23"));
        assert_eq!(general_fit(123_456_789_012.0, 47.0, &w).as_deref(), Some("1.2E+11"));
        assert_eq!(general_fit(123_456.0, 12.0, &w), None);
    }

    #[test]
    fn number_formats() {
        let f = |v: f64, code: &str| format_number(v, code, false).0;
        assert_eq!(f(1234.5, "General"), "1234.5");
        assert_eq!(f(1234.5, "#,##0.00"), "1,234.50");
        assert_eq!(f(-1234.5, "#,##0.00"), "-1,234.50");
        assert_eq!(f(0.256, "0.0%"), "25.6%");
        assert_eq!(f(1234.5, "[$R$-416] #,##0.00"), "R$ 1.234,50");
        assert_eq!(f(-5.0, "#,##0;(#,##0)"), "(5)");
        assert_eq!(f(0.0, "#,##0;(#,##0);\"-\""), "-");
        assert_eq!(f(12345.678, "0.00E+00"), "1.23E+04");
        assert_eq!(f(1500000.0, "#,##0,,\"M\""), "2M");
        assert_eq!(f(45000.0, "yyyy-mm-dd"), "2023-03-15");
        assert_eq!(f(45000.75, "dd/mm/yyyy hh:mm"), "15/03/2023 18:00");
        assert_eq!(f(45000.0, "[$-416]d \"de\" mmmm \"de\" yyyy"), "15 de março de 2023");
        assert_eq!(f(45000.0, "mmm d, yyyy"), "Mar 15, 2023");
        assert_eq!(f(0.5, "h:mm AM/PM"), "12:00 PM");
        assert_eq!(f(1.0 / 3.0, "General"), "0.3333333333");
        assert_eq!(f(2.5, "0"), "3", "halves round away from zero, as in Excel");
        assert_eq!(f(7.0, "000"), "007");
        assert_eq!(f(-1234.0, "000000"), "-001234");
        assert_eq!(f(45000.5, "\"Hoje é\" h AM/PM"), "Hoje é 12 PM");
        assert_eq!(f(1e20, "General"), "1E+20");
        assert_eq!(f(f64::MAX, "yyyy"), general(f64::MAX));
    }

    #[test]
    fn header_dates_follow_the_locale() {
        assert_eq!(date_formats(""), ("%-m/%-d/%y", "%-I:%M %p"));
        assert_eq!(date_formats("en_US.UTF-8"), ("%-m/%-d/%y", "%-I:%M %p"));
        assert_eq!(date_formats("en_GB.UTF-8").0, "%d/%m/%Y");
        assert_eq!(date_formats("de_DE").0, "%d.%m.%Y");
        assert_eq!(date_formats("ja_JP.UTF-8").0, "%Y/%m/%d");
        assert_eq!(date_formats("pt-BR").0, "%d/%m/%Y");
        let (d, t, serial) = now(false);
        assert!(!d.is_empty() && !t.is_empty() && serial > 45_000.0);
    }

    #[test]
    fn col_letters_count_past_z() {
        assert_eq!(col_letters(1), "A");
        assert_eq!(col_letters(26), "Z");
        assert_eq!(col_letters(27), "AA");
        assert_eq!(col_letters(16_384), "XFD");
        assert_eq!(col_letters(0), "");
    }

    #[test]
    fn cell_references() {
        assert_eq!(col_index("A1"), Some((1, 1)));
        assert_eq!(col_index("AB12"), Some((12, 28)));
        assert_eq!(col_index("XFD1048576"), Some((1_048_576, 16_384)));
        assert_eq!(col_index("ZZZZ1"), None);
        assert_eq!(col_index("A0"), None);
        assert_eq!(col_index("1A"), None);
    }
}
