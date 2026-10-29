//! Charts (DrawingML charts, ECMA-376 Part 1 §21.2) drawn from the values cached in the chart
//! part: bar and column charts (clustered, stacked and 100 % stacked, either direction), line
//! and area charts, pie and doughnut charts, and scatter charts, with the title, the value and
//! category axes (unless deleted), major gridlines, the legend and pie percentages. Colours come
//! from each series' or point's fill, else the theme's accent colours in Office's order. 3-D
//! charts are drawn flat. Text is set like the rest of the import, sizes as Office's defaults
//! unless the chart sets them.

use roxmltree::Node;

use super::canvas::{Page, n};
use super::xml::{self, attr, attr_f64, child, children, is};
use super::zip::Zip;
use super::{Fam, Notes, RunStyle};

/// Most series, points and categories read from one chart.
const MAX_SERIES: usize = 64;
const MAX_POINTS: usize = 4_096;

const AXIS_TEXT: [f64; 3] = [0.349, 0.349, 0.349];
const GRID: [f64; 3] = [0.851, 0.851, 0.851];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Bar,
    Line,
    Area,
    Pie,
    Doughnut,
    Scatter,
    /// Values on spokes around a centre; `filled` radar charts fill their polygons.
    Radar,
    /// A surface seen from above: value bands as colours over categories and series.
    Surface,
}

#[derive(Clone, Debug)]
struct Series {
    name: String,
    values: Vec<Option<f64>>,
    /// Scatter x values.
    xs: Vec<Option<f64>>,
    fill: Option<[f64; 3]>,
    line: Option<Option<[f64; 3]>>,
    marker: bool,
    /// Point colours (`c:dPt`), by point.
    points: Vec<(usize, [f64; 3])>,
    /// Its values written at its points (`c:dLbls/c:showVal`).
    labels: bool,
    /// A trendline: linear (0) or a moving average (1) over a period, and its colour.
    trend: Option<(u8, usize, Option<[f64; 3]>)>,
}

#[derive(Clone, Debug)]
struct Group {
    kind: Kind,
    horizontal: bool,
    /// 0 clustered or standard, 1 stacked, 2 100 % stacked, 3 a filled radar chart.
    stacking: u8,
    vary: bool,
    gap: f64,
    percent_labels: bool,
    value_labels: bool,
    series: Vec<Series>,
    /// Plotted against the secondary value axis, on the right.
    secondary: bool,
}

/// A chart read from its part, ready to draw at any size.
#[derive(Clone, Debug)]
pub(crate) struct Chart {
    title: Option<(String, f64)>,
    groups: Vec<Group>,
    categories: Vec<String>,
    /// The legend's side: 'r', 'b', 't' or 'l'.
    legend: Option<char>,
    axes: bool,
    gridlines: bool,
    grid_color: [f64; 3],
    border: Option<[f64; 3]>,
    rounded: bool,
    accents: Vec<[f64; 3]>,
}

/// The cached points of a `c:strRef`/`c:numRef`/`c:strLit`/`c:numLit` under `n`, by index.
fn points(n: Node<'_, '_>) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = n
        .descendants()
        .filter(|d| is(*d, "pt"))
        .take(MAX_POINTS)
        .filter_map(|p| Some((attr_f64(p, "idx").filter(|v| v.is_finite() && *v >= 0.0)?.min(MAX_POINTS as f64) as usize, xml::text_of(p, "v"))))
        .collect();
    out.sort_by_key(|(i, _)| *i);
    out
}

fn numbers(n: Option<Node<'_, '_>>) -> Vec<Option<f64>> {
    let Some(n) = n else { return Vec::new() };
    let pts = points(n);
    let len = pts.last().map_or(0, |(i, _)| i + 1);
    let mut out = vec![None; len.min(MAX_POINTS)];
    for (i, v) in pts {
        if let Some(slot) = out.get_mut(i) {
            *slot = v.trim().parse::<f64>().ok().filter(|v| v.is_finite());
        }
    }
    out
}

/// A DrawingML colour under `n` (`srgbClr`, `schemeClr` accents and text colours).
fn color(n: Node<'_, '_>, accents: &[[f64; 3]]) -> Option<[f64; 3]> {
    let c = n.children().find(Node::is_element)?;
    match c.tag_name().name() {
        "srgbClr" => super::hex_color(attr(c, "val")?),
        "sysClr" => super::hex_color(attr(c, "lastClr")?),
        "schemeClr" => {
            let v = attr(c, "val")?;
            let i = v.strip_prefix("accent").and_then(|d| d.parse::<usize>().ok())?;
            accents.get(i.checked_sub(1)?).copied()
        }
        _ => None,
    }
}

fn fill_of(sp: Option<Node<'_, '_>>, accents: &[[f64; 3]]) -> Option<Option<[f64; 3]>> {
    let sp = sp?;
    if child(sp, "noFill").is_some() {
        return Some(None);
    }
    child(sp, "solidFill").map(|f| color(f, accents))
}

fn line_of(sp: Option<Node<'_, '_>>, accents: &[[f64; 3]]) -> Option<Option<[f64; 3]>> {
    let ln = child(sp?, "ln")?;
    if child(ln, "noFill").is_some() {
        return Some(None);
    }
    child(ln, "solidFill").map(|f| color(f, accents))
}

