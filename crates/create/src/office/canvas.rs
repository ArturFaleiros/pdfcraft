//! Page content for imported Office documents: text in the standard-14 faces (WinAnsi, plus the
//! Latin letters of [`super::latin`] through a second encoding of each face), filled and
//! stroked paths, image XObjects, link areas and bookmarks. Pages are kept until
//! [`Canvas::finish`] so a converter can go back to a page (headers and footers are drawn once
//! the page count is known).

use std::collections::HashMap;
use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};
use pdfcraft_fonts::{Std14, literal, win_ansi_byte};

use super::{LinkTarget, RunStyle, latin};
use crate::CreateError;

/// Most pages one conversion may produce.
pub(crate) const MAX_PAGES: usize = 5_000;

const FACES: [Std14; 12] = [
    Std14::Helvetica,
    Std14::HelveticaBold,
    Std14::HelveticaOblique,
    Std14::HelveticaBoldOblique,
    Std14::TimesRoman,
    Std14::TimesBold,
    Std14::TimesItalic,
    Std14::TimesBoldItalic,
    Std14::Courier,
    Std14::CourierBold,
    Std14::CourierOblique,
    Std14::CourierBoldOblique,
];

fn face_index(f: Std14) -> usize {
    FACES.iter().position(|x| *x == f).unwrap_or(0)
}

/// A number for a content stream: finite, short.
pub(crate) fn n(v: f64) -> String {
    let v = if v.is_finite() { v.clamp(-1e6, 1e6) } else { 0.0 };
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() { "0".into() } else { s.to_string() }
}

fn rgb(c: [f64; 3]) -> String {
    format!("{} {} {}", n(c[0].clamp(0.0, 1.0)), n(c[1].clamp(0.0, 1.0)), n(c[2].clamp(0.0, 1.0)))
}

pub(crate) struct Page {
    pub w: f64,
    pub h: f64,
    ops: Vec<u8>,
    /// The faces used, by [`FACES`] index: in WinAnsi (`/F{i}`) and in the extra Latin encoding
    /// (`/L{i}`).
    fonts: u16,
    latin_fonts: u16,
    images: Vec<ObjRef>,
    /// Clickable areas (left, bottom, right, top) and where each goes.
    links: Vec<([f64; 4], Arc<LinkTarget>)>,
}

impl Page {
    fn push(&mut self, s: &str) {
        self.ops.extend_from_slice(s.as_bytes());
    }

    /// Draw already-drawable text with its baseline at `(x, y)` (PDF coordinates). Highlight,
    /// underline and strike-through are drawn with it.
    pub(crate) fn text(&mut self, x: f64, y: f64, style: &RunStyle, s: &str) {
        if s.is_empty() || style.hidden {
            return;
        }
        let size = style.draw_size();
        let y = y + match style.vert {
            1 => style.size * 0.33,
            -1 => -style.size * 0.14,
            _ => 0.0,
        };
        let w = style.width(s);
        if let Some(target) = &style.link {
            self.link(x, y - size * 0.25, x + w, y + size * 0.95, target);
        }
        if let Some(h) = style.highlight {
            self.fill_rect(x, y - size * 0.22, w, size * 1.12, h);
        }
        let face = face_index(style.face());
        self.push(&format!("BT {} rg 1 0 0 1 {} {} Tm ", rgb(style.color), n(x), n(y)));
        // Runs of WinAnsi text and runs of extra Latin letters, each shown in its own font.
        let mut run: Option<(bool, Vec<u8>)> = None;
        for c in s.chars() {
            let (extra, byte) = match (win_ansi_byte(if c == '\t' { ' ' } else { c }), latin::code(c)) {
                (Some(b), _) => (false, b),
                (None, Some(b)) => (true, b),
                (None, None) => (false, b'?'),
            };
            match &mut run {
                Some((e, bytes)) if *e == extra => bytes.push(byte),
                _ => {
                    if let Some((e, bytes)) = run.take() {
                        self.show(face, e, size, &bytes);
                    }
                    run = Some((extra, vec![byte]));
                }
            }
        }
        if let Some((e, bytes)) = run {
            self.show(face, e, size, &bytes);
        }
        self.push("ET\n");
        let t = (size * 0.06).max(0.4);
        if style.underline {
            self.fill_rect(x, y - size * 0.12, w, t, style.color);
        }
        if style.strike {
            self.fill_rect(x, y + size * 0.28, w, t, style.color);
        }
    }

