//! Excel conditional formatting (ECMA-376 Part 1 §18.3.1.10): which cells a sheet's rules
//! change and how. Cell value comparisons, top and bottom N (or N %), above and below average,
//! duplicate and unique values, text tests, blanks, time periods, colour scales, data bars and
//! icon sets are evaluated on the saved values, in priority order with `stopIfTrue`. Rules that
//! need Excel's formula engine (`expression`) are left out and reported.

use std::collections::{BTreeMap, HashMap};

use roxmltree::Node;

use super::Notes;
use super::canvas::{Page, n};
use super::xml::{attr, attr_f64, child, children, flag};

/// Most cell evaluations one sheet's rules may make.
const MAX_WORK: usize = 4_000_000;
/// Most rules read from one sheet.
const MAX_RULES: usize = 1_000;

/// A differential format (`dxf`): what a matching rule changes.
#[derive(Clone, Debug, Default)]
pub(crate) struct Dxf {
    pub fill: Option<[f64; 3]>,
    pub color: Option<[f64; 3]>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
}

/// An icon from an icon set: its shape and colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Icon {
    Arrow(i8, [f64; 3]),
    Flag([f64; 3]),
    Dot([f64; 3]),
    /// Signal bars: how many of `of` are lit.
    Bars(u8, u8),
}

/// How conditional formatting changes one cell.
#[derive(Clone, Debug, Default)]
pub(crate) struct Look {
    pub fill: Option<[f64; 3]>,
    pub color: Option<[f64; 3]>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    /// A data bar: the share of the cell it fills and its colour.
    pub bar: Option<(f64, [f64; 3])>,
    pub icon: Option<Icon>,
    /// Whether the value is shown (icon sets and data bars may hide it).
    pub hide_value: bool,
}

impl Look {
    /// Take what `d` sets and this look doesn't have yet (higher priority came first).
    fn add(&mut self, d: &Dxf) {
        self.fill = self.fill.or(d.fill);
        self.color = self.color.or(d.color);
        self.bold = self.bold.or(d.bold);
        self.italic = self.italic.or(d.italic);
        self.underline = self.underline.or(d.underline);
        self.strike = self.strike.or(d.strike);
    }
}

/// The value of a cell the rules look at: its number, or its text.
pub(crate) struct Value<'a> {
    pub number: Option<f64>,
    pub text: &'a str,
}

/// Whether a rule matches a cell's value.
type Matcher = Box<dyn Fn(&Value<'_>) -> bool>;

/// A sheet's cells by (row, column), as the rules see them.
pub(crate) type Cells<'a> = BTreeMap<(u32, u32), Value<'a>>;

/// Parse the `dxfs` of a styles part. `color` reads a colour element.
pub(crate) fn dxfs(root: Node<'_, '_>, color: &dyn Fn(Node<'_, '_>) -> Option<[f64; 3]>) -> Vec<Dxf> {
    let Some(list) = child(root, "dxfs") else { return Vec::new() };
    children(list, "dxf")
        .take(65_536)
        .map(|d| {
            let font = child(d, "font");
            let on = |k: &str| font.and_then(|f| child(f, k)).map(|c| attr(c, "val").is_none_or(|v| flag(Some(v))));
            // A solid pattern's colour is its background colour in a dxf.
            let fill = child(d, "fill").and_then(|f| child(f, "patternFill")).and_then(|p| {
                if attr(p, "patternType") == Some("none") {
                    return None;
                }
                child(p, "bgColor").and_then(color).or_else(|| child(p, "fgColor").and_then(color))
            });
            Dxf {
                fill,
                color: font.and_then(|f| child(f, "color")).and_then(color),
                bold: on("b"),
                italic: on("i"),
                underline: font.and_then(|f| child(f, "u")).map(|u| attr(u, "val") != Some("none")),
                strike: on("strike"),
            }
        })
        .collect()
}

#[derive(Clone, Copy)]
enum Cfvo {
    Min,
    Max,
    Num(f64),
    Percent(f64),
    Percentile(f64),
}

fn cfvo(n: Node<'_, '_>) -> Cfvo {
    let v = attr(n, "val").and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite()).unwrap_or(0.0);
    match attr(n, "type") {
        Some("min") => Cfvo::Min,
        Some("max") => Cfvo::Max,
        Some("percent") => Cfvo::Percent(v),
        Some("percentile") => Cfvo::Percentile(v),
        _ => Cfvo::Num(v),
    }
}

