//! Page content for imported Office documents: text in the standard-14 faces, filled and
//! stroked paths, and image XObjects. Pages are kept until [`Canvas::finish`] so a converter can
//! go back to a page (headers and footers are drawn once the page count is known).

use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use pdfcraft_fonts::{Std14, literal, win_ansi};

use super::RunStyle;
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
    fonts: u16,
    images: Vec<ObjRef>,
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
        if let Some(h) = style.highlight {
            self.fill_rect(x, y - size * 0.22, w, size * 1.12, h);
        }
        let face = face_index(style.face());
        self.fonts |= 1 << face;
        let bytes = literal(&win_ansi(s));
        self.push(&format!("BT /F{face} {} Tf {} rg 1 0 0 1 {} {} Tm ", n(size), rgb(style.color), n(x), n(y)));
        self.ops.extend_from_slice(&bytes);
        self.push(" Tj ET\n");
        let t = (size * 0.06).max(0.4);
        if style.underline {
            self.fill_rect(x, y - size * 0.12, w, t, style.color);
        }
        if style.strike {
            self.fill_rect(x, y + size * 0.28, w, t, style.color);
        }
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
}

impl Canvas {
    /// Start a page; `Err` past [`MAX_PAGES`].
    pub(crate) fn new_page(&mut self, w: f64, h: f64) -> Result<usize, String> {
        if self.pages.len() >= MAX_PAGES {
            return Err(format!("the document needs more than {MAX_PAGES} pages"));
        }
        let clamp = |v: f64| if v.is_finite() { v.clamp(72.0, 14_400.0) } else { 612.0 };
        self.pages.push(Page { w: clamp(w), h: clamp(h), ops: Vec::new(), fonts: 0, images: Vec::new() });
        Ok(self.pages.len() - 1)
    }

    pub(crate) fn page(&mut self, i: usize) -> Option<&mut Page> {
        self.pages.get_mut(i)
    }

    /// Write every page into `doc`. A document always gets at least one page.
    pub(crate) fn finish(mut self, doc: &mut Document) -> Result<usize, CreateError> {
        if self.pages.is_empty() {
            self.pages.push(Page { w: 612.0, h: 792.0, ops: Vec::new(), fonts: 0, images: Vec::new() });
        }
        let mut fonts: [Option<ObjRef>; 12] = [None; 12];
        let count = self.pages.len();
        for page in self.pages {
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
            crate::add_page(doc, page.w, page.h, res, Some(page.ops))?;
        }
        Ok(count)
    }
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