    /// Draw `s` beneath everything already on the page (a page colour).
    pub(crate) fn underlay(&mut self, s: &str) {
        let mut ops = s.as_bytes().to_vec();
        ops.extend_from_slice(&self.ops);
        self.ops = ops;
    }

    /// A clickable area going to `target`, joined to the last one when it continues it on the
    /// same line (the words and spaces of one link).
    pub(crate) fn link(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, target: &Arc<LinkTarget>) {
        if !(x1.is_finite() && y1.is_finite() && x2.is_finite() && y2.is_finite()) || self.links.len() >= MAX_LINKS {
            return;
        }
        if let Some((r, t)) = self.links.last_mut()
            && **t == **target
            && (r[1] - y1).abs() < 1.0
            && x1 >= r[0] - 0.5
            && x1 <= r[2] + (y2 - y1) * 0.5
        {
            r[2] = r[2].max(x2);
            r[3] = r[3].max(y2);
            return;
        }
        self.links.push(([x1, y1, x2, y2], target.clone()));
    }

    /// One `Tj` of `bytes` in face `face`, WinAnsi or (`extra`) the extra Latin encoding.
    fn show(&mut self, face: usize, extra: bool, size: f64, bytes: &[u8]) {
        let prefix = if extra {
            self.latin_fonts |= 1 << face;
            'L'
        } else {
            self.fonts |= 1 << face;
            'F'
        };
        self.push(&format!("/{prefix}{face} {} Tf ", n(size)));
        self.ops.extend_from_slice(&literal(bytes));
        self.push(" Tj ");
    }

    pub(crate) fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64, c: [f64; 3]) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        self.push(&format!("{} rg {} {} {} {} re f\n", rgb(c), n(x), n(y), n(w), n(h)));
    }

    pub(crate) fn stroke_rect(&mut self, x: f64, y: f64, w: f64, h: f64, width: f64, c: [f64; 3]) {
        self.push(&format!("{} RG {} w {} {} {} {} re S\n", rgb(c), n(width.max(0.1)), n(x), n(y), n(w), n(h)));
    }

    pub(crate) fn line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, width: f64, c: [f64; 3]) {
        self.push(&format!("{} RG {} w 0 J {} {} m {} {} l S\n", rgb(c), n(width.max(0.1)), n(x1), n(y1), n(x2), n(y2)));
    }

    /// A dashed line, `dash` being the PDF dash array (`[3 1.5]`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn dashed_line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, width: f64, c: [f64; 3], dash: &str) {
        self.push(&format!("q {} RG {} w 0 J {dash} 0 d {} {} m {} {} l S Q\n", rgb(c), n(width.max(0.1)), n(x1), n(y1), n(x2), n(y2)));
    }

    /// An image XObject filling `(x, y, w, h)`.
    pub(crate) fn image(&mut self, r: ObjRef, x: f64, y: f64, w: f64, h: f64) {
        let i = match self.images.iter().position(|e| *e == r) {
            Some(i) => i,
            None => {
                self.images.push(r);
                self.images.len() - 1
            }
        };
        self.push(&format!("q {} 0 0 {} {} {} cm /Im{i} Do Q\n", n(w), n(h), n(x), n(y)));
    }

    /// The outlined box drawn in place of content the importer can't draw.
    pub(crate) fn placeholder(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let (w, h) = (w.max(4.0), h.max(4.0));
        self.fill_rect(x, y, w, h, [0.95, 0.95, 0.95]);
        self.stroke_rect(x, y, w, h, 0.5, [0.6, 0.6, 0.6]);
        self.line(x, y, x + w, y + h, 0.5, [0.75, 0.75, 0.75]);
        self.line(x, y + h, x + w, y, 0.5, [0.75, 0.75, 0.75]);
    }

    /// Raw content operators (paths, clips, transforms) built by a converter.
    pub(crate) fn raw(&mut self, s: &str) {
        self.push(s);
    }

    pub(crate) fn save(&mut self) {
        self.push("q\n");
    }

    pub(crate) fn restore(&mut self) {
        self.push("Q\n");
    }

    /// Clip what follows (until [`Page::restore`]) to a rectangle.
    pub(crate) fn clip_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        self.push(&format!("{} {} {} {} re W n\n", n(x), n(y), n(w.max(0.0)), n(h.max(0.0))));
    }
}