/// A threshold's value among `sorted` numbers.
fn threshold(c: Cfvo, sorted: &[f64]) -> f64 {
    let (min, max) = (sorted.first().copied().unwrap_or(0.0), sorted.last().copied().unwrap_or(0.0));
    match c {
        Cfvo::Min => min,
        Cfvo::Max => max,
        Cfvo::Num(v) => v,
        Cfvo::Percent(p) => min + (max - min) * p.clamp(0.0, 100.0) / 100.0,
        Cfvo::Percentile(p) => {
            // Linear between ranks, as PERCENTILE does.
            let pos = (sorted.len().saturating_sub(1)) as f64 * p.clamp(0.0, 100.0) / 100.0;
            let lo = sorted.get(pos.floor() as usize).copied().unwrap_or(min);
            let hi = sorted.get(pos.ceil() as usize).copied().unwrap_or(max);
            lo + (hi - lo) * pos.fract()
        }
    }
}

fn argb(n: Node<'_, '_>, color: &dyn Fn(Node<'_, '_>) -> Option<[f64; 3]>) -> [f64; 3] {
    color(n).unwrap_or([0.5; 3])
}

/// The cells of `sqref` ("A1:B9 D4") that hold something, in reading order.
fn cells_in<'c, 'a>(sqref: &str, cells: &'c Cells<'a>, work: &mut usize) -> Vec<(&'c (u32, u32), &'c Value<'a>)> {
    let mut out = Vec::new();
    for part in sqref.split_whitespace().take(256) {
        let (a, b) = part.split_once(':').unwrap_or((part, part));
        let (Some((r1, c1)), Some((r2, c2))) = (super::xlsx::col_index(a), super::xlsx::col_index(b)) else { continue };
        let (r1, r2, c1, c2) = (r1.min(r2), r1.max(r2), c1.min(c2), c1.max(c2));
        // Row by occupied row: a whole-column range is a million rows tall.
        let mut row = cells.range((r1, 0)..=(r2, u32::MAX)).next().map(|((r, _), _)| *r);
        while let Some(r) = row {
            for (k, v) in cells.range((r, c1)..=(r, c2)) {
                *work = work.saturating_add(1);
                if *work > MAX_WORK {
                    return out;
                }
                out.push((k, v));
            }
            // A range's start must not pass its end: the last row is checked first.
            row = r.checked_add(1).filter(|next| *next <= r2).and_then(|next| cells.range((next, 0)..=(r2, u32::MAX)).next().map(|((r, _), _)| *r));
        }
    }
    out
}

fn scale_color(v: f64, stops: &[(f64, [f64; 3])]) -> Option<[f64; 3]> {
    let first = stops.first()?;
    if v <= first.0 {
        return Some(first.1);
    }
    for w in stops.windows(2) {
        let (Some(a), Some(b)) = (w.first(), w.get(1)) else { continue };
        if v <= b.0 {
            let t = if b.0 > a.0 { (v - a.0) / (b.0 - a.0) } else { 1.0 };
            return Some([0, 1, 2].map(|i| a.1[i] + (b.1[i] - a.1[i]) * t));
        }
    }
    stops.last().map(|s| s.1)
}

