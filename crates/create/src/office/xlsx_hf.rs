//! Excel page headers and footers (ECMA-376 Part 1 §18.3.1.46, `&` codes): left, centre and
//! right sections with page numbers, the page count, the date and time, the file and sheet
//! names, font, size, colour and style changes, and pictures (`&G`, from the sheet's
//! `legacyDrawingHF` VML drawing).

use std::collections::HashMap;

use pdfcraft_cos::ObjRef;

use super::canvas::Page;
use super::{Notes, RunStyle, family_of, hex_color};

/// Longest header or footer text read, in characters (Excel allows 255).
const MAX_CHARS: usize = 2_048;

/// A sheet's header and footer texts (`headerFooter`).
#[derive(Clone, Debug, Default)]
pub(crate) struct HeaderFooter {
    pub odd_header: Option<String>,
    pub odd_footer: Option<String>,
    pub even_header: Option<String>,
    pub even_footer: Option<String>,
    pub first_header: Option<String>,
    pub first_footer: Option<String>,
    pub different_first: bool,
    pub different_odd_even: bool,
}

impl HeaderFooter {
    /// The header and footer of a page: the first page's of its sheet, or by page number.
    pub(crate) fn pick(&self, first: bool, number: usize) -> (Option<&str>, Option<&str>, &'static str) {
        if first && self.different_first {
            (self.first_header.as_deref(), self.first_footer.as_deref(), "FIRST")
        } else if self.different_odd_even && number.is_multiple_of(2) {
            (self.even_header.as_deref(), self.even_footer.as_deref(), "EVEN")
        } else {
            (self.odd_header.as_deref(), self.odd_footer.as_deref(), "")
        }
    }
}

/// What a page's header and footer fields show.
pub(crate) struct Vars<'a> {
    pub page: usize,
    pub pages: usize,
    pub date: &'a str,
    pub time: &'a str,
    pub file: &'a str,
    pub sheet: &'a str,
}

/// Header and footer pictures by VML shape id (`LH`, `CF`, `RHFIRST`…): the image (or `None`
/// for a placeholder) and its size in points.
pub(crate) type Pictures = HashMap<String, (Option<ObjRef>, f64, f64)>;

#[derive(Clone, Debug)]
enum Item {
    Text(String, RunStyle),
    Image(Option<ObjRef>, f64, f64),
}

/// One section's lines.
type Lines = Vec<Vec<Item>>;