fn on(n: Option<Node<'_, '_>>, name: &str) -> Option<bool> {
    n.and_then(|n| child(n, name)).map(|c| attr(c, "val").is_none_or(|v| v == "1" || v == "true"))
}

/// Read chart part `part`. `accents` are the theme's accent1–6.
pub(crate) fn read(zip: &Zip<'_>, part: &str, accents: &[[f64; 3]], notes: &mut Notes) -> Option<Chart> {
    let text = zip.text(part).ok()??;
    let d = xml::parse(&text).ok()?;
    let space = d.root_element();
    let chart = child(space, "chart")?;
    let plot = child(chart, "plotArea")?;
    let accents: Vec<[f64; 3]> = if accents.len() >= 6 {
        accents.to_vec()
    } else {
        // Office's default accents.
        [0x4472C4, 0xED7D31, 0xA5A5A5, 0xFFC000, 0x5B9BD5, 0x70AD47]
            .iter()
            .map(|v: &u32| [(v >> 16) & 255, (v >> 8) & 255, v & 255].map(|c| f64::from(c) / 255.0))
            .collect()
    };
    let mut groups = Vec::new();
    let mut categories: Vec<String> = Vec::new();
    for g in plot.children().filter(Node::is_element) {
        let kind = match g.tag_name().name() {
            "barChart" | "bar3DChart" => Kind::Bar,
            "lineChart" | "line3DChart" | "stockChart" => Kind::Line,
            "areaChart" | "area3DChart" => Kind::Area,
            "pieChart" | "pie3DChart" | "ofPieChart" => Kind::Pie,
            "doughnutChart" => Kind::Doughnut,
            "scatterChart" | "bubbleChart" => Kind::Scatter,
            "radarChart" => Kind::Radar,
            "surfaceChart" | "surface3DChart" => Kind::Surface,
            _ => continue,
        };
        let grouping = child(g, "grouping").and_then(|v| attr(v, "val")).unwrap_or("clustered");
        let dl = child(g, "dLbls");
        let mut series = Vec::new();
        for s in children(g, "ser").take(MAX_SERIES) {
            let sp = child(s, "spPr");
            let name =
                child(s, "tx").map(|t| points(t).into_iter().next().map(|(_, v)| v).unwrap_or_else(|| xml::text_of(t, "v"))).unwrap_or_default();
            if categories.is_empty()
                && let Some(c) = child(s, "cat")
            {
                categories = points(c).into_iter().map(|(_, v)| v).collect();
            }
            let pts = children(s, "dPt")
                .take(MAX_POINTS)
                .filter_map(|p| {
                    Some((attr_f64(child(p, "idx")?, "val")?.clamp(0.0, MAX_POINTS as f64) as usize, fill_of(child(p, "spPr"), &accents)??))
                })
                .collect();
            let marker = child(s, "marker").and_then(|m| child(m, "symbol")).and_then(|v| attr(v, "val")).is_none_or(|v| v != "none");
            let trend = child(s, "trendline").map(|t| {
                let kind = match child(t, "trendlineType").and_then(|v| attr(v, "val")) {
                    Some("movingAvg") => 1,
                    _ => 0,
                };
                let period = child(t, "period").and_then(|v| attr_f64(v, "val")).map_or(2, |v| v.clamp(2.0, 255.0) as usize);
                (kind, period, line_of(child(t, "spPr"), &accents).flatten())
            });
            series.push(Series {
                name: name.chars().take(256).collect(),
                values: numbers(child(s, "val").or_else(|| child(s, "yVal"))),
                xs: numbers(child(s, "xVal")),
                fill: fill_of(sp, &accents).flatten(),
                line: line_of(sp, &accents),
                marker: marker && matches!(kind, Kind::Line | Kind::Scatter),
                points: pts,
                labels: on(child(s, "dLbls"), "showVal") == Some(true)
                    || (on(dl, "showVal") == Some(true) && on(child(s, "dLbls"), "showVal") != Some(false)),
                trend,
            });
        }
        let ser_percent = children(g, "ser").any(|s| on(child(s, "dLbls"), "showPercent") == Some(true));
        let ser_value = children(g, "ser").any(|s| on(child(s, "dLbls"), "showVal") == Some(true));
        groups.push(Group {
            kind,
            horizontal: child(g, "barDir").and_then(|v| attr(v, "val")) == Some("bar"),
            // A filled radar chart is marked like a fourth stacking kind.
            stacking: match (grouping, child(g, "radarStyle").and_then(|v| attr(v, "val"))) {
                (_, Some("filled")) => 3,
                ("stacked", _) => 1,
                ("percentStacked", _) => 2,
                _ => 0,
            },
            vary: on(Some(g), "varyColors") == Some(true),
            gap: child(g, "gapWidth").and_then(|v| attr_f64(v, "val")).map_or(150.0, |v| v.clamp(0.0, 500.0)),
            percent_labels: ser_percent || on(dl, "showPercent") == Some(true),
            value_labels: ser_value || on(dl, "showVal") == Some(true),
            series,
            // On the right when its value axis is (`c:axPos r`, or top for bars).
            secondary: children(g, "axId").filter_map(|a| attr(a, "val")).any(|id| {
                children(plot, "valAx").any(|ax| {
                    child(ax, "axId").and_then(|a| attr(a, "val")) == Some(id)
                        && matches!(child(ax, "axPos").and_then(|p| attr(p, "val")), Some("r" | "t"))
                })
            }),
        });
    }
    if groups.is_empty() {
        return None;
    }
    // A secondary axis only when some group stays on the primary one.
    if groups.iter().all(|g| g.secondary) {
        groups.iter_mut().for_each(|g| g.secondary = false);
    }
    let deleted = |name: &str| children(plot, name).all(|a| on(Some(a), "delete") == Some(true));
    let axes = !(deleted("valAx") && deleted("catAx") && deleted("dateAx"));
    let grid = plot.children().filter(|a| a.tag_name().name().ends_with("Ax")).find_map(|a| child(a, "majorGridlines"));
    let gridlines = grid.is_some();
    let grid_color = line_of(grid.and_then(|g| child(g, "spPr")), &accents).flatten().unwrap_or(GRID);
    let title = if on(Some(chart), "autoTitleDeleted") == Some(true) && child(chart, "title").is_none() {
        None
    } else {
        child(chart, "title").map(|t| {
            let text: String = t.descendants().filter(|d| is(*d, "t")).filter_map(|d| d.text()).collect();
            let size = t
                .descendants()
                .find(|d| is(*d, "rPr") || is(*d, "defRPr"))
                .and_then(|r| attr_f64(r, "sz"))
                .map_or(14.0, |s| (s / 100.0).clamp(4.0, 72.0));
            // A title without text shows the series name.
            let text =
                if text.trim().is_empty() { groups.first().and_then(|g| g.series.first()).map(|s| s.name.clone()).unwrap_or_default() } else { text };
            (notes.drawable(&text), size)
        })
    };
    let legend = child(chart, "legend").map(|l| child(l, "legendPos").and_then(|p| attr(p, "val")).and_then(|v| v.chars().next()).unwrap_or('r'));
    // The chart area's outline: its own, else Office's light grey one (none when it says so).
    let border = match line_of(child(space, "spPr"), &accents) {
        Some(c) => c,
        None => Some(GRID),
    };
    let rounded = on(Some(space), "roundedCorners") != Some(false);
    let mut c = Chart {
        title,
        groups,
        categories: categories.iter().map(|s| notes.drawable(s)).collect(),
        legend,
        axes,
        gridlines,
        grid_color,
        border,
        rounded,
        accents,
    };
    for g in &mut c.groups {
        for s in &mut g.series {
            s.name = notes.drawable(&s.name);
        }
    }
    Some(c)
}

