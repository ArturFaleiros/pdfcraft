//! Metric-compatible faces for Office text, from the optional craft-fonts build input
//! (`CRAFT_FONTS_DIR`, [`pdfcraft_fonts::CRAFT_FONTS`]): Carlito for Calibri, Caladea for Cambria,
//! and Liberation Sans, Serif and Mono for Arial, Times New Roman and Courier New. They have the
//! same advance widths as the faces they stand in for, so lines break where Office breaks them.
//! They are embedded (subset, [`super::ttf`]) by [`super::canvas`].
//!
//! Without craft-fonts (or with [`standard_fonts_only`]) every lookup here returns `None` and
//! text is set in the standard-14 faces, as before.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use skrifa::raw::TableProvider;
use skrifa::{FontRef, MetadataProvider, instance::LocationRef, instance::Size};

/// A metric-compatible family; `Std` uses the standard-14 face of the run's [`super::Fam`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum Face {
    #[default]
    Std,
    Carlito,
    Caladea,
    LiberationSans,
    LiberationSerif,
    LiberationMono,
    /// Georgia's metric twin.
    Gelasio,
    /// Segoe UI's metric twin.
    Selawik,
    /// Arial Narrow's metric twin.
    LiberationSansNarrow,
    /// A font the document embeds itself, by its place among the document's fonts
    /// ([`with_document_fonts`]).
    Doc(u8),
}

/// Every embedded family, in slot order.
const FAMILIES: [Face; 8] = [
    Face::Carlito,
    Face::Caladea,
    Face::LiberationSans,
    Face::LiberationSerif,
    Face::LiberationMono,
    Face::Gelasio,
    Face::Selawik,
    Face::LiberationSansNarrow,
];

impl Face {
    fn family(self) -> Option<&'static str> {
        Some(match self {
            Face::Std => return None,
            Face::Carlito => "Carlito",
            Face::Caladea => "Caladea",
            Face::LiberationSans => "Liberation Sans",
            Face::LiberationSerif => "Liberation Serif",
            Face::LiberationMono => "Liberation Mono",
            Face::Gelasio => "Gelasio",
            Face::Selawik => "Selawik",
            Face::LiberationSansNarrow => "Liberation Sans Narrow",
            Face::Doc(_) => return None,
        })
    }

    fn index(self) -> usize {
        match self {
            Face::Std => 0,
            Face::Carlito => 1,
            Face::Caladea => 2,
            Face::LiberationSans => 3,
            Face::LiberationSerif => 4,
            Face::LiberationMono => 5,
            Face::Gelasio => 6,
            Face::Selawik => 7,
            Face::LiberationSansNarrow => 8,
            Face::Doc(_) => 0,
        }
    }
}

/// The metric-compatible family for a document font name.
pub(crate) fn face_of(name: &str) -> Face {
    let n = name.to_ascii_lowercase();
    let n = n.trim();
    // A font the document embeds is drawn in itself.
    if let Some(i) = DOC_FACES.with(|d| d.borrow().iter().position(|(f, _)| f == n)) {
        return Face::Doc(u8::try_from(i).unwrap_or(u8::MAX));
    }
    if n.starts_with("calibri") || n == "carlito" {
        Face::Carlito
    } else if n.starts_with("cambria") || n == "caladea" {
        Face::Caladea
    } else if n == "georgia" || n == "gelasio" {
        Face::Gelasio
    } else if n.starts_with("segoe ui") || n == "selawik" {
        Face::Selawik
    } else if matches!(n, "arial narrow" | "liberation sans narrow") {
        Face::LiberationSansNarrow
    } else if matches!(n, "arial" | "helvetica" | "liberation sans" | "arimo" | "arial nova") {
        Face::LiberationSans
    } else if matches!(n, "times new roman" | "times" | "liberation serif" | "tinos") {
        Face::LiberationSerif
    } else if matches!(n, "courier new" | "courier" | "liberation mono" | "cousine") {
        Face::LiberationMono
    } else {
        Face::Std
    }
}

