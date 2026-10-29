//! Windows Metafile (WMF, [MS-WMF]) pictures as PDF vector drawings, the older cousin of
//! [`super::emf`]: pens, brushes and fonts (with the object table's lowest-free-slot rule),
//! rectangles, rounded rectangles, ellipses, polygons and polylines, lines, text (`TEXTOUT`,
//! `EXTTEXTOUT`, Windows-1252), window origin and extent, and saved states. The placeable header
//! gives the picture's size; without one, the window extent does. Bitmaps and other records are
//! skipped. The file is untrusted: every read is bounds-checked and records and output capped.

use std::fmt::Write as _;

use pdfcraft_cos::{Document, ObjRef};
use pdfcraft_fonts::win_ansi_byte;

use super::canvas::n;
use super::emf::{face_index, form_xobject};
use super::{Fam, Notes, RunStyle, family_of};

const MAX_RECORDS: usize = 200_000;
const MAX_OUTPUT: usize = 8 << 20;
const MAX_OBJECTS: usize = 4_096;

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn i16_at(b: &[u8], at: usize) -> Option<f64> {
    Some(f64::from(i16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?)))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn colorref(b: &[u8], at: usize) -> Option<[f64; 3]> {
    let [r, g, bl]: [u8; 3] = b.get(at..at.checked_add(3)?)?.try_into().ok()?;
    Some([f64::from(r) / 255.0, f64::from(g) / 255.0, f64::from(bl) / 255.0])
}

/// Whether `bytes` start like a WMF: a placeable header, or a standard header.
pub(crate) fn is_wmf(bytes: &[u8]) -> bool {
    u32_at(bytes, 0) == Some(0x9AC6_CDD7)
        || (matches!(u16_at(bytes, 0), Some(1 | 2)) && u16_at(bytes, 2) == Some(9) && matches!(u16_at(bytes, 4), Some(0x0100 | 0x0300)))
}

#[derive(Clone, Debug)]
enum Obj {
    Pen(Option<(f64, [f64; 3])>),
    Brush(Option<[f64; 3]>),
    Font(f64, bool, bool, Fam),
    Other,
}

#[derive(Clone, Debug)]
struct Dc {
    pen: Option<(f64, [f64; 3])>,
    brush: Option<[f64; 3]>,
    font: (f64, bool, bool, Fam),
    text: [f64; 3],
    align: u16,
    org: (f64, f64),
    ext: (f64, f64),
    winding: bool,
}

