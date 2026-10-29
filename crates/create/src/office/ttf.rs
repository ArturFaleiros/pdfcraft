//! TrueType subsetting for embedded Office faces: the program keeps its glyph numbering, but
//! only the glyphs a document uses (and the components of composite ones) keep their outlines;
//! the rest are emptied. The tables a PDF TrueType program needs are kept (`head`, `hhea`,
//! `hmtx`, `maxp`, `loca`, `glyf`, and the hinting tables `cvt `, `fpgm`, `prep`, plus `OS/2`);
//! the character map (codes are glyph ids, ISO 32000-2 §9.7.4.2) and the layout and naming
//! tables are dropped. Built from the OpenType specification
//! (`glyf`, `loca`, `head`, table directory); the program comes from craft-fonts and is
//! validated by skrifa before it gets here, but every read is still bounds-checked.

use std::collections::BTreeSet;

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn i16_at(b: &[u8], at: usize) -> Option<i16> {
    Some(i16::from_be_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// A table's bytes by tag.
fn table<'a>(font: &'a [u8], tag: &[u8; 4]) -> Option<&'a [u8]> {
    let n = usize::from(u16_at(font, 4)?);
    (0..n).find_map(|i| {
        let rec = 12usize.checked_add(i.checked_mul(16)?)?;
        if font.get(rec..rec + 4)? != tag {
            return None;
        }
        let off = u32_at(font, rec + 8)? as usize;
        let len = u32_at(font, rec + 12)? as usize;
        font.get(off..off.checked_add(len)?)
    })
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, c| {
        let mut w = [0u8; 4];
        for (d, s) in w.iter_mut().zip(c) {
            *d = *s;
        }
        sum.wrapping_add(u32::from_be_bytes(w))
    })
}

/// The program of `font` with outlines only for `keep` (glyph 0 is always kept).
pub(crate) fn subset(font: &[u8], keep: &BTreeSet<u16>) -> Option<Vec<u8>> {
    let head = table(font, b"head")?;
    let maxp = table(font, b"maxp")?;
    let loca = table(font, b"loca")?;
    let glyf = table(font, b"glyf")?;
    let count = usize::from(u16_at(maxp, 4)?);
    let long = i16_at(head, 50)? == 1;
    let offset =
        |g: usize| -> Option<usize> { if long { u32_at(loca, g * 4).map(|v| v as usize) } else { u16_at(loca, g * 2).map(|v| usize::from(v) * 2) } };
    let data = |g: usize| -> Option<&[u8]> {
        let (a, b) = (offset(g)?, offset(g + 1)?);
        if b < a {
            return None;
        }
        glyf.get(a..b)
    };
    // Composite glyphs bring their components (bounded: each glyph is visited once).
    let mut wanted: BTreeSet<u16> = keep.iter().copied().filter(|g| usize::from(*g) < count).collect();
    wanted.insert(0);
    let mut todo: Vec<u16> = wanted.iter().copied().collect();
    while let Some(g) = todo.pop() {
        let Some(d) = data(usize::from(g)) else { continue };
        if d.len() < 10 || i16_at(d, 0)? >= 0 {
            continue;
        }
        let mut at = 10usize;
        for _ in 0..256 {
            let (Some(flags), Some(comp)) = (u16_at(d, at), u16_at(d, at + 2)) else { break };
            if usize::from(comp) < count && wanted.insert(comp) {
                todo.push(comp);
            }
            at += 4 + if flags & 1 != 0 { 4 } else { 2 };
            at += if flags & 0x8 != 0 {
                2
            } else if flags & 0x40 != 0 {
                4
            } else if flags & 0x80 != 0 {
                8
            } else {
                0
            };
            if flags & 0x20 == 0 {
                break;
            }
        }
    }
    let mut new_glyf = Vec::new();
    let mut new_loca = Vec::with_capacity((count + 1) * 4);
    for g in 0..count {
        new_loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes());
        if wanted.contains(&(g as u16))
            && let Some(d) = data(g)
        {
            new_glyf.extend_from_slice(d);
            while new_glyf.len() % 4 != 0 {
                new_glyf.push(0);
            }
        }
    }
    new_loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes());
    let mut new_head = head.to_vec();
    // Long offsets, and a checksum adjustment worked out below.
    new_head.get_mut(50..52)?.copy_from_slice(&1i16.to_be_bytes());
    new_head.get_mut(8..12)?.copy_from_slice(&[0; 4]);
    let mut tables: Vec<([u8; 4], Vec<u8>)> = vec![(*b"head", new_head), (*b"loca", new_loca), (*b"glyf", new_glyf)];
    for tag in [b"OS/2", b"cvt ", b"fpgm", b"hhea", b"hmtx", b"maxp", b"prep"] {
        if let Some(t) = table(font, tag) {
            tables.push((*tag, t.to_vec()));
        }
    }
    tables.sort_by_key(|a| a.0);
    let n = tables.len() as u16;
    let pow = 1u16 << (15 - n.max(1).leading_zeros() as u16).min(15);
    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&n.to_be_bytes());
    out.extend_from_slice(&(pow * 16).to_be_bytes());
    out.extend_from_slice(&(pow.trailing_zeros() as u16).to_be_bytes());
    out.extend_from_slice(&(n * 16 - pow * 16).to_be_bytes());
    let mut at = 12 + usize::from(n) * 16;
    let mut body = Vec::new();
    let mut head_at = None;
    for (tag, data) in &tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(data).to_be_bytes());
        out.extend_from_slice(&(at as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        if tag == b"head" {
            head_at = Some(at);
        }
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        at = 12 + usize::from(n) * 16 + body.len();
    }
    out.extend_from_slice(&body);
    if let Some(h) = head_at {
        let adjust = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        out.get_mut(h + 8..h + 12)?.copy_from_slice(&adjust.to_be_bytes());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundled Inter face (in the repository, present in every build).
    fn inter() -> &'static [u8] {
        pdfcraft_fonts::document_latin_fonts(false).first().unwrap().bytes
    }

    #[test]
    fn a_subset_keeps_the_used_outlines_and_still_parses() {
        use skrifa::{FontRef, MetadataProvider};
        let font = FontRef::new(inter()).unwrap();
        let a = font.charmap().map('A').unwrap().to_u32() as u16;
        let out = subset(inter(), &BTreeSet::from([a])).expect("a subset");
        assert!(out.len() < inter().len() / 2, "{} of {}", out.len(), inter().len());
        FontRef::new(&out).expect("the subset is a font");
        // Glyph numbers are kept: the glyph's outline is where it was.
        let outline = |font: &[u8], g: u16| -> Vec<u8> {
            let (head, loca, glyf) = (table(font, b"head").unwrap(), table(font, b"loca").unwrap(), table(font, b"glyf").unwrap());
            let at = |i: usize| {
                if i16_at(head, 50) == Some(1) { u32_at(loca, i * 4).unwrap() as usize } else { usize::from(u16_at(loca, i * 2).unwrap()) * 2 }
            };
            glyf[at(usize::from(g))..at(usize::from(g) + 1)].to_vec()
        };
        assert_eq!(outline(&out, a), outline(inter(), a));
        assert!(outline(&out, a + 1).is_empty(), "other glyphs are emptied");
        assert_eq!(checksum(&out), 0xB1B0_AFBA, "the checksum adjustment balances the font");
    }

    #[test]
    fn damaged_programs_are_refused_not_read_past() {
        assert!(subset(b"", &BTreeSet::new()).is_none());
        let mut cut = inter().to_vec();
        cut.truncate(400);
        assert!(subset(&cut, &BTreeSet::from([5])).is_none());
    }
}
