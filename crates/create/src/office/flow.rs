//! Paragraph layout shared by the Word and PowerPoint importers: runs of styled text, tabs, line
//! breaks and inline images are broken into lines (greedy, at spaces and after hyphens), aligned,
//! and drawn. Measurements use the standard-14 metrics the text is drawn with, so what is measured
//! is what is drawn.

use std::sync::Arc;

use pdfcraft_cos::ObjRef;

use super::chart::{self, Chart};

use super::RunStyle;
use super::canvas::{Page, n};

/// Default tab stops every half inch, as Word and PowerPoint use.
const DEFAULT_TAB: f64 = 36.0;
/// Most lines one paragraph may lay out to.
const MAX_LINES: usize = 50_000;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Field {
    /// The current page (or slide) number, in a numbering format (`lowerRoman`…) when the field
    /// names one (`\* roman`).
    Page(Option<&'static str>),
    /// The number of pages.
    Pages,
    /// The number of pages in the section (the number of pages where sections aren't known).
    SectionPages,
    /// The text of the nearest paragraph in a style (`STYLEREF`, by style id): resolved for each
    /// page before headers are laid out, empty elsewhere.
    StyleRef(Arc<str>),
}

#[derive(Clone, Debug)]
pub(crate) enum Inline {
    /// Drawable text (see `Notes::drawable`); no tabs or line breaks inside.
    Text(String, RunStyle),
    Tab,
    Break,
    /// An image of `w × h` points; `None` draws a placeholder box.
    Image(Option<ObjRef>, f64, f64),
    /// A cropped image of `w × h` points (see [`Pic::Cropped`]).
    Cropped(ObjRef, [f64; 4], f64, f64),
    /// A chart of `w × h` points.
    Chart(Arc<Chart>, f64, f64),
    /// Shapes, text boxes and pictures drawn together (a shape, a group or a drawing canvas).
    Shapes(Arc<Vec<ShapePart>>, f64, f64),
    /// An equation, sitting on the line's baseline.
    Math(Arc<super::math::MathBox>),
    Field(Field, RunStyle),
}

/// How text lines up at a tab stop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TabKind {
    Left,
    Center,
    Right,
    /// The first `.` of the text after the tab sits at the stop (right-aligned without one).
    Decimal,
}

/// A custom tab stop, in points from the text area's left edge, with the character that fills
/// the gap before it (dot leaders in a table of contents).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TabStop {
    pub pos: f64,
    pub kind: TabKind,
    pub leader: Option<char>,
}

impl TabStop {
    pub(crate) fn left(pos: f64) -> Self {
        TabStop { pos, kind: TabKind::Left, leader: None }
    }
}

/// One side of a paragraph's border: its width, colour and distance from the text, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rule {
    pub width: f64,
    pub color: [f64; 3],
    pub space: f64,
    pub double: bool,
    pub dash: LineStyle,
}

/// How a border line is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum LineStyle {
    #[default]
    Solid,
    Dotted,
    Dashed,
    DashDot,
    DashDotDot,
    Wave,
}

impl LineStyle {
    /// A Word border style's (`w:val`) dashes.
    pub(crate) fn of(val: &str) -> LineStyle {
        match val {
            "dotted" => LineStyle::Dotted,
            "dashed" | "dashSmallGap" => LineStyle::Dashed,
            "dotDash" | "dashDotStroked" => LineStyle::DashDot,
            "dotDotDash" => LineStyle::DashDotDot,
            "wave" | "doubleWave" => LineStyle::Wave,
            _ => LineStyle::Solid,
        }
    }
}

/// Stroke a border line in its dash style: dashes in proportion to its width, a wave as a
/// zigzag of small curves along it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stroke(page: &mut Page, x1: f64, y1: f64, x2: f64, y2: f64, width: f64, color: [f64; 3], dash: LineStyle) {
    let w = width.max(0.25);
    let pattern = match dash {
        LineStyle::Solid => return page.line(x1, y1, x2, y2, width, color),
        LineStyle::Dotted => format!("[{} {}]", n(w), n(w * 2.0)),
        LineStyle::Dashed => format!("[{} {}]", n(w * 6.0), n(w * 3.0)),
        LineStyle::DashDot => format!("[{} {} {} {}]", n(w * 6.0), n(w * 2.0), n(w), n(w * 2.0)),
        LineStyle::DashDotDot => format!("[{} {} {} {} {} {}]", n(w * 6.0), n(w * 2.0), n(w), n(w * 2.0), n(w), n(w * 2.0)),
        LineStyle::Wave => {
            let len = (x2 - x1).hypot(y2 - y1);
            if !len.is_finite() || len < 0.1 {
                return;
            }
            // Unit vectors along and across the line.
            let (ux, uy) = ((x2 - x1) / len, (y2 - y1) / len);
            let (vx, vy) = (-uy, ux);
            let step = (w * 3.0).max(1.5);
            let amp = (w * 1.5).max(0.75);
            let mut path = format!("q {} {} {} RG {} w {} {} m ", n(color[0]), n(color[1]), n(color[2]), n(w), n(x1), n(y1));
            let mut at = 0.0;
            let mut up = 1.0;
            while at < len && path.len() < 1 << 20 {
                let next = (at + step).min(len);
                let mid = (at + next) / 2.0;
                let (cx, cy) = (x1 + ux * mid + vx * amp * up, y1 + uy * mid + vy * amp * up);
                let (ex, ey) = (x1 + ux * next, y1 + uy * next);
                path.push_str(&format!("{} {} {} {} {} {} c ", n(cx), n(cy), n(cx), n(cy), n(ex), n(ey)));
                at = next;
                up = -up;
            }
            path.push_str("S Q\n");
            return page.raw(&path);
        }
    };
    page.dashed_line(x1, y1, x2, y2, width, color, &pattern);
}

