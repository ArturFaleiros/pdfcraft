//! Office Math (OMML, ECMA-376 Part 1 §22.1) laid out as boxes: fractions, superscripts and
//! subscripts, radicals, n-ary operators with their limits, delimiters that grow with their
//! content, matrices, accents and bars, function names, lower and upper limits and equation
//! arrays. Letters are italic and other characters upright, in Cambria Math's metric twin when
//! it is embedded. Other constructs show their content in a row.

use roxmltree::Node;

use super::canvas::{Page, n};
use super::faces::Face;
use super::flow::fin;
use super::xml::{attr, child, children, is};
use super::{Fam, Notes, RunStyle};

/// Deepest nesting followed; deeper content is left out.
const MAX_DEPTH: usize = 24;
/// Most pieces one equation may hold.
const MAX_ITEMS: usize = 20_000;

#[derive(Clone, Debug)]
enum Item {
    /// Text at `(x, y)` from the box's origin (y up from the baseline).
    Text(f64, f64, String, RunStyle),
    /// A filled rectangle: x, y (bottom), width, height.
    Rule(f64, f64, f64, f64),
    /// A stroked path in box coordinates (`m`/`l` points) of a given width.
    Path(Vec<(f64, f64)>, f64),
}

/// A laid-out piece of an equation: its width, its height above and depth below the baseline,
/// and what it draws, from its baseline's left end.
#[derive(Clone, Debug, Default)]
pub(crate) struct MathBox {
    pub w: f64,
    pub asc: f64,
    pub desc: f64,
    items: Vec<Item>,
}

impl MathBox {
    fn shifted(mut self, dx: f64, dy: f64) -> Vec<Item> {
        for it in &mut self.items {
            match it {
                Item::Text(x, y, _, _) | Item::Rule(x, y, _, _) => {
                    *x += dx;
                    *y += dy;
                }
                Item::Path(pts, _) => pts.iter_mut().for_each(|(x, y)| {
                    *x += dx;
                    *y += dy;
                }),
            }
        }
        self.items
    }

    /// Boxes side by side on one baseline.
    fn row(parts: Vec<MathBox>) -> MathBox {
        let mut out = MathBox::default();
        for p in parts {
            let (w, asc, desc) = (p.w, p.asc, p.desc);
            let x = out.w;
            if out.items.len() + p.items.len() > MAX_ITEMS {
                break;
            }
            out.items.extend(p.shifted(x, 0.0));
            out.w += w;
            out.asc = out.asc.max(asc);
            out.desc = out.desc.max(desc);
        }
        out
    }
}

struct Ctx<'n> {
    notes: &'n mut Notes,
    color: [f64; 3],
}

fn style(size: f64, italic: bool, color: [f64; 3]) -> RunStyle {
    RunStyle { fam: Fam::Serif, font: Face::Caladea, size, italic, color, ..RunStyle::default() }
}

fn text_box(s: &str, st: RunStyle) -> MathBox {
    let (a, d, _) = st.line_metrics();
    MathBox { w: st.width(s), asc: st.size * a.min(0.8), desc: st.size * d.min(0.25), items: vec![Item::Text(0.0, 0.0, s.to_owned(), st)] }
}

fn space(w: f64) -> MathBox {
    MathBox { w, ..MathBox::default() }
}

/// The value of a property child (`m:chr`, `m:begChr`…) of `pr`.
fn prop<'a>(pr: Option<Node<'a, '_>>, name: &str) -> Option<&'a str> {
    pr.and_then(|p| child(p, name)).and_then(|c| attr(c, "val"))
}

fn on(pr: Option<Node<'_, '_>>, name: &str) -> bool {
    pr.and_then(|p| child(p, name)).is_some_and(|c| attr(c, "val").is_none_or(|v| v == "1" || v == "on" || v == "true"))
}

