//! Pictures and shapes on Excel sheets (SpreadsheetML drawings, ECMA-376 Part 1 §20.5): each
//! object's anchor (two cells, one cell and a size, or an absolute position), pictures (PNG,
//! JPEG, EMF), and shapes with their fill, outline, arrowheads and text (rectangles, rounded
//! rectangles, ellipses, arrows, triangles, diamonds and lines; other presets use their
//! bounding rectangle). Charts are drawn as outlined boxes.

use pdfcraft_cos::{Document, ObjRef};
use roxmltree::Node;

use super::canvas::{Page, n};
use super::xml::{self, Rels, attr, attr_f64, child, is, rel_attr};
use super::zip::Zip;
use super::{Notes, RunStyle, family_of, hex_color};

/// Most objects read from one drawing.
const MAX_OBJECTS: usize = 10_000;
/// Most paragraphs and runs read from one shape's text.
const MAX_RUNS: usize = 2_000;

fn emu(v: f64) -> f64 {
    let p = v / 12_700.0;
    if p.is_finite() { p.clamp(-1.0e6, 1.0e6) } else { 0.0 }
}

/// A cell corner and an offset from it in points; `col` and `row` count from 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Corner {
    pub col: u32,
    pub dx: f64,
    pub row: u32,
    pub dy: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Anchor {
    /// From one cell corner to another.
    Cells(Corner, Corner),
    /// From a cell corner, this wide and tall.
    Sized(Corner, f64, f64),
    /// At a position on the sheet, this wide and tall.
    Absolute(f64, f64, f64, f64),
}

#[derive(Clone, Debug)]
pub(crate) struct Shape {
    prst: String,
    fill: Option<[f64; 3]>,
    line: Option<(f64, [f64; 3])>,
    head: bool,
    flip_h: bool,
    flip_v: bool,
    /// Paragraphs: runs, and alignment (0 left, 0.5 centre, 1 right).
    paras: Vec<(Vec<(String, RunStyle)>, f64)>,
    /// Text anchoring: 0 top, 0.5 middle, 1 bottom.
    anchor: f64,
}

#[derive(Clone, Debug)]
pub(crate) enum What {
    Image(Option<ObjRef>),
    Shape(Shape),
    Chart,
}

#[derive(Clone, Debug)]
pub(crate) struct Drawn {
    pub anchor: Anchor,
    pub what: What,
}

fn corner(n: Node<'_, '_>) -> Corner {
    let num = |k: &str| child(n, k).and_then(|c| c.text()).and_then(|t| t.trim().parse::<f64>().ok()).filter(|v| v.is_finite()).unwrap_or(0.0);
    Corner {
        col: num("col").clamp(0.0, 16_383.0) as u32,
        dx: emu(num("colOff")),
        row: num("row").clamp(0.0, 1_048_575.0) as u32,
        dy: emu(num("rowOff")),
    }
}

/// An RGB value from HSL-adjusting a colour's lightness (`lumMod`, `lumOff`), as theme tints do.
fn lum(c: [f64; 3], m: f64, off: f64) -> [f64; 3] {
    let (max, min) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
    let l = (max + min) / 2.0;
    let d = max - min;
    let s = if d == 0.0 { 0.0 } else { d / (1.0 - (2.0 * l - 1.0).abs()) };
    let h = if d == 0.0 {
        0.0
    } else if max == c[0] {
        ((c[1] - c[2]) / d).rem_euclid(6.0)
    } else if max == c[1] {
        (c[2] - c[0]) / d + 2.0
    } else {
        (c[0] - c[1]) / d + 4.0
    };
    let l = (l * m + off).clamp(0.0, 1.0);
    let ch = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = ch * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (ch, x, 0.0),
        1 => (x, ch, 0.0),
        2 => (0.0, ch, x),
        3 => (0.0, x, ch),
        4 => (x, 0.0, ch),
        _ => (ch, 0.0, x),
    };
    let m0 = l - ch / 2.0;
    [r + m0, g + m0, b + m0].map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 })
}