/// A loaded face: its program, metrics and the glyph and advance of each character it maps.
pub(crate) struct Loaded {
    pub bytes: &'static [u8],
    /// The PostScript-style name written as `/BaseFont` ("Carlito-Bold").
    pub name: String,
    pub ascent: f64,
    pub descent: f64,
    pub cap_height: f64,
    pub italic_angle: f64,
    pub bbox: [f64; 4],
    /// The `hhea` ascender, descender (positive) and line gap per em: Word's single line is
    /// their sum, with the gap above the text.
    pub line: (f64, f64, f64),
    /// Glyph id and advance in thousandths of an em, by character.
    glyphs: HashMap<char, (u16, f64)>,
    /// Advances by glyph id, for every glyph (component glyphs included).
    pub advances: Vec<f64>,
    /// For a face drawing shaped text: the first of the private-use characters that stand for
    /// its glyph ids ([`shape_rtl`]).
    pua: Option<u32>,
}

impl Loaded {
    pub(crate) fn glyph(&self, c: char) -> Option<(u16, f64)> {
        if let Some(base) = self.pua {
            let g = u32::from(c).checked_sub(base).filter(|g| *g < PUA_SPAN).and_then(|g| u16::try_from(g).ok())?;
            return self.advances.get(usize::from(g)).map(|w| (g, *w));
        }
        self.glyphs.get(&c).copied()
    }

    pub(crate) fn covers(&self, c: char) -> bool {
        self.glyph(c).is_some()
    }

    fn load(bytes: &'static [u8], family: &str, style: &str) -> Option<Loaded> {
        let font = FontRef::new(bytes).ok()?;
        // Only TrueType outlines are subset and embedded here.
        font.glyf().ok()?;
        let loc = LocationRef::default();
        let m = font.metrics(Size::unscaled(), loc);
        let upem = f64::from(m.units_per_em);
        if upem <= 0.0 {
            return None;
        }
        let k = 1000.0 / upem;
        let gm = font.glyph_metrics(Size::unscaled(), loc);
        let count = usize::from(m.glyph_count).min(65_535);
        let advances: Vec<f64> = (0..count)
            .map(|g| f64::from(gm.advance_width(skrifa::GlyphId::new(g as u32)).unwrap_or(0.0)) * k)
            .map(|w| if w.is_finite() { w.clamp(0.0, 16_000.0) } else { 0.0 })
            .collect();
        let mut glyphs = HashMap::new();
        for (cp, gid) in font.charmap().mappings().take(200_000) {
            let (Some(c), Ok(g)) = (char::from_u32(cp), u16::try_from(gid.to_u32())) else { continue };
            if g == 0 {
                continue;
            }
            if let Some(w) = advances.get(usize::from(g)) {
                glyphs.insert(c, (g, *w));
            }
        }
        let bounds = m.bounds?;
        let hhea = font.hhea().ok()?;
        let line =
            (f64::from(hhea.ascender().to_i16()) / upem, -f64::from(hhea.descender().to_i16()) / upem, f64::from(hhea.line_gap().to_i16()) / upem);
        if !(line.0 > 0.0 && line.1 >= 0.0 && line.2 >= 0.0 && line.0 + line.1 + line.2 < 4.0) {
            return None;
        }
        let name: String =
            format!("{}-{}", family.replace(' ', ""), style.replace(' ', "")).chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        Some(Loaded {
            bytes,
            name,
            ascent: f64::from(m.ascent) * k,
            descent: f64::from(m.descent) * k,
            cap_height: f64::from(m.cap_height.unwrap_or(m.ascent)) * k,
            italic_angle: f64::from(m.italic_angle),
            bbox: [bounds.x_min, bounds.y_min, bounds.x_max, bounds.y_max].map(|v| f64::from(v) * k),
            line,
            glyphs,
            advances,
            pua: None,
        })
    }
}