impl Ctx<'_> {
    /// A run (`m:r`): letters italic unless the run is plain (`m:sty p`, `m:nor`), operators upright
    /// and spaced.
    fn run(&mut self, r: Node<'_, '_>, size: f64) -> MathBox {
        let text: String = children(r, "t").filter_map(|t| t.text()).collect();
        let text = self.notes.drawable(&text);
        let plain = matches!(prop(child(r, "rPr"), "sty"), Some("p" | "b")) || child(r, "rPr").is_some_and(|p| child(p, "nor").is_some());
        let mut parts = Vec::new();
        let mut word = String::new();
        let flush = |word: &mut String, parts: &mut Vec<MathBox>, italic: bool| {
            if !word.is_empty() {
                parts.push(text_box(word, style(size, italic, self.color)));
                word.clear();
            }
        };
        for c in text.chars() {
            if c.is_alphabetic() {
                word.push(c);
            } else {
                flush(&mut word, &mut parts, !plain);
                let s = c.to_string();
                let op = matches!(c, '+' | '-' | '−' | '=' | '<' | '>' | '±' | '×' | '÷' | '≤' | '≥' | '≠' | '≈' | '→' | '∈');
                if op {
                    parts.push(space(size * 0.27));
                }
                parts.push(text_box(&s, style(size, false, self.color)));
                if op {
                    parts.push(space(size * 0.27));
                }
            }
        }
        flush(&mut word, &mut parts, !plain);
        MathBox::row(parts)
    }

    /// The children of an argument element (`m:e`, `m:num`…) in a row.
    fn arg(&mut self, n: Option<Node<'_, '_>>, size: f64, depth: usize) -> MathBox {
        match n {
            Some(n) => self.row(n, size, depth + 1),
            None => MathBox::default(),
        }
    }

    pub(crate) fn row(&mut self, n: Node<'_, '_>, size: f64, depth: usize) -> MathBox {
        if depth > MAX_DEPTH {
            return MathBox::default();
        }
        let parts: Vec<MathBox> = n.children().filter(Node::is_element).map(|c| self.node(c, size, depth)).collect();
        MathBox::row(parts)
    }

    fn node(&mut self, c: Node<'_, '_>, size: f64, depth: usize) -> MathBox {
        let small = (size * 0.7).max(4.0);
        let axis = size * 0.27;
        match c.tag_name().name() {
            "r" => self.run(c, size),
            "f" => {
                let (num, den) = (self.arg(child(c, "num"), size, depth), self.arg(child(c, "den"), size, depth));
                let linear = prop(child(c, "fPr"), "type") == Some("lin");
                if linear {
                    let slash = text_box("/", style(size, false, self.color));
                    return MathBox::row(vec![num, slash, den]);
                }
                let w = num.w.max(den.w) + size * 0.2;
                let t = (size * 0.05).max(0.4);
                let gap = size * 0.12;
                let (nw, ndesc, nasc) = (num.w, num.desc, num.asc);
                let (dw, dasc, ddesc) = (den.w, den.asc, den.desc);
                let num_y = axis + t / 2.0 + gap + ndesc;
                let den_y = axis - t / 2.0 - gap - dasc;
                let mut items = Vec::new();
                if prop(child(c, "fPr"), "type") != Some("noBar") {
                    items.push(Item::Rule(0.0, axis - t / 2.0, w, t));
                }
                items.extend(num.shifted((w - nw) / 2.0, num_y));
                items.extend(den.shifted((w - dw) / 2.0, den_y));
                MathBox { w: w + size * 0.1, asc: num_y + nasc, desc: (-den_y + ddesc).max(0.0), items }
            }
            "sSup" | "sSub" | "sSubSup" => {
                let base = self.arg(child(c, "e"), size, depth);
                let sup = child(c, "sup").map(|s| self.row(s, small, depth + 1));
                let sub = child(c, "sub").map(|s| self.row(s, small, depth + 1));
                scripts(base, sup, sub, size)
            }
            "sPre" => {
                let base = self.arg(child(c, "e"), size, depth);
                let sup = child(c, "sup").map(|s| self.row(s, small, depth + 1));
                let sub = child(c, "sub").map(|s| self.row(s, small, depth + 1));
                let pre = scripts(MathBox::default(), sup, sub, size);
                MathBox::row(vec![pre, base])
            }
            "rad" => {
                let body = self.arg(child(c, "e"), size, depth);
                let deg = child(c, "deg").filter(|d| d.children().any(|x| x.is_element())).map(|d| self.row(d, small * 0.85, depth + 1));
                let (bw, basc, bdesc) = (body.w, body.asc, body.desc);
                let top = basc + size * 0.15;
                let hook = size * 0.5;
                let lw = (size * 0.06).max(0.5);
                let deg_w = deg.as_ref().map_or(0.0, |d| (d.w - hook * 0.4).max(0.0));
                let x0 = deg_w;
                let path = vec![
                    (x0, axis * 0.6),
                    (x0 + hook * 0.25, axis * 0.8),
                    (x0 + hook * 0.55, -bdesc),
                    (x0 + hook, top),
                    (x0 + hook + bw + size * 0.1, top),
                ];
                let mut items = vec![Item::Path(path, lw)];
                if let Some(d) = deg {
                    let dy = axis * 0.8 + d.desc + size * 0.05;
                    items.extend(d.shifted(0.0, dy));
                }
                items.extend(body.shifted(x0 + hook + size * 0.05, 0.0));
                MathBox { w: x0 + hook + bw + size * 0.2, asc: top + lw, desc: bdesc, items }
            }
            "nary" => {
                let pr = child(c, "naryPr");
                let chr = prop(pr, "chr").unwrap_or("∫");
                let chr = self.notes.drawable(chr);
                let integral = chr.starts_with('∫') || chr.starts_with('∮');
                let op = text_box(&chr, style(size * if integral { 1.6 } else { 1.8 }, false, self.color));
                let sub =
                    child(c, "sub").filter(|s| !on(pr, "subHide") && s.children().any(|x| x.is_element())).map(|s| self.row(s, small, depth + 1));
                let sup =
                    child(c, "sup").filter(|s| !on(pr, "supHide") && s.children().any(|x| x.is_element())).map(|s| self.row(s, small, depth + 1));
                let body = self.arg(child(c, "e"), size, depth);
                let below = prop(pr, "limLoc").map_or(!integral, |v| v == "undOvr");
                // The operator is centred on the math axis.
                let lift = axis - (op.asc - op.desc) / 2.0;
                let (ow, oasc, odesc) = (op.w, op.asc, op.desc);
                let op = MathBox { asc: oasc + lift, desc: (odesc - lift).max(0.0), items: op.shifted(0.0, lift), w: ow };
                let op = if below { limits(op, sup, sub, size) } else { scripts(op, sup, sub, size) };
                MathBox::row(vec![op, space(size * 0.12), body])
            }
            "d" => {
                let pr = child(c, "dPr");
                let beg = prop(pr, "begChr").unwrap_or("(");
                let end = prop(pr, "endChr").unwrap_or(")");
                let sep = prop(pr, "sepChr").unwrap_or("|");
                let mut parts = Vec::new();
                for (i, e) in children(c, "e").take(256).enumerate() {
                    if i > 0 {
                        parts.push(text_box(&self.notes.drawable(sep), style(size, false, self.color)));
                    }
                    parts.push(self.row(e, size, depth + 1));
                }
                let inner = MathBox::row(parts);
                // Delimiters as tall as what they hold.
                let h = (inner.asc + inner.desc).max(size * 0.9);
                let dsize = (h / 0.95).max(size);
                let fence = |s: &str, ctx: &mut Ctx<'_>| -> MathBox {
                    if s.is_empty() {
                        return MathBox::default();
                    }
                    let b = text_box(&ctx.notes.drawable(s), style(dsize, false, ctx.color));
                    let lift = (inner.asc - inner.desc) / 2.0 - (b.asc - b.desc) / 2.0;
                    let (bw, basc, bdesc) = (b.w, b.asc, b.desc);
                    MathBox { w: bw, asc: basc + lift, desc: (bdesc - lift).max(0.0), items: b.shifted(0.0, lift) }
                };
                let (open, close) = (fence(beg, self), fence(end, self));
                MathBox::row(vec![open, inner, close])
            }
            "m" => {
                let rows: Vec<Vec<MathBox>> =
                    children(c, "mr").take(100).map(|r| children(r, "e").take(100).map(|e| self.row(e, size, depth + 1)).collect()).collect();
                let ncol = rows.iter().map(Vec::len).max().unwrap_or(0);
                let widths: Vec<f64> = (0..ncol).map(|j| rows.iter().filter_map(|r| r.get(j)).map(|b| b.w).fold(0.0, f64::max)).collect();
                let gap = size * 0.6;
                let heights: Vec<(f64, f64)> = rows
                    .iter()
                    .map(|r| (r.iter().map(|b| b.asc).fold(size * 0.7, f64::max), r.iter().map(|b| b.desc).fold(size * 0.2, f64::max)))
                    .collect();
                let total: f64 = heights.iter().map(|(a, d)| a + d + size * 0.2).sum();
                let mut y = axis + total / 2.0;
                let mut items = Vec::new();
                for (r, (a, d)) in rows.into_iter().zip(&heights) {
                    y -= a + size * 0.1;
                    let mut x = 0.0;
                    for (j, b) in r.into_iter().enumerate() {
                        let cw = widths.get(j).copied().unwrap_or(0.0);
                        let bw = b.w;
                        items.extend(b.shifted(x + (cw - bw) / 2.0, y));
                        x += cw + gap;
                    }
                    y -= d + size * 0.1;
                }
                let w = widths.iter().sum::<f64>() + gap * (ncol.saturating_sub(1)) as f64;
                MathBox { w, asc: axis + total / 2.0, desc: (total / 2.0 - axis).max(0.0), items }
            }
            "eqArr" => {
                let rows: Vec<MathBox> = children(c, "e").take(256).map(|e| self.row(e, size, depth + 1)).collect();
                let w = rows.iter().map(|b| b.w).fold(0.0, f64::max);
                let mut items = Vec::new();
                let mut y = 0.0;
                let mut asc = 0.0;
                let mut desc = 0.0;
                for (i, b) in rows.into_iter().enumerate() {
                    if i == 0 {
                        asc = b.asc;
                    } else {
                        y -= b.asc + size * 0.3;
                    }
                    let bdesc = b.desc;
                    items.extend(b.shifted(0.0, y));
                    desc = -y + bdesc;
                    y -= bdesc;
                }
                MathBox { w, asc, desc, items }
            }
            "acc" | "bar" | "groupChr" => {
                let body = self.arg(child(c, "e"), size, depth);
                let pr = c.children().find(|x| x.tag_name().name().ends_with("Pr"));
                let under = prop(pr, "pos") == Some("bot");
                let mark = if c.tag_name().name() == "acc" { prop(pr, "chr").unwrap_or("\u{302}") } else { "" };
                let (bw, basc, bdesc) = (body.w, body.asc, body.desc);
                let mut items = body.shifted(0.0, 0.0);
                let t = (size * 0.05).max(0.4);
                let (asc, desc) = if mark.is_empty() || mark == "\u{305}" || mark == "\u{304}" {
                    let y = if under { -bdesc - size * 0.1 } else { basc + size * 0.08 };
                    items.push(Item::Rule(0.0, y, bw, t));
                    (if under { basc } else { y + t }, if under { -y + t } else { bdesc })
                } else {
                    // Combining accents are drawn as their spacing forms, centred above.
                    let shown = match mark {
                        "\u{302}" => "^",
                        "\u{303}" => "~",
                        "\u{307}" => "\u{2d9}",
                        "\u{308}" => "\u{a8}",
                        "\u{20d7}" => "\u{2192}",
                        m => m,
                    };
                    let a = text_box(&self.notes.drawable(shown), style(size * 0.8, false, self.color));
                    let aw = a.w;
                    let y = basc - size * 0.35;
                    let top = y + a.asc;
                    items.extend(a.shifted((bw - aw) / 2.0, y));
                    (top, bdesc)
                };
                MathBox { w: bw, asc, desc, items }
            }
            "func" => {
                let name = self.arg(child(c, "fName"), size, depth);
                let arg = self.arg(child(c, "e"), size, depth);
                MathBox::row(vec![name, space(size * 0.17), arg])
            }
            "limLow" | "limUpp" => {
                let base = self.arg(child(c, "e"), size, depth);
                let lim = child(c, "lim").map(|l| self.row(l, small, depth + 1));
                if c.tag_name().name() == "limLow" { limits(base, None, lim, size) } else { limits(base, lim, None, size) }
            }
            // Containers and anything else: their content in a row.
            "e" | "box" | "borderBox" | "phant" | "oMath" | "num" | "den" | "sub" | "sup" | "fName" | "lim" | "deg" => self.row(c, size, depth + 1),
            name if name.ends_with("Pr") => MathBox::default(),
            _ => self.row(c, size, depth + 1),
        }
    }
}