fn style(size: f64, color: [f64; 3]) -> RunStyle {
    RunStyle { fam: Fam::Sans, size, color, ..RunStyle::default() }
}

/// A rounded axis: step, lowest and highest gridline for values `lo..hi`.
fn nice(lo: f64, hi: f64) -> (f64, f64, f64) {
    let (lo, hi) = if lo == hi { (lo.min(0.0), hi.max(lo + 1.0)) } else { (lo, hi) };
    let span = (hi - lo).abs().max(1e-9);
    let raw = span / 6.0;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0].iter().map(|m| m * mag).find(|s| *s >= raw).unwrap_or(10.0 * mag);
    let step = if step.is_finite() && step > 0.0 { step } else { 1.0 };
    ((lo / step).floor() * step, (hi / step).ceil() * step, step)
}

fn label(v: f64) -> String {
    if v.fract().abs() < 1e-9 { format!("{v:.0}") } else { format!("{}", (v * 1e6).round() / 1e6) }
}

impl Chart {
    fn color(&self, i: usize) -> [f64; 3] {
        let base = self.accents.get(i % 6).copied().unwrap_or([0.5; 3]);
        // Past six, Office darkens the accents.
        if i >= 6 { base.map(|v| v * 0.6) } else { base }
    }

    fn series_color(&self, g: &Group, si: usize, s: &Series) -> [f64; 3] {
        let _ = g;
        s.fill.or_else(|| s.line.flatten()).unwrap_or_else(|| self.color(si))
    }

    /// The legend's entries: categories for pies and varied single series, else series.
    fn legend_entries(&self) -> Vec<(String, [f64; 3])> {
        let Some(g) = self.groups.first() else { return Vec::new() };
        if matches!(g.kind, Kind::Pie | Kind::Doughnut) || (g.vary && g.series.len() == 1 && g.kind == Kind::Bar) {
            let s = g.series.first();
            return self
                .categories
                .iter()
                .enumerate()
                .map(|(i, c)| (c.clone(), s.and_then(|s| s.points.iter().find(|(p, _)| *p == i).map(|(_, c)| *c)).unwrap_or_else(|| self.color(i))))
                .collect();
        }
        self.groups.iter().flat_map(|g| g.series.iter().enumerate().map(move |(i, s)| (s.name.clone(), self.series_color(g, i, s)))).collect()
    }
}