/// Faces by slot: `(face index - 1) * 4 + bold * 2 + italic`, then the script fallbacks.
const SLOTS: usize = FAMILIES.len() * 4;
/// Faces for characters the Office faces lack, tried in order: Japanese (kana and kanji),
/// Simplified Chinese, Korean, symbols. Scripts that need shaping or right-to-left ordering
/// (Arabic, Hebrew, Devanagari, Thai) are not drawn from fallbacks.
const FALLBACKS: [(&str, &str); 6] = [
    ("BIZ UDPGothic", "Regular"),
    ("LXGW WenKai Lite", "Regular"),
    ("NanumGothic", "Regular"),
    ("Noto Sans Symbols", "Regular"),
    ("Noto Sans Symbols 2", "Regular"),
    ("Noto Sans Georgian", "Regular"),
];
/// Slots of the shaped Arabic and Hebrew faces, after the fallbacks.
const ARABIC: usize = SLOTS + FALLBACKS.len();
const HEBREW: usize = ARABIC + 1;
static LOADED: [OnceLock<Option<Loaded>>; HEBREW + 1] = [const { OnceLock::new() }; HEBREW + 1];

/// Shaped glyphs travel through layout as private-use characters of plane 15: Arabic glyph `g`
/// as U+F0000 + g, Hebrew as U+F8000 + g.
const PUA_ARABIC: u32 = 0xF_0000;
const PUA_HEBREW: u32 = 0xF_8000;
const PUA_SPAN: u32 = 0x7FFE;

/// The text each shaped glyph shows, for `/ToUnicode` (shared by every conversion; a glyph
/// shows the same letters wherever it is used).
static SHAPED_TEXT: Mutex<Option<HashMap<u32, String>>> = Mutex::new(None);

thread_local! {
    /// Set while a conversion asked for the standard faces only.
    static STANDARD_ONLY: Cell<bool> = const { Cell::new(false) };
}

/// Run `f` with the standard-14 faces only (no embedding), on this thread.
pub(crate) fn standard_fonts_only<T>(on: bool, f: impl FnOnce() -> T) -> T {
    /// Puts the flag back even if `f` unwinds (conversions run under a panic guard).
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            STANDARD_ONLY.with(|s| s.set(self.0));
        }
    }
    let _restore = Restore(STANDARD_ONLY.with(|s| s.replace(on)));
    f()
}

/// The slot of a face and style, when the face is not `Std`.
pub(crate) fn slot(face: Face, bold: bool, italic: bool) -> Option<usize> {
    if let Face::Doc(i) = face {
        return Some(DOC_BASE + usize::from(i) * 4 + usize::from(bold) * 2 + usize::from(italic));
    }
    let i = face.index().checked_sub(1)?;
    Some(i * 4 + usize::from(bold) * 2 + usize::from(italic))
}

/// The loaded face in `slot`, if craft-fonts has it and embedding is on.
pub(crate) fn by_slot(slot: usize) -> Option<&'static Loaded> {
    if STANDARD_ONLY.with(Cell::get) {
        return None;
    }
    if let Some(i) = slot.checked_sub(DOC_BASE) {
        // A document font's style, or its regular style for a style it doesn't embed.
        return DOC_FACES.with(|d| d.borrow().get(i / 4).and_then(|(_, styles)| styles.get(i % 4).copied().flatten().or(styles[0])));
    }
    let cell = LOADED.get(slot)?;
    cell.get_or_init(|| {
        if slot == ARABIC || slot == HEBREW {
            let font = if slot == ARABIC { pdfcraft_fonts::document_arabic_font() } else { pdfcraft_fonts::document_hebrew_font(false) }?;
            let mut l = Loaded::load(font.bytes, font.family, font.style)?;
            l.pua = Some(if slot == ARABIC { PUA_ARABIC } else { PUA_HEBREW });
            return Some(l);
        }
        if let Some((family, style)) = slot.checked_sub(SLOTS).and_then(|i| FALLBACKS.get(i)) {
            let f = pdfcraft_fonts::CRAFT_FONTS.iter().find(|f| f.family == *family && f.style == *style)?;
            return Loaded::load(f.bytes, family, style);
        }
        let face = FAMILIES.get(slot / 4).copied()?;
        let family = face.family()?;
        let style = match slot % 4 {
            0 => "Regular",
            1 => "Italic",
            2 => "Bold",
            _ => "Bold Italic",
        };
        let f = pdfcraft_fonts::CRAFT_FONTS.iter().find(|f| f.family == family && f.style == style)?;
        Loaded::load(f.bytes, family, style)
    })
    .as_ref()
}