/// `base` with a superscript and/or a subscript at its right.
fn scripts(base: MathBox, sup: Option<MathBox>, sub: Option<MathBox>, size: f64) -> MathBox {
    let (bw, basc, bdesc) = (base.w, base.asc, base.desc);
    let mut items = base.shifted(0.0, 0.0);
    let (mut w, mut asc, mut desc) = (bw, basc, bdesc);
    let gap = size * 0.05;
    let mut sw = 0.0f64;
    if let Some(s) = sup {
        let y = (basc - s.asc * 0.6).max(size * 0.35);
        asc = asc.max(y + s.asc);
        sw = sw.max(s.w);
        items.extend(s.shifted(bw + gap, y));
    }
    if let Some(s) = sub {
        let y = -(bdesc + s.asc * 0.4).max(size * 0.18);
        desc = desc.max(-y + s.desc);
        sw = sw.max(s.w);
        items.extend(s.shifted(bw + gap, y));
    }
    w += gap + sw;
    MathBox { w, asc, desc, items }
}

/// `base` with limits centred above and/or below it.
fn limits(base: MathBox, over: Option<MathBox>, under: Option<MathBox>, size: f64) -> MathBox {
    let w = [Some(base.w), over.as_ref().map(|b| b.w), under.as_ref().map(|b| b.w)].into_iter().flatten().fold(0.0, f64::max);
    let (bw, basc, bdesc) = (base.w, base.asc, base.desc);
    let mut items = base.shifted((w - bw) / 2.0, 0.0);
    let (mut asc, mut desc) = (basc, bdesc);
    let gap = size * 0.1;
    if let Some(o) = over {
        let y = basc + gap + o.desc;
        asc = y + o.asc;
        let ow = o.w;
        items.extend(o.shifted((w - ow) / 2.0, y));
    }
    if let Some(u) = under {
        let y = -bdesc - gap - u.asc;
        desc = -y + u.desc;
        let uw = u.w;
        items.extend(u.shifted((w - uw) / 2.0, y));
    }
    MathBox { w, asc, desc, items }
}

