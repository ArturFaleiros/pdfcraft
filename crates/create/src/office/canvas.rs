//! Page content for imported Office documents: text in the standard-14 faces (WinAnsi, plus the
//! Latin letters of [`super::latin`] through a second encoding of each face) or in the embedded
//! metric-compatible faces of [`super::faces`] (subset, one Type0 font per face for the whole
//! document), filled and stroked paths, and image XObjects. Pages are kept until [`Canvas::finish`] so a converter can
//! go back to a page (headers and footers are drawn once the page count is known).

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};
use pdfcraft_fonts::{Std14, literal, win_ansi_byte};

use super::{LinkTarget, RunStyle, faces, latin};
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
    /// Glyphs drawn in embedded faces: (face slot, glyph id) and the character it shows.
    glyphs: BTreeMap<(usize, u16), char>,
    /// Clickable areas (left, bottom, right, top) and where each goes.
    links: Vec<([f64; 4], Arc<LinkTarget>)>,
}

/// A piece of a text run shown with one font.
enum Run {
    /// Standard-14 bytes, in WinAnsi or (`true`) the extra Latin encoding.
    Std(bool, Vec<u8>),
    /// Glyph ids of an embedded face slot.
    Embedded(usize, Vec<u16>),
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
        // The run's embedded face, or the standard face with its embedded stand-in for the
        // characters it can't draw.
        let embedded = faces::loaded(style.font, style.bold, style.italic);
        let fallback = if embedded.is_none() { faces::fallback(style.fam, style.bold, style.italic) } else { None };
        let mut runs: Vec<Run> = Vec::new();
        for c in s.chars() {
            let c = if c == '\t' { ' ' } else { c };
            let glyph = |l: &faces::Loaded| l.glyph(c).or_else(|| l.glyph(' ')).map_or(0, |g| g.0);
            // A face for scripts the run's face lacks (Japanese, Chinese, Korean, symbols).
            let script = || faces::script_fallback(c).and_then(|(slot, l)| l.glyph(c).map(|g| Run::Embedded(slot, vec![g.0])));
            let piece = match (embedded, win_ansi_byte(c), latin::code(c)) {
                (Some((slot, l)), _, _) if l.covers(c) => Run::Embedded(slot, vec![glyph(l)]),
                (Some((slot, l)), _, _) => script().unwrap_or_else(|| Run::Embedded(slot, vec![glyph(l)])),
                (None, Some(b), _) => Run::Std(false, vec![b]),
                (None, None, Some(b)) => Run::Std(true, vec![b]),
                (None, None, None) => match fallback {
                    Some((slot, l)) if l.covers(c) => Run::Embedded(slot, vec![glyph(l)]),
                    _ => script().unwrap_or_else(|| Run::Std(false, vec![b'?'])),
                },
            };
            if let Run::Embedded(slot, g) = &piece
                && let Some(gid) = g.first()
            {
                self.glyphs.entry((*slot, *gid)).or_insert(c);
            }
            match (runs.last_mut(), piece) {
                (Some(Run::Std(e, bytes)), Run::Std(pe, pb)) if *e == pe => bytes.extend(pb),
                (Some(Run::Embedded(s1, gids)), Run::Embedded(s2, pg)) if *s1 == s2 => gids.extend(pg),
                (_, piece) => runs.push(piece),
            }
        }
        for run in runs {
            match run {
                Run::Std(e, bytes) => self.show(face, e, size, &bytes),
                Run::Embedded(slot, gids) => {
                    let mut hex = String::with_capacity(gids.len() * 4 + 2);
                    hex.push('<');
                    for g in gids {
                        let _ = write!(hex, "{g:04X}");
                    }
                    hex.push('>');
                    self.push(&format!("/E{slot} {} Tf {hex} Tj ", n(size)));
                }
            }
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
        self.pages.push(Page {
            w: side(w),
            h: side(h),
            ops: Vec::new(),
            fonts: 0,
            latin_fonts: 0,
            images: Vec::new(),
            glyphs: BTreeMap::new(),
            links: Vec::new(),
        });
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

    /// How many embedded faces the pages use so far.
    pub(crate) fn embedded_faces(&self) -> usize {
        let mut slots: Vec<usize> = self.pages.iter().flat_map(|p| p.glyphs.keys().map(|(s, _)| *s)).collect();
        slots.sort_unstable();
        slots.dedup();
        slots.len()
    }

    /// Write every page into `doc`. A document always gets at least one page.
    pub(crate) fn finish(mut self, doc: &mut Document) -> Result<usize, CreateError> {
        if self.pages.is_empty() {
            self.pages.push(Page {
                w: 612.0,
                h: 792.0,
                ops: Vec::new(),
                fonts: 0,
                latin_fonts: 0,
                images: Vec::new(),
                glyphs: BTreeMap::new(),
                links: Vec::new(),
            });
        }
        // One subset font per embedded face, holding the glyphs of every page.
        let mut used: BTreeMap<usize, BTreeMap<u16, char>> = BTreeMap::new();
        for page in &self.pages {
            for ((slot, gid), c) in &page.glyphs {
                used.entry(*slot).or_default().entry(*gid).or_insert(*c);
            }
        }
        let mut embedded: BTreeMap<usize, ObjRef> = BTreeMap::new();
        for (slot, glyphs) in &used {
            if let Some(l) = faces::by_slot(*slot)
                && let Some(r) = embed_face(doc, l, glyphs)
            {
                embedded.insert(*slot, r);
            }
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
            let slots: Vec<usize> = {
                let mut s: Vec<usize> = page.glyphs.keys().map(|(s, _)| *s).collect();
                s.dedup();
                s
            };
            for slot in slots {
                if let Some(r) = embedded.get(&slot) {
                    font_dict.set(format!("E{slot}").into_bytes(), Object::Ref(*r));
                }
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

/// A six-letter subset tag from the glyphs a subset holds (ISO 32000-2 §9.6.4).
fn subset_tag(glyphs: &BTreeMap<u16, char>) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for g in glyphs.keys() {
        for b in g.to_be_bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    (0..6).map(|i| char::from(b'A' + ((h >> (i * 8)) % 26) as u8)).collect()
}

/// Embed `glyphs` of face `l` as a Type0 font with a subset TrueType program, Identity-H
/// glyph-id codes, its advances and a `/ToUnicode` map. `None` when the program can't be subset.
fn embed_face(doc: &mut Document, l: &faces::Loaded, glyphs: &BTreeMap<u16, char>) -> Option<ObjRef> {
    let keep = glyphs.keys().copied().collect();
    let program = super::ttf::subset(l.bytes, &keep)?;
    let name = format!("{}+{}", subset_tag(glyphs), l.name);
    let mut file = Dict::new();
    file.set(b"Length1".to_vec(), Object::Int(program.len() as i64));
    let file = doc.add(Object::Stream(Stream::flate(file, &program)));
    let mut flags = 32;
    if l.name.contains("Mono") {
        flags |= 1;
    }
    if l.name.starts_with("Caladea") || l.name.contains("Serif") {
        flags |= 2;
    }
    if l.italic_angle != 0.0 {
        flags |= 64;
    }
    let mut desc = Dict::new();
    desc.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    desc.set(b"FontName".to_vec(), Object::name(&name));
    desc.set(b"Flags".to_vec(), Object::Int(flags));
    desc.set(b"FontBBox".to_vec(), Object::Array(l.bbox.iter().map(|v| Object::Real(v.round())).collect()));
    desc.set(b"ItalicAngle".to_vec(), Object::Real(l.italic_angle));
    desc.set(b"Ascent".to_vec(), Object::Real(l.ascent.round()));
    desc.set(b"Descent".to_vec(), Object::Real(l.descent.round()));
    desc.set(b"CapHeight".to_vec(), Object::Real(l.cap_height.round()));
    desc.set(b"StemV".to_vec(), Object::Int(80));
    desc.set(b"FontFile2".to_vec(), Object::Ref(file));
    let desc = doc.add(desc);
    let widths: Vec<Object> = glyphs
        .keys()
        .flat_map(|g| {
            [
                Object::Int(i64::from(*g)),
                Object::Array(vec![Object::Real((l.advances.get(usize::from(*g)).copied().unwrap_or(0.0) * 1000.0).round() / 1000.0)]),
            ]
        })
        .collect();
    let mut system = Dict::new();
    system.set(b"Registry".to_vec(), Object::String(PdfString::literal(b"Adobe".to_vec())));
    system.set(b"Ordering".to_vec(), Object::String(PdfString::literal(b"Identity".to_vec())));
    system.set(b"Supplement".to_vec(), Object::Int(0));
    let mut cid = Dict::new();
    cid.set(b"Type".to_vec(), Object::name("Font"));
    cid.set(b"Subtype".to_vec(), Object::name("CIDFontType2"));
    cid.set(b"BaseFont".to_vec(), Object::name(&name));
    cid.set(b"CIDSystemInfo".to_vec(), Object::Dict(system));
    cid.set(b"FontDescriptor".to_vec(), Object::Ref(desc));
    cid.set(b"CIDToGIDMap".to_vec(), Object::name("Identity"));
    cid.set(b"W".to_vec(), Object::Array(widths));
    let cid = doc.add(cid);
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    let pairs: Vec<(&u16, &char)> = glyphs.iter().collect();
    for block in pairs.chunks(100) {
        let _ = writeln!(cmap, "{} beginbfchar", block.len());
        for (g, c) in block {
            // A shaped glyph shows the letters it was shaped from (maybe none, for a mark).
            let text = faces::shaped_text(**c).unwrap_or_else(|| c.to_string());
            let hex: String = text.encode_utf16().take(64).map(|u| format!("{u:04X}")).collect();
            let hex = if hex.is_empty() { "200B".to_owned() } else { hex };
            let _ = writeln!(cmap, "<{g:04X}> <{hex}>");
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let to_unicode = doc.add(Object::Stream(Stream::flate(Dict::new(), cmap.as_bytes())));
    let mut top = Dict::new();
    top.set(b"Type".to_vec(), Object::name("Font"));
    top.set(b"Subtype".to_vec(), Object::name("Type0"));
    top.set(b"BaseFont".to_vec(), Object::name(&name));
    top.set(b"Encoding".to_vec(), Object::name("Identity-H"));
    top.set(b"DescendantFonts".to_vec(), Object::Array(vec![Object::Ref(cid)]));
    top.set(b"ToUnicode".to_vec(), Object::Ref(to_unicode));
    Some(doc.add(top))
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