/// Split a header or footer text into its left, centre and right sections, with fields filled
/// in. `place` is `H` or `F`, `suffix` `FIRST`, `EVEN` or empty, for the picture names.
fn sections(src: &str, base: &RunStyle, vars: &Vars<'_>, pictures: &Pictures, place: char, suffix: &str, notes: &mut Notes) -> [Lines; 3] {
    let mut out: [Lines; 3] = [vec![Vec::new()], vec![Vec::new()], vec![Vec::new()]];
    let mut sec = 1usize;
    let mut style = base.clone();
    let mut text = String::new();
    let chars: Vec<char> = src.chars().take(MAX_CHARS).collect();
    // Move the pending text into the current line, in the style it was written in.
    let flush = |out: &mut [Lines; 3], sec: usize, text: &mut String, style: &RunStyle, notes: &mut Notes| {
        if text.is_empty() {
            return;
        }
        let t = notes.drawable(text);
        text.clear();
        if let Some(line) = out.get_mut(sec).and_then(|l| l.last_mut()) {
            line.push(Item::Text(t, style.clone()));
        }
    };
    let mut i = 0usize;
    while let Some(&c) = chars.get(i) {
        i += 1;
        if c == '\n' {
            flush(&mut out, sec, &mut text, &style, notes);
            if let Some(s) = out.get_mut(sec) {
                s.push(Vec::new());
            }
            continue;
        }
        if c != '&' {
            text.push(c);
            continue;
        }
        let Some(&code) = chars.get(i) else { break };
        i += 1;
        match code {
            '&' => text.push('&'),
            'L' | 'C' | 'R' => {
                flush(&mut out, sec, &mut text, &style, notes);
                sec = match code {
                    'L' => 0,
                    'C' => 1,
                    _ => 2,
                };
                style = base.clone();
            }
            'P' => text.push_str(&vars.page.to_string()),
            'N' => text.push_str(&vars.pages.to_string()),
            'D' => text.push_str(vars.date),
            'T' => text.push_str(vars.time),
            'F' => text.push_str(vars.file),
            'A' => text.push_str(vars.sheet),
            'Z' => {}
            'G' => {
                flush(&mut out, sec, &mut text, &style, notes);
                let side = ['L', 'C', 'R'].get(sec).copied().unwrap_or('C');
                let pic = pictures.get(&format!("{side}{place}{suffix}")).copied();
                if pic.is_none() {
                    notes.add("header and footer pictures that can't be found, left out");
                }
                if let (Some((r, w, h)), Some(line)) = (pic, out.get_mut(sec).and_then(|l| l.last_mut())) {
                    line.push(Item::Image(r, w, h));
                }
            }
            'B' | 'I' | 'U' | 'E' | 'S' | 'X' | 'Y' => {
                flush(&mut out, sec, &mut text, &style, notes);
                match code {
                    'B' => style.bold = !style.bold,
                    'I' => style.italic = !style.italic,
                    'U' | 'E' => style.underline = !style.underline,
                    'S' => style.strike = !style.strike,
                    'X' => style.vert = if style.vert == 1 { 0 } else { 1 },
                    _ => style.vert = if style.vert == -1 { 0 } else { -1 },
                }
            }
            '"' => {
                flush(&mut out, sec, &mut text, &style, notes);
                let mut spec = String::new();
                while let Some(&q) = chars.get(i) {
                    i += 1;
                    if q == '"' {
                        break;
                    }
                    spec.push(q);
                }
                let (name, kind) = spec.split_once(',').unwrap_or((spec.as_str(), ""));
                if name.trim() != "-" && !name.trim().is_empty() {
                    style.fam = family_of(name);
                }
                let kind = kind.to_ascii_lowercase();
                if !kind.is_empty() {
                    style.bold = kind.contains("bold");
                    style.italic = kind.contains("italic") || kind.contains("oblique");
                }
            }
            'K' => {
                flush(&mut out, sec, &mut text, &style, notes);
                // An RGB colour, or a theme colour (`ttSnnn`) whose shade isn't known here.
                let spec: String = chars.get(i..i.saturating_add(6).min(chars.len())).unwrap_or_default().iter().collect();
                i = i.saturating_add(spec.chars().count());
                if let Some(c) = hex_color(&spec) {
                    style.color = c;
                }
            }
            d if d.is_ascii_digit() => {
                flush(&mut out, sec, &mut text, &style, notes);
                let mut num = d.to_string();
                while let Some(&q) = chars.get(i).filter(|q| q.is_ascii_digit()) {
                    num.push(q);
                    i += 1;
                }
                if let Ok(v) = num.parse::<f64>() {
                    style.size = v.clamp(1.0, 400.0);
                }
            }
            _ => {}
        }
    }
    flush(&mut out, sec, &mut text, &style, notes);
    out
}

/// Width and height of a line, and its ascent (top to baseline).
fn metrics(line: &[Item]) -> (f64, f64, f64) {
    let (mut w, mut asc, mut desc) = (0.0, 0.0f64, 0.0f64);
    for item in line {
        match item {
            Item::Text(t, s) => {
                w += s.width(t);
                asc = asc.max(s.size * 0.905);
                desc = desc.max(s.size * 0.295);
            }
            Item::Image(_, iw, ih) => {
                w += iw;
                asc = asc.max(*ih);
            }
        }
    }
    (w, asc + desc, asc)
}

/// Where the header and footer go on a page `w` points wide.
pub(crate) struct Frame {
    pub w: f64,
    pub h: f64,
    pub left: f64,
    pub right: f64,
    /// From the page's top to the header's top, and from the bottom to the footer's bottom.
    pub header: f64,
    pub footer: f64,
}