/// Draw `c` into the rectangle with bottom-left `(x, y)`, `w × h` points.
pub(crate) fn draw(page: &mut Page, c: &Chart, x: f64, y: f64, w: f64, h: f64) {
    if !(w.is_finite() && h.is_finite()) || w < 10.0 || h < 10.0 {
        return;
    }
    page.save();
    page.clip_rect(x, y, w, h);
    page.fill_rect(x, y, w, h, [1.0; 3]);
    if let Some(b) = c.border {
        let (x1, y1, x2, y2) = (x + 0.5, y + 0.5, x + w - 0.5, y + h - 0.5);
        let r = if c.rounded { (w.min(h) * 0.03).clamp(2.0, 10.0) } else { 0.0 };
        let k = r * 0.447_715_25;
        page.raw(&format!(
            "q {} {} {} RG 0.75 w {} {} m {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c {} {} l {} {} {} {} {} {} c S Q\n",
            n(b[0]),
            n(b[1]),
            n(b[2]),
            n(x1 + r),
            n(y1),
            n(x2 - r),
            n(y1),
            n(x2 - k),
            n(y1),
            n(x2),
            n(y1 + k),
            n(x2),
            n(y1 + r),
            n(x2),
            n(y2 - r),
            n(x2),
            n(y2 - k),
            n(x2 - k),
            n(y2),
            n(x2 - r),
            n(y2),
            n(x1 + r),
            n(y2),
            n(x1 + k),
            n(y2),
            n(x1),
            n(y2 - k),
            n(x1),
            n(y2 - r),
            n(x1),
            n(y1 + r),
            n(x1),
            n(y1 + k),
            n(x1 + k),
            n(y1),
            n(x1 + r),
            n(y1)
        ));
    }
    let pad = (w.min(h) * 0.03).clamp(4.0, 10.0);
    let (mut l, mut b, mut r, mut t) = (x + pad, y + pad, x + w - pad, y + h - pad);
    if let Some((text, size)) = &c.title {
        let st = style(*size, AXIS_TEXT);
        let tw = st.width(text);
        page.text(x + (w - tw) / 2.0, t - size * 0.95, &st, text);
        t -= size * 1.5;
    }
    // Legend.
    let entries = c.legend_entries();
    if let Some(side) = c.legend
        && !entries.is_empty()
    {
        let st = style(9.0, AXIS_TEXT);
        let key = 6.0;
        if matches!(side, 'r' | 'l') {
            let lw = entries.iter().map(|(n, _)| st.width(n)).fold(0.0, f64::max) + key + 6.0;
            let lh = entries.len() as f64 * 13.0;
            let lx = if side == 'r' { r - lw } else { l };
            let mut ly = (b + t) / 2.0 + lh / 2.0;
            for (name, col) in &entries {
                page.fill_rect(lx, ly - 9.0, key, key, *col);
                page.text(lx + key + 4.0, ly - 9.0, &st, name);
                ly -= 13.0;
            }
            if side == 'r' {
                r -= lw + 8.0;
            } else {
                l += lw + 8.0;
            }
        } else {
            let total: f64 = entries.iter().map(|(n, _)| st.width(n) + key + 14.0).sum();
            let mut lx = x + (w - total) / 2.0;
            let ly = if side == 'b' { b } else { t - 10.0 };
            for (name, col) in &entries {
                page.fill_rect(lx, ly, key, key, *col);
                page.text(lx + key + 4.0, ly, &st, name);
                lx += st.width(name) + key + 14.0;
            }
            if side == 'b' {
                b += 16.0;
            } else {
                t -= 16.0;
            }
        }
    }
    let Some(g) = c.groups.first() else {
        page.restore();
        return;
    };
    if matches!(g.kind, Kind::Pie | Kind::Doughnut) {
        draw_pie(page, c, g, (l, b, r, t));
    } else if g.kind == Kind::Radar {
        draw_radar(page, c, g, (l, b, r, t));
    } else if g.kind == Kind::Surface {
        draw_surface(page, c, g, (l, b, r, t));
    } else {
        draw_axes_chart(page, c, (l, b, r, t));
    }
    page.restore();
}

fn draw_pie(page: &mut Page, c: &Chart, g: &Group, (l, b, r, t): (f64, f64, f64, f64)) {
    let Some(s) = g.series.first() else { return };
    let vals: Vec<f64> = s.values.iter().map(|v| v.unwrap_or(0.0).max(0.0)).collect();
    let total: f64 = vals.iter().sum();
    if total <= 0.0 {
        return;
    }
    let (cx, cy) = ((l + r) / 2.0, (b + t) / 2.0);
    let rad = ((r - l).min(t - b) / 2.0 - 2.0).max(4.0);
    let mut a0 = std::f64::consts::FRAC_PI_2;
    for (i, v) in vals.iter().enumerate() {
        let sweep = v / total * std::f64::consts::TAU;
        if sweep <= 0.0 {
            continue;
        }
        let col = s.points.iter().find(|(p, _)| *p == i).map(|(_, c)| *c).unwrap_or_else(|| c.color(i));
        // Clockwise from twelve o'clock, in Béziers of at most a quarter turn.
        let mut ops = format!("q {} {} {} rg 1 1 1 RG 1.5 w {} {} m ", n(col[0]), n(col[1]), n(col[2]), n(cx), n(cy));
        let pieces = (sweep / std::f64::consts::FRAC_PI_2).ceil().max(1.0) as usize;
        let step = sweep / pieces as f64;
        let mut a = a0;
        ops.push_str(&format!("{} {} l ", n(cx + rad * a.cos()), n(cy + rad * a.sin())));
        for _ in 0..pieces {
            let a2 = a - step;
            let k = 4.0 / 3.0 * (step / 4.0).tan() * rad;
            let (p0, p3) = ((cx + rad * a.cos(), cy + rad * a.sin()), (cx + rad * a2.cos(), cy + rad * a2.sin()));
            let p1 = (p0.0 + k * a.sin(), p0.1 - k * a.cos());
            let p2 = (p3.0 - k * a2.sin(), p3.1 + k * a2.cos());
            ops.push_str(&format!("{} {} {} {} {} {} c ", n(p1.0), n(p1.1), n(p2.0), n(p2.1), n(p3.0), n(p3.1)));
            a = a2;
        }
        ops.push_str("h B Q\n");
        page.raw(&ops);
        if g.percent_labels || g.value_labels {
            let mid = a0 - sweep / 2.0;
            // Halves round away from zero, as Office rounds them (62.5 % shows as 63 %).
            let text = if g.percent_labels { format!("{}%", (v / total * 100.0).round()) } else { label(*v) };
            let st = style(9.0, [0.25; 3]);
            let tw = st.width(&text);
            page.text(cx + rad * 0.65 * mid.cos() - tw / 2.0, cy + rad * 0.65 * mid.sin() - 3.0, &st, &text);
        }
        a0 -= sweep;
    }
    if g.kind == Kind::Doughnut {
        let hole = rad * 0.5;
        let k = 0.552_284_75 * hole;
        page.raw(&format!(
            "q 1 1 1 rg {} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c f Q\n",
            n(cx + hole),
            n(cy),
            n(cx + hole),
            n(cy + k),
            n(cx + k),
            n(cy + hole),
            n(cx),
            n(cy + hole),
            n(cx - k),
            n(cy + hole),
            n(cx - hole),
            n(cy + k),
            n(cx - hole),
            n(cy),
            n(cx - hole),
            n(cy - k),
            n(cx - k),
            n(cy - hole),
            n(cx),
            n(cy - hole),
            n(cx + k),
            n(cy - hole),
            n(cx + hole),
            n(cy - k),
            n(cx + hole),
            n(cy)
        ));
    }
}

