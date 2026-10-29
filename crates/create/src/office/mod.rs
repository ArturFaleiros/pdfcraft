//! Office Open XML import: Word (`.docx`), PowerPoint (`.pptx`) and Excel (`.xlsx`) documents
//! become PDFs natively, without an office suite. Behaviour follows ECMA-376 (Office Open XML,
//! 5th edition) and the files' own markup; no other implementation was read.
//!
//! Text is set in the standard-14 faces (Helvetica, Times, Courier and their bold and italic
//! styles) chosen from each run's font family, so the result needs no embedded font program and
//! stays searchable. Latin letters beyond WinAnsi (Turkish, Central European, Baltic, Romanian)
//! are drawn from the same faces through a second encoding ([`latin`]). Characters those faces
//! can't draw at all are written as `[U+XXXX]` and counted in the [`OfficeReport`], never set as
//! `?`. EMF pictures are drawn as vectors ([`emf`]). Content the importer can't draw (charts,
//! SmartArt, WMF and SVG images) is replaced by an outlined box of the same size and counted too.

mod canvas;
mod chart;
mod docx;
mod emf;
mod faces;
mod flow;
mod latin;
mod math;
mod pptx;
mod ttf;
mod wmf;
mod xlsx;
mod xlsx_cf;
mod xlsx_drawing;
mod xlsx_formula;
mod xlsx_hf;
mod xml;
pub(crate) mod zip;

#[cfg(test)]
mod tests;

use pdfcraft_cos::Document;
use pdfcraft_fonts::Std14;

use crate::CreateError;

/// Extensions of the Office Open XML files [`from_office`] converts. The package's content, not
/// the extension, decides how it is read.
pub const OFFICE_EXTENSIONS: [&str; 10] = ["docx", "docm", "dotx", "pptx", "pptm", "ppsx", "potx", "xlsx", "xlsm", "xltx"];

/// Which application's document a package holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfficeKind {
    Word,
    PowerPoint,
    Excel,
}

impl OfficeKind {
    pub fn label(self) -> &'static str {
        match self {
            OfficeKind::Word => "Word",
            OfficeKind::PowerPoint => "PowerPoint",
            OfficeKind::Excel => "Excel",
        }
    }
}

/// What the conversion could not carry over exactly. Nothing in the source is changed.
#[derive(Clone, Debug, PartialEq)]
pub struct OfficeReport {
    pub kind: OfficeKind,
    pub pages: usize,
    /// Characters the standard fonts can't draw, written as `[U+XXXX]`.
    pub escaped: usize,
    /// One line per kind of content that was approximated or left out, with how many.
    pub notes: Vec<String>,
    /// How many metric-compatible faces (Carlito, Caladea, Liberation) were embedded; 0 when
    /// text is set in the standard PDF faces only.
    pub fonts_embedded: usize,
}

/// How [`from_office_with`] converts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OfficeOptions {
    /// Embed the metric-compatible faces of the craft-fonts build input for Calibri, Cambria,
    /// Arial, Times New Roman and Courier New (when the build has them). Off sets every run in
    /// the standard PDF faces.
    pub embed_fonts: bool,
}

impl Default for OfficeOptions {
    fn default() -> Self {
        OfficeOptions { embed_fonts: true }
    }
}

/// Tallies of approximations, turned into [`OfficeReport::notes`].
#[derive(Default)]
pub(crate) struct Notes {
    escaped: usize,
    /// Embedded faces, set by the converter from its canvas.
    pub(crate) fonts: usize,
    counts: Vec<(&'static str, usize)>,
}

impl Notes {
    pub(crate) fn add(&mut self, what: &'static str) {
        match self.counts.iter_mut().find(|(w, _)| *w == what) {
            Some((_, n)) => *n = n.saturating_add(1),
            None => self.counts.push((what, 1)),
        }
    }

    fn report(self, kind: OfficeKind, pages: usize) -> OfficeReport {
        let mut notes: Vec<String> = self.counts.into_iter().map(|(what, n)| format!("{what}: {n}")).collect();
        if self.escaped > 0 {
            notes.insert(0, format!("characters the built-in fonts can't draw, written as [U+XXXX]: {}", self.escaped));
        }
        OfficeReport { kind, pages, escaped: self.escaped, notes, fonts_embedded: self.fonts }
    }