#[derive(Default)]
pub(crate) struct Canvas {
    pub pages: Vec<Page>,
    /// Named places links can go to: page and the PDF y of their top.
    pub anchors: HashMap<String, (usize, f64)>,
    /// Bookmarks in order: level (0 at the top), title, page and y.
    pub outline: Vec<(u8, String, usize, f64)>,
}

/// Most clickable areas on one page, and bookmarks in one document.
const MAX_LINKS: usize = 10_000;
const MAX_OUTLINE: usize = 20_000;

/// A page side in points: at least an inch, at most PDF's 200 inches, Letter if not a number.
fn side(v: f64) -> f64 {
    if v.is_finite() { v.clamp(72.0, 14_400.0) } else { 612.0 }
}

impl Canvas {
    /// Start a page; `Err` past [`MAX_PAGES`].
    pub(crate) fn new_page(&mut self, w: f64, h: f64) -> Result<usize, String> {
        if self.pages.len() >= MAX_PAGES {
            return Err(format!("the document needs more than {MAX_PAGES} pages"));
        }
        self.pages.push(Page { w: side(w), h: side(h), ops: Vec::new(), fonts: 0, latin_fonts: 0, images: Vec::new(), links: Vec::new() });
        Ok(self.pages.len() - 1)
    }

    /// Give a page that has nothing on it yet another size.
    pub(crate) fn resize(&mut self, i: usize, w: f64, h: f64) {
        if let Some(p) = self.pages.get_mut(i) {
            p.w = side(w);
            p.h = side(h);
        }
    }

    pub(crate) fn page(&mut self, i: usize) -> Option<&mut Page> {
        self.pages.get_mut(i)
    }

    /// Write every page into `doc`. A document always gets at least one page.
    pub(crate) fn finish(mut self, doc: &mut Document) -> Result<usize, CreateError> {
        if self.pages.is_empty() {
            self.pages.push(Page { w: 612.0, h: 792.0, ops: Vec::new(), fonts: 0, latin_fonts: 0, images: Vec::new(), links: Vec::new() });
        }
        let mut fonts: [Option<ObjRef>; 12] = [None; 12];
        let mut latin_fonts: [Option<ObjRef>; 12] = [None; 12];
        // One encoding and one ToUnicode map serve every face.
        let mut latin_shared: Option<(ObjRef, ObjRef)> = None;
        let count = self.pages.len();
        let mut refs: Vec<ObjRef> = Vec::with_capacity(count);
        let mut links: Vec<Vec<([f64; 4], Arc<LinkTarget>)>> = Vec::with_capacity(count);
        for mut page in self.pages {
            links.push(std::mem::take(&mut page.links));
            let mut font_dict = Dict::new();
            for (i, face) in FACES.iter().enumerate() {
                if page.fonts & (1 << i) == 0 {
                    continue;
                }
                let r = match fonts.get(i).copied().flatten() {
                    Some(r) => r,
                    None => {
                        let mut f = Dict::new();
                        f.set(b"Type".to_vec(), Object::name("Font"));
                        f.set(b"Subtype".to_vec(), Object::name("Type1"));
                        f.set(b"BaseFont".to_vec(), Object::name(face.base_font()));
                        f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
                        let r = doc.add(f);
                        if let Some(slot) = fonts.get_mut(i) {
                            *slot = Some(r);
                        }
                        r
                    }
                };
                font_dict.set(format!("F{i}").into_bytes(), Object::Ref(r));
            }
            for (i, face) in FACES.iter().enumerate() {
                if page.latin_fonts & (1 << i) == 0 {
                    continue;
                }
                let r = match latin_fonts.get(i).copied().flatten() {
                    Some(r) => r,
                    None => {
                        let (encoding, to_unicode) = *latin_shared.get_or_insert_with(|| latin_resources(doc));
                        let r = doc.add(latin_font(*face, encoding, to_unicode));
                        if let Some(slot) = latin_fonts.get_mut(i) {
                            *slot = Some(r);
                        }
                        r
                    }
                };
                font_dict.set(format!("L{i}").into_bytes(), Object::Ref(r));
            }
            let mut res = Dict::new();
            if !font_dict.is_empty() {
                res.set(b"Font".to_vec(), Object::Dict(font_dict));
            }
            if !page.images.is_empty() {
                let mut xo = Dict::new();
                for (i, r) in page.images.iter().enumerate() {
                    xo.set(format!("Im{i}").into_bytes(), Object::Ref(*r));
                }
                res.set(b"XObject".to_vec(), Object::Dict(xo));
            }
            refs.push(crate::add_page(doc, page.w, page.h, res, Some(page.ops))?);
        }
        let dest = |page: usize, y: f64| -> Option<Object> {
            let r = refs.get(page)?;
            Some(Object::Array(vec![Object::Ref(*r), Object::name("XYZ"), Object::Null, Object::Real(y), Object::Null]))
        };
        for (pi, page_links) in links.iter().enumerate() {
            let mut annots = Vec::new();
            for (rect, target) in page_links {
                let mut a = Dict::new();
                a.set(b"Type".to_vec(), Object::name("Annot"));
                a.set(b"Subtype".to_vec(), Object::name("Link"));
                a.set(b"Rect".to_vec(), Object::Array(rect.iter().map(|v| Object::Real(*v)).collect()));
                a.set(b"Border".to_vec(), Object::Array(vec![0.into(), 0.into(), 0.into()]));
                match &**target {
                    LinkTarget::Uri(uri) if safe_uri(uri) => {
                        let mut action = Dict::new();
                        action.set(b"S".to_vec(), Object::name("URI"));
                        action.set(b"URI".to_vec(), Object::String(PdfString::literal(uri.trim().as_bytes().to_vec())));
                        a.set(b"A".to_vec(), Object::Dict(action));
                    }
                    LinkTarget::Anchor(name) => match self.anchors.get(name).and_then(|(p, y)| dest(*p, *y)) {
                        Some(d) => a.set(b"Dest".to_vec(), d),
                        None => continue,
                    },
                    LinkTarget::Uri(_) => continue,
                }
                annots.push(Object::Ref(doc.add(a)));
            }
            if let (false, Some(r)) = (annots.is_empty(), refs.get(pi)) {
                let _ = doc.update_dict(*r, |d| d.set(b"Annots".to_vec(), Object::Array(annots)));
            }
        }
        write_outline(doc, &self.outline, &dest);
        Ok(count)
    }
}