/// A font a document embeds: its lower-case name and its regular, italic, bold and bold italic
/// styles.
pub(crate) type DocFont = (String, [Option<&'static Loaded>; 4]);

/// The first slot of the document's own fonts: four a font, regular, italic, bold, bold italic.
const DOC_BASE: usize = 1_000;
/// Most bytes of document fonts kept in a process.
const MAX_DOC_FONT_BYTES: usize = 64 << 20;

thread_local! {
    /// The fonts the document being converted embeds: lower-case name and its four styles.
    static DOC_FACES: std::cell::RefCell<Vec<DocFont>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Fonts read from documents, by a hash of their bytes, and the bytes kept so far. Kept for the
/// process (a font is read once however many documents embed it), up to [`MAX_DOC_FONT_BYTES`].
static DOC_FONTS: Mutex<Option<(usize, HashMap<u64, &'static Loaded>)>> = Mutex::new(None);

/// Read a font a document embeds (`bytes`, a TrueType program) for drawing: `None` when it isn't
/// TrueType, its licence forbids embedding (restricted or bitmap-only `fsType`), it can't be
/// subset, or the process keeps too many document fonts already.
pub(crate) fn document_font(bytes: Vec<u8>, family: &str, style: &str) -> Option<&'static Loaded> {
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3));
    let mut guard = DOC_FONTS.lock().ok()?;
    let (total, map) = guard.get_or_insert_with(|| (0, HashMap::new()));
    if let Some(l) = map.get(&hash) {
        return Some(*l);
    }
    if *total + bytes.len() > MAX_DOC_FONT_BYTES {
        return None;
    }
    {
        let font = FontRef::new(&bytes).ok()?;
        let fs_type = font.os2().ok().map_or(0, |o| o.fs_type());
        if fs_type & 0x000F == 0x0002 || fs_type & 0x0200 != 0 {
            return None;
        }
        let keep = (0u16..4).collect();
        super::ttf::subset(&bytes, &keep)?;
    }
    *total += bytes.len();
    let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let loaded: &'static Loaded = Box::leak(Box::new(Loaded::load(bytes, family, style)?));
    map.insert(hash, loaded);
    Some(loaded)
}

/// Whether faces are embedded at all (not [`standard_fonts_only`]).
pub(crate) fn embedding_on() -> bool {
    !STANDARD_ONLY.with(Cell::get)
}

/// Run `f` with `fonts` (lower-case name, four styles) as the document's own fonts.
pub(crate) fn with_document_fonts<T>(fonts: Vec<DocFont>, f: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            DOC_FACES.with(|d| d.borrow_mut().clear());
        }
    }
    let fonts: Vec<_> = fonts.into_iter().take(250).collect();
    DOC_FACES.with(|d| *d.borrow_mut() = fonts);
    let _reset = Reset;
    f()
}

/// The face a run is drawn in, when it is embedded.
pub(crate) fn loaded(face: Face, bold: bool, italic: bool) -> Option<(usize, &'static Loaded)> {
    let s = slot(face, bold, italic)?;
    Some((s, by_slot(s)?))
}

/// The embedded face that draws characters a standard-14 face can't, for its family: Liberation
/// Sans, Serif or Mono.
pub(crate) fn fallback(fam: super::Fam, bold: bool, italic: bool) -> Option<(usize, &'static Loaded)> {
    let face = match fam {
        super::Fam::Sans => Face::LiberationSans,
        super::Fam::Serif => Face::LiberationSerif,
        super::Fam::Mono => Face::LiberationMono,
    };
    loaded(face, bold, italic)
}