/// The icons of a named icon set, lowest value first.
fn icon_set(name: &str) -> Vec<Icon> {
    const RED: [f64; 3] = [0.87, 0.2, 0.2];
    const YELLOW: [f64; 3] = [0.98, 0.75, 0.1];
    const GREEN: [f64; 3] = [0.2, 0.65, 0.3];
    const GREY: [f64; 3] = [0.55, 0.55, 0.55];
    match name {
        "3Arrows" => vec![Icon::Arrow(-1, RED), Icon::Arrow(0, YELLOW), Icon::Arrow(1, GREEN)],
        "3ArrowsGray" => vec![Icon::Arrow(-1, GREY), Icon::Arrow(0, GREY), Icon::Arrow(1, GREY)],
        "4Arrows" => vec![Icon::Arrow(-1, RED), Icon::Arrow(-1, YELLOW), Icon::Arrow(1, YELLOW), Icon::Arrow(1, GREEN)],
        "5Arrows" => vec![Icon::Arrow(-1, RED), Icon::Arrow(-1, YELLOW), Icon::Arrow(0, YELLOW), Icon::Arrow(1, YELLOW), Icon::Arrow(1, GREEN)],
        "3Flags" => vec![Icon::Flag(RED), Icon::Flag(YELLOW), Icon::Flag(GREEN)],
        "4Rating" => (1..=4).map(|i| Icon::Bars(i, 4)).collect(),
        "5Rating" => (0..=4).map(|i| Icon::Bars(i, 4)).collect(),
        "4RedToBlack" => vec![Icon::Dot([0.1; 3]), Icon::Dot(GREY), Icon::Dot([0.95, 0.6, 0.6]), Icon::Dot(RED)],
        "4TrafficLights" => vec![Icon::Dot([0.1; 3]), Icon::Dot(RED), Icon::Dot(YELLOW), Icon::Dot(GREEN)],
        n if n.starts_with('5') => vec![Icon::Dot(RED), Icon::Dot([0.95, 0.5, 0.2]), Icon::Dot(YELLOW), Icon::Dot([0.6, 0.8, 0.3]), Icon::Dot(GREEN)],
        _ => vec![Icon::Dot(RED), Icon::Dot(YELLOW), Icon::Dot(GREEN)],
    }
}

/// Whether `serial` (a date) falls in Excel's time period `p`, `today` being today's serial.
fn in_period(serial: f64, p: &str, today: f64) -> bool {
    let d = serial.floor();
    let t = today.floor();
    // Weeks start on Sunday; serial 1 (1900-01-01) was a Sunday.
    let week = |x: f64| ((x - 1.0) / 7.0).floor();
    let month = |x: f64| super::xlsx::year_month(x);
    match p {
        "today" => d == t,
        "yesterday" => d == t - 1.0,
        "tomorrow" => d == t + 1.0,
        "last7Days" => d <= t && d > t - 7.0,
        "thisWeek" => week(d) == week(t),
        "lastWeek" => week(d) == week(t) - 1.0,
        "nextWeek" => week(d) == week(t) + 1.0,
        "thisMonth" | "lastMonth" | "nextMonth" => {
            let (Some((y, m)), Some((ty, tm))) = (month(d), month(t)) else { return false };
            let (dm, tm) = (y * 12 + i64::from(m), ty * 12 + i64::from(tm));
            match p {
                "thisMonth" => dm == tm,
                "lastMonth" => dm == tm - 1,
                _ => dm == tm + 1,
            }
        }
        _ => false,
    }
}