/// Draw a header (`top`) or footer from its `&` code text.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw(
    page: &mut Page,
    src: &str,
    top: bool,
    frame: &Frame,
    base: &RunStyle,
    vars: &Vars<'_>,
    pictures: &Pictures,
    suffix: &str,
    notes: &mut Notes,
) {
    let secs = sections(src, base, vars, pictures, if top { 'H' } else { 'F' }, suffix, notes);
    for (si, lines) in secs.iter().enumerate() {
        let lines: Vec<&Vec<Item>> = lines.iter().collect();
        let total: f64 = lines.iter().map(|l| metrics(l).1).sum();
        if total <= 0.0 {
            continue;
        }
        let mut y = if top { frame.h - frame.header } else { frame.footer + total };
        for line in lines {
            let (w, h, asc) = metrics(line);
            let base_y = y - asc;
            let mut x = match si {
                0 => frame.left,
                1 => (frame.w - w) / 2.0,
                _ => frame.w - frame.right - w,
            };
            for item in line {
                match item {
                    Item::Text(t, s) => {
                        page.text(x, base_y, s, t);
                        x += s.width(t);
                    }
                    Item::Image(r, iw, ih) => {
                        match r {
                            Some(r) => page.image(*r, x, base_y, *iw, *ih),
                            None => page.placeholder(x, base_y, *iw, *ih),
                        }
                        x += iw;
                    }
                }
            }
            y -= h;
        }
    }
}

/// `width:78pt;height:21pt` from a VML style, in points.
pub(crate) fn vml_size(style: &str) -> (f64, f64) {
    let get = |key: &str| {
        style.split(';').find_map(|kv| {
            let (k, v) = kv.split_once(':')?;
            if k.trim() != key {
                return None;
            }
            let v = v.trim();
            let at = v.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(v.len());
            let x: f64 = v.get(..at)?.trim().parse().ok().filter(|x: &f64| x.is_finite())?;
            Some(match v.get(at..).unwrap_or("") {
                "in" => x * 72.0,
                "cm" => x * 72.0 / 2.54,
                "mm" => x * 72.0 / 25.4,
                "px" => x * 0.75,
                _ => x,
            })
        })
    };
    (get("width").unwrap_or(72.0).clamp(1.0, 2_000.0), get("height").unwrap_or(36.0).clamp(1.0, 2_000.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lines: &Lines) -> Vec<String> {
        lines.iter().map(|l| l.iter().filter_map(|i| if let Item::Text(t, _) = i { Some(t.as_str()) } else { None }).collect()).collect()
    }

    #[test]
    fn codes_fill_sections_fields_and_styles() {
        let vars = Vars { page: 3, pages: 9, date: "2026-01-02", time: "1:05 PM", file: "book.xlsx", sheet: "Data" };
        let mut notes = Notes::default();
        let mut pics = Pictures::new();
        pics.insert("LH".into(), (None, 50.0, 20.0));
        let src = "&L&G&C&\"Arial,Bold\"&14&KFF0000Title &&Co\n&A&R&P of &N · &D &T &F";
        let [l, c, r] = sections(src, &RunStyle::default(), &vars, &pics, 'H', "", &mut notes);
        assert!(matches!(l.first().and_then(|l| l.first()), Some(Item::Image(None, w, _)) if *w == 50.0));
        assert_eq!(texts(&c), ["Title &Co", "Data"]);
        let Some(Item::Text(_, s)) = c.first().and_then(|l| l.first()) else { panic!() };
        assert!(s.bold && s.size == 14.0 && s.color == [1.0, 0.0, 0.0]);
        assert_eq!(texts(&r), ["3 of 9 · 2026-01-02 1:05 PM book.xlsx"]);
        // Hostile input: unterminated font, colour and size codes, a lone ampersand.
        let _ = sections("&\"Arial&K12&99999999999&", &RunStyle::default(), &vars, &pics, 'F', "", &mut notes);
    }

    #[test]
    fn first_and_even_pages_pick_their_own() {
        let hf = HeaderFooter {
            odd_header: Some("odd".into()),
            even_header: Some("even".into()),
            first_header: Some("first".into()),
            different_first: true,
            different_odd_even: true,
            ..HeaderFooter::default()
        };
        assert_eq!(hf.pick(true, 4).0, Some("first"));
        assert_eq!(hf.pick(false, 4).0, Some("even"));
        assert_eq!(hf.pick(false, 5).0, Some("odd"));
    }
}