/// The fallback face that draws `c` when a run's own face can't, if any.
pub(crate) fn script_fallback(c: char) -> Option<(usize, &'static Loaded)> {
    match u32::from(c) {
        PUA_ARABIC..PUA_HEBREW => return by_slot(ARABIC).map(|l| (ARABIC, l)),
        PUA_HEBREW..=0xF_FFFD => return by_slot(HEBREW).map(|l| (HEBREW, l)),
        _ => {}
    }
    // Right-to-left and shaped scripts stay escaped: they would be drawn in the wrong order or
    // with the wrong letter forms.
    if matches!(u32::from(c), 0x0590..=0x08FF | 0x0900..=0x0DFF | 0x0E00..=0x0EFF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF) {
        return None;
    }
    (SLOTS..SLOTS + FALLBACKS.len()).find_map(|s| by_slot(s).filter(|l| l.covers(c)).map(|l| (s, l)))
}

/// Whether `c` is written right to left in Arabic or Hebrew script.
/// Whether `c` is right-to-left text: Arabic or Hebrew, or a glyph [`shape_rtl`] stands in
/// for them.
pub(crate) fn is_rtl(c: char) -> bool {
    u32::from(c) >= PUA_ARABIC || rtl_script(c).is_some()
}

fn rtl_script(c: char) -> Option<bool> {
    match u32::from(c) {
        0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => Some(true),
        0x0590..=0x05FF | 0xFB1D..=0xFB4F => Some(false),
        _ => None,
    }
}

/// `s` with each run of Arabic or Hebrew text (and the spaces and punctuation inside it) shaped
/// by craft-fonts' face for it and laid out right to left, as private-use characters standing
/// for the glyphs (spaces stay spaces so lines can break there). Unchanged without the faces or
/// embedding.
pub(crate) fn shape_rtl(s: &str) -> String {
    if !s.chars().any(|c| rtl_script(c).is_some()) || STANDARD_ONLY.with(Cell::get) {
        return s.to_owned();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while let Some(&c) = chars.get(i) {
        if rtl_script(c).is_none() {
            out.push(c);
            i += 1;
            continue;
        }
        // The run goes on through spaces, neutral punctuation and numbers followed by more of the
        // script: a number between right-to-left words stays between them.
        let mut end = i;
        let mut j = i;
        while let Some(&d) = chars.get(j) {
            if rtl_script(d).is_some() {
                end = j + 1;
            } else if !(d == ' ' || d.is_ascii_punctuation() || is_digit(d) || matches!(d, '\u{060C}' | '\u{061B}' | '\u{061F}' | '\u{06D4}')) {
                break;
            }
            j += 1;
        }
        let run: String = chars.get(i..end).unwrap_or_default().iter().collect();
        let arabic = run.chars().any(|c| rtl_script(c) == Some(true));
        let (slot, base) = if arabic { (ARABIC, PUA_ARABIC) } else { (HEBREW, PUA_HEBREW) };
        let shaped = if by_slot(slot).is_none() {
            None
        } else if arabic {
            pdfcraft_fonts::shape_arabic(&run, true).ok()
        } else {
            pdfcraft_fonts::shape_hebrew(&run, true).ok()
        };
        match shaped {
            Some(mut clusters) => {
                // Shaping sets the run right to left, numbers too: each number's digits are put
                // back to read left to right.
                let numeric = |t: &str| !t.is_empty() && t.chars().all(|c| is_digit(c) || matches!(c, '.' | ',' | '\u{066B}' | '\u{066C}'));
                let mut k = 0usize;
                while k < clusters.len() {
                    if !clusters[k].text.chars().any(is_digit) {
                        k += 1;
                        continue;
                    }
                    let mut e = k;
                    while e + 1 < clusters.len() && numeric(&clusters[e + 1].text) {
                        e += 1;
                    }
                    clusters[k..=e].reverse();
                    k = e + 1;
                }
                let mut texts = SHAPED_TEXT.lock().ok();
                for cl in clusters {
                    if cl.text == " " {
                        out.push(' ');
                        continue;
                    }
                    for (k, (gid, _)) in cl.glyphs.iter().enumerate() {
                        let Some(pc) = gid.checked_add(base).filter(|_| *gid < PUA_SPAN).and_then(char::from_u32) else { continue };
                        out.push(pc);
                        if let Some(map) = texts.as_mut().map(|t| t.get_or_insert_with(HashMap::new)) {
                            map.entry(u32::from(pc)).or_insert_with(|| if k == 0 { cl.text.clone() } else { String::new() });
                        }
                    }
                }
            }
            None => out.push_str(&run),
        }
        i = end;
    }
    out
}

/// A digit: ASCII, Arabic-Indic or extended Arabic-Indic.
fn is_digit(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '\u{0660}'..='\u{0669}' | '\u{06F0}'..='\u{06F9}')
}