/// Only web, mail and FTP addresses become link actions: not scripts or local programs.
fn safe_uri(uri: &str) -> bool {
    let lower = uri.trim().to_ascii_lowercase();
    ["http://", "https://", "mailto:", "ftp://"].iter().any(|p| lower.starts_with(p)) && uri.len() <= 4096 && !uri.chars().any(char::is_control)
}

/// The document outline (bookmarks) from `items` (level, title, page, y), nested by level.
fn write_outline(doc: &mut Document, items: &[(u8, String, usize, f64)], dest: &dyn Fn(usize, f64) -> Option<Object>) {
    let items: Vec<&(u8, String, usize, f64)> =
        items.iter().filter(|(_, t, p, y)| !t.trim().is_empty() && dest(*p, *y).is_some()).take(MAX_OUTLINE).collect();
    if items.is_empty() {
        return;
    }
    let Some(catalog) = doc.root() else { return };
    let root = doc.add(Dict::new());
    let refs: Vec<ObjRef> = items.iter().map(|_| doc.add(Dict::new())).collect();
    // Each item's parent: the nearest earlier item of a lower level, else the root.
    let mut parent: Vec<Option<usize>> = Vec::with_capacity(items.len());
    let mut stack: Vec<usize> = Vec::new();
    for (i, (level, ..)) in items.iter().enumerate() {
        while stack.last().is_some_and(|&j| items[j].0 >= *level) {
            stack.pop();
        }
        parent.push(stack.last().copied());
        stack.push(i);
    }
    let kids = |p: Option<usize>| -> Vec<usize> { (0..items.len()).filter(|i| parent[*i] == p).collect() };
    let link = |doc: &mut Document, list: &[usize], up: ObjRef| {
        for (k, &i) in list.iter().enumerate() {
            let (_, title, page, y) = items[i];
            let mut d = Dict::new();
            let title: String = title.trim().chars().take(512).collect();
            d.set(b"Title".to_vec(), PdfString::text(&title));
            d.set(b"Parent".to_vec(), Object::Ref(up));
            if let Some(dst) = dest(*page, *y) {
                d.set(b"Dest".to_vec(), dst);
            }
            if k > 0 {
                d.set(b"Prev".to_vec(), Object::Ref(refs[list[k - 1]]));
            }
            if let Some(&next) = list.get(k + 1) {
                d.set(b"Next".to_vec(), Object::Ref(refs[next]));
            }
            let _ = doc.update_dict(refs[i], |slot| *slot = d);
        }
    };
    let top = kids(None);
    link(doc, &top, root);
    for i in 0..items.len() {
        let mine = kids(Some(i));
        if mine.is_empty() {
            continue;
        }
        link(doc, &mine, refs[i]);
        let (first, last) = (refs[mine[0]], refs[mine[mine.len() - 1]]);
        // Open, showing its whole subtree.
        let under = (i + 1..items.len()).take_while(|j| items[*j].0 > items[i].0).count();
        let _ = doc.update_dict(refs[i], |d| {
            d.set(b"First".to_vec(), Object::Ref(first));
            d.set(b"Last".to_vec(), Object::Ref(last));
            d.set(b"Count".to_vec(), Object::Int(under as i64));
        });
    }
    let (first, last) = (refs[top[0]], refs[top[top.len() - 1]]);
    let _ = doc.update_dict(root, |d| {
        d.set(b"Type".to_vec(), Object::name("Outlines"));
        d.set(b"First".to_vec(), Object::Ref(first));
        d.set(b"Last".to_vec(), Object::Ref(last));
        d.set(b"Count".to_vec(), Object::Int(items.len() as i64));
    });
    let _ = doc.update_dict(catalog, |d| d.set(b"Outlines".to_vec(), Object::Ref(root)));
}