/// Evaluate a worksheet's conditional formatting over its cells. `today` is today's date serial.
pub(crate) fn evaluate(
    root: Node<'_, '_>,
    cells: &Cells<'_>,
    dxfs: &[Dxf],
    color: &dyn Fn(Node<'_, '_>) -> Option<[f64; 3]>,
    today: f64,
    notes: &mut Notes,
) -> HashMap<(u32, u32), Look> {
    let mut out: HashMap<(u32, u32), Look> = HashMap::new();
    // Rules from every block, by priority (lowest first), each with its range.
    let mut rules: Vec<(f64, &str, Node<'_, '_>)> = Vec::new();
    for cf in children(root, "conditionalFormatting") {
        let sqref = attr(cf, "sqref").unwrap_or_default();
        for r in children(cf, "cfRule") {
            if rules.len() >= MAX_RULES {
                notes.add("conditional formatting rules beyond the first thousand of a sheet, left out");
                break;
            }
            rules.push((attr_f64(r, "priority").filter(|v| v.is_finite()).unwrap_or(f64::MAX), sqref, r));
        }
    }
    rules.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut stopped: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
    let mut work = 0usize;
    for (_, sqref, rule) in rules {
        let targets = cells_in(sqref, cells, &mut work);
        let numbers: Vec<f64> = {
            let mut v: Vec<f64> = targets.iter().filter_map(|(_, c)| c.number).collect();
            v.sort_by(f64::total_cmp);
            v
        };
        let dxf = attr_f64(rule, "dxfId").and_then(|i| dxfs.get(i.clamp(0.0, 1e6) as usize));
        let formulas: Vec<String> = children(rule, "formula").filter_map(|f| f.text()).map(|t| t.trim().to_owned()).collect();
        let num = |i: usize| formulas.get(i).and_then(|f| f.trim_matches('"').parse::<f64>().ok());
        let text_arg = |i: usize| formulas.get(i).map(|f| f.trim_matches('"').to_lowercase());
        let ty = attr(rule, "type").unwrap_or_default();
        // Which targets the rule matches, for the rules that apply a dxf.
        let matcher: Option<Matcher> = match ty {
            "cellIs" => {
                let op = attr(rule, "operator").unwrap_or("equal").to_owned();
                let (a, b) = (num(0), num(1));
                let (ta, _) = (text_arg(0), text_arg(1));
                Some(Box::new(move |v: &Value<'_>| match (v.number, a) {
                    (Some(x), Some(a)) => match op.as_str() {
                        "lessThan" => x < a,
                        "lessThanOrEqual" => x <= a,
                        "greaterThan" => x > a,
                        "greaterThanOrEqual" => x >= a,
                        "notEqual" => x != a,
                        "between" => b.is_some_and(|b| x >= a.min(b) && x <= a.max(b)),
                        "notBetween" => b.is_some_and(|b| x < a.min(b) || x > a.max(b)),
                        _ => x == a,
                    },
                    // Text compared with text, for equality only.
                    (None, None) => ta.as_ref().is_some_and(|t| match op.as_str() {
                        "equal" => v.text.to_lowercase() == *t,
                        "notEqual" => v.text.to_lowercase() != *t,
                        _ => false,
                    }),
                    _ => false,
                }))
            }
            "top10" => {
                let rank = attr_f64(rule, "rank").filter(|v| v.is_finite()).unwrap_or(10.0).clamp(0.0, 1e7);
                let count = if flag(attr(rule, "percent")) && attr(rule, "percent").is_some() {
                    (numbers.len() as f64 * rank / 100.0).floor().max(1.0) as usize
                } else {
                    rank as usize
                };
                let bottom = flag(attr(rule, "bottom")) && attr(rule, "bottom").is_some();
                // Ties with the last one in are in too.
                let cut = if bottom {
                    numbers.get(count.saturating_sub(1)).copied()
                } else {
                    numbers.len().checked_sub(count).and_then(|i| numbers.get(i)).copied()
                };
                let cut = cut.or(if bottom { numbers.last() } else { numbers.first() }.copied());
                Some(Box::new(move |v: &Value<'_>| match (v.number, cut) {
                    (Some(x), Some(c)) => {
                        if bottom {
                            x <= c
                        } else {
                            x >= c
                        }
                    }
                    _ => false,
                }))
            }
            "aboveAverage" => {
                let mean = if numbers.is_empty() { 0.0 } else { numbers.iter().sum::<f64>() / numbers.len() as f64 };
                let above = attr(rule, "aboveAverage").is_none_or(|v| flag(Some(v)));
                let equal = flag(attr(rule, "equalAverage")) && attr(rule, "equalAverage").is_some();
                Some(Box::new(move |v: &Value<'_>| {
                    v.number.is_some_and(|x| if above { x > mean || (equal && x == mean) } else { x < mean || (equal && x == mean) })
                }))
            }
            "duplicateValues" | "uniqueValues" => {
                let mut counts: HashMap<String, usize> = HashMap::new();
                for (_, v) in &targets {
                    *counts.entry(v.text.to_lowercase()).or_default() += 1;
                }
                let dup = ty == "duplicateValues";
                Some(Box::new(move |v: &Value<'_>| counts.get(&v.text.to_lowercase()).is_some_and(|n| (*n > 1) == dup)))
            }
            "containsText" | "notContainsText" | "beginsWith" | "endsWith" => {
                let t = attr(rule, "text").unwrap_or_default().to_lowercase();
                let ty = ty.to_owned();
                Some(Box::new(move |v: &Value<'_>| {
                    let s = v.text.to_lowercase();
                    match ty.as_str() {
                        "containsText" => s.contains(&t),
                        "notContainsText" => !s.contains(&t),
                        "beginsWith" => s.starts_with(&t),
                        _ => s.ends_with(&t),
                    }
                }))
            }
            "containsBlanks" => Some(Box::new(|v: &Value<'_>| v.text.trim().is_empty())),
            "notContainsBlanks" => Some(Box::new(|v: &Value<'_>| !v.text.trim().is_empty())),
            "containsErrors" => Some(Box::new(|v: &Value<'_>| v.text.starts_with('#'))),
            "notContainsErrors" => Some(Box::new(|v: &Value<'_>| !v.text.starts_with('#'))),
            "timePeriod" => {
                let p = attr(rule, "timePeriod").unwrap_or_default().to_owned();
                Some(Box::new(move |v: &Value<'_>| v.number.is_some_and(|x| in_period(x, &p, today))))
            }
            "colorScale" | "dataBar" | "iconSet" => None,
            _ => {
                notes.add("conditional formatting rules that need a formula, left out");
                continue;
            }
        };
        let stop = flag(attr(rule, "stopIfTrue")) && attr(rule, "stopIfTrue").is_some();
        for (key, v) in &targets {
            if stopped.contains(key) {
                continue;
            }
            let matched = match &matcher {
                Some(m) => {
                    if !m(v) {
                        continue;
                    }
                    if let Some(d) = dxf {
                        out.entry(**key).or_default().add(d);
                    }
                    true
                }
                None => {
                    let Some(x) = v.number else { continue };
                    let entry = out.entry(**key).or_default();
                    if let Some(cs) = child(rule, "colorScale") {
                        let vos: Vec<Cfvo> = children(cs, "cfvo").take(3).map(cfvo).collect();
                        let cols: Vec<[f64; 3]> = children(cs, "color").take(3).map(|c| argb(c, color)).collect();
                        let stops: Vec<(f64, [f64; 3])> = vos.iter().zip(cols).map(|(c, col)| (threshold(*c, &numbers), col)).collect();
                        if entry.fill.is_none() {
                            entry.fill = scale_color(x, &stops);
                        }
                    } else if let Some(db) = child(rule, "dataBar") {
                        let vos: Vec<Cfvo> = children(db, "cfvo").take(2).map(cfvo).collect();
                        let lo = vos.first().map_or(0.0, |c| threshold(*c, &numbers));
                        let hi = vos.get(1).map_or(1.0, |c| threshold(*c, &numbers));
                        let share = if hi > lo { ((x - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 1.0 };
                        let col = child(db, "color").map_or([0.39, 0.56, 0.78], |c| argb(c, color));
                        entry.bar = entry.bar.or(Some((share, col)));
                        entry.hide_value |= attr(db, "showValue").is_some_and(|v| !flag(Some(v)));
                    } else if let Some(is) = child(rule, "iconSet") {
                        let icons = icon_set(attr(is, "iconSet").unwrap_or("3TrafficLights1"));
                        let vos: Vec<f64> = children(is, "cfvo").take(5).map(|c| threshold(cfvo(c), &numbers)).collect();
                        let mut i = vos.iter().rposition(|t| x >= *t).unwrap_or(0);
                        if flag(attr(is, "reverse")) && attr(is, "reverse").is_some() {
                            i = icons.len().saturating_sub(1).saturating_sub(i);
                        }
                        if entry.icon.is_none() {
                            entry.icon = icons.get(i).copied();
                        }
                        entry.hide_value |= attr(is, "showValue").is_some_and(|v| !flag(Some(v)));
                    }
                    true
                }
            };
            if matched && stop {
                stopped.insert(**key);
            }
        }
    }
    if work > MAX_WORK {
        notes.add("conditional formatting on very large ranges, applied in part");
    }
    out
}

/// Draw an icon `size` points tall with its bottom-left corner at `(x, y)`.
pub(crate) fn draw_icon(page: &mut Page, icon: Icon, x: f64, y: f64, size: f64) {
    let s = size;
    let fill = |c: [f64; 3]| format!("{} {} {} rg ", n(c[0]), n(c[1]), n(c[2]));
    let poly = |pts: &[(f64, f64)]| {
        let mut o = String::new();
        for (i, (px, py)) in pts.iter().enumerate() {
            o.push_str(&format!("{} {} {} ", n(x + px * s), n(y + py * s), if i == 0 { "m" } else { "l" }));
        }
        o + "h f "
    };
    let ops = match icon {
        Icon::Arrow(dir, c) => {
            let up = [(0.5, 1.0), (0.95, 0.55), (0.65, 0.55), (0.65, 0.0), (0.35, 0.0), (0.35, 0.55), (0.05, 0.55)];
            let pts: Vec<(f64, f64)> = match dir {
                1 => up.to_vec(),
                -1 => up.iter().map(|(px, py)| (*px, 1.0 - py)).collect(),
                _ => up.iter().map(|(px, py)| (*py, *px)).collect(),
            };
            fill(c) + &poly(&pts)
        }
        Icon::Flag(c) => format!(
            "{}{}0.3 0.3 0.3 rg {} {} {} {} re f ",
            fill(c),
            poly(&[(0.2, 1.0), (0.9, 0.8), (0.2, 0.6)]),
            n(x + 0.15 * s),
            n(y),
            n(0.08 * s),
            n(s)
        ),
        Icon::Dot(c) => {
            let (cx, cy, r) = (x + s / 2.0, y + s / 2.0, s * 0.42);
            let k = r * 0.552_284_75;
            format!(
                "{}{} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c f ",
                fill(c),
                n(cx + r),
                n(cy),
                n(cx + r),
                n(cy + k),
                n(cx + k),
                n(cy + r),
                n(cx),
                n(cy + r),
                n(cx - k),
                n(cy + r),
                n(cx - r),
                n(cy + k),
                n(cx - r),
                n(cy),
                n(cx - r),
                n(cy - k),
                n(cx - k),
                n(cy - r),
                n(cx),
                n(cy - r),
                n(cx + k),
                n(cy - r),
                n(cx + r),
                n(cy - k),
                n(cx + r),
                n(cy)
            )
        }
        Icon::Bars(lit, of) => {
            let mut o = String::new();
            let of = of.max(1);
            let w = s / f64::from(of) * 0.7;
            for i in 0..of {
                let c = if i < lit { [0.27, 0.45, 0.77] } else { [0.8; 3] };
                let h = s * f64::from(i + 1) / f64::from(of);
                o.push_str(&format!("{}{} {} {} {} re f ", fill(c), n(x + f64::from(i) * s / f64::from(of)), n(y), n(w), n(h)));
            }
            o
        }
    };
    page.raw(&format!("q {ops}Q\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_and_scales_interpolate() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(threshold(Cfvo::Percentile(50.0), &v), 3.0);
        assert_eq!(threshold(Cfvo::Percent(25.0), &v), 2.0);
        assert_eq!(threshold(Cfvo::Min, &[]), 0.0);
        let c = scale_color(2.0, &[(1.0, [0.0; 3]), (3.0, [1.0; 3])]).unwrap();
        assert!((c[0] - 0.5).abs() < 1e-9);
        assert!(scale_color(1.0, &[]).is_none());
    }

    #[test]
    fn time_periods_follow_today() {
        // 45000 is 2023-03-15, a Wednesday.
        assert!(in_period(45000.0, "today", 45000.4));
        assert!(in_period(44999.0, "yesterday", 45000.0));
        assert!(in_period(44990.0, "lastMonth", 45020.0));
        assert!(in_period(44997.0, "thisWeek", 45000.0) && !in_period(44996.0, "thisWeek", 45000.0));
        assert!(!in_period(f64::NAN, "thisMonth", 45000.0));
    }
}