/// The text a shaped glyph's private-use character shows, for `/ToUnicode`.
pub(crate) fn shaped_text(c: char) -> Option<String> {
    let code = u32::from(c);
    if !(PUA_ARABIC..=0xF_FFFD).contains(&code) {
        return None;
    }
    SHAPED_TEXT.lock().ok()?.as_ref()?.get(&code).cloned()
}

/// Whether every embedded face covers `c`, so any run can draw it. False without craft-fonts.
pub(crate) fn everyone_draws(c: char) -> bool {
    if script_fallback(c).is_some() && fallback(super::Fam::Sans, false, false).is_some() {
        return true;
    }
    // The five always shipped together; the optional twins (Gelasio, Selawik, Narrow) are
    // only used for their own fonts and don't decide what every run can draw.
    let faces = [Face::Carlito, Face::Caladea, Face::LiberationSans, Face::LiberationSerif, Face::LiberationMono];
    let mut any = false;
    for f in faces {
        for (bold, italic) in [(false, false), (true, false), (false, true), (true, true)] {
            match loaded(f, bold, italic) {
                Some((_, l)) if l.covers(c) => any = true,
                Some(_) => return false,
                None => {}
            }
        }
    }
    // The standard faces' fallbacks must all be there too.
    any && [super::Fam::Sans, super::Fam::Serif, super::Fam::Mono].iter().all(|f| fallback(*f, false, false).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn office_names_map_to_their_metric_twins() {
        assert_eq!(face_of("Calibri"), Face::Carlito);
        assert_eq!(face_of("Calibri Light"), Face::Carlito);
        assert_eq!(face_of("Cambria Math"), Face::Caladea);
        assert_eq!(face_of("Arial"), Face::LiberationSans);
        assert_eq!(face_of("Times New Roman"), Face::LiberationSerif);
        assert_eq!(face_of("Courier New"), Face::LiberationMono);
        assert_eq!(face_of("Georgia"), Face::Gelasio);
        assert_eq!(face_of("Segoe UI Semibold"), Face::Selawik);
        assert_eq!(face_of("Arial Narrow"), Face::LiberationSansNarrow);
        assert_eq!(face_of("Verdana"), Face::Std);
        assert_eq!(slot(Face::LiberationSansNarrow, false, false), Some(28));
        assert_eq!(slot(Face::Std, true, true), None);
        assert_eq!(slot(Face::LiberationMono, true, true), Some(19));
    }

    #[test]
    fn numbers_in_hebrew_text_keep_their_place_and_order() {
        // Only with craft-fonts' Hebrew face; without it the text is left as it is.
        if by_slot(HEBREW).is_none() {
            return;
        }
        let out = shape_rtl("אב 12 גד");
        let read: String = out.chars().map(|c| shaped_text(c).unwrap_or_else(|| c.to_string())).collect();
        assert_eq!(read.replace(' ', ""), "דג12בא", "the words right to left, the number between them left to right: {read}");
    }

    #[test]
    fn standard_only_turns_embedding_off_for_its_scope() {
        standard_fonts_only(true, || assert!(loaded(Face::Carlito, false, false).is_none()));
        // Without craft-fonts nothing loads either way; with it, Carlito loads again.
        let has = pdfcraft_fonts::CRAFT_FONTS.iter().any(|f| f.family == "Carlito" && f.style == "Regular");
        assert_eq!(loaded(Face::Carlito, false, false).is_some(), has);
    }
}