/// A paragraph's borders. Consecutive paragraphs with the same borders share one box, with
/// `between` drawn between them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ParaBorders {
    pub top: Option<Rule>,
    pub bottom: Option<Rule>,
    pub left: Option<Rule>,
    pub right: Option<Rule>,
    pub between: Option<Rule>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Align {
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LineRule {
    /// A multiple of single spacing.
    Auto(f64),
    Exact(f64),
    AtLeast(f64),
}

#[derive(Clone, Debug)]
pub(crate) struct Para {
    pub items: Vec<Inline>,
    pub align: Align,
    /// Indents in points from the text area's edges; `first` is added to `left` on the first line
    /// (negative for a hanging indent).
    pub left: f64,
    pub right: f64,
    pub first: f64,
    pub before: f64,
    pub after: f64,
    pub line: LineRule,
    /// Custom tab stops.
    pub tabs: Vec<TabStop>,
    /// A list number or bullet, drawn at the start of the first line.
    pub label: Option<(String, RunStyle)>,
    /// The paragraph mark's style: the height of an empty paragraph.
    pub base: RunStyle,
    pub page_break_before: bool,
    pub shade: Option<[f64; 3]>,
    /// Boxed: most paragraphs have none, and a block list holds many paragraphs.
    pub borders: Option<Box<ParaBorders>>,
    /// Keep on the same page as the next paragraph (headings).
    pub keep_next: bool,
    /// With contextual spacing, the style whose neighbours' spacing between them is dropped.
    pub contextual: Option<String>,
    /// Footnotes the paragraph references (by the converter's numbering), each with the item
    /// of its reference mark: a note goes at the foot of the page its mark's line is placed on.
    pub notes: Vec<(usize, usize)>,
    /// Pictures and charts anchored to the paragraph rather than set in its lines.
    pub floats: Vec<Floating>,
    /// The paragraph style's id, for style references.
    pub style: Option<Arc<str>>,
    /// Widow and orphan control: a paragraph's first or last line is not left alone on a page.
    pub widow: bool,
    /// Keep the paragraph's lines on one page.
    pub keep_lines: bool,
    /// Left out of line numbering.
    pub no_line_numbers: bool,
    /// A drop cap (`w:framePr w:dropCap`): in the margin (`true`) or dropped into the text, and
    /// how many lines it spans. It floats beside the paragraph after it.
    pub drop_cap: Option<(bool, u8, f64)>,
    /// A heading's outline level (0 at the top): it becomes a bookmark.
    pub outline: Option<u8>,
    /// Bookmarks (named places) that start in the paragraph, for links to go to.
    pub anchors: Vec<String>,
    /// A right-to-left paragraph: its lines read from the right, the first words on the first
    /// line (right-to-left runs already in visual order).
    pub rtl: bool,
}

/// How text goes around a floating picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Wrap {
    /// In front of or behind the text: it takes no room.
    None,
    /// Text above and below it only.
    TopBottom,
    /// Text beside it.
    Square,
}

/// Where a floating picture's edge is: aligned (0 start, 0.5 centre, 1 end) or at an offset in
/// points, within the text area (or column) or the whole page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Place {
    Align(f64, bool),
    Offset(f64, bool),
}

/// A picture or chart anchored to a paragraph (`wp:anchor`).
#[derive(Clone, Debug)]
pub(crate) struct Floating {
    pub pic: Pic,
    pub w: f64,
    pub h: f64,
    pub x: Place,
    /// Down from the paragraph's top (`None`), or `Some(place)` on the page.
    pub y: Option<Place>,
    pub y_offset: f64,
    pub wrap: Wrap,
    /// The distance text keeps from it at its sides, and below it.
    pub gap: f64,
    pub gap_below: f64,
}

impl Default for Para {
    fn default() -> Self {
        Para {
            items: Vec::new(),
            align: Align::Left,
            left: 0.0,
            right: 0.0,
            first: 0.0,
            before: 0.0,
            after: 0.0,
            line: LineRule::Auto(1.0),
            tabs: Vec::new(),
            label: None,
            base: RunStyle::default(),
            page_break_before: false,
            shade: None,
            borders: None,
            keep_next: false,
            contextual: None,
            notes: Vec::new(),
            floats: Vec::new(),
            style: None,
            widow: true,
            keep_lines: false,
            no_line_numbers: false,
            drop_cap: None,
            outline: None,
            anchors: Vec::new(),
            rtl: false,
        }
    }
}

/// What a picture-like item shows.
#[derive(Clone, Debug)]
pub(crate) enum Pic {
    /// An image XObject, or `None` for a placeholder box.
    Image(Option<ObjRef>),
    /// An image cropped by fractions of its size (left, top, right, bottom; negative pads).
    Cropped(ObjRef, [f64; 4]),
    Chart(Arc<Chart>),
    Shapes(Arc<Vec<ShapePart>>),
    Math(Arc<super::math::MathBox>),
}

/// One shape of a drawing: where it is in the drawing (left, top, width, height as fractions of
/// the drawing's size), its outline and fill, a picture, and the paragraphs of its text box.
#[derive(Clone, Debug)]
pub(crate) struct ShapePart {
    pub rect: [f64; 4],
    pub geom: Option<super::xlsx_drawing::Shape>,
    pub image: Option<Option<ObjRef>>,
    pub paras: Vec<Para>,
    /// Text insets (left, top, right, bottom) in points, and where the text sits: 0 top,
    /// 0.5 middle, 1 bottom.
    pub inset: [f64; 4],
    pub anchor: f64,
    /// Clockwise rotation in degrees.
    pub rot: f64,
    /// The text is the shape's own DrawingML text (SmartArt), not paragraphs.
    pub own_text: bool,
    /// Vertical text: turned a quarter clockwise (-1, top to bottom) or anticlockwise (1).
    pub vert: i8,
}

#[derive(Clone, Debug)]
pub(crate) enum Placed {
    Text(String, usize),
    Image(Pic, f64, f64),
}

#[derive(Clone, Debug)]
pub(crate) struct Line {
    /// x from the text area's left edge.
    pub items: Vec<(f64, Placed)>,
    pub height: f64,
    /// From the line's top to its baseline.
    pub baseline: f64,
    /// Where the line's text starts in the paragraph: item and character. A paragraph that
    /// moves to a column of another width is laid out again from there ([`rest`]).
    pub src: Option<(usize, usize)>,
}