    /// `s` with every character the standard fonts can't draw written as `[U+XXXX]`, symbol-font
    /// private-use bullets mapped to their usual shapes, and invisible format characters dropped.
    pub(crate) fn drawable(&mut self, s: &str) -> String {
        // Arabic and Hebrew are shaped and set right to left first, when craft-fonts has them.
        let s = faces::shape_rtl(s);
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            // An embedded face draws the character itself (a true minus, say): keep it.
            if !c.is_ascii()
                && !c.is_control()
                && faces::everyone_draws(c)
                && !matches!(c, '\u{00AD}' | '\u{200B}'..='\u{200F}' | '\u{2060}' | '\u{FEFF}' | '\u{F000}'..='\u{F0FF}')
            {
                out.push(c);
                continue;
            }
            let c = match c {
                // Zero-width and soft-hyphen characters draw nothing.
                '\u{00AD}' | '\u{200B}'..='\u{200F}' | '\u{2060}' | '\u{FEFF}' => continue,
                // Symbol / Wingdings bullets stored in the private use area (U+F0xx).
                '\u{F0B7}' | '\u{F06C}' | '\u{F0A7}' | '\u{F0D8}' | '\u{F0FC}' | '\u{F076}' | '\u{F0A8}' | '\u{25CF}' | '\u{25AA}' | '\u{25A0}' => {
                    '•'
                }
                '\u{F0E8}' | '\u{F0E0}' | '\u{27A2}' | '\u{25BA}' | '\u{25B6}' => '>',
                '\u{2013}'..='\u{2014}'
                | '\u{2018}'..='\u{201E}'
                | '\u{2020}'..='\u{2022}'
                | '\u{2026}'
                | '\u{2030}'
                | '\u{2039}'
                | '\u{203A}'
                | '\u{20AC}' => c,
                '\u{2010}' | '\u{2011}' | '\u{2212}' => '-',
                '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' => ' ',
                '\u{25E6}' | '\u{2219}' => '·',
                '\u{2713}' | '\u{2714}' => 'x',
                c => c,
            };
            if pdfcraft_fonts::win_ansi_byte(c).is_some() || c == ' ' || latin::code(c).is_some() || faces::everyone_draws(c) {
                out.push(c);
            } else if c.is_control() {
                // Other control characters carry no ink.
            } else {
                self.escaped = self.escaped.saturating_add(1);
                out.push_str(&format!("[U+{:04X}]", u32::from(c)));
            }
        }
        out
    }
}

/// A standard-14 family chosen from a document font name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fam {
    Sans,
    Serif,
    Mono,
}

/// The standard family closest to a font name: serif faces become Times, monospaced faces Courier,
/// everything else (Calibri, Arial, Aptos, Segoe…) Helvetica.
pub(crate) fn family_of(name: &str) -> Fam {
    let n = name.to_ascii_lowercase();
    // Code faces by name ("Fira Code", "Source Code Pro"): a bare "code" would match "Unicode".
    const MONO: [&str; 12] = [
        "courier",
        "consolas",
        "mono",
        "menlo",
        "lucida console",
        "source code",
        "fira code",
        "cascadia code",
        "typewriter",
        "fixedsys",
        "terminal",
        "code new roman",
    ];
    const SERIF: [&str; 16] = [
        "times",
        "georgia",
        "garamond",
        "cambria",
        "book",
        "palatino",
        "century",
        "constantia",
        "baskerville",
        "minion",
        "roman",
        "serif",
        "bodoni",
        "didot",
        "caslon",
        "liberation serif",
    ];
    // "Monotype" is a foundry ("Monotype Corsiva"), not a code face: it is left out of the check.
    let unfounded = n.replace("monotype", "");
    if MONO.iter().any(|m| unfounded.contains(m)) {
        Fam::Mono
    } else if SERIF.iter().any(|s| n.contains(s)) && !n.contains("sans") && !n.contains("gothic") {
        Fam::Serif
    } else {
        Fam::Sans
    }
}

