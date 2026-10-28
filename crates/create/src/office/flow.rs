//! Paragraph layout shared by the Word and PowerPoint importers: runs of styled text, tabs, line
//! breaks and inline images are broken into lines (greedy, at spaces and after hyphens), aligned,
//! and drawn. Measurements use the standard-14 metrics the text is drawn with, so what is measured
//! is what is drawn.

use pdfcraft_cos::ObjRef;

use super::RunStyle;
use super::canvas::Page;

/// Default tab stops every half inch, as Word and PowerPoint use.
const DEFAULT_TAB: f64 = 36.0;
/// Most lines one paragraph may lay out to.
const MAX_LINES: usize = 50_000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Field {
    /// The current page (or slide) number.
    Page,
    /// The number of pages.
    Pages,
}

#[derive(Clone, Debug)]
pub(crate) enum Inline {
    /// Drawable text (see `Notes::drawable`); no tabs or line breaks inside.
    Text(String, RunStyle),
    Tab,
    Break,
    /// An image of `w × h` points; `None` draws a placeholder box.
    Image(Option<ObjRef>, f64, f64),
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
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Placed {
    Text(String, usize),
    Image(Option<ObjRef>, f64, f64),
}

#[derive(Clone, Debug)]
pub(crate) struct Line {
    /// x from the text area's left edge.
    pub items: Vec<(f64, Placed)>,
    pub height: f64,
    /// From the line's top to its baseline.
    pub baseline: f64,
}

/// A laid-out paragraph.
#[derive(Clone, Debug)]
pub(crate) struct Laid {
    pub lines: Vec<Line>,
    pub styles: Vec<RunStyle>,
    pub before: f64,
    pub after: f64,
    pub shade: Option<[f64; 3]>,
    pub page_break_before: bool,
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
    Image(Option<ObjRef>, f64, f64),
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
fn push_text(out: &mut Vec<Atom>, open: &mut bool, text: &str, si: usize) {
    for c in text.chars() {
        if c == ' ' {
            match out.last_mut() {
                Some(Atom::Space(s, _)) => s.push(' '),
                _ => out.push(Atom::Space(" ".into(), si)),
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
        }
        // A line may break after a hyphen or slash.
        *open = !matches!(c, '-' | '/');
    }
}

fn atoms(p: &Para, fields: (usize, usize), styles: &mut Vec<RunStyle>) -> Vec<Atom> {
    let mut out: Vec<Atom> = Vec::new();
    let mut open = false;
    // The style of the latest text, which a following tab's leader is drawn in.
    let mut last = 0usize;
    for item in &p.items {
        match item {
            Inline::Text(t, s) => {
                if s.hidden || t.is_empty() {
                    continue;
                }
                let si = style_index(styles, s);
                last = si;
                let t = if s.caps { t.to_uppercase() } else { t.clone() };
                push_text(&mut out, &mut open, &t, si);
            }
            Inline::Field(f, s) => {
                let si = style_index(styles, s);
                let v = match f {
                    Field::Page => fields.0,
                    Field::Pages => fields.1,
                };
                push_text(&mut out, &mut open, &v.to_string(), si);
            }
            Inline::Tab => {
                out.push(Atom::Tab(last));
                open = false;
            }
            Inline::Break => {
                out.push(Atom::Break);
                open = false;
            }
            Inline::Image(r, w, h) => {
                out.push(Atom::Image(*r, fin(*w, 1.0).clamp(1.0, 14_400.0), fin(*h, 1.0).clamp(1.0, 14_400.0)));
                open = false;
            }
        }
    }
    out
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
        Building { items: Vec::new(), x: start, ink: start, start, spaces: Vec::new(), after_tab: 0, forced: false, pending: None }
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
    let atoms = atoms(p, fields, &mut styles);
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

    for atom in atoms {
        if lines.len() >= MAX_LINES {
            break;
        }
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
        match p.align {
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
                        asc = asc.max(s.size * 0.905);
                        desc = desc.max(s.size * 0.212);
                        gap = gap.max(s.size * 0.033);
                    }
                }
                Placed::Image(_, _, h) => asc = asc.max(*h),
            }
        }
        if asc <= 0.0 {
            // An empty line is as tall as the paragraph mark's font.
            let s = &p.base;
            asc = s.size * 0.905;
            desc = s.size * 0.212;
            gap = s.size * 0.033;
        }
        let natural = asc + desc + gap;
        let (height, baseline) = match p.line {
            LineRule::Auto(m) => {
                let m = if m.is_finite() { m.clamp(0.25, 10.0) } else { 1.0 };
                let h = natural * m;
                (h, h - desc - gap)
            }
            LineRule::Exact(v) => {
                let v = if v.is_finite() { v.clamp(0.5, 2000.0) } else { natural };
                (v, (v * 0.8).min(v - desc * 0.5).max(0.0))
            }
            LineRule::AtLeast(v) => {
                let h = if v.is_finite() { natural.max(v) } else { natural };
                (h, h - desc - gap)
            }
        };
        out.push(Line { items: b.items, height, baseline });
    }
    Laid {
        lines: out,
        styles,
        before: fin(p.before, 0.0).clamp(0.0, 2000.0),
        after: fin(p.after, 0.0).clamp(0.0, 2000.0),
        shade: p.shade,
        page_break_before: p.page_break_before,
        left,
        right: right_edge,
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
            Placed::Image(Some(r), w, h) => page.image(*r, x0 + x, base, *w, *h),
            Placed::Image(None, w, h) => page.placeholder(x0 + x, base, *w, *h),
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
