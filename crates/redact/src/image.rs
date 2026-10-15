//! Clearing the pixels of an image under redaction regions. The image is decoded, every pixel
//! whose centre maps into a region is set to zero (no ink for image masks), and the result is
//! written as a new Flate image; the original object is untouched (other pages may use it).
//! Gray/RGB JPEGs are decoded to samples and saved losslessly, without JPEG application data.
//! Images whose codec PdfCraft can't decode (JPX, JBIG2, CCITT) return `None`, and the
//! caller removes the whole image instead; that is fail-closed.

use ::image::ImageDecoder;
use pdfcraft_content::{Matrix, overlaps};
use pdfcraft_cos::{Document, Object, Stream};

/// Most pixels an image may have to be cleared in place (larger ones are removed).
const MAX_PIXELS: u64 = 64 << 20;

fn components(doc: &Document, s: &Stream) -> Option<usize> {
    let cs = doc.resolve(s.dict.get(b"ColorSpace")?);
    let cs = &*cs;
    let name = match cs {
        Object::Name(n) => n.as_slice(),
        Object::Array(a) => a.first()?.as_name()?,
        _ => return None,
    };
    Some(match name {
        b"DeviceGray" | b"G" | b"CalGray" | b"Indexed" | b"I" | b"Separation" => 1,
        b"DeviceRGB" | b"RGB" | b"CalRGB" | b"Lab" => 3,
        b"DeviceCMYK" | b"CMYK" => 4,
        b"ICCBased" => {
            // /N lives in the ICC profile stream, and shall be 1, 3 or 4 (ISO 32000-2 §8.6.5.5).
            match &*doc.resolve(cs.as_array()?.get(1)?) {
                Object::Stream(icc) => usize::try_from(icc.dict.int(b"N")?).ok().filter(|n| matches!(n, 1 | 3 | 4))?,
                _ => return None,
            }
        }
        _ => return None,
    })
}

/// `Ok(Some(copy))` with the covered pixels cleared, `Ok(None)` when no pixel centre is covered,
/// `Err(())` when the image can't be cleared (the caller removes it).
#[allow(clippy::result_unit_err)]
pub(crate) fn clear(doc: &Document, s: &Stream, ctm: &Matrix, rects: &[[f64; 4]]) -> Result<Option<Stream>, ()> {
    decode_and_clear(doc, s, ctm, rects).ok_or(())
}

fn decode_and_clear(doc: &Document, s: &Stream, ctm: &Matrix, rects: &[[f64; 4]]) -> Option<Option<Stream>> {
    // Separate masks can retain the original image data.
    if s.dict.contains(b"SMask") || s.dict.contains(b"Mask") {
        return None;
    }
    let jpeg = match s.dict.get(b"Filter") {
        Some(Object::Name(n)) => n == b"DCTDecode" || n == b"DCT",
        Some(Object::Array(a)) if a.len() == 1 => matches!(a.first()?.as_name(), Some(b"DCTDecode" | b"DCT")),
        _ => false,
    };
    let supported = |n: &[u8]| {
        matches!(
            n,
            b"FlateDecode" | b"Fl" | b"LZWDecode" | b"LZW" | b"ASCII85Decode" | b"A85" | b"ASCIIHexDecode" | b"AHx" | b"RunLengthDecode" | b"RL"
        )
    };
    let ok = match s.dict.get(b"Filter") {
        None => true,
        Some(Object::Name(n)) => supported(n),
        Some(Object::Array(a)) => a.iter().all(|f| f.as_name().is_some_and(supported)),
        _ => false,
    };
    if !ok && !jpeg {
        return None;
    }
    let (w, h) = (s.dict.int(b"Width")?, s.dict.int(b"Height")?);
    if w <= 0 || h <= 0 || (w as u64).checked_mul(h as u64)? > MAX_PIXELS {
        return None;
    }
    let (w, h) = (w as usize, h as usize);
    let mask = matches!(s.dict.get(b"ImageMask"), Some(Object::Bool(true)));
    let (ncomp, bpc) = if mask { (1, 1) } else { (components(doc, s)?, s.dict.int(b"BitsPerComponent")? as usize) };
    if !matches!(bpc, 1 | 2 | 4 | 8 | 16) {
        return None;
    }
    // Every size comes from the file: multiply checked, so an overflowing product refuses the image
    // instead of sizing the writes below.
    let bits_per_pixel = ncomp.checked_mul(bpc)?;
    let row = w.checked_mul(bits_per_pixel)?.div_ceil(8);
    let bytes = row.checked_mul(h)?;
    let row_bits = row.checked_mul(8)?;
    bytes.checked_mul(8)?;
    if bytes > (MAX_PIXELS as usize).checked_mul(8)? {
        return None;
    }
    let mut data = if jpeg { jpeg_samples(doc, s, w, h, ncomp, bpc)? } else { s.decoded_within(bytes).ok()? };
    if data.len() < bytes {
        return None;
    }
    data.truncate(bytes);
    // Image masks: a sample of 0 paints unless /Decode is [1 0]; cleared samples must not paint.
    let clear_bit = if mask {
        let inverted = s.dict.get(b"Decode").and_then(Object::as_array).and_then(|d| d.first()).and_then(Object::as_f64) == Some(1.0);
        !inverted
    } else {
        false
    };
    let mut cleared = 0usize;
    for j in 0..h {
        // Image space: (0,0) is the bottom-left of the unit square; row 0 is the top.
        let v = 1.0 - (j as f64 + 0.5) / h as f64;
        let (ax, ay) = ctm.apply(0.0, v);
        let (bx, by) = ctm.apply(1.0, v);
        let row_box = [ax.min(bx), ay.min(by), ax.max(bx), ay.max(by)];
        if !rects.iter().any(|r| overlaps(*r, [row_box[0], row_box[1] - 0.01, row_box[2], row_box[3] + 0.01], 0.0)) {
            continue;
        }
        for i in 0..w {
            let u = (i as f64 + 0.5) / w as f64;
            let (x, y) = ctm.apply(u, v);
            if !rects.iter().any(|r| x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3]) {
                continue;
            }
            cleared += 1;
            // `bytes * 8` fits, and every bit index is below it, so this arithmetic can't overflow.
            let start = j * row_bits + i * bits_per_pixel;
            for bit in start..start + bits_per_pixel {
                let (byte, shift) = (bit / 8, 7 - bit % 8);
                let b = data.get_mut(byte)?;
                if clear_bit {
                    *b |= 1 << shift;
                } else {
                    *b &= !(1 << shift);
                }
            }
        }
    }
    if cleared == 0 {
        return Some(None);
    }
    let mut dict = s.dict.clone();
    dict.remove(b"Length");
    dict.remove(b"DecodeParms");
    dict.remove(b"Alternates");
    Some(Some(Stream::flate(dict, &data)))
}