/// Where a link goes: a web or mail address, or a named place in the document (a Word bookmark,
/// an Excel sheet).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LinkTarget {
    Uri(String),
    Anchor(String),
}

/// How a run of text looks.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RunStyle {
    pub fam: Fam,
    pub bold: bool,
    pub italic: bool,
    /// Points.
    pub size: f64,
    pub color: [f64; 3],
    pub underline: bool,
    pub strike: bool,
    pub highlight: Option<[f64; 3]>,
    /// 1 superscript, -1 subscript, 0 baseline.
    pub vert: i8,
    pub caps: bool,
    pub hidden: bool,
    /// The metric-compatible face embedded for the run's font, when craft-fonts has it.
    pub font: faces::Face,
    /// The link the run is part of: its text becomes a clickable area.
    pub link: Option<std::sync::Arc<LinkTarget>>,
}

impl Default for RunStyle {
    fn default() -> Self {
        RunStyle {
            fam: Fam::Sans,
            bold: false,
            italic: false,
            size: 11.0,
            color: [0.0; 3],
            underline: false,
            strike: false,
            highlight: None,
            vert: 0,
            caps: false,
            hidden: false,
            font: faces::Face::Std,
            link: None,
        }
    }
}

impl RunStyle {
    pub(crate) fn face(&self) -> Std14 {
        match (self.fam, self.bold, self.italic) {
            (Fam::Sans, false, false) => Std14::Helvetica,
            (Fam::Sans, true, false) => Std14::HelveticaBold,
            (Fam::Sans, false, true) => Std14::HelveticaOblique,
            (Fam::Sans, true, true) => Std14::HelveticaBoldOblique,
            (Fam::Serif, false, false) => Std14::TimesRoman,
            (Fam::Serif, true, false) => Std14::TimesBold,
            (Fam::Serif, false, true) => Std14::TimesItalic,
            (Fam::Serif, true, true) => Std14::TimesBoldItalic,
            (Fam::Mono, false, false) => Std14::Courier,
            (Fam::Mono, true, false) => Std14::CourierBold,
            (Fam::Mono, false, true) => Std14::CourierOblique,
            (Fam::Mono, true, true) => Std14::CourierBoldOblique,
        }
    }

    /// The size glyphs are drawn at (super- and subscripts are smaller).
    pub(crate) fn draw_size(&self) -> f64 {
        let s = if self.vert != 0 { self.size * 0.65 } else { self.size };
        if s.is_finite() { s.clamp(1.0, 400.0) } else { 11.0 }
    }

    /// Width of already-drawable text in points: in the embedded face when there is one,
    /// else in the standard face (characters it lacks in its embedded stand-in).
    pub(crate) fn width(&self, s: &str) -> f64 {
        let size = self.draw_size();
        if let Some((_, l)) = faces::loaded(self.font, self.bold, self.italic) {
            let space = l.glyph(' ').map_or(250.0, |g| g.1);
            return s
                .chars()
                .map(|c| {
                    let c = if c == '\t' { ' ' } else { c };
                    l.glyph(c).or_else(|| faces::script_fallback(c).and_then(|(_, f)| f.glyph(c))).map_or(space, |g| g.1)
                })
                .sum::<f64>()
                * size
                / 1000.0;
        }
        let face = self.face();
        if s.chars().all(|c| latin::code(c).is_none() && (c.is_ascii() || pdfcraft_fonts::win_ansi_byte(c).is_some())) {
            return face.text_width(s, size);
        }
        let fallback = faces::fallback(self.fam, self.bold, self.italic).map(|(_, l)| l);
        s.chars()
            .map(|c| {
                if pdfcraft_fonts::win_ansi_byte(c).is_some() || c == '\t' || latin::code(c).is_some() {
                    face.text_width(&latin::measured_as(c).to_string(), size)
                } else {
                    fallback
                        .and_then(|l| l.glyph(c))
                        .or_else(|| faces::script_fallback(c).and_then(|(_, f)| f.glyph(c)))
                        .map_or_else(|| face.text_width("?", size), |g| g.1 * size / 1000.0)
                }
            })
            .sum()
    }