/// A DrawingML colour element's value (`srgbClr`, `schemeClr`, `sysClr`) with its lightness
/// changes. `theme` is in Excel's order: lt1, dk1, lt2, dk2, accent1–6, hlink, folHlink.
fn color(n: Node<'_, '_>, theme: &[[f64; 3]]) -> Option<[f64; 3]> {
    let base = match n.tag_name().name() {
        "srgbClr" => hex_color(attr(n, "val")?)?,
        "sysClr" => hex_color(attr(n, "lastClr")?)?,
        "schemeClr" => {
            let i = match attr(n, "val")? {
                "bg1" | "lt1" => 0,
                "tx1" | "dk1" => 1,
                "bg2" | "lt2" => 2,
                "tx2" | "dk2" => 3,
                "accent1" => 4,
                "accent2" => 5,
                "accent3" => 6,
                "accent4" => 7,
                "accent5" => 8,
                "accent6" => 9,
                "hlink" => 10,
                "folHlink" => 11,
                _ => return None,
            };
            *theme.get(i)?
        }
        _ => return None,
    };
    let pct = |k: &str| child(n, k).and_then(|c| attr_f64(c, "val")).map(|v| (v / 100_000.0).clamp(-10.0, 10.0));
    let (m, off) = (pct("lumMod").unwrap_or(1.0), pct("lumOff").unwrap_or(0.0));
    Some(if m != 1.0 || off != 0.0 { lum(base, m, off) } else { base })
}

/// The colour of a `solidFill` child of `n`; `Some(None)` for `noFill`; `None` when unset.
fn fill_of(n: Option<Node<'_, '_>>, theme: &[[f64; 3]]) -> Option<Option<[f64; 3]>> {
    let n = n?;
    if child(n, "noFill").is_some() {
        return Some(None);
    }
    let f = child(n, "solidFill").or_else(|| child(n, "gradFill").and_then(|g| g.descendants().find(|d| is(*d, "gs"))))?;
    Some(f.children().find(Node::is_element).and_then(|c| color(c, theme)))
}

fn shape(sp: Node<'_, '_>, theme: &[[f64; 3]], notes: &mut Notes) -> Shape {
    let sppr = child(sp, "spPr");
    let style = child(sp, "style");
    let style_color = |k: &str| style.and_then(|s| child(s, k)).and_then(|r| r.children().find(Node::is_element)).and_then(|c| color(c, theme));
    let fill = fill_of(sppr, theme).unwrap_or_else(|| style_color("fillRef"));
    let ln = sppr.and_then(|p| child(p, "ln"));
    let line_color = match fill_of(ln, theme) {
        Some(c) => c,
        None => ln.is_none().then(|| style_color("lnRef")).flatten().or(style_color("lnRef")),
    };
    let width = ln.and_then(|l| attr_f64(l, "w")).map_or(0.75, emu).clamp(0.1, 50.0);
    let xfrm = sppr.and_then(|p| child(p, "xfrm"));
    let prst = sppr.and_then(|p| child(p, "prstGeom")).and_then(|g| attr(g, "prst")).unwrap_or("rect").to_owned();
    let font = style_color("fontRef");
    let mut paras = Vec::new();
    let mut runs = 0usize;
    let body = child(sp, "txBody");
    for p in body.into_iter().flat_map(|b| b.children().filter(|c| is(*c, "p"))) {
        let align = match child(p, "pPr").and_then(|pp| attr(pp, "algn")) {
            Some("ctr") => 0.5,
            Some("r") => 1.0,
            _ => 0.0,
        };
        let mut items = Vec::new();
        for r in p.children().filter(|c| is(*c, "r")) {
            runs += 1;
            if runs > MAX_RUNS {
                break;
            }
            let rpr = child(r, "rPr");
            let st = RunStyle {
                fam: family_of(rpr.and_then(|r| child(r, "latin")).and_then(|l| attr(l, "typeface")).unwrap_or("Calibri")),
                bold: rpr.and_then(|r| attr(r, "b")).is_some_and(|v| v == "1" || v == "true"),
                italic: rpr.and_then(|r| attr(r, "i")).is_some_and(|v| v == "1" || v == "true"),
                size: rpr.and_then(|r| attr_f64(r, "sz")).map_or(11.0, |s| (s / 100.0).clamp(1.0, 400.0)),
                color: fill_of(rpr, theme).flatten().or(font).unwrap_or([0.0; 3]),
                ..RunStyle::default()
            };
            items.push((notes.drawable(&xml::text_of(r, "t")), st));
        }
        paras.push((items, align));
    }
    let anchor = match body.and_then(|b| child(b, "bodyPr")).and_then(|b| attr(b, "anchor")) {
        Some("ctr") => 0.5,
        Some("b") => 1.0,
        _ => 0.0,
    };
    let flag = |k: &str| xfrm.and_then(|x| attr(x, k)).is_some_and(|v| v == "1" || v == "true");
    Shape {
        prst,
        fill,
        line: line_color.map(|c| (width, c)),
        head: ln.is_some_and(|l| child(l, "tailEnd").and_then(|t| attr(t, "type")).is_some_and(|t| t != "none")),
        flip_h: flag("flipH"),
        flip_v: flag("flipV"),
        paras,
        anchor,
    }
}