/// Spokes from the centre, one per category, with rings at the value steps, and each series as
/// a polygon through its values (filled when the chart asks).
fn draw_radar(page: &mut Page, c: &Chart, g: &Group, (l, b, r, t): (f64, f64, f64, f64)) {
    let ncat = g.series.iter().map(|s| s.values.len()).max().unwrap_or(0).max(c.categories.len());
    if ncat < 3 {
        return;
    }
    let hi = g.series.iter().flat_map(|s| s.values.iter().flatten().copied()).fold(0.0f64, f64::max);
    let (_, top, step) = nice(0.0, hi);
    let st = style(9.0, AXIS_TEXT);
    let (cx, cy) = ((l + r) / 2.0, (b + t) / 2.0 - 4.0);
    let rad = ((r - l).min(t - b) / 2.0 - 14.0).max(4.0);
    let at = |i: usize, v: f64| {
        let a = std::f64::consts::FRAC_PI_2 - std::f64::consts::TAU * i as f64 / ncat as f64;
        let k = if top > 0.0 { (v / top).clamp(0.0, 1.5) } else { 0.0 };
        (cx + rad * k * a.cos(), cy + rad * k * a.sin())
    };
    let rings = ((top / step).round() as usize).clamp(1, 20);
    for ring in 1..=rings {
        let v = step * ring as f64;
        let pts: String =
            (0..ncat).map(|i| at(i, v)).enumerate().map(|(i, (x, y))| format!("{} {} {} ", n(x), n(y), if i == 0 { "m" } else { "l" })).collect();
        page.raw(&format!("q {} {} {} RG 0.75 w {pts}h S Q\n", n(c.grid_color[0]), n(c.grid_color[1]), n(c.grid_color[2])));
    }
    for i in 0..ncat {
        let (x, y) = at(i, top);
        page.line(cx, cy, x, y, 0.75, c.grid_color);
        if let Some(name) = c.categories.get(i) {
            let (lx, ly) = at(i, top * 1.12);
            page.text(lx - st.width(name) / 2.0, ly - 3.0, &st, name);
        }
    }
    let filled = g.stacking == 3;
    for (si, s) in g.series.iter().enumerate() {
        let col = c.series_color(g, si, s);
        let pts: String = s
            .values
            .iter()
            .enumerate()
            .map(|(i, v)| at(i, v.unwrap_or(0.0)))
            .enumerate()
            .map(|(i, (x, y))| format!("{} {} {} ", n(x), n(y), if i == 0 { "m" } else { "l" }))
            .collect();
        if filled {
            page.raw(&format!("q {} {} {} rg {pts}h f Q\n", n(col[0]), n(col[1]), n(col[2])));
        } else {
            page.raw(&format!("q {} {} {} RG 2.25 w 1 j {pts}h S Q\n", n(col[0]), n(col[1]), n(col[2])));
        }
    }
}