/// Restrict the JPEG path to color spaces whose decoded samples retain the PDF semantics.
/// Explicit DCT ColorTransform and complex color spaces stay fail-closed until supported.
fn jpeg_samples(doc: &Document, s: &Stream, w: usize, h: usize, channels: usize, bpc: usize) -> Option<Vec<u8>> {
    let cs = doc.resolve(s.dict.get(b"ColorSpace")?);
    let expected = match cs.as_name()? {
        b"DeviceGray" | b"G" => ::image::ColorType::L8,
        b"DeviceRGB" | b"RGB" => ::image::ColorType::Rgb8,
        _ => return None,
    };
    if bpc != 8
        || s.dict.contains(b"DecodeParms")
        || s.dict.contains(b"ImageMask")
        || s.raw.len() > (MAX_PIXELS as usize) * 4
        || jpeg_components(&s.raw)? != channels
    {
        return None;
    }
    let size = w.checked_mul(h)?.checked_mul(channels)?;
    let mut reader = ::image::ImageReader::with_format(std::io::Cursor::new(s.raw.as_ref()), ::image::ImageFormat::Jpeg);
    let mut limits = ::image::Limits::default();
    limits.max_image_width = Some(u32::try_from(w).ok()?);
    limits.max_image_height = Some(u32::try_from(h).ok()?);
    limits.max_alloc = Some(MAX_PIXELS * 4);
    reader.limits(limits);
    let decoder = reader.into_decoder().ok()?;
    if decoder.dimensions() != (u32::try_from(w).ok()?, u32::try_from(h).ok()?)
        || decoder.color_type() != expected
        || decoder.original_color_type() != expected.into()
        || decoder.total_bytes() != size as u64
    {
        return None;
    }
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(size).ok()?;
    pixels.resize(size, 0);
    decoder.read_image(&mut pixels).ok()?;
    Some(pixels)
}

/// `image` converts CMYK JPEGs to RGB. Check the encoded component count first, so that
/// even a PDF incorrectly declaring DeviceRGB cannot silently reinterpret those samples.
fn jpeg_components(bytes: &[u8]) -> Option<usize> {
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut pos = 2usize;
    while pos < bytes.len() {
        if *bytes.get(pos)? != 0xff {
            return None;
        }
        while bytes.get(pos) == Some(&0xff) {
            pos = pos.checked_add(1)?;
        }
        let marker = *bytes.get(pos)?;
        pos = pos.checked_add(1)?;
        if matches!(marker, 0xd9 | 0xda) {
            return None;
        }
        if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let length = u16::from_be_bytes([*bytes.get(pos)?, *bytes.get(pos.checked_add(1)?)?]) as usize;
        if length < 2 {
            return None;
        }
        let end = pos.checked_add(length)?;
        let segment = bytes.get(pos.checked_add(2)?..end)?;
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            if segment.first() != Some(&8) {
                return None;
            }
            return Some(usize::from(*segment.get(5)?));
        }
        pos = end;
    }
    None
}