/// Lay out an `m:oMath` at `size` points in `color`.
pub(crate) fn layout(math: Node<'_, '_>, size: f64, color: [f64; 3], notes: &mut Notes) -> MathBox {
    let size = fin(size, 11.0).clamp(4.0, 200.0);
    let mut cx = Ctx { notes, color };
    let b = cx.row(math, size, 0);
    MathBox { w: b.w + size * 0.1, ..b }
}

/// Whether `n` is an Office Math element worth laying out (it has content).
pub(crate) fn has_content(n: Node<'_, '_>) -> bool {
    n.descendants().any(|d| is(d, "t") || is(d, "f") || is(d, "rad") || is(d, "nary") || is(d, "m"))
}

/// Draw `b` with its baseline's left end at `(x, y)`.
pub(crate) fn draw(page: &mut Page, b: &MathBox, x: f64, y: f64) {
    for it in &b.items {
        match it {
            Item::Text(ix, iy, s, st) => page.text(x + ix, y + iy, st, s),
            Item::Rule(ix, iy, w, h) => page.fill_rect(x + ix, y + iy, *w, *h, [0.0; 3]),
            Item::Path(pts, w) => {
                let mut ops = format!("q 0 0 0 RG {} w 1 j ", n(*w));
                for (i, (px, py)) in pts.iter().enumerate() {
                    ops.push_str(&format!("{} {} {} ", n(x + px), n(y + py), if i == 0 { "m" } else { "l" }));
                }
                ops.push_str("S Q\n");
                page.raw(&ops);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lay(xml: &str) -> MathBox {
        let doc = format!(r#"<m:oMath xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math">{xml}</m:oMath>"#);
        let d = roxmltree::Document::parse(&doc).unwrap();
        layout(d.root_element(), 11.0, [0.0; 3], &mut Notes::default())
    }

    #[test]
    fn fractions_stack_and_scripts_shrink() {
        let f = lay("<m:f><m:num><m:r><m:t>a</m:t></m:r></m:num><m:den><m:r><m:t>b</m:t></m:r></m:den></m:f>");
        assert!(f.asc > 8.0 && f.desc > 3.0, "{f:?}");
        assert!(f.items.iter().any(|i| matches!(i, Item::Rule(..))), "the fraction bar");
        let s = lay("<m:sSup><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup>");
        let sizes: Vec<f64> = s.items.iter().filter_map(|i| if let Item::Text(_, _, _, st) = i { Some(st.size) } else { None }).collect();
        assert!(sizes.iter().any(|z| *z < 11.0) && sizes.contains(&11.0), "{sizes:?}");
    }

    #[test]
    fn deep_and_empty_equations_are_bounded() {
        let deep = "<m:f><m:num>".repeat(200) + &"</m:num></m:f>".repeat(200);
        let _ = lay(&deep);
        let e = lay("");
        assert_eq!(e.items.len(), 0);
        let m = lay(
            "<m:m><m:mr><m:e><m:r><m:t>1</m:t></m:r></m:e><m:e><m:r><m:t>0</m:t></m:r></m:e></m:mr><m:mr><m:e><m:r><m:t>0</m:t></m:r></m:e><m:e><m:r><m:t>1</m:t></m:r></m:e></m:mr></m:m>",
        );
        assert!(m.asc + m.desc > 20.0, "two rows: {m:?}");
    }
}