/// A laid-out paragraph.
#[derive(Clone, Debug)]
pub(crate) struct Laid {
    pub lines: Vec<Line>,
    pub styles: Vec<RunStyle>,
    pub before: f64,
    pub after: f64,
    pub shade: Option<[f64; 3]>,
    pub left: f64,
    pub right: f64,
}

impl Laid {
    pub(crate) fn height(&self) -> f64 {
        self.before + self.lines.iter().map(|l| l.height).sum::<f64>() + self.after
    }
}

enum Atom {
    Word(Vec<(String, usize)>),
    Space(String, usize),
    /// A tab, with the style of the text before it (for its leader).
    Tab(usize),
    Break,
    Image(Pic, f64, f64),
}

fn style_index(styles: &mut Vec<RunStyle>, s: &RunStyle) -> usize {
    // Only recent styles are searched, so a paragraph of many distinct runs stays linear; a
    // duplicate entry is harmless.
    let from = styles.len().saturating_sub(32);
    match styles.iter().skip(from).position(|x| x == s).map(|i| i + from) {
        Some(i) => i,
        None => {
            styles.push(s.clone());
            styles.len() - 1
        }
    }
}

/// A finite number, or `or` for NaN and infinities from a hostile file.
pub(crate) fn fin(v: f64, or: f64) -> f64 {
    if v.is_finite() { v } else { or }
}

/// Append text to the atoms; `open` says whether the last atom is a word the text may continue
/// (no space between, even across runs of different styles).
/// `srcs` gets, for each new atom, where it starts: item `item`, character of its text.
fn push_text(out: &mut Vec<Atom>, srcs: &mut Vec<(usize, usize)>, open: &mut bool, text: &str, si: usize, item: usize) {
    for (k, c) in text.chars().enumerate() {
        if c == ' ' {
            match out.last_mut() {
                Some(Atom::Space(s, _)) => s.push(' '),
                _ => {
                    out.push(Atom::Space(" ".into(), si));
                    srcs.push((item, k));
                }
            }
            *open = false;
            continue;
        }
        if *open && let Some(Atom::Word(pieces)) = out.last_mut() {
            match pieces.last_mut() {
                Some((t, i)) if *i == si => t.push(c),
                _ => pieces.push((c.to_string(), si)),
            }
        } else {
            out.push(Atom::Word(vec![(c.to_string(), si)]));
            srcs.push((item, k));
        }
        // A line may break after a hyphen or slash.
        *open = !matches!(c, '-' | '/');
    }
}

/// The paragraph's atoms and where each starts (see [`push_text`]).
fn atoms(p: &Para, fields: (usize, usize), styles: &mut Vec<RunStyle>) -> (Vec<Atom>, Vec<(usize, usize)>) {
    let mut out: Vec<Atom> = Vec::new();
    let mut srcs: Vec<(usize, usize)> = Vec::new();
    let mut open = false;
    // The style of the latest text, which a following tab's leader is drawn in.
    let mut last = 0usize;
    for (ii, item) in p.items.iter().enumerate() {
        match item {
            Inline::Text(t, s) => {
                if s.hidden || t.is_empty() {
                    continue;
                }
                let si = style_index(styles, s);
                last = si;
                let t = if s.caps { t.to_uppercase() } else { t.clone() };
                push_text(&mut out, &mut srcs, &mut open, &t, si, ii);
            }
            Inline::Field(f, s) => {
                let si = style_index(styles, s);
                let v = match f {
                    Field::Page(_) => fields.0.to_string(),
                    Field::Pages | Field::SectionPages => fields.1.to_string(),
                    Field::StyleRef(_) => String::new(),
                };
                push_text(&mut out, &mut srcs, &mut open, &v, si, ii);
            }
            Inline::Tab => {
                out.push(Atom::Tab(last));
                srcs.push((ii, 0));
                open = false;
            }
            Inline::Break => {
                out.push(Atom::Break);
                srcs.push((ii, 0));
                open = false;
            }
            Inline::Image(r, w, h) => {
                out.push(Atom::Image(Pic::Image(*r), fin(*w, 1.0).clamp(1.0, 14_400.0), fin(*h, 1.0).clamp(1.0, 14_400.0)));
                srcs.push((ii, 0));
                open = false;
            }
            Inline::Cropped(r, crop, w, h) => {
                out.push(Atom::Image(Pic::Cropped(*r, *crop), fin(*w, 1.0).clamp(1.0, 14_400.0), fin(*h, 1.0).clamp(1.0, 14_400.0)));
                srcs.push((ii, 0));
                open = false;
            }
            Inline::Chart(c, w, h) => {
                out.push(Atom::Image(Pic::Chart(c.clone()), fin(*w, 1.0).clamp(1.0, 14_400.0), fin(*h, 1.0).clamp(1.0, 14_400.0)));
                srcs.push((ii, 0));
                open = false;
            }
            Inline::Math(m) => {
                out.push(Atom::Image(Pic::Math(m.clone()), fin(m.w, 1.0).clamp(1.0, 14_400.0), fin(m.asc + m.desc, 1.0).clamp(1.0, 14_400.0)));
                srcs.push((ii, 0));
                open = false;
            }
            Inline::Shapes(s, w, h) => {
                out.push(Atom::Image(Pic::Shapes(s.clone()), fin(*w, 1.0).clamp(1.0, 14_400.0), fin(*h, 1.0).clamp(1.0, 14_400.0)));
                srcs.push((ii, 0));
                open = false;
            }
        }
    }
    (out, srcs)
}