/// A surface seen from above: a grid of categories across and series down, each cell coloured by
/// the value band it falls in (accent colours from low to high), as Excel's contour view shows it.
fn draw_surface(page: &mut Page, c: &Chart, g: &Group, (l, b, r, t): (f64, f64, f64, f64)) {
    let rows = g.series.len();
    let cols = g.series.iter().map(|s| s.values.len()).max().unwrap_or(0);
    if rows == 0 || cols == 0 {
        return;
    }
    let vals: Vec<f64> = g.series.iter().flat_map(|s| s.values.iter().flatten().copied()).collect();
    let (lo, hi, step) = nice(vals.iter().copied().fold(f64::INFINITY, f64::min).min(0.0), vals.iter().copied().fold(0.0, f64::max));
    let st = style(9.0, AXIS_TEXT);
    let label_w = g.series.iter().map(|s| st.width(&s.name)).fold(0.0, f64::max) + 6.0;
    let (gl, gb) = (l + label_w, b + 14.0);
    let (cw, ch) = ((r - gl).max(1.0) / cols as f64, (t - gb).max(1.0) / rows as f64);
    for (ri, s) in g.series.iter().enumerate() {
        let y = t - ch * (ri as f64 + 1.0);
        page.text(l, y + ch / 2.0 - 3.0, &st, &s.name);
        for (ci, v) in s.values.iter().enumerate() {
            let Some(v) = v else { continue };
            let band = (((v - lo) / step).floor().max(0.0) as usize).min(((hi - lo) / step).round().max(1.0) as usize - 1);
            page.fill_rect(gl + cw * ci as f64, y, cw, ch, c.color(band));
        }
    }
    for (ci, name) in c.categories.iter().enumerate().take(cols) {
        page.text(gl + cw * (ci as f64 + 0.5) - st.width(name) / 2.0, b, &st, name);
    }
}

/// A series' index and its points as drawn: x, y and the value.
type SeriesPoints = (usize, Vec<(f64, f64, f64)>);