    /// The line's ascent, descent and gap per point of size: the embedded face's `hhea`
    /// metrics, or the standard faces' (Helvetica's) proportions.
    pub(crate) fn line_metrics(&self) -> (f64, f64, f64) {
        faces::loaded(self.font, self.bold, self.italic).map_or((0.905, 0.212, 0.033), |(_, l)| l.line)
    }
}

/// A hex `RRGGBB` colour (an optional leading `#` or alpha byte `AARRGGBB` is ignored).
pub(crate) fn hex_color(v: &str) -> Option<[f64; 3]> {
    let v = v.trim().trim_start_matches('#');
    let v = if v.len() == 8 { v.get(2..)? } else { v };
    if v.len() != 6 || !v.is_ascii() {
        return None;
    }
    let ch = |i: usize| u8::from_str_radix(v.get(i..i + 2)?, 16).ok().map(|b| f64::from(b) / 255.0);
    Some([ch(0)?, ch(2)?, ch(4)?])
}

/// Which application a package belongs to, from its main part's root element.
fn detect(zip: &zip::Zip<'_>) -> Result<(OfficeKind, String), String> {
    let rels = xml::Rels::load(zip, "")?;
    let main = rels
        .of_type("officeDocument")
        .map(str::to_owned)
        .or_else(|| ["word/document.xml", "ppt/presentation.xml", "xl/workbook.xml"].into_iter().find(|p| zip.has(p)).map(str::to_owned))
        .ok_or("the package has no main document part")?;
    let text = zip.text(&main)?.ok_or_else(|| format!("{main} is missing"))?;
    let doc = xml::parse(&text).map_err(|e| format!("{main}: {e}"))?;
    let kind = match doc.root_element().tag_name().name() {
        "document" => OfficeKind::Word,
        "presentation" => OfficeKind::PowerPoint,
        "workbook" => OfficeKind::Excel,
        other => return Err(format!("the main part is a <{other}>, not a Word, PowerPoint or Excel document")),
    };
    Ok((kind, main))
}

/// Convert a Word, PowerPoint or Excel (Office Open XML) file to a new PDF. `name` is used for
/// messages and the document title. Returns the document and what could not be carried over.
pub fn from_office(name: &str, bytes: &[u8]) -> Result<(Document, OfficeReport), CreateError> {
    from_office_with(name, bytes, OfficeOptions::default())
}

/// [`from_office`] with options.
pub fn from_office_with(name: &str, bytes: &[u8], options: OfficeOptions) -> Result<(Document, OfficeReport), CreateError> {
    faces::standard_fonts_only(!options.embed_fonts, || convert_office(name, bytes))
}

fn convert_office(name: &str, bytes: &[u8]) -> Result<(Document, OfficeReport), CreateError> {
    let fail = |m: String| CreateError::Image(name.to_string(), m);
    let zip = zip::Zip::open(bytes).map_err(fail)?;
    let (kind, main) = detect(&zip).map_err(fail)?;
    let mut doc = Document::new_empty();
    let mut notes = Notes::default();
    let pages = match kind {
        OfficeKind::Word => docx::convert(&zip, &main, &mut doc, &mut notes),
        OfficeKind::PowerPoint => pptx::convert(&zip, &main, &mut doc, &mut notes),
        OfficeKind::Excel => xlsx::convert(&zip, &main, name, &mut doc, &mut notes),
    }
    .map_err(fail)?;
    crate::set_title(&mut doc, name.rsplit_once('.').map_or(name, |(s, _)| s));
    Ok((doc, notes.report(kind, pages)))
}

impl OfficeReport {
    /// Font substitutions can change pagination even when no content-specific note is needed.
    pub fn notice(&self) -> String {
        let mut notice = if self.fonts_embedded > 0 {
            "Office import sets Calibri, Cambria, Arial, Times New Roman and Courier New in metric-compatible open faces; other fonts use standard PDF fonts, so layout may differ. Review the converted pages.".to_owned()
        } else {
            "Office import uses standard PDF fonts; layout may differ. Review the converted pages.".to_owned()
        };
        if !self.notes.is_empty() {
            notice.push(' ');
            notice.push_str(&self.notes.join("; "));
        }
        notice
    }
}