/// Whether `c` is right-to-left text: Arabic or Hebrew.
fn is_rtl(c: char) -> bool {
    matches!(u32::from(c), 0x0590..=0x05FF | 0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

fn rtl_word(a: &Atom) -> bool {
    matches!(a, Atom::Word(pieces) if pieces.iter().any(|(t, _)| t.chars().any(is_rtl)))
}

/// Turn each run of right-to-left words (with the spaces between them) of one text item back
/// into reading order: shaping set it in visual order, last word first.
fn logical_order(atoms: &mut [Atom], srcs: &mut [(usize, usize)]) {
    let mut i = 0usize;
    while i < atoms.len() {
        if !atoms.get(i).is_some_and(rtl_word) {
            i += 1;
            continue;
        }
        let item = srcs.get(i).map(|s| s.0);
        let (mut end, mut j) = (i, i + 1);
        while let Some(a) = atoms.get(j) {
            if srcs.get(j).map(|s| s.0) != item || !(rtl_word(a) || matches!(a, Atom::Space(..))) {
                break;
            }
            if rtl_word(a) {
                end = j;
            }
            j += 1;
        }
        if let (Some(run), Some(src)) = (atoms.get_mut(i..=end), srcs.get_mut(i..=end)) {
            run.reverse();
            src.reverse();
        }
        i = end + 1;
    }
}

/// Mirror a laid-out line of a right-to-left paragraph between `left` and `right`, so its
/// first item is at the right; runs of left-to-right text inside it keep their own order.
fn mirror_line(items: &mut [(f64, Placed)], styles: &[RunStyle], left: f64, right: f64) {
    let width = |p: &Placed| match p {
        Placed::Text(t, si) => styles.get(*si).map_or(0.0, |s| s.width(t)),
        Placed::Image(_, w, _) => *w,
    };
    for (x, p) in items.iter_mut() {
        *x = left + right - *x - width(p);
    }
    // Left-to-right runs: text with letters or digits and none right-to-left, with the spaces
    // between them, mirrored back within the room they take.
    let ltr = |p: &Placed| matches!(p, Placed::Text(t, _) if t.chars().any(char::is_alphanumeric) && !t.chars().any(is_rtl));
    let blank = |p: &Placed| matches!(p, Placed::Text(t, _) if t.trim().is_empty());
    let mut i = 0usize;
    while i < items.len() {
        if !items.get(i).is_some_and(|(_, p)| ltr(p)) {
            i += 1;
            continue;
        }
        let (mut end, mut j) = (i, i + 1);
        while let Some((_, p)) = items.get(j) {
            if ltr(p) {
                end = j;
            } else if !blank(p) {
                break;
            }
            j += 1;
        }
        if end > i {
            let lo = items[i..=end].iter().map(|(x, _)| *x).fold(f64::INFINITY, f64::min);
            let hi = items[i..=end].iter().map(|(x, p)| *x + width(p)).fold(f64::NEG_INFINITY, f64::max);
            for (x, p) in &mut items[i..=end] {
                *x = lo + hi - *x - width(p);
            }
        }
        i = end + 1;
    }
}

/// What is left of `p` from `src` (item, character) on: a paragraph to lay out again where the
/// rest of `p` goes on in a column of another width. Its first line is a continuation line.
pub(crate) fn rest(p: &Para, (item, offset): (usize, usize)) -> Para {
    let mut items: Vec<Inline> = p.items.iter().skip(item).cloned().collect();
    if let Some(Inline::Text(t, _)) = items.first_mut() {
        *t = t.chars().skip(offset).collect();
    }
    Para { items, first: 0.0, before: 0.0, label: None, page_break_before: false, ..p.clone() }
}

struct Building {
    items: Vec<(f64, Placed)>,
    x: f64,
    /// Right edge of the last ink (trailing spaces excluded).
    ink: f64,
    start: f64,
    /// Indices of space items, for justification.
    spaces: Vec<usize>,
    /// Indices after the last tab: only those move when the line is aligned or justified.
    after_tab: usize,
    forced: bool,
    /// A right, centre or decimal tab whose text is still being placed (from the tab's x on):
    /// it is moved into place when the next tab, a line break or the paragraph's end comes.
    pending: Option<PendingTab>,
    /// Where the line's first word, tab or picture comes from.
    src: Option<(usize, usize)>,
}

struct PendingTab {
    stop: TabStop,
    /// Where the tab started, and the first item of the text after it.
    from: f64,
    first: usize,
    /// The style its leader is drawn in.
    style: usize,
}

impl Building {
    fn new(start: f64) -> Self {
        Building { items: Vec::new(), x: start, ink: start, start, spaces: Vec::new(), after_tab: 0, forced: false, pending: None, src: None }
    }

    /// Fill `from..to` with `leader` characters, ending a little before `to`.
    fn leader(&mut self, styles: &[RunStyle], si: usize, c: char, from: f64, to: f64) {
        let Some(st) = styles.get(si) else { return };
        let cw = st.width(c.encode_utf8(&mut [0; 4]));
        if cw <= 0.0 || !(to - from).is_finite() {
            return;
        }
        // A quarter of a character clear of the text on each side, at most a line's worth.
        let n = ((to - from - cw * 0.5) / cw).floor().clamp(0.0, 2_000.0) as usize;
        if n == 0 {
            return;
        }
        let x = to - cw * 0.25 - n as f64 * cw;
        self.items.push((x, Placed::Text(c.to_string().repeat(n), si)));
    }

    /// Move the text after a pending right, centre or decimal tab into place, and draw its
    /// leader. Text wider than the room before the stop stays where the tab began.
    fn resolve_tab(&mut self, styles: &[RunStyle]) {
        let Some(t) = self.pending.take() else { return };
        let w = (self.ink - t.from).max(0.0);
        let anchor = match t.stop.kind {
            TabKind::Left => 0.0,
            TabKind::Right => w,
            TabKind::Center => w / 2.0,
            TabKind::Decimal => self.decimal_offset(styles, t.first, t.from).unwrap_or(w),
        };
        let shift = (t.stop.pos - anchor - t.from).max(0.0);
        if shift > 0.0 {
            for (x, _) in self.items.iter_mut().skip(t.first) {
                *x += shift;
            }
            self.x += shift;
            self.ink += shift;
        }
        if let Some(c) = t.stop.leader {
            self.leader(styles, t.style, c, t.from, t.from + shift);
        }
    }

    /// The distance from `from` to the first `.` of the text from item `first` on.
    fn decimal_offset(&self, styles: &[RunStyle], first: usize, from: f64) -> Option<f64> {
        self.items.iter().skip(first).find_map(|(x, p)| match p {
            Placed::Text(t, si) => {
                let at = t.find('.')?;
                let st = styles.get(*si)?;
                Some(x + st.width(t.get(..at)?) - from)
            }
            Placed::Image(..) => None,
        })
    }

    fn has_ink(&self) -> bool {
        self.items.iter().any(|(_, p)| !matches!(p, Placed::Text(t, _) if t.trim().is_empty()))
    }
}

/// Lay a paragraph out in a text area `width` points wide. `fields` gives the page number and
/// count for page fields.
pub(crate) fn layout(p: &Para, width: f64, fields: (usize, usize)) -> Laid {
    let mut styles = vec![p.base.clone()];
    let (mut atoms, mut srcs) = atoms(p, fields, &mut styles);
    // A right-to-left paragraph is broken into lines in reading order: each run of right-to-left
    // words, set in visual order by shaping, is turned back first; each line is mirrored after.
    if p.rtl {
        logical_order(&mut atoms, &mut srcs);
    }
    let align = match p.align {
        Align::Left if p.rtl => Align::Right,
        Align::Right if p.rtl => Align::Left,
        a => a,
    };
    let width = fin(width, 468.0).clamp(10.0, 100_000.0);
    let left = fin(p.left, 0.0).clamp(-width, width);
    let right_edge = (width - fin(p.right, 0.0).clamp(-width, width)).max(left + 10.0);
    let first_start = (left + fin(p.first, 0.0)).clamp(-width, right_edge - 5.0);
    // Tab stops: custom ones, a hanging indent's left edge, then every half inch.
    let mut stops: Vec<TabStop> = p.tabs.iter().copied().filter(|t| t.pos.is_finite()).collect();
    if p.first < 0.0 && !stops.iter().any(|t| (t.pos - left).abs() < 0.5) {
        stops.push(TabStop::left(left));
    }
    stops.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    let default_stop = |x: f64| TabStop::left(((x / DEFAULT_TAB).floor() + 1.0) * DEFAULT_TAB);

    let mut lines: Vec<Building> = Vec::new();
    let mut cur = Building::new(first_start);
    if let Some((label, st)) = &p.label {
        let si = style_index(&mut styles, st);
        let w = st.width(label);
        cur.items.push((cur.x, Placed::Text(label.clone(), si)));
        cur.x += w;
        cur.ink = cur.x;
        // The text after a list label starts at the next tab stop (the hanging indent).
        let next = stops.iter().map(|t| t.pos).find(|t| *t > cur.x + 1.0).unwrap_or_else(|| default_stop(cur.x).pos);
        cur.x = if next <= right_edge { next } else { cur.x + st.width(" ") };
        cur.after_tab = cur.items.len();
    }

    let new_line = |lines: &mut Vec<Building>, cur: &mut Building, forced: bool| {
        cur.forced = forced;
        let done = std::mem::replace(cur, Building::new(left));
        lines.push(done);
    };

    for (ai, atom) in atoms.into_iter().enumerate() {
        if lines.len() >= MAX_LINES {
            break;
        }
        // A line's source is its first word, tab or picture (its leading spaces are dropped).
        let src = srcs.get(ai).copied();
        let starts = !matches!(atom, Atom::Space(..) | Atom::Break);
        match atom {
            Atom::Break => {
                cur.resolve_tab(&styles);
                new_line(&mut lines, &mut cur, true);
            }
            Atom::Space(s, si) => {
                let Some(st) = styles.get(si) else { continue };
                let w = st.width(&s);
                cur.spaces.push(cur.items.len());
                cur.items.push((cur.x, Placed::Text(s, si)));
                cur.x += w;
            }
            Atom::Tab(si) => {
                cur.resolve_tab(&styles);
                let next = stops.iter().copied().find(|t| t.pos > cur.x + 0.5).unwrap_or_else(|| default_stop(cur.x));
                if next.kind == TabKind::Left && next.pos > right_edge {
                    // Past the right edge a tab moves to the next line, as Word does.
                    new_line(&mut lines, &mut cur, false);
                } else if next.kind == TabKind::Left {
                    if let Some(c) = next.leader {
                        let from = cur.x;
                        cur.leader(&styles, si, c, from, next.pos);
                    }
                    cur.x = next.pos;
                } else {
                    // The text that follows is placed from here and moved to the stop later.
                    cur.pending = Some(PendingTab { stop: next, from: cur.x, first: cur.items.len(), style: si });
                }
                cur.after_tab = cur.items.len();
                cur.spaces.clear();
            }
            Atom::Image(r, w, h) => {
                if cur.x + w > right_edge && cur.has_ink() {
                    new_line(&mut lines, &mut cur, false);
                }
                // An image wider than the line is scaled to fit it.
                let k = if w > right_edge - cur.x && right_edge - cur.x > 1.0 { (right_edge - cur.x) / w } else { 1.0 };
                cur.items.push((cur.x, Placed::Image(r, w * k, h * k)));
                cur.x += w * k;
                cur.ink = cur.x;
            }
            Atom::Word(pieces) => {
                let w: f64 = pieces.iter().map(|(t, si)| styles.get(*si).map_or(0.0, |s| s.width(t))).sum();
                if cur.x + w > right_edge + 0.01 && cur.has_ink() {
                    new_line(&mut lines, &mut cur, false);
                    // Leading spaces of a wrapped line are not drawn.
                }
                if cur.x + w <= right_edge + 0.01 || w <= 0.0 {
                    for (t, si) in pieces {
                        let pw = styles.get(si).map_or(0.0, |s| s.width(&t));
                        cur.items.push((cur.x, Placed::Text(t, si)));
                        cur.x += pw;
                    }
                    cur.ink = cur.x;
                } else {
                    // A word longer than the whole line is broken between characters.
                    for (t, si) in pieces {
                        let Some(st) = styles.get(si).cloned() else { continue };
                        let mut chunk = String::new();
                        // The chunk's width, kept as it grows: re-measuring would be quadratic.
                        let mut chunk_w = 0.0;
                        for c in t.chars() {
                            let cw = st.width(c.encode_utf8(&mut [0; 4]));
                            if cur.x + chunk_w + cw > right_edge && (!chunk.is_empty() || cur.has_ink()) {
                                if !chunk.is_empty() {
                                    cur.items.push((cur.x, Placed::Text(std::mem::take(&mut chunk), si)));
                                    cur.x += chunk_w;
                                    cur.ink = cur.x;
                                    chunk_w = 0.0;
                                }
                                new_line(&mut lines, &mut cur, false);
                                if lines.len() >= MAX_LINES {
                                    break;
                                }
                            }
                            chunk.push(c);
                            chunk_w += cw;
                        }
                        if !chunk.is_empty() {
                            cur.items.push((cur.x, Placed::Text(chunk, si)));
                            cur.x += chunk_w;
                            cur.ink = cur.x;
                        }
                    }
                }
            }
        }
        if starts && cur.src.is_none() {
            cur.src = src;
        }
    }
    cur.resolve_tab(&styles);
    cur.forced = true;
    lines.push(cur);

    let count = lines.len();
    let mut out = Vec::with_capacity(count);
    for (i, mut b) in lines.into_iter().enumerate() {
        let last = i + 1 == count || b.forced;
        // Spaces at the start of a wrapped line and at the end of any line carry no ink.
        if i > 0 && !b.forced {
            while matches!(b.items.first(), Some((_, Placed::Text(t, _))) if t.trim().is_empty()) {
                b.items.remove(0);
                b.spaces.retain(|s| *s != 0);
                b.spaces.iter_mut().for_each(|s| *s = s.saturating_sub(1));
                b.after_tab = b.after_tab.saturating_sub(1);
                if let Some((x0, _)) = b.items.first() {
                    let shift = x0 - b.start;
                    b.items.iter_mut().for_each(|(x, _)| *x -= shift);
                    b.ink -= shift;
                }
            }
        }
        let trailing = b.items.iter().rposition(|(_, p)| !matches!(p, Placed::Text(t, _) if t.trim().is_empty())).map_or(0, |i| i + 1);
        b.items.truncate(trailing);
        b.spaces.retain(|s| *s < trailing);
        let free = right_edge - b.ink;
        let tail = b.after_tab.min(b.items.len());
        match align {
            Align::Center if free > 0.0 => b.items.iter_mut().skip(tail).for_each(|(x, _)| *x += free / 2.0),
            Align::Right if free > 0.0 => b.items.iter_mut().skip(tail).for_each(|(x, _)| *x += free),
            Align::Justify if free > 0.0 && !last => {
                let spaces: Vec<usize> = b.spaces.iter().copied().filter(|s| *s >= tail).collect();
                if !spaces.is_empty() {
                    let extra = free / spaces.len() as f64;
                    let mut shift = 0.0;
                    for (i, (x, _)) in b.items.iter_mut().enumerate() {
                        *x += shift;
                        if spaces.contains(&i) {
                            shift += extra;
                        }
                    }
                }
            }
            _ => {}
        }
        let (mut asc, mut desc, mut gap) = (0.0f64, 0.0f64, 0.0f64);
        for (_, item) in &b.items {
            match item {
                Placed::Text(_, si) => {
                    if let Some(s) = styles.get(*si) {
                        let (a, d, g) = s.line_metrics();
                        asc = asc.max(s.size * a);
                        desc = desc.max(s.size * d);
                        gap = gap.max(s.size * g);
                    }
                }
                // An equation has a depth below the baseline, like text.
                Placed::Image(Pic::Math(m), _, _) => {
                    asc = asc.max(m.asc);
                    desc = desc.max(m.desc);
                }
                Placed::Image(_, _, h) => asc = asc.max(*h),
            }
        }
        if asc <= 0.0 {
            // An empty line is as tall as the paragraph mark's font.
            let s = &p.base;
            let (a, d, g) = s.line_metrics();
            asc = s.size * a;
            desc = s.size * d;
            gap = s.size * g;
        }
        let natural = asc + desc + gap;
        let (height, baseline) = match p.line {
            LineRule::Auto(m) => {
                // Word puts the line gap above the text; a multiple over one adds the extra
                // below, one under one shrinks the space above too.
                let m = if m.is_finite() { m.clamp(0.25, 10.0) } else { 1.0 };
                let h = natural * m;
                (h, if m >= 1.0 { gap + asc } else { (gap + asc) * m })
            }
            LineRule::Exact(v) => {
                let v = if v.is_finite() { v.clamp(0.5, 2000.0) } else { natural };
                (v, (v * 0.8).min(v - desc * 0.5).max(0.0))
            }
            LineRule::AtLeast(v) => {
                let h = if v.is_finite() { natural.max(v) } else { natural };
                (h, h - desc)
            }
        };
        if p.rtl {
            mirror_line(&mut b.items, &styles, left, right_edge);
        }
        out.push(Line { items: b.items, height, baseline, src: b.src });
    }
    Laid {
        lines: out,
        styles,
        before: fin(p.before, 0.0).clamp(0.0, 2000.0),
        after: fin(p.after, 0.0).clamp(0.0, 2000.0),
        shade: p.shade,
        left,
        right: right_edge,
    }
}

/// Draw a picture or chart with its bottom-left corner at `(x, y)`.
pub(crate) fn draw_pic(page: &mut Page, pic: &Pic, x: f64, y: f64, w: f64, h: f64) {
    match pic {
        Pic::Image(Some(r)) => page.image(*r, x, y, w, h),
        Pic::Image(None) => page.placeholder(x, y, w, h),
        Pic::Cropped(r, [l, t, rr, b]) => {
            // The whole image is drawn larger and shifted so the kept part fills the box.
            let (fw, fh) = (w / (1.0 - l - rr).max(0.01), h / (1.0 - t - b).max(0.01));
            page.save();
            page.clip_rect(x, y, w, h);
            page.image(*r, x - l * fw, y - b * fh, fw, fh);
            page.restore();
        }
        Pic::Chart(c) => chart::draw(page, c, x, y, w, h),
        Pic::Shapes(parts) => {
            for p in parts.iter() {
                draw_shape_part(page, p, x, y, w, h);
            }
        }
        Pic::Math(m) => super::math::draw(page, m, x, y + m.desc),
    }
}

/// Draw one shape of a drawing whose box is `(x, y)` (bottom left) `w × h`.
fn draw_shape_part(page: &mut Page, p: &ShapePart, x: f64, y: f64, w: f64, h: f64) {
    let [fx, fy, fw, fh] = p.rect.map(|v| fin(v, 0.0).clamp(-10.0, 10.0));
    let (pw, ph) = (fw * w, fh * h);
    let (px, top) = (x + fx * w, y + h - fy * h);
    if !(pw.is_finite() && ph.is_finite()) || pw < 0.0 || ph < 0.0 {
        return;
    }
    page.save();
    if p.rot.is_finite() && p.rot.abs() > 0.01 {
        // About the shape's centre, clockwise as Office turns it.
        let (cx, cy) = (px + pw / 2.0, top - ph / 2.0);
        let (s, c) = (-p.rot.to_radians()).sin_cos();
        page.raw(&format!(
            "{} {} {} {} {} {} cm\n",
            super::canvas::n(c),
            super::canvas::n(s),
            super::canvas::n(-s),
            super::canvas::n(c),
            super::canvas::n(cx - c * cx + s * cy),
            super::canvas::n(cy - s * cx - c * cy)
        ));
    }
    if let Some(g) = &p.geom {
        if p.own_text {
            super::xlsx_drawing::draw_shape(page, g, px, top - ph, pw, ph);
        } else {
            super::xlsx_drawing::draw_geometry(page, g, px, top - ph, pw, ph);
        }
    }
    match p.image {
        Some(Some(r)) => page.image(r, px, top - ph, pw, ph),
        Some(None) => page.placeholder(px, top - ph, pw, ph),
        None => {}
    }
    if !p.paras.is_empty() {
        let [l, t, r, b] = p.inset.map(|v| fin(v, 0.0).clamp(0.0, 500.0));
        // Vertical text is laid out along the box's height, then turned into it.
        let (bx, btop, bw, bh) = if p.vert != 0 {
            turn(page, p.vert, px, top - ph, pw, ph);
            (0.0, pw, ph, pw)
        } else {
            (px, top, pw, ph)
        };
        let inner = (bw - l - r).max(4.0);
        let laids: Vec<Laid> = p.paras.iter().map(|q| layout(q, inner, (1, 1))).collect();
        let total: f64 = laids.iter().map(Laid::height).sum();
        let mut yy = btop - t - (bh - t - b - total).max(0.0) * p.anchor.clamp(0.0, 1.0);
        let px = bx;
        for (laid, q) in laids.iter().zip(&p.paras) {
            yy -= draw(page, laid, px + l, yy);
            // Shapes anchored in the box's own paragraphs (a box in a box) follow them.
            for f in q.floats.iter().take(64) {
                draw_pic(page, &f.pic, px + l, yy - f.h, f.w, f.h);
                yy -= f.h;
            }
        }
    }
    page.restore();
}

/// Turn the coordinates so that a box `h` wide and `w` tall, its corner at the origin, fills the
/// box at `(x, y)` `w × h` turned a quarter: anticlockwise (`dir` 1, reading upwards, the text's
/// top to the left) or clockwise (-1, reading downwards, its top to the right).
pub(crate) fn turn(page: &mut Page, dir: i8, x: f64, y: f64, w: f64, h: f64) {
    if dir > 0 {
        page.raw(&format!("0 1 -1 0 {} {} cm\n", n(x + w), n(y)));
    } else {
        page.raw(&format!("0 -1 1 0 {} {} cm\n", n(x), n(y + h)));
    }
}

/// Draw one line whose top is at PDF `y_top`, in a text area starting at PDF `x0`.
pub(crate) fn draw_line(page: &mut Page, laid: &Laid, line: &Line, x0: f64, y_top: f64) {
    let base = y_top - line.baseline;
    for (x, item) in &line.items {
        match item {
            Placed::Text(t, si) => {
                if let Some(s) = laid.styles.get(*si) {
                    page.text(x0 + x, base, s, t);
                }
            }
            Placed::Image(Pic::Image(Some(r)), w, h) => page.image(*r, x0 + x, base, *w, *h),
            Placed::Image(Pic::Image(None), w, h) => page.placeholder(x0 + x, base, *w, *h),
            Placed::Image(Pic::Chart(c), w, h) => chart::draw(page, c, x0 + x, base, *w, *h),
            Placed::Image(pic @ (Pic::Shapes(_) | Pic::Cropped(..)), w, h) => draw_pic(page, pic, x0 + x, base, *w, *h),
            Placed::Image(Pic::Math(m), _, _) => super::math::draw(page, m, x0 + x, base),
        }
    }
}

/// Draw a whole paragraph from PDF `y_top` (no page breaks); returns the height used.
pub(crate) fn draw(page: &mut Page, laid: &Laid, x0: f64, y_top: f64) -> f64 {
    let mut y = y_top - laid.before;
    if let Some(c) = laid.shade {
        let h: f64 = laid.lines.iter().map(|l| l.height).sum();
        page.fill_rect(x0 + laid.left, y - h, laid.right - laid.left, h, c);
    }
    for line in &laid.lines {
        draw_line(page, laid, line, x0, y);
        y -= line.height;
    }
    laid.height()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(text: &str) -> Para {
        Para { items: vec![Inline::Text(text.into(), RunStyle::default())], ..Para::default() }
    }

    fn texts(l: &Line) -> String {
        l.items.iter().filter_map(|(_, p)| if let Placed::Text(t, _) = p { Some(t.as_str()) } else { None }).collect()
    }

    #[test]
    fn wraps_at_spaces_and_keeps_every_word() {
        let laid = layout(&para("alpha beta gamma delta epsilon zeta eta theta"), 80.0, (1, 1));
        assert!(laid.lines.len() > 2);
        let all: Vec<String> = laid.lines.iter().map(texts).collect();
        assert_eq!(all.join(" ").split_whitespace().collect::<Vec<_>>().join(" "), "alpha beta gamma delta epsilon zeta eta theta");
        for l in &laid.lines {
            let (x, _) = l.items.first().unwrap();
            assert!(*x >= 0.0);
        }
    }

    #[test]
    fn a_word_longer_than_the_line_is_split_not_lost() {
        let laid = layout(&para(&"W".repeat(200)), 50.0, (1, 1));
        let joined: String = laid.lines.iter().map(texts).collect();
        assert_eq!(joined, "W".repeat(200));
    }

    #[test]
    fn alignment_moves_lines() {
        let mut p = para("ab");
        p.align = Align::Right;
        let laid = layout(&p, 200.0, (1, 1));
        let (x, _) = laid.lines[0].items[0];
        assert!(x > 150.0, "{x}");
        p.align = Align::Center;
        let (x, _) = layout(&p, 200.0, (1, 1)).lines[0].items[0];
        assert!(x > 80.0 && x < 100.0, "{x}");
    }

    #[test]
    fn justify_spreads_all_but_the_last_line() {
        let mut p = para("one two three four five six seven eight nine ten eleven twelve");
        p.align = Align::Justify;
        let laid = layout(&p, 120.0, (1, 1));
        let first = &laid.lines[0];
        let (x, Placed::Text(t, si)) = first.items.last().unwrap() else { panic!() };
        let end = x + laid.styles[*si].width(t);
        assert!((end - 120.0).abs() < 0.5, "{end}");
    }

    #[test]
    fn right_to_left_paragraphs_wrap_in_reading_order_from_the_right() {
        let s = RunStyle::default();
        // Words one to four, as shaping leaves them: in visual order, the last word first.
        let p = Para { items: vec![Inline::Text("דד גג בב אא".into(), s.clone())], rtl: true, align: Align::Right, ..Para::default() };
        let width = s.width("אא בב") + 4.0;
        let laid = layout(&p, width, (1, 1));
        let words = |li: usize| -> Vec<(f64, String)> {
            laid.lines[li]
                .items
                .iter()
                .filter_map(|(x, it)| if let Placed::Text(t, _) = it { (!t.trim().is_empty()).then(|| (*x, t.clone())) } else { None })
                .collect()
        };
        assert_eq!(laid.lines.len(), 2);
        let (first, second) = (words(0), words(1));
        let x_of = |ws: &[(f64, String)], w: &str| ws.iter().find(|(_, t)| t == w).map(|(x, _)| *x).unwrap();
        assert!(first.iter().any(|(_, t)| t == "אא") && first.iter().any(|(_, t)| t == "בב"), "the first two words on the first line: {first:?}");
        assert!(x_of(&first, "אא") > x_of(&first, "בב"), "the first word at the right");
        assert!(second.iter().any(|(_, t)| t == "גג") && x_of(&second, "גג") > x_of(&second, "דד"), "{second:?}");
        // Right-aligned: the first word ends at the right edge.
        assert!((x_of(&first, "אא") + s.width("אא") - laid.right).abs() < 0.5);
        // Left-to-right words in a right-to-left line keep their order (shaping turns only the
        // right-to-left runs, so "אא Alfa Beta בב" stays in this order).
        let p = Para { items: vec![Inline::Text("אא Alfa Beta בב".into(), s.clone())], rtl: true, ..Para::default() };
        let laid = layout(&p, 400.0, (1, 1));
        let line: Vec<(f64, String)> =
            laid.lines[0].items.iter().filter_map(|(x, it)| if let Placed::Text(t, _) = it { Some((*x, t.clone())) } else { None }).collect();
        let at = |w: &str| line.iter().find(|(_, t)| t == w).map(|(x, _)| *x).unwrap();
        assert!(at("אא") > at("Beta") && at("Beta") > at("Alfa") && at("Alfa") > at("בב"), "{line:?}");
    }

    #[test]
    fn breaks_tabs_and_labels() {
        let s = RunStyle::default();
        let p = Para {
            items: vec![
                Inline::Text("a".into(), s.clone()),
                Inline::Tab,
                Inline::Text("b".into(), s.clone()),
                Inline::Break,
                Inline::Text("c".into(), s.clone()),
            ],
            left: 36.0,
            first: -18.0,
            label: Some(("1.".into(), s.clone())),
            ..Para::default()
        };
        let laid = layout(&p, 400.0, (1, 1));
        assert_eq!(laid.lines.len(), 2);
        let xs: Vec<f64> = laid.lines[0].items.iter().map(|(x, _)| *x).collect();
        assert_eq!(xs[0], 18.0, "the label sits at the hanging indent");
        assert_eq!(xs[1], 36.0, "text starts at the left indent");
        assert_eq!(texts(&laid.lines[1]), "c");
    }

    #[test]
    fn right_tabs_move_their_text_to_the_stop_and_draw_leaders() {
        let s = RunStyle::default();
        let p = Para {
            items: vec![Inline::Text("Title".into(), s.clone()), Inline::Tab, Inline::Text("9".into(), s.clone())],
            tabs: vec![TabStop { pos: 300.0, kind: TabKind::Right, leader: Some('.') }],
            ..Para::default()
        };
        let laid = layout(&p, 400.0, (1, 1));
        assert_eq!(laid.lines.len(), 1);
        let line = &laid.lines[0];
        let (x, _) = line.items.iter().find(|(_, p)| matches!(p, Placed::Text(t, _) if t == "9")).unwrap();
        assert!((x + s.width("9") - 300.0).abs() < 1e-6, "{x}");
        assert!(line.items.iter().any(|(_, p)| matches!(p, Placed::Text(t, _) if t.starts_with("..."))));
        // Text too wide for the room before the stop stays after the tab.
        let p = Para {
            items: vec![Inline::Tab, Inline::Text("W".repeat(40), s.clone())],
            tabs: vec![TabStop { pos: 20.0, kind: TabKind::Right, leader: None }],
            ..Para::default()
        };
        let laid = layout(&p, 400.0, (1, 1));
        assert!(laid.lines[0].items.iter().all(|(x, _)| *x >= 0.0));
    }

    #[test]
    fn hostile_numbers_do_not_break_layout() {
        let mut p = para("x y z");
        p.left = f64::NAN;
        p.first = f64::INFINITY;
        p.line = LineRule::Exact(f64::NAN);
        let _ = layout(&p, f64::NAN, (1, 1));
        let _ = layout(&para(""), -5.0, (1, 1));
    }
}