fn draw_axes_chart(page: &mut Page, c: &Chart, (mut l, mut b, mut r, t): (f64, f64, f64, f64)) {
    let Some(first) = c.groups.first() else { return };
    let horizontal = first.horizontal && first.kind == Kind::Bar;
    let scatter = first.kind == Kind::Scatter;
    let ncat = c.groups.iter().flat_map(|g| g.series.iter().map(|s| s.values.len())).max().unwrap_or(0).max(c.categories.len()).max(1);
    // Value range of the primary or secondary axis's groups, stacked groups summing their series.
    let range = |secondary: bool| {
        let (mut lo, mut hi) = (0.0f64, 0.0f64);
        for g in c.groups.iter().filter(|g| g.secondary == secondary) {
            if g.stacking == 2 {
                hi = hi.max(1.0);
                continue;
            }
            for i in 0..ncat {
                let vals = g.series.iter().filter_map(|s| s.values.get(i).copied().flatten());
                if g.stacking == 1 {
                    let (pos, neg): (Vec<f64>, Vec<f64>) = vals.partition(|v| *v >= 0.0);
                    hi = hi.max(pos.iter().sum());
                    lo = lo.min(neg.iter().sum());
                } else {
                    for v in vals {
                        hi = hi.max(v);
                        lo = lo.min(v);
                    }
                }
            }
        }
        (lo, hi)
    };
    let (mut lo, mut hi) = range(false);
    let two = c.groups.iter().any(|g| g.secondary);
    let (v2lo, v2hi, step2) = {
        let (a, z) = range(true);
        nice(a, z)
    };
    if scatter {
        // Scatter values need not start at zero.
        let ys: Vec<f64> = first.series.iter().flat_map(|s| s.values.iter().flatten().copied()).collect();
        if let (Some(mn), Some(mx)) = (ys.iter().copied().reduce(f64::min), ys.iter().copied().reduce(f64::max)) {
            lo = mn.min(0.0);
            hi = mx;
        }
    }
    let (vlo, vhi, step) = nice(lo, hi);
    let st = style(9.0, AXIS_TEXT);
    // Room for the value labels and the category labels.
    let labels: Vec<f64> = (0..=((vhi - vlo) / step).round().clamp(0.0, 100.0) as usize).map(|i| vlo + step * i as f64).collect();
    let fmt = |v: f64| if first.stacking == 2 { format!("{:.0}%", v * 100.0) } else { label(v) };
    let labels2: Vec<f64> =
        if two { (0..=((v2hi - v2lo) / step2).round().clamp(0.0, 100.0) as usize).map(|i| v2lo + step2 * i as f64).collect() } else { Vec::new() };
    if c.axes && two && !horizontal {
        r -= labels2.iter().map(|v| st.width(&label(*v))).fold(0.0, f64::max) + 6.0;
    }
    if c.axes {
        if horizontal {
            l += c.categories.iter().map(|s| st.width(s)).fold(0.0, f64::max) + 6.0;
            b += 14.0;
        } else {
            l += labels.iter().map(|v| st.width(&fmt(*v))).fold(0.0, f64::max) + 6.0;
            b += 14.0;
        }
    }
    let (pw, ph) = ((r - l).max(1.0), (t - b).max(1.0));
    let span = (vhi - vlo).max(1e-9);
    // Value to position along the value axis.
    let vpos = |v: f64| if horizontal { l + (v - vlo) / span * pw } else { b + (v - vlo) / span * ph };
    let span2 = (v2hi - v2lo).max(1e-9);
    let vpos2 = |v: f64| if horizontal { l + (v - v2lo) / span2 * pw } else { b + (v - v2lo) / span2 * ph };
    if c.axes && !horizontal {
        for v in &labels2 {
            page.text(r + 4.0, vpos2(*v) - 3.0, &st, &label(*v));
        }
    }
    if c.gridlines || c.axes {
        for v in &labels {
            let p = vpos(*v);
            if c.gridlines {
                if horizontal {
                    page.line(p, b, p, t, 0.75, c.grid_color);
                } else {
                    page.line(l, p, r, p, 0.75, c.grid_color);
                }
            }
            if c.axes {
                let s = fmt(*v);
                if horizontal {
                    page.text(p - st.width(&s) / 2.0, b - 11.0, &st, &s);
                } else {
                    page.text(l - st.width(&s) - 4.0, p - 3.0, &st, &s);
                }
            }
        }
    }
    let slot = if horizontal { ph } else { pw } / ncat as f64;
    // Category position: slot centres (top to bottom for bars, as Office draws them reversed).
    let cpos = |i: usize| if horizontal { b + slot * (i as f64 + 0.5) } else { l + slot * (i as f64 + 0.5) };
    if c.axes && !scatter {
        if horizontal {
            page.line(l, b, l, t, 0.75, GRID);
        } else {
            page.line(l, vpos(0.0f64.clamp(vlo, vhi)), r, vpos(0.0f64.clamp(vlo, vhi)), 0.75, GRID);
        }
        for (i, name) in c.categories.iter().enumerate().take(ncat) {
            let p = cpos(i);
            if horizontal {
                page.text(l - st.width(name) - 4.0, p - 3.0, &st, name);
            } else if st.width(name) < slot * 1.2 || i % 2 == 0 {
                page.text(p - st.width(name) / 2.0, b - 11.0, &st, name);
            }
        }
    }
    let xs_range = if scatter {
        let xs: Vec<f64> = first.series.iter().flat_map(|s| s.xs.iter().flatten().copied()).collect();
        let (mn, mx) = (xs.iter().copied().reduce(f64::min).unwrap_or(0.0), xs.iter().copied().reduce(f64::max).unwrap_or(1.0));
        Some(nice(mn.min(0.0), mx))
    } else {
        None
    };
    for g in &c.groups {
        let nser = g.series.len().max(1);
        // A group on the secondary axis is placed against its range.
        let vpos = |v: f64| if g.secondary { vpos2(v) } else { vpos(v) };
        let (vlo, vhi) = if g.secondary { (v2lo, v2hi) } else { (vlo, vhi) };
        // Each series' points, for its labels and trendline.
        let mut placed: Vec<SeriesPoints> = Vec::new();
        match g.kind {
            Kind::Bar => {
                let group_w = slot / (1.0 + g.gap / 100.0);
                let bar_w = if g.stacking > 0 { group_w } else { group_w / nser as f64 };
                for i in 0..ncat {
                    let total: f64 = g.series.iter().filter_map(|s| s.values.get(i).copied().flatten()).map(f64::abs).sum();
                    let (mut up, mut down) = (0.0, 0.0);
                    for (si, s) in g.series.iter().enumerate() {
                        let Some(mut v) = s.values.get(i).copied().flatten() else { continue };
                        if g.stacking == 2 && total > 0.0 {
                            v /= total;
                        }
                        let (from, to) = if g.stacking > 0 {
                            let base = if v >= 0.0 { up } else { down };
                            if v >= 0.0 {
                                up += v;
                            } else {
                                down += v;
                            }
                            (base, base + v)
                        } else {
                            (0.0f64.clamp(vlo, vhi), v)
                        };
                        let col = s
                            .points
                            .iter()
                            .find(|(p, _)| *p == i)
                            .map(|(_, c)| *c)
                            .unwrap_or_else(|| if g.vary && nser == 1 { c.color(i) } else { c.series_color(g, si, s) });
                        let off = if g.stacking > 0 { -group_w / 2.0 } else { -group_w / 2.0 + bar_w * si as f64 };
                        let (a, z) = (vpos(from.min(to)), vpos(from.max(to)));
                        if horizontal {
                            page.fill_rect(a, cpos(i) + off, z - a, bar_w, col);
                        } else {
                            page.fill_rect(cpos(i) + off, a, bar_w, z - a, col);
                        }
                        let raw = s.values.get(i).copied().flatten().unwrap_or(v);
                        let (cx, end) = (cpos(i) + off + bar_w / 2.0, vpos(to));
                        match placed.iter_mut().find(|(k, _)| *k == si) {
                            Some((_, p)) => p.push((cx, end, raw)),
                            None => placed.push((si, vec![(cx, end, raw)])),
                        }
                    }
                }
            }
            Kind::Line | Kind::Area | Kind::Scatter => {
                let mut cum = vec![0.0; ncat];
                for (si, s) in g.series.iter().enumerate() {
                    let col = c.series_color(g, si, s);
                    let pts: Vec<(f64, f64)> = s
                        .values
                        .iter()
                        .enumerate()
                        .filter_map(|(i, v)| {
                            let v = (*v)?;
                            let v = if g.stacking > 0 {
                                let slot = cum.get_mut(i)?;
                                *slot += v;
                                *slot
                            } else {
                                v
                            };
                            let xp = match xs_range {
                                Some((xlo, xhi, _)) => l + (s.xs.get(i).copied().flatten().unwrap_or(i as f64) - xlo) / (xhi - xlo).max(1e-9) * pw,
                                None => cpos(i),
                            };
                            Some((xp, vpos(v)))
                        })
                        .collect();
                    if pts.is_empty() {
                        continue;
                    }
                    let raw: Vec<f64> = s.values.iter().flatten().copied().collect();
                    placed.push((si, pts.iter().zip(raw.iter().chain(std::iter::repeat(&0.0))).map(|((x, y), v)| (*x, *y, *v)).collect()));
                    let path: String =
                        pts.iter().enumerate().map(|(i, (px, py))| format!("{} {} {} ", n(*px), n(*py), if i == 0 { "m" } else { "l" })).collect();
                    if g.kind == Kind::Area {
                        let (fx, lx) = (pts.first().map_or(l, |p| p.0), pts.last().map_or(r, |p| p.0));
                        let base = vpos(0.0f64.clamp(vlo, vhi));
                        page.raw(&format!(
                            "q {} {} {} rg {path}{} {} l {} {} l h f Q\n",
                            n(col[0]),
                            n(col[1]),
                            n(col[2]),
                            n(lx),
                            n(base),
                            n(fx),
                            n(base)
                        ));
                    } else if s.line != Some(None) && !(g.kind == Kind::Scatter && s.line.is_none()) {
                        let lc = s.line.flatten().unwrap_or(col);
                        page.raw(&format!("q {} {} {} RG 2.25 w 1 J 1 j {path}S Q\n", n(lc[0]), n(lc[1]), n(lc[2])));
                    }
                    if s.marker {
                        // Round markers; a single series with varied colours colours each point.
                        let vary = g.vary && nser == 1;
                        for (i, (px, py)) in pts.iter().enumerate() {
                            let mc = if vary { c.color(i) } else { col };
                            let (rr, k) = (2.5, 2.5 * 0.552_284_75);
                            page.raw(&format!(
                                "q {} {} {} rg {} {} m {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c {} {} {} {} {} {} c f Q\n",
                                n(mc[0]),
                                n(mc[1]),
                                n(mc[2]),
                                n(px + rr),
                                n(*py),
                                n(px + rr),
                                n(py + k),
                                n(px + k),
                                n(py + rr),
                                n(*px),
                                n(py + rr),
                                n(px - k),
                                n(py + rr),
                                n(px - rr),
                                n(py + k),
                                n(px - rr),
                                n(*py),
                                n(px - rr),
                                n(py - k),
                                n(px - k),
                                n(py - rr),
                                n(*px),
                                n(py - rr),
                                n(px + k),
                                n(py - rr),
                                n(px + rr),
                                n(py - k),
                                n(px + rr),
                                n(*py)
                            ));
                        }
                    }
                }
            }
            Kind::Pie | Kind::Doughnut | Kind::Radar | Kind::Surface => {}
        }
        for (si, pts) in &placed {
            let Some(s) = g.series.get(*si) else { continue };
            // Values written by their points: above a column or point, right of a bar.
            if s.labels {
                for (x, y, v) in pts {
                    let text = label(*v);
                    if horizontal {
                        page.text(y + 3.0, x - 3.0, &st, &text);
                    } else {
                        page.text(x - st.width(&text) / 2.0, y + 3.0, &st, &text);
                    }
                }
            }
            // A trendline through the points, dotted in the series' colour.
            if let (Some((kind, period, colour)), false) = (s.trend, horizontal) {
                let col = colour.unwrap_or_else(|| c.series_color(g, *si, s));
                let line: Vec<(f64, f64)> = if kind == 1 {
                    pts.windows(period.min(pts.len()).max(1))
                        .map(|w| (w[w.len() - 1].0, w.iter().map(|p| p.1).sum::<f64>() / w.len() as f64))
                        .collect()
                } else {
                    let nf = pts.len() as f64;
                    let (sx, sy) = (pts.iter().map(|p| p.0).sum::<f64>(), pts.iter().map(|p| p.1).sum::<f64>());
                    let sxx = pts.iter().map(|p| p.0 * p.0).sum::<f64>();
                    let sxy = pts.iter().map(|p| p.0 * p.1).sum::<f64>();
                    let den = nf * sxx - sx * sx;
                    if pts.len() < 2 || den.abs() < 1e-9 {
                        Vec::new()
                    } else {
                        let k = (nf * sxy - sx * sy) / den;
                        let b0 = (sy - k * sx) / nf;
                        let (x1, x2) = (pts[0].0, pts[pts.len() - 1].0);
                        vec![(x1, b0 + k * x1), (x2, b0 + k * x2)]
                    }
                };
                if line.len() >= 2 {
                    let path: String =
                        line.iter().enumerate().map(|(i, (px, py))| format!("{} {} {} ", n(*px), n(*py), if i == 0 { "m" } else { "l" })).collect();
                    page.raw(&format!("q {} {} {} RG 1.5 w [1.5 2] 0 d {path}S Q\n", n(col[0]), n(col[1]), n(col[2])));
                }
            }
        }
    }
    if let (true, Some((xlo, xhi, xstep))) = (c.axes, xs_range) {
        let mut v = xlo;
        while v <= xhi + xstep * 0.01 {
            let p = l + (v - xlo) / (xhi - xlo).max(1e-9) * pw;
            let s = label(v);
            page.text(p - st.width(&s) / 2.0, b - 11.0, &st, &s);
            v += xstep;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_steps_are_round() {
        assert_eq!(nice(0.0, 93.0), (0.0, 100.0, 20.0));
        let (lo, hi, step) = nice(0.0, 0.0);
        assert!(lo == 0.0 && (hi - 1.0).abs() < 1e-9 && (step - 0.2).abs() < 1e-9);
        let (lo, hi, step) = nice(-5.0, f64::MAX / 1e10);
        assert!(lo.is_finite() && hi.is_finite() && step > 0.0);
    }
}