/// The objects of drawing part `part`, with their pictures added to `doc`.
pub(crate) fn read(zip: &Zip<'_>, part: &str, doc: &mut Document, theme: &[[f64; 3]], notes: &mut Notes) -> Vec<Drawn> {
    let mut out = Vec::new();
    let Ok(Some(text)) = zip.text(part) else { return out };
    let Ok(d) = xml::parse(&text) else { return out };
    let rels = Rels::load(zip, part).ok();
    for a in d.root_element().children().filter(Node::is_element).take(MAX_OBJECTS) {
        let anchor = match a.tag_name().name() {
            "twoCellAnchor" => match (child(a, "from"), child(a, "to")) {
                (Some(f), Some(t)) => Anchor::Cells(corner(f), corner(t)),
                _ => continue,
            },
            "oneCellAnchor" => {
                let (Some(f), Some(e)) = (child(a, "from"), child(a, "ext")) else { continue };
                Anchor::Sized(corner(f), emu(attr_f64(e, "cx").unwrap_or(0.0)), emu(attr_f64(e, "cy").unwrap_or(0.0)))
            }
            "absoluteAnchor" => {
                let (Some(p), Some(e)) = (child(a, "pos"), child(a, "ext")) else { continue };
                let g = |n: Node<'_, '_>, k: &str| emu(attr_f64(n, k).unwrap_or(0.0));
                Anchor::Absolute(g(p, "x"), g(p, "y"), g(e, "cx"), g(e, "cy"))
            }
            _ => continue,
        };
        let Some(obj) = a.children().find(|c| matches!(c.tag_name().name(), "pic" | "sp" | "cxnSp" | "graphicFrame" | "grpSp")) else { continue };
        let what = match obj.tag_name().name() {
            "pic" => {
                let id = obj.descendants().find(|n| is(*n, "blip")).and_then(|b| rel_attr(b, "embed"));
                let target = id.and_then(|id| rels.as_ref()?.target(id)).map(str::to_owned);
                let img = target.and_then(|t| {
                    let bytes = zip.read(&t).ok()??;
                    crate::image_xobject(doc, &t, &bytes).ok().map(|(r, _)| r).or_else(|| super::emf::form(doc, &bytes, notes))
                });
                if img.is_none() {
                    notes.add("sheet pictures in a format the importer can't draw, drawn as empty boxes");
                }
                What::Image(img)
            }
            "sp" | "cxnSp" => What::Shape(shape(obj, theme, notes)),
            "graphicFrame" => {
                notes.add("charts on sheets, drawn as empty boxes");
                What::Chart
            }
            _ => {
                notes.add("grouped shapes on sheets, left out");
                continue;
            }
        };
        out.push(Drawn { anchor, what });
    }
    out
}