/// Play a WMF into a form XObject the size of the picture. `None` when it is not a WMF or draws
/// nothing.
pub(crate) fn form(doc: &mut Document, bytes: &[u8], notes: &mut Notes) -> Option<ObjRef> {
    if !is_wmf(bytes) {
        return None;
    }
    // The placeable header: bounds in logical units and units per inch.
    let (start, bounds) = if u32_at(bytes, 0) == Some(0x9AC6_CDD7) {
        let (l, t, r, b) = (i16_at(bytes, 6)?, i16_at(bytes, 8)?, i16_at(bytes, 10)?, i16_at(bytes, 12)?);
        let inch = f64::from(u16_at(bytes, 14)?).max(1.0);
        (22usize, Some((l, t, r, b, inch)))
    } else {
        (0, None)
    };
    let header_words = usize::from(u16_at(bytes, start + 2)?);
    let mut at = start.checked_add(header_words.checked_mul(2)?)?;
    let mut dc = Dc {
        pen: Some((0.0, [0.0; 3])),
        brush: Some([1.0; 3]),
        font: (12.0, false, false, Fam::Sans),
        text: [0.0; 3],
        align: 0,
        org: (0.0, 0.0),
        ext: (0.0, 0.0),
        winding: false,
    };
    if let Some((l, t, r, b, _)) = bounds {
        dc.org = (l, t);
        dc.ext = (r - l, b - t);
    }
    let size = match bounds {
        Some((l, t, r, b, inch)) => (((r - l).abs() / inch * 72.0), ((b - t).abs() / inch * 72.0)),
        None => (0.0, 0.0),
    };
    let mut saved: Vec<Dc> = Vec::new();
    let mut objects: Vec<Option<Obj>> = Vec::new();
    let mut out = String::new();
    let mut fonts = 0u16;
    let mut cur = (0.0, 0.0);
    // The frame drawing happens in; set once the window is known.
    let mut records: Vec<(u16, usize, usize)> = Vec::new();
    for _ in 0..MAX_RECORDS {
        let (Some(words), Some(func)) = (u32_at(bytes, at), u16_at(bytes, at + 4)) else { break };
        let len = (words as usize).checked_mul(2)?;
        if func == 0 || len < 6 {
            break;
        }
        let end = at.checked_add(len)?;
        if end > bytes.len() {
            break;
        }
        records.push((func, at + 6, end));
        at = end;
    }
    // Without a placeable header, the window extent sets the size (at 96 units an inch).
    let size = if size.0 > 0.1 && size.1 > 0.1 {
        size
    } else {
        let ext = records.iter().find(|(f, _, _)| *f == 0x020C).and_then(|(_, p, _)| Some((i16_at(bytes, p + 2)?, i16_at(bytes, *p)?)))?;
        (ext.0.abs() * 0.75, ext.1.abs() * 0.75)
    };
    if !(size.0 > 0.1 && size.1 > 0.1 && size.0 < 14_400.0 && size.1 < 14_400.0) {
        return None;
    }
    for (func, p, end) in records {
        let pt = |dc: &Dc, x: f64, y: f64| -> (f64, f64) {
            let (ex, ey) = (if dc.ext.0.abs() > 1e-9 { dc.ext.0 } else { 1.0 }, if dc.ext.1.abs() > 1e-9 { dc.ext.1 } else { 1.0 });
            let u = (x - dc.org.0) / ex * size.0;
            let v = size.1 - (y - dc.org.1) / ey * size.1;
            (if u.is_finite() { u } else { 0.0 }, if v.is_finite() { v } else { 0.0 })
        };
        let unit = |dc: &Dc| (size.0 / if dc.ext.0.abs() > 1e-9 { dc.ext.0.abs() } else { 1.0 }).min(1_000.0);
        let paint = |dc: &Dc, ops: &str, fill: bool, out: &mut String| {
            let brush = if fill { dc.brush } else { None };
            if brush.is_none() && dc.pen.is_none() {
                return;
            }
            let mut s = String::from("q ");
            if let Some(c) = brush {
                let _ = write!(s, "{} {} {} rg ", n(c[0]), n(c[1]), n(c[2]));
            }
            if let Some((w, c)) = dc.pen {
                let w = if w <= 0.0 { 0.75 } else { (w * unit(dc)).clamp(0.1, 200.0) };
                let _ = write!(s, "{} {} {} RG {} w 1 j ", n(c[0]), n(c[1]), n(c[2]), n(w));
            }
            s.push_str(ops);
            s.push_str(match (brush.is_some(), dc.pen.is_some(), dc.winding) {
                (true, true, true) => "B",
                (true, true, false) => "B*",
                (true, false, true) => "f",
                (true, false, false) => "f*",
                _ => "S",
            });
            s.push_str(" Q\n");
            if out.len() + s.len() <= MAX_OUTPUT {
                out.push_str(&s);
            }
        };
        let rect = |dc: &Dc, l: f64, t: f64, r: f64, b: f64, rx: f64, ry: f64| -> String {
            let (a, z) = (pt(dc, l, t), pt(dc, r, b));
            let (x1, x2, y1, y2) = (a.0.min(z.0), a.0.max(z.0), a.1.min(z.1), a.1.max(z.1));
            let (rx, ry) = ((rx * unit(dc)).min((x2 - x1) / 2.0), (ry * unit(dc)).min((y2 - y1) / 2.0));
            if rx <= 0.0 || ry <= 0.0 {
                return format!("{} {} {} {} re ", n(x1), n(y1), n(x2 - x1), n(y2 - y1));
            }
            let (kx, ky) = (rx * 0.552_284_75, ry * 0.552_284_75);
            format!(
                "{} {} m {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c h ",
                n(x1 + rx),
                n(y1),
                n(x2 - rx),
                n(y1),
                n(x2 - rx + kx),
                n(y1),
                n(x2),
                n(y1 + ry - ky),
                n(x2),
                n(y1 + ry),
                n(x2),
                n(y2 - ry),
                n(x2),
                n(y2 - ry + ky),
                n(x2 - rx + kx),
                n(y2),
                n(x2 - rx),
                n(y2),
                n(x1 + rx),
                n(y2),
                n(x1 + rx - kx),
                n(y2),
                n(x1),
                n(y2 - ry + ky),
                n(x1),
                n(y2 - ry),
                n(x1),
                n(y1 + ry),
                n(x1),
                n(y1 + ry - ky),
                n(x1 + rx - kx),
                n(y1),
                n(x1 + rx),
                n(y1)
            )
        };
        let points = |dc: &Dc, at: usize, count: usize| -> String {
            let mut s = String::new();
            for i in 0..count.min(100_000) {
                let (Some(x), Some(y)) = (i16_at(bytes, at + i * 4), i16_at(bytes, at + i * 4 + 2)) else { break };
                let (u, v) = pt(dc, x, y);
                let _ = write!(s, "{} {} {} ", n(u), n(v), if i == 0 { "m" } else { "l" });
            }
            s
        };
        let add = |objects: &mut Vec<Option<Obj>>, o: Obj| match objects.iter().position(Option::is_none) {
            Some(i) => {
                if let Some(slot) = objects.get_mut(i) {
                    *slot = Some(o);
                }
            }
            None if objects.len() < MAX_OBJECTS => objects.push(Some(o)),
            None => {}
        };
        match func {
            0x020B => dc.org = (i16_at(bytes, p + 2).unwrap_or(0.0), i16_at(bytes, p).unwrap_or(0.0)),
            0x020C => dc.ext = (i16_at(bytes, p + 2).unwrap_or(1.0), i16_at(bytes, p).unwrap_or(1.0)),
            0x0106 => dc.winding = u16_at(bytes, p) == Some(2),
            0x0209 => dc.text = colorref(bytes, p).unwrap_or([0.0; 3]),
            0x012E => dc.align = u16_at(bytes, p).unwrap_or(0),
            0x001E => {
                if saved.len() < 64 {
                    saved.push(dc.clone());
                }
            }
            0x0127 => {
                if let Some(d) = saved.pop() {
                    dc = d;
                }
            }
            0x02FA => {
                let style = u16_at(bytes, p).unwrap_or(0) & 0xF;
                let pen = (style != 5).then(|| (i16_at(bytes, p + 2).unwrap_or(0.0).abs(), colorref(bytes, p + 6).unwrap_or([0.0; 3])));
                add(&mut objects, Obj::Pen(pen));
            }
            0x02FC => {
                let brush = (u16_at(bytes, p) != Some(1)).then(|| colorref(bytes, p + 2).unwrap_or([1.0; 3]));
                add(&mut objects, Obj::Brush(brush));
            }
            0x02FB => {
                let h = i16_at(bytes, p).unwrap_or(-12.0);
                let weight = i16_at(bytes, p + 8).unwrap_or(400.0);
                let italic = bytes.get(p + 10).is_some_and(|v| *v != 0);
                let face: String =
                    bytes.get(p + 18..end).unwrap_or_default().iter().take_while(|b| **b != 0).take(32).map(|b| char::from(*b)).collect();
                add(&mut objects, Obj::Font(h, weight >= 600.0, italic, family_of(&face)));
            }
            0x00F7 | 0x0142 | 0x01F9 | 0x06FF | 0x00F8 => add(&mut objects, Obj::Other),
            0x012D => {
                let i = usize::from(u16_at(bytes, p).unwrap_or(u16::MAX));
                match objects.get(i).cloned().flatten() {
                    Some(Obj::Pen(pen)) => dc.pen = pen,
                    Some(Obj::Brush(b)) => dc.brush = b,
                    Some(Obj::Font(h, b, it, fam)) => dc.font = (h, b, it, fam),
                    _ => {}
                }
            }
            0x01F0 => {
                if let Some(slot) = objects.get_mut(usize::from(u16_at(bytes, p).unwrap_or(u16::MAX))) {
                    *slot = None;
                }
            }
            0x0214 => cur = (i16_at(bytes, p + 2).unwrap_or(0.0), i16_at(bytes, p).unwrap_or(0.0)),
            0x0213 => {
                let to = (i16_at(bytes, p + 2).unwrap_or(0.0), i16_at(bytes, p).unwrap_or(0.0));
                let (a, b) = (pt(&dc, cur.0, cur.1), pt(&dc, to.0, to.1));
                paint(&dc, &format!("{} {} m {} {} l ", n(a.0), n(a.1), n(b.0), n(b.1)), false, &mut out);
                cur = to;
            }
            0x041B | 0x0418 | 0x061C => {
                // Rectangle and ellipse: bottom, right, top, left; a rounded rectangle has the
                // corner's height and width first.
                let base = if func == 0x061C { p + 4 } else { p };
                let (b, r, t, l) = (i16_at(bytes, base), i16_at(bytes, base + 2), i16_at(bytes, base + 4), i16_at(bytes, base + 6));
                let (Some(b), Some(r), Some(t), Some(l)) = (b, r, t, l) else { continue };
                let (rx, ry) = match func {
                    0x0418 => ((r - l).abs() / 2.0, (b - t).abs() / 2.0),
                    0x061C => (i16_at(bytes, p + 2).unwrap_or(0.0) / 2.0, i16_at(bytes, p).unwrap_or(0.0) / 2.0),
                    _ => (0.0, 0.0),
                };
                let ops = rect(&dc, l, t, r, b, rx, ry);
                paint(&dc, &ops, true, &mut out);
            }
            0x0324 | 0x0325 => {
                let count = usize::from(u16_at(bytes, p).unwrap_or(0));
                let mut ops = points(&dc, p + 2, count);
                if func == 0x0324 {
                    ops.push_str("h ");
                }
                paint(&dc, &ops, func == 0x0324, &mut out);
            }
            0x0538 => {
                let polys = usize::from(u16_at(bytes, p).unwrap_or(0)).min(10_000);
                let mut at = p + 2 + polys * 2;
                let mut ops = String::new();
                for i in 0..polys {
                    let count = usize::from(u16_at(bytes, p + 2 + i * 2).unwrap_or(0));
                    ops.push_str(&points(&dc, at, count));
                    ops.push_str("h ");
                    at = at.saturating_add(count * 4);
                }
                paint(&dc, &ops, true, &mut out);
            }
            0x0521 | 0x0A32 => {
                let (x, y, text) = if func == 0x0521 {
                    let count = usize::from(u16_at(bytes, p).unwrap_or(0));
                    let padded = count + count % 2;
                    let s = bytes.get(p + 2..p + 2 + count).unwrap_or_default().to_vec();
                    (i16_at(bytes, p + 2 + padded + 2), i16_at(bytes, p + 2 + padded), s)
                } else {
                    let count = usize::from(u16_at(bytes, p + 4).unwrap_or(0));
                    let opts = u16_at(bytes, p + 6).unwrap_or(0);
                    let s_at = p + 8 + if opts & 0x6 != 0 { 8 } else { 0 };
                    (i16_at(bytes, p + 2), i16_at(bytes, p), bytes.get(s_at..s_at + count).unwrap_or_default().to_vec())
                };
                let (Some(x), Some(y)) = (x, y) else { continue };
                let (text, _, _) = encoding_rs::WINDOWS_1252.decode(&text);
                let text = notes.drawable(&text);
                if text.trim().is_empty() {
                    continue;
                }
                let (h, bold, italic, fam) = dc.font;
                let st = RunStyle { fam, bold, italic, ..RunStyle::default() };
                let em = if h < 0.0 { -h } else { h / 1.2 };
                let size = (em * unit(&dc)).clamp(0.5, 1_000.0);
                let width = RunStyle { size, ..st.clone() }.width(&text);
                let (mut u, mut v) = pt(&dc, x, y);
                match dc.align & 6 {
                    6 => u -= width / 2.0,
                    2 => u -= width,
                    _ => {}
                }
                match dc.align & 24 {
                    24 => {}
                    8 => v += size * 0.212,
                    _ => v -= size * 0.905,
                }
                let fi = face_index(st.face());
                fonts |= 1 << fi;
                let c = dc.text;
                let mut op = format!("BT {} {} {} rg /F{fi} {} Tf 1 0 0 1 {} {} Tm (", n(c[0]), n(c[1]), n(c[2]), n(size), n(u), n(v));
                for ch in text.chars() {
                    match win_ansi_byte(ch).unwrap_or(b'?') {
                        b @ (b'(' | b')' | b'\\') => {
                            op.push('\\');
                            op.push(char::from(b));
                        }
                        b @ 0x20..=0x7E => op.push(char::from(b)),
                        b => {
                            let _ = write!(op, "\\{b:03o}");
                        }
                    }
                }
                op.push_str(") Tj ET\n");
                if out.len() + op.len() <= MAX_OUTPUT {
                    out.push_str(&op);
                }
            }
            _ => {}
        }
    }
    if out.is_empty() {
        return None;
    }
    Some(form_xobject(doc, size, out.as_bytes(), fonts))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn rec(func: u16, params: &[i16]) -> Vec<u8> {
        let words = 3 + params.len() as u32;
        let mut out = words.to_le_bytes().to_vec();
        out.extend(func.to_le_bytes());
        out.extend(params.iter().flat_map(|p| p.to_le_bytes()));
        out
    }

    /// A placeable metafile an inch wide and half an inch tall holding `records`.
    pub(crate) fn wmf(records: &[Vec<u8>]) -> Vec<u8> {
        let mut out = 0x9AC6_CDD7u32.to_le_bytes().to_vec();
        out.extend([0, 0]);
        out.extend([0i16, 0, 1440, 720].iter().flat_map(|v| v.to_le_bytes()));
        out.extend(1440u16.to_le_bytes());
        out.extend([0u8; 6]);
        out.extend([1u16, 9, 0x0300].iter().flat_map(|v| v.to_le_bytes()));
        out.extend([0u8; 12]);
        for r in records {
            out.extend(r);
        }
        out.extend(rec(0, &[]));
        out
    }

    #[test]
    fn shapes_and_text_become_a_form_an_inch_wide() {
        let records = vec![
            rec(0x02FC, &[0, 0x00FF, 0, 0]),
            rec(0x012D, &[0]),
            rec(0x041B, &[700, 1400, 20, 20]),
            rec(0x0521, &[2, i16::from_le_bytes(*b"Hi"), 360, 100]),
        ];
        let mut doc = Document::new_empty();
        let r = form(&mut doc, &wmf(&records), &mut Notes::default()).expect("a form");
        let pdfcraft_cos::Object::Stream(s) = &*doc.get(r) else { panic!() };
        let bbox = s.dict.get(b"BBox").unwrap().as_array().unwrap().clone();
        assert!((bbox[2].as_f64().unwrap() - 72.0).abs() < 0.01);
        let ops = String::from_utf8_lossy(&s.decoded().unwrap()).into_owned();
        assert!(ops.contains("1 0 0 rg") && ops.contains(" re ") && ops.contains("(Hi) Tj"), "{ops}");
    }

    #[test]
    fn damaged_metafiles_are_refused_or_cut_short() {
        let mut doc = Document::new_empty();
        assert!(form(&mut doc, b"nope", &mut Notes::default()).is_none());
        let mut bad = wmf(&[rec(0x0324, &[i16::MAX]), rec(0x0538, &[i16::MAX])]);
        bad.truncate(bad.len() - 3);
        let _ = form(&mut doc, &bad, &mut Notes::default());
    }
}
