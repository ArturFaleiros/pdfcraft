//! Enhanced Metafile (EMF, [MS-EMF]) pictures as PDF vector drawings. Word and Excel keep logos,
//! diagrams pasted from drawing tools and the previews of embedded objects as EMF, which
//! [`crate::image_xobject`] can't decode. The common GDI drawing records are played into a form
//! XObject the size of the picture's frame: pens, brushes and fonts, rectangles, rounded
//! rectangles, ellipses, polygons, polylines and Béziers (16- and 32-bit), paths, text
//! (`EXTTEXTOUTW`, set in the standard-14 faces like the rest of the import), mapping modes,
//! window and viewport origins and extents, and world transforms. Bitmaps, clipping regions,
//! arcs and other records are skipped. The file is untrusted: every read is bounds-checked and
//! the records, handles, points and output are capped.

use std::collections::HashMap;
use std::fmt::Write as _;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};
use pdfcraft_fonts::{Std14, win_ansi_byte};

use super::canvas::n;
use super::{Fam, Notes, RunStyle, family_of, latin};

/// Most records played from one picture.
const MAX_RECORDS: usize = 200_000;
/// Most content bytes one picture may produce.
const MAX_OUTPUT: usize = 8 << 20;
/// Most points read from one record.
const MAX_POINTS: usize = 100_000;
/// Most object handles kept, and the deepest saved-state stack.
const MAX_HANDLES: u32 = 65_536;
const MAX_SAVED: usize = 64;

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn i32_at(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn i16_at(b: &[u8], at: usize) -> Option<i16> {
    Some(i16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn f32_at(b: &[u8], at: usize) -> Option<f64> {
    let v = f64::from(f32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?));
    v.is_finite().then_some(v)
}

/// Whether `bytes` start with an EMF header record.
pub(crate) fn is_emf(bytes: &[u8]) -> bool {
    u32_at(bytes, 0) == Some(1) && bytes.get(40..44) == Some(b" EMF")
}

fn colorref(b: &[u8], at: usize) -> Option<[f64; 3]> {
    let [r, g, bl]: [u8; 3] = b.get(at..at.checked_add(3)?)?.try_into().ok()?;
    Some([f64::from(r) / 255.0, f64::from(g) / 255.0, f64::from(bl) / 255.0])
}

#[derive(Clone, Copy, Debug)]
struct Pen {
    /// Logical units; 0 is one device pixel.
    width: f64,
    color: [f64; 3],
}

#[derive(Clone, Debug)]
struct Font {
    /// Logical units; negative is the em height, positive the cell height.
    height: f64,
    bold: bool,
    italic: bool,
    fam: Fam,
}

#[derive(Clone, Debug)]
enum Obj {
    Pen(Option<Pen>),
    Brush(Option<[f64; 3]>),
    Font(Font),
}

/// What a saved device context holds.
#[derive(Clone, Debug)]
struct Dc {
    pen: Option<Pen>,
    brush: Option<[f64; 3]>,
    font: Font,
    text_color: [f64; 3],
    text_align: u32,
    winding: bool,
    map_mode: u32,
    win_org: (f64, f64),
    win_ext: (f64, f64),
    vp_org: (f64, f64),
    vp_ext: (f64, f64),
    /// m11, m12, m21, m22, dx, dy.
    world: [f64; 6],
}

impl Default for Dc {
    fn default() -> Self {
        Dc {
            pen: Some(Pen { width: 0.0, color: [0.0; 3] }),
            brush: Some([1.0; 3]),
            font: Font { height: -12.0, bold: false, italic: false, fam: Fam::Sans },
            text_color: [0.0; 3],
            text_align: 0,
            winding: false,
            map_mode: 1,
            win_org: (0.0, 0.0),
            win_ext: (1.0, 1.0),
            vp_org: (0.0, 0.0),
            vp_ext: (1.0, 1.0),
            world: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        }
    }
}

/// The GDI stock object `i` (`0x80000000 | i` in `SELECTOBJECT`).
fn stock(i: u32) -> Option<Obj> {
    Some(match i {
        0 => Obj::Brush(Some([1.0; 3])),
        1 => Obj::Brush(Some([0.75; 3])),
        2 => Obj::Brush(Some([0.5; 3])),
        3 => Obj::Brush(Some([0.25; 3])),
        4 => Obj::Brush(Some([0.0; 3])),
        5 => Obj::Brush(None),
        6 => Obj::Pen(Some(Pen { width: 0.0, color: [1.0; 3] })),
        7 => Obj::Pen(Some(Pen { width: 0.0, color: [0.0; 3] })),
        8 => Obj::Pen(None),
        10..=17 => Obj::Font(Font { height: -12.0, bold: false, italic: false, fam: if matches!(i, 10 | 11) { Fam::Mono } else { Fam::Sans } }),
        _ => return None,
    })
}

struct Player<'n> {
    notes: &'n mut Notes,
    dc: Dc,
    saved: Vec<Dc>,
    objects: HashMap<u32, Obj>,
    /// Device pixels per millimetre (from the header's reference device).
    ppmm: (f64, f64),
    /// The frame in device pixels, and the form's size in points.
    frame: (f64, f64, f64, f64),
    size: (f64, f64),
    out: String,
    /// The path being collected between `BEGINPATH` and `ENDPATH`.
    path: Option<String>,
    cur: (f64, f64),
    /// Faces used by text, by [`face_index`].
    fonts: u16,
}

pub(crate) const FACES: [Std14; 12] = [
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

pub(crate) fn face_index(f: Std14) -> usize {
    FACES.iter().position(|x| *x == f).unwrap_or(0)
}

impl Player<'_> {
    /// A logical point in the form's space (points, y up).
    fn pt(&self, x: f64, y: f64) -> (f64, f64) {
        let d = &self.dc;
        let [m11, m12, m21, m22, dx, dy] = d.world;
        let (wx, wy) = (m11 * x + m21 * y + dx, m12 * x + m22 * y + dy);
        let (kx, ky) = self.scale();
        let devx = (wx - d.win_org.0) * kx + d.vp_org.0;
        let devy = (wy - d.win_org.1) * ky + d.vp_org.1;
        let (fx0, fy0, fx1, fy1) = self.frame;
        let u = (devx - fx0) / (fx1 - fx0) * self.size.0;
        let v = (1.0 - (devy - fy0) / (fy1 - fy0)) * self.size.1;
        (if u.is_finite() { u } else { 0.0 }, if v.is_finite() { v } else { 0.0 })
    }

    /// Device pixels per logical unit for the mapping mode (y negative where y grows up).
    fn scale(&self) -> (f64, f64) {
        let d = &self.dc;
        let ratio = |v: f64, w: f64| if w.abs() > 1e-9 && (v / w).is_finite() { v / w } else { 1.0 };
        let (px, py) = self.ppmm;
        match d.map_mode {
            2 => (0.1 * px, -0.1 * py),
            3 => (0.01 * px, -0.01 * py),
            4 => (0.254 * px, -0.254 * py),
            5 => (0.0254 * px, -0.0254 * py),
            6 => (25.4 / 1440.0 * px, -25.4 / 1440.0 * py),
            7 => {
                // Isotropic: the same scale on both axes, the smaller one.
                let (sx, sy) = (ratio(d.vp_ext.0, d.win_ext.0), ratio(d.vp_ext.1, d.win_ext.1));
                let k = sx.abs().min(sy.abs());
                (k * sx.signum(), k * sy.signum())
            }
            8 => (ratio(d.vp_ext.0, d.win_ext.0), ratio(d.vp_ext.1, d.win_ext.1)),
            _ => (1.0, 1.0),
        }
    }

    /// Points per logical unit along x, for pen widths and font sizes.
    fn unit(&self) -> f64 {
        let (a, b) = (self.pt(0.0, 0.0), self.pt(1.0, 0.0));
        let (c, d) = (self.pt(0.0, 0.0), self.pt(0.0, 1.0));
        let k = ((b.0 - a.0).hypot(b.1 - a.1) + (d.0 - c.0).hypot(d.1 - c.1)) / 2.0;
        if k.is_finite() { k } else { 0.0 }
    }

    fn emit(&mut self, s: &str) {
        if self.out.len().saturating_add(s.len()) <= MAX_OUTPUT {
            self.out.push_str(s);
        }
    }

    /// Path construction operators go to the open path, or are painted at once.
    fn shape(&mut self, ops: &str, fill: bool, stroke: bool) {
        if let Some(p) = &mut self.path {
            if p.len().saturating_add(ops.len()) <= MAX_OUTPUT {
                p.push_str(ops);
            }
            return;
        }
        self.paint(ops.to_owned(), fill, stroke);
    }

    fn paint(&mut self, ops: String, fill: bool, stroke: bool) {
        let brush = if fill { self.dc.brush } else { None };
        let pen = if stroke { self.dc.pen } else { None };
        if brush.is_none() && pen.is_none() {
            return;
        }
        let mut s = String::from("q ");
        if let Some(c) = brush {
            let _ = write!(s, "{} {} {} rg ", n(c[0]), n(c[1]), n(c[2]));
        }
        if let Some(p) = pen {
            // A zero-width pen is one device pixel wide.
            let w = if p.width <= 0.0 { 0.75 } else { (p.width * self.unit()).clamp(0.1, 200.0) };
            let _ = write!(s, "{} {} {} RG {} w 1 j ", n(p.color[0]), n(p.color[1]), n(p.color[2]), n(w));
        }
        s.push_str(&ops);
        let op = match (brush.is_some(), pen.is_some(), self.dc.winding) {
            (true, true, true) => "B",
            (true, true, false) => "B*",
            (true, false, true) => "f",
            (true, false, false) => "f*",
            _ => "S",
        };
        let _ = writeln!(s, "{op} Q");
        self.emit(&s);
    }

    fn m(&self, x: f64, y: f64) -> String {
        let (u, v) = self.pt(x, y);
        format!("{} {} m ", n(u), n(v))
    }

    fn l(&self, x: f64, y: f64) -> String {
        let (u, v) = self.pt(x, y);
        format!("{} {} l ", n(u), n(v))
    }

    fn c(&self, p: [(f64, f64); 3]) -> String {
        let [a, b, c] = p.map(|(x, y)| self.pt(x, y));
        format!("{} {} {} {} {} {} c ", n(a.0), n(a.1), n(b.0), n(b.1), n(c.0), n(c.1))
    }

    /// A rectangle with corners rounded by an ellipse `rx × ry` (0 for square corners).
    fn rounded(&self, (l, t, r, b): (f64, f64, f64, f64), rx: f64, ry: f64) -> String {
        let (l, r) = (l.min(r), l.max(r));
        let (t, b) = (t.min(b), t.max(b));
        let rx = rx.abs().min((r - l) / 2.0);
        let ry = ry.abs().min((b - t) / 2.0);
        if rx <= 0.0 || ry <= 0.0 {
            return format!("{}{}{}{}h ", self.m(l, t), self.l(r, t), self.l(r, b), self.l(l, b));
        }
        // A quarter ellipse as one Bézier.
        let k = 0.552_284_75;
        let (kx, ky) = (rx * k, ry * k);
        let mut s = self.m(l + rx, t);
        s += &self.l(r - rx, t);
        s += &self.c([(r - rx + kx, t), (r, t + ry - ky), (r, t + ry)]);
        s += &self.l(r, b - ry);
        s += &self.c([(r, b - ry + ky), (r - rx + kx, b), (r - rx, b)]);
        s += &self.l(l + rx, b);
        s += &self.c([(l + rx - kx, b), (l, b - ry + ky), (l, b - ry)]);
        s += &self.l(l, t + ry);
        s += &self.c([(l, t + ry - ky), (l + rx - kx, t), (l + rx, t)]);
        s + "h "
    }

    fn rect_at(b: &[u8], at: usize) -> Option<(f64, f64, f64, f64)> {
        Some((f64::from(i32_at(b, at)?), f64::from(i32_at(b, at + 4)?), f64::from(i32_at(b, at + 8)?), f64::from(i32_at(b, at + 12)?)))
    }

    /// The points of a `POLY…16` (`small`) or 32-bit record: a bounds rectangle, a count, then
    /// the points.
    fn points(b: &[u8], small: bool) -> Vec<(f64, f64)> {
        let count = u32_at(b, 24).map_or(0, |c| c as usize).min(MAX_POINTS);
        let step = if small { 4 } else { 8 };
        (0..count)
            .map_while(|i| {
                let at = 28usize.checked_add(i.checked_mul(step)?)?;
                if small {
                    Some((f64::from(i16_at(b, at)?), f64::from(i16_at(b, at + 2)?)))
                } else {
                    Some((f64::from(i32_at(b, at)?), f64::from(i32_at(b, at + 4)?)))
                }
            })
            .collect()
    }

    fn select(&mut self, ih: u32) {
        let obj = if ih & 0x8000_0000 != 0 { stock(ih & 0x7FFF_FFFF) } else { self.objects.get(&ih).cloned() };
        match obj {
            Some(Obj::Pen(p)) => self.dc.pen = p,
            Some(Obj::Brush(c)) => self.dc.brush = c,
            Some(Obj::Font(f)) => self.dc.font = f,
            None => {}
        }
    }

    fn keep(&mut self, ih: u32, o: Obj) {
        if ih < MAX_HANDLES {
            self.objects.insert(ih, o);
        }
    }

    fn text(&mut self, b: &[u8]) -> Option<()> {
        // EMR_EXTTEXTOUTW: bounds (16), graphics mode, x and y scale, then the EmrText.
        let (rx, ry) = (f64::from(i32_at(b, 36)?), f64::from(i32_at(b, 40)?));
        let chars = (u32_at(b, 44)? as usize).min(8_192);
        let off = u32_at(b, 48)? as usize;
        let units: Vec<u16> = (0..chars).map_while(|i| Some(i16_at(b, off.checked_add(i * 2)?)? as u16)).collect();
        let raw: String = char::decode_utf16(units).map(|c| c.unwrap_or('\u{FFFD}')).collect();
        let s = self.notes.drawable(&raw);
        if s.trim().is_empty() {
            return Some(());
        }
        let f = self.dc.font.clone();
        let style = RunStyle { fam: f.fam, bold: f.bold, italic: f.italic, ..RunStyle::default() };
        let face = style.face();
        // A positive height is the cell height, about 1.2 em in the faces drawn.
        let em = if f.height < 0.0 { -f.height } else { f.height / 1.2 };
        let size = (em * self.unit()).clamp(0.5, 1_000.0);
        let width = RunStyle { size, ..style.clone() }.width(&s);
        let (mut x, mut y) = self.pt(rx, ry);
        let align = self.dc.text_align;
        match align & 6 {
            6 => x -= width / 2.0,
            2 => x -= width,
            _ => {}
        }
        // Top (0), bottom (8) or baseline (24) of the text at the reference point.
        match align & 24 {
            24 => {}
            8 => y += size * 0.212,
            _ => y -= size * 0.905,
        }
        let bytes: Vec<u8> = s.chars().map(|c| win_ansi_byte(c).or_else(|| win_ansi_byte(latin::measured_as(c))).unwrap_or(b' ')).collect();
        let fi = face_index(face);
        self.fonts |= 1 << fi;
        let c = self.dc.text_color;
        let mut op = format!("BT {} {} {} rg /F{fi} {} Tf 1 0 0 1 {} {} Tm (", n(c[0]), n(c[1]), n(c[2]), n(size), n(x), n(y));
        // The content is kept as text: bytes past ASCII are written as octal escapes.
        for b in bytes {
            match b {
                b'(' | b')' | b'\\' => {
                    op.push('\\');
                    op.push(char::from(b));
                }
                0x20..=0x7E => op.push(char::from(b)),
                _ => {
                    let _ = write!(op, "\\{b:03o}");
                }
            }
        }
        op.push_str(") Tj ET\n");
        self.emit(&op);
        Some(())
    }

    /// Play one record. `None` when it is too short to read; it is skipped.
    fn record(&mut self, ty: u32, b: &[u8]) -> Option<()> {
        let pair = |at: usize| -> Option<(f64, f64)> { Some((f64::from(i32_at(b, at)?), f64::from(i32_at(b, at + 4)?))) };
        match ty {
            // Poly records: 2 Bézier, 3 polygon, 4 polyline, 5 Bézier-to, 6 polyline-to, and
            // their 16-bit forms 85–89.
            2..=6 | 85..=89 => {
                let small = ty >= 85;
                let kind = if small { ty - 83 } else { ty };
                let pts = Self::points(b, small);
                let (first, rest) = pts.split_first()?;
                let mut ops = String::new();
                match kind {
                    5 | 6 => {
                        // From the current position.
                        ops += &self.m(self.cur.0, self.cur.1);
                        if kind == 6 {
                            for p in &pts {
                                ops += &self.l(p.0, p.1);
                            }
                        } else {
                            for c in pts.chunks_exact(3) {
                                ops += &self.c([c[0], c[1], c[2]]);
                            }
                        }
                        self.cur = *pts.last()?;
                        if let Some(p) = &mut self.path {
                            // Inside a path, the figure continues: drop the move.
                            let ops = ops.split_once(" m ").map_or(ops.as_str(), |(_, r)| r);
                            if p.len().saturating_add(ops.len()) <= MAX_OUTPUT {
                                p.push_str(ops);
                            }
                            return Some(());
                        }
                        self.paint(ops, false, true);
                        return Some(());
                    }
                    2 => {
                        ops += &self.m(first.0, first.1);
                        for c in rest.chunks_exact(3) {
                            ops += &self.c([c[0], c[1], c[2]]);
                        }
                    }
                    _ => {
                        ops += &self.m(first.0, first.1);
                        for p in rest {
                            ops += &self.l(p.0, p.1);
                        }
                        if kind == 3 {
                            ops += "h ";
                        }
                    }
                }
                self.shape(&ops, kind == 3, true);
            }
            // POLYPOLYGON16 (91), POLYPOLYLINE16 (90), POLYPOLYGON (8), POLYPOLYLINE (7).
            7 | 8 | 90 | 91 => {
                let small = ty >= 90;
                let polys = (u32_at(b, 24)? as usize).min(MAX_POINTS);
                let total = (u32_at(b, 28)? as usize).min(MAX_POINTS);
                let mut at = 32usize.checked_add(polys.checked_mul(4)?)?;
                let step = if small { 4 } else { 8 };
                let mut ops = String::new();
                let mut used = 0usize;
                for i in 0..polys {
                    let count = (u32_at(b, 32 + i * 4)? as usize).min(total.saturating_sub(used));
                    used += count;
                    for k in 0..count {
                        let (x, y) = if small {
                            (f64::from(i16_at(b, at)?), f64::from(i16_at(b, at + 2)?))
                        } else {
                            (f64::from(i32_at(b, at)?), f64::from(i32_at(b, at + 4)?))
                        };
                        at = at.checked_add(step)?;
                        ops += &if k == 0 { self.m(x, y) } else { self.l(x, y) };
                    }
                    if ty == 8 || ty == 91 {
                        ops += "h ";
                    }
                }
                self.shape(&ops, ty == 8 || ty == 91, true);
            }
            // SETWINDOWEXTEX, SETWINDOWORGEX, SETVIEWPORTEXTEX, SETVIEWPORTORGEX.
            9 => self.dc.win_ext = pair(8)?,
            10 => self.dc.win_org = pair(8)?,
            11 => self.dc.vp_ext = pair(8)?,
            12 => self.dc.vp_org = pair(8)?,
            17 => self.dc.map_mode = u32_at(b, 8)?,
            19 => self.dc.winding = u32_at(b, 8)? == 2,
            22 => self.dc.text_align = u32_at(b, 8)?,
            24 => self.dc.text_color = colorref(b, 8)?,
            27 => self.cur = pair(8)?,
            33 => {
                if self.saved.len() < MAX_SAVED {
                    self.saved.push(self.dc.clone());
                }
            }
            34 => {
                // A negative count is relative: -1 restores the latest saved state.
                let i = i32_at(b, 8)?;
                let keep = if i < 0 { self.saved.len().saturating_sub(i.unsigned_abs() as usize) } else { (i as usize).saturating_sub(1) };
                if keep < self.saved.len() {
                    self.saved.truncate(keep + 1);
                    if let Some(dc) = self.saved.pop() {
                        self.dc = dc;
                    }
                }
            }
            // SETWORLDTRANSFORM and MODIFYWORLDTRANSFORM.
            35 | 36 => {
                let m = [f32_at(b, 8)?, f32_at(b, 12)?, f32_at(b, 16)?, f32_at(b, 20)?, f32_at(b, 24)?, f32_at(b, 28)?];
                if ty == 35 {
                    self.dc.world = m;
                } else {
                    let w = self.dc.world;
                    let mul = |a: [f64; 6], b: [f64; 6]| {
                        [
                            a[0] * b[0] + a[1] * b[2],
                            a[0] * b[1] + a[1] * b[3],
                            a[2] * b[0] + a[3] * b[2],
                            a[2] * b[1] + a[3] * b[3],
                            a[4] * b[0] + a[5] * b[2] + b[4],
                            a[4] * b[1] + a[5] * b[3] + b[5],
                        ]
                    };
                    match u32_at(b, 32)? {
                        1 => self.dc.world = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                        2 => self.dc.world = mul(m, w),
                        3 => self.dc.world = mul(w, m),
                        4 => self.dc.world = m,
                        _ => {}
                    }
                }
            }
            37 => self.select(u32_at(b, 8)?),
            38 => {
                // CREATEPEN: handle, style, width (a point), colour.
                let ih = u32_at(b, 8)?;
                let style = u32_at(b, 12)? & 0xF;
                let pen =
                    (style != 5).then(|| Pen { width: f64::from(i32_at(b, 16).unwrap_or(0)).abs(), color: colorref(b, 24).unwrap_or([0.0; 3]) });
                self.keep(ih, Obj::Pen(pen));
            }
            39 => {
                // CREATEBRUSHINDIRECT: handle, style (1 is hollow), colour.
                let ih = u32_at(b, 8)?;
                let brush = (u32_at(b, 12)? != 1).then(|| colorref(b, 16).unwrap_or([1.0; 3]));
                self.keep(ih, Obj::Brush(brush));
            }
            40 => {
                let ih = u32_at(b, 8)?;
                self.objects.remove(&ih);
            }
            // ELLIPSE, RECTANGLE, ROUNDRECT.
            42 => {
                let r = Self::rect_at(b, 8)?;
                let ops = self.rounded(r, (r.2 - r.0) / 2.0, (r.3 - r.1) / 2.0);
                self.shape(&ops, true, true);
            }
            43 => {
                let ops = self.rounded(Self::rect_at(b, 8)?, 0.0, 0.0);
                self.shape(&ops, true, true);
            }
            44 => {
                let (w, h) = pair(24)?;
                let ops = self.rounded(Self::rect_at(b, 8)?, w / 2.0, h / 2.0);
                self.shape(&ops, true, true);
            }
            54 => {
                let (x, y) = pair(8)?;
                let ops = format!("{}{}", self.m(self.cur.0, self.cur.1), self.l(x, y));
                self.cur = (x, y);
                if let Some(p) = &mut self.path {
                    let ops = ops.split_once(" m ").map_or(ops.as_str(), |(_, r)| r);
                    if p.len().saturating_add(ops.len()) <= MAX_OUTPUT {
                        p.push_str(ops);
                    }
                } else {
                    self.paint(ops, false, true);
                }
            }
            59 => self.path = Some(String::new()),
            60 => {}
            61 => {
                if let Some(p) = &mut self.path {
                    p.push_str("h ");
                }
            }
            // FILLPATH, STROKEANDFILLPATH, STROKEPATH.
            62..=64 => {
                if let Some(p) = self.path.take() {
                    self.paint(p, ty != 64, ty != 62);
                }
            }
            // ABORTPATH.
            68 => self.path = None,
            82 => {
                // EXTCREATEFONTINDIRECTW: handle, then a LOGFONTW.
                let ih = u32_at(b, 8)?;
                let height = f64::from(i32_at(b, 12)?);
                let weight = i32_at(b, 28).unwrap_or(400);
                let italic = b.get(32).is_some_and(|v| *v != 0);
                let face: Vec<u16> = (0..32).map_while(|i| i16_at(b, 40 + i * 2).map(|v| v as u16)).take_while(|c| *c != 0).collect();
                let name = String::from_utf16_lossy(&face);
                self.keep(ih, Obj::Font(Font { height, bold: weight >= 600, italic, fam: family_of(&name) }));
            }
            84 => {
                self.text(b)?;
            }
            95 => {
                // EXTCREATEPEN: handle, bitmap offsets and sizes, then a LOGPENEX.
                let ih = u32_at(b, 8)?;
                let style = u32_at(b, 28)? & 0xF;
                let pen = (style != 5).then(|| Pen { width: f64::from(u32_at(b, 32).unwrap_or(0)), color: colorref(b, 40).unwrap_or([0.0; 3]) });
                self.keep(ih, Obj::Pen(pen));
            }
            _ => {}
        }
        Some(())
    }
}

/// Play an EMF picture into a form XObject whose unit square is the picture (so it is placed
/// like an image). `None` when the bytes are not an EMF or draw nothing.
pub(crate) fn form(doc: &mut Document, bytes: &[u8], notes: &mut Notes) -> Option<ObjRef> {
    // The older Windows Metafile format has its own player.
    if !is_emf(bytes) {
        return super::wmf::form(doc, bytes, notes);
    }
    let rect = |at: usize| -> Option<[f64; 4]> {
        Some([i32_at(bytes, at)?, i32_at(bytes, at + 4)?, i32_at(bytes, at + 8)?, i32_at(bytes, at + 12)?].map(f64::from))
    };
    let bounds = rect(8)?;
    let frame = rect(24)?;
    let dev = (f64::from(i32_at(bytes, 72)?), f64::from(i32_at(bytes, 76)?));
    let mm = (f64::from(i32_at(bytes, 80)?), f64::from(i32_at(bytes, 84)?));
    let ppmm = if dev.0 > 0.0 && dev.1 > 0.0 && mm.0 > 0.0 && mm.1 > 0.0 { (dev.0 / mm.0, dev.1 / mm.1) } else { (96.0 / 25.4, 96.0 / 25.4) };
    // The frame (hundredths of a millimetre) in device pixels; the bounds when it is empty.
    let (mut fr, mut size) = (
        (frame[0] / 100.0 * ppmm.0, frame[1] / 100.0 * ppmm.1, frame[2] / 100.0 * ppmm.0, frame[3] / 100.0 * ppmm.1),
        ((frame[2] - frame[0]) / 100.0 / 25.4 * 72.0, (frame[3] - frame[1]) / 100.0 / 25.4 * 72.0),
    );
    if !(size.0 > 0.1 && size.1 > 0.1 && size.0.is_finite() && size.1.is_finite()) {
        fr = (bounds[0], bounds[1], bounds[2] + 1.0, bounds[3] + 1.0);
        size = ((fr.2 - fr.0) / ppmm.0 / 25.4 * 72.0, (fr.3 - fr.1) / ppmm.1 / 25.4 * 72.0);
    }
    if !(size.0 > 0.1 && size.1 > 0.1 && size.0 < 14_400.0 && size.1 < 14_400.0) {
        return None;
    }
    let mut p = Player {
        notes,
        dc: Dc::default(),
        saved: Vec::new(),
        objects: HashMap::new(),
        ppmm,
        frame: fr,
        size,
        out: String::new(),
        path: None,
        cur: (0.0, 0.0),
        fonts: 0,
    };
    let mut at = 0usize;
    for _ in 0..MAX_RECORDS {
        let (Some(ty), Some(len)) = (u32_at(bytes, at), u32_at(bytes, at + 4)) else { break };
        let len = len as usize;
        if len < 8 || ty == 14 {
            break;
        }
        let Some(rec) = at.checked_add(len).and_then(|end| bytes.get(at..end)) else { break };
        let _ = p.record(ty, rec);
        at += len;
    }
    if p.out.is_empty() {
        return None;
    }
    Some(form_xobject(doc, size, p.out.as_bytes(), p.fonts))
}

/// A form XObject `size` points big holding content `ops`, shown in the unit square an image
/// fills, with the standard faces `fonts` (a mask of [`FACES`]) as `/F{i}`.
pub(crate) fn form_xobject(doc: &mut Document, size: (f64, f64), ops: &[u8], fonts_used: u16) -> ObjRef {
    let mut fonts = Dict::new();
    for (i, face) in FACES.iter().enumerate() {
        if fonts_used & (1 << i) != 0 {
            let mut f = Dict::new();
            f.set(b"Type".to_vec(), Object::name("Font"));
            f.set(b"Subtype".to_vec(), Object::name("Type1"));
            f.set(b"BaseFont".to_vec(), Object::name(face.base_font()));
            f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
            fonts.set(format!("F{i}").into_bytes(), Object::Ref(doc.add(f)));
        }
    }
    let mut res = Dict::new();
    if !fonts.is_empty() {
        res.set(b"Font".to_vec(), Object::Dict(fonts));
    }
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Real(size.0), Object::Real(size.1)]));
    // Drawn in points, shown in the unit square an image fills.
    d.set(
        b"Matrix".to_vec(),
        Object::Array(vec![Object::Real(1.0 / size.0), Object::Int(0), Object::Int(0), Object::Real(1.0 / size.1), Object::Int(0), Object::Int(0)]),
    );
    d.set(b"Resources".to_vec(), Object::Dict(res));
    doc.add(Object::Stream(Stream::flate(d, ops)))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An EMF record of type `ty` with `body` after its type and size.
    pub(crate) fn rec(ty: u32, body: &[u8]) -> Vec<u8> {
        let mut out = ty.to_le_bytes().to_vec();
        out.extend_from_slice(&((body.len() + 8) as u32).to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    fn ints(v: &[i32]) -> Vec<u8> {
        v.iter().flat_map(|i| i.to_le_bytes()).collect()
    }

    /// A synthetic EMF: a 2 × 1 cm frame on a 96 dpi device, then `records`.
    pub(crate) fn emf(records: &[Vec<u8>]) -> Vec<u8> {
        let mut h = ints(&[0, 0, 75, 37, 0, 0, 2000, 1000]);
        h.extend_from_slice(b" EMF");
        h.extend_from_slice(&ints(&[0x10000, 0, 0, 0, 0, 0, 0, 1000, 1000, 264, 264]));
        let mut out = rec(1, &h);
        for r in records {
            out.extend_from_slice(r);
        }
        out.extend(rec(14, &ints(&[0, 0, 0])));
        let len = out.len() as u32;
        out.splice(48..52, len.to_le_bytes());
        out
    }

    /// The records drawing a blue rounded rectangle with "Logo" in it.
    pub(crate) fn logo_records() -> Vec<Vec<u8>> {
        let mut font = ints(&[3, -12, 0, 0, 0, 700]);
        font.extend_from_slice(&[0; 8]);
        font.extend("Arial".encode_utf16().flat_map(u16::to_le_bytes));
        font.resize(8 + 92, 0);
        let text: Vec<u8> = "Logo".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut out_text = ints(&[0, 0, 75, 37, 1]);
        out_text.extend(1.0f32.to_le_bytes());
        out_text.extend(1.0f32.to_le_bytes());
        out_text.extend(ints(&[10, 10, 4, 76, 0, 0, 0, 0, 0, 0]));
        out_text.extend(&text);
        vec![
            rec(39, &ints(&[1, 0, 0x00C0_4020_u32 as i32, 0])),
            rec(37, &ints(&[1])),
            rec(38, &ints(&[2, 0, 2, 0, 0x0000_0000])),
            rec(37, &ints(&[2])),
            rec(44, &ints(&[2, 2, 70, 34, 10, 10])),
            rec(82, &font),
            rec(37, &ints(&[3])),
            rec(24, &ints(&[0x00FF_FFFF])),
            rec(84, &out_text),
            // A truncated record is skipped, not read past.
            rec(86, &ints(&[0, 0, 1, 1, 1_000_000])),
        ]
    }

    #[test]
    fn shapes_and_text_become_a_form_the_size_of_the_frame() {
        let mut doc = Document::new_empty();
        let mut notes = Notes::default();
        let r = form(&mut doc, &emf(&logo_records()), &mut notes).expect("a form");
        let Object::Stream(s) = &*doc.get(r) else { panic!("a stream") };
        let bbox = s.dict.get(b"BBox").unwrap().as_array().unwrap().clone();
        let w = bbox[2].as_f64().unwrap();
        assert!((w - 56.69).abs() < 0.1, "2 cm wide: {w}");
        let ops = String::from_utf8_lossy(&s.decoded().unwrap()).into_owned();
        assert!(ops.contains(" c "), "rounded corners are curves: {ops}");
        assert!(ops.contains("0.125 0.251 0.753 rg"), "the brush colour fills it: {ops}");
        assert!(ops.contains("(Logo) Tj") && ops.contains("/F1 "), "bold Arial text in Helvetica-Bold: {ops}");
    }

    #[test]
    fn hostile_metafiles_are_refused_or_cut_short() {
        let mut doc = Document::new_empty();
        let mut notes = Notes::default();
        assert!(form(&mut doc, b"not an emf", &mut notes).is_none());
        assert!(form(&mut doc, &emf(&[]), &mut notes).is_none(), "nothing drawn");
        // Records claiming huge sizes, counts and handles.
        let mut bad = emf(&[rec(86, &ints(&[0, 0, 1, 1, i32::MAX])), rec(38, &ints(&[i32::MAX, 0, 1, 0, 0])), rec(34, &ints(&[i32::MIN]))]);
        bad.extend(ints(&[43, i32::MAX]));
        let _ = form(&mut doc, &bad, &mut notes);
        let mut header = emf(&logo_records());
        header.truncate(100);
        let _ = form(&mut doc, &header, &mut notes);
    }
}