/// The extra Latin encoding (`/Differences` naming [`latin::LETTERS`] from
/// [`latin::FIRST_CODE`]) and the `/ToUnicode` map that reads its codes back as text.
fn latin_resources(doc: &mut Document) -> (ObjRef, ObjRef) {
    let mut differences = vec![Object::Int(i64::from(latin::FIRST_CODE))];
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<00> <FF>\nendcodespacerange\n",
    );
    // A CMap block holds at most 100 entries.
    for block in latin::LETTERS.chunks(100) {
        cmap.push_str(&format!("{} beginbfchar\n", block.len()));
        for (c, name, _) in block {
            differences.push(Object::name(name));
            if let Some(code) = latin::code(*c) {
                cmap.push_str(&format!("<{code:02X}> <{:04X}>\n", u32::from(*c)));
            }
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let mut encoding = Dict::new();
    encoding.set(b"Type".to_vec(), Object::name("Encoding"));
    encoding.set(b"Differences".to_vec(), Object::Array(differences));
    let encoding = doc.add(encoding);
    let to_unicode = doc.add(Object::Stream(Stream::flate(Dict::new(), cmap.as_bytes())));
    (encoding, to_unicode)
}

/// `face` re-encoded for the extra Latin letters, with widths matching how the importer measured
/// them ([`latin::measured_as`]).
fn latin_font(face: Std14, encoding: ObjRef, to_unicode: ObjRef) -> Dict {
    let widths: Vec<Object> =
        latin::LETTERS.iter().map(|(_, _, base)| Object::Int(win_ansi_byte(*base).map_or(0, |b| face.width(b) as i64))).collect();
    let last = i64::from(latin::FIRST_CODE) + widths.len() as i64 - 1;
    let mut f = Dict::new();
    f.set(b"Type".to_vec(), Object::name("Font"));
    f.set(b"Subtype".to_vec(), Object::name("Type1"));
    f.set(b"BaseFont".to_vec(), Object::name(face.base_font()));
    f.set(b"Encoding".to_vec(), Object::Ref(encoding));
    f.set(b"FirstChar".to_vec(), Object::Int(i64::from(latin::FIRST_CODE)));
    f.set(b"LastChar".to_vec(), Object::Int(last));
    f.set(b"Widths".to_vec(), Object::Array(widths));
    f.set(b"ToUnicode".to_vec(), Object::Ref(to_unicode));
    f
}

#[cfg(test)]
mod tests {
    use super::n;

    #[test]
    fn numbers_are_finite_and_short() {
        assert_eq!(n(1.0), "1");
        assert_eq!(n(-0.0001), "0");
        assert_eq!(n(2.5), "2.5");
        assert_eq!(n(f64::NAN), "0");
        assert_eq!(n(f64::INFINITY), "0");
        assert_eq!(n(5e9), "1000000");
    }
}