/// Draw an object into the rectangle `(x, y)` (bottom left, PDF coordinates) `w × h`.
pub(crate) fn draw(page: &mut Page, d: &Drawn, x: f64, y: f64, w: f64, h: f64) {
    if !(w.is_finite() && h.is_finite()) || w < 0.0 || h < 0.0 {
        return;
    }
    match &d.what {
        What::Image(Some(r)) => page.image(*r, x, y, w, h),
        What::Image(None) | What::Chart => page.placeholder(x, y, w, h),
        What::Shape(s) => draw_shape(page, s, x, y, w, h),
    }
}

/// A line of shape text: its runs, alignment and height.
type TextLine = (Vec<(String, RunStyle)>, f64, f64);

fn draw_shape(page: &mut Page, s: &Shape, x: f64, y: f64, w: f64, h: f64) {
    let (x2, y2) = (x + w, y + h);
    let pts = |p: &[(f64, f64)]| -> String {
        let mut o = String::new();
        for (i, (px, py)) in p.iter().enumerate() {
            o.push_str(&format!("{} {} {} ", n(*px), n(*py), if i == 0 { "m" } else { "l" }));
        }
        o + "h "
    };
    let path = match s.prst.as_str() {
        "line" | "straightConnector1" => {
            // Corner to corner, flipped as the shape is.
            let (sx, ex) = if s.flip_h { (x2, x) } else { (x, x2) };
            let (sy, ey) = if s.flip_v { (y, y2) } else { (y2, y) };
            if let Some((lw, c)) = s.line {
                page.line(sx, sy, ex, ey, lw, c);
                if s.head {
                    let (dx, dy) = (ex - sx, ey - sy);
                    let len = dx.hypot(dy);
                    if len > 0.0 {
                        let (ux, uy) = (dx / len, dy / len);
                        let a = (lw * 3.0).max(4.0);
                        let (bx, by) = (ex - ux * a * 1.5, ey - uy * a * 1.5);
                        let head = pts(&[(ex, ey), (bx - uy * a, by + ux * a), (bx + uy * a, by - ux * a)]);
                        page.raw(&format!("q {} {} {} rg {head}f Q\n", n(c[0]), n(c[1]), n(c[2])));
                    }
                }
            }
            return;
        }
        "ellipse" => {
            let k = 0.552_284_75;
            let (cx, cy, rx, ry) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
            format!(
                "{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c h ",
                n(cx + rx),
                n(cy),
                n(cx + rx),
                n(cy + ry * k),
                n(cx + rx * k),
                n(cy + ry),
                n(cx),
                n(cy + ry),
                n(cx - rx * k),
                n(cy + ry),
                n(cx - rx),
                n(cy + ry * k),
                n(cx - rx),
                n(cy),
                n(cx - rx),
                n(cy - ry * k),
                n(cx - rx * k),
                n(cy - ry),
                n(cx),
                n(cy - ry),
                n(cx + rx * k),
                n(cy - ry),
                n(cx + rx),
                n(cy - ry * k),
                n(cx + rx),
                n(cy)
            )
        }
        "roundRect" => {
            // The default corner radius is a sixth of the shorter side.
            let r = w.min(h) / 6.0;
            let k = r * 0.447_715_25;
            format!(
                "{} {} m {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c h ",
                n(x + r),
                n(y),
                n(x2 - r),
                n(y),
                n(x2 - k),
                n(y),
                n(x2),
                n(y + k),
                n(x2),
                n(y + r),
                n(x2),
                n(y2 - r),
                n(x2),
                n(y2 - k),
                n(x2 - k),
                n(y2),
                n(x2 - r),
                n(y2),
                n(x + r),
                n(y2),
                n(x + k),
                n(y2),
                n(x),
                n(y2 - k),
                n(x),
                n(y2 - r),
                n(x),
                n(y + r),
                n(x),
                n(y + k),
                n(x + k),
                n(y),
                n(x + r),
                n(y)
            )
        }
        "rightArrow" | "leftArrow" => {
            // A shaft half as tall as the shape, a head as long as half its height.
            let head = (h / 2.0).min(w);
            let (q1, q3, mid) = (y + h * 0.25, y + h * 0.75, y + h / 2.0);
            let p = if s.prst == "rightArrow" {
                vec![(x, q1), (x2 - head, q1), (x2 - head, y), (x2, mid), (x2 - head, y2), (x2 - head, q3), (x, q3)]
            } else {
                vec![(x2, q1), (x + head, q1), (x + head, y), (x, mid), (x + head, y2), (x + head, q3), (x2, q3)]
            };
            pts(&p)
        }
        "triangle" => pts(&[(x, y), (x2, y), (x + w / 2.0, y2)]),
        "diamond" => pts(&[(x + w / 2.0, y), (x2, y + h / 2.0), (x + w / 2.0, y2), (x, y + h / 2.0)]),
        _ => pts(&[(x, y), (x2, y), (x2, y2), (x, y2)]),
    };
    let mut ops = String::from("q ");
    if let Some(c) = s.fill {
        ops.push_str(&format!("{} {} {} rg ", n(c[0]), n(c[1]), n(c[2])));
    }
    if let Some((lw, c)) = s.line {
        ops.push_str(&format!("{} {} {} RG {} w ", n(c[0]), n(c[1]), n(c[2]), n(lw)));
    }
    let op = match (s.fill.is_some(), s.line.is_some()) {
        (true, true) => "B",
        (true, false) => "f",
        (false, true) => "S",
        (false, false) => "n",
    };
    ops.push_str(&path);
    ops.push_str(op);
    ops.push_str(" Q\n");
    page.raw(&ops);
    // Text, wrapped inside the default insets (0.1 in left and right, 0.05 in top and bottom).
    let inner = (w - 14.4).max(4.0);
    let mut lines: Vec<TextLine> = Vec::new();
    for (runs, align) in &s.paras {
        let size = runs.iter().map(|(_, st)| st.size).fold(0.0, f64::max).max(1.0);
        if runs.iter().all(|(t, _)| t.trim().is_empty()) {
            lines.push((Vec::new(), *align, size * 1.2));
            continue;
        }
        // Each run is wrapped on its own; runs share a line only when the paragraph fits.
        let total: f64 = runs.iter().map(|(t, st)| st.width(t)).sum();
        if total <= inner || runs.len() > 1 {
            lines.push((runs.clone(), *align, size * 1.2));
        } else if let Some((t, st)) = runs.first() {
            for l in pdfcraft_fonts::wrap_with(t, st.size, inner, |s, size| RunStyle { size, ..st.clone() }.width(s)) {
                lines.push((vec![(l, st.clone())], *align, size * 1.2));
            }
        }
    }
    let block: f64 = lines.iter().map(|(_, _, lh)| lh).sum();
    let mut top = y2 - 3.6 - (h - 7.2 - block).max(0.0) * s.anchor;
    page.save();
    page.clip_rect(x, y, w, h);
    for (runs, align, lh) in lines {
        let lw: f64 = runs.iter().map(|(t, st)| st.width(t)).sum();
        let mut lx = x + 7.2 + (inner - lw).max(0.0) * align;
        let base = top - lh * 0.8;
        for (t, st) in &runs {
            page.text(lx, base, st, t);
            lx += st.width(t);
        }
        top -= lh;
    }
    page.restore();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_lightness_changes_follow_hsl() {
        let c = lum([0.2667, 0.4471, 0.7686], 0.75, 0.0);
        assert!((c[2] - 0.59).abs() < 0.01, "{c:?}");
        let w = lum([0.0; 3], 1.0, 0.5);
        assert!((w[0] - 0.5).abs() < 1e-9);
        let _ = lum([f64::NAN; 3], f64::INFINITY, 0.0);
    }
}
