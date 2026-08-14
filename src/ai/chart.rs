//! Chart drawing for `vis-chart` specs, the agent's own answer dialect.
//!
//! This is the single place charts are drawn. It produces `Vec<Line<'static>>`
//! for the TUI, and [`crate::cli::agent::render::render_vis_chart`] flattens
//! those lines to ANSI for `agent chat`'s stdout — that direction is lossless,
//! whereas emitting ANSI and parsing it back would throw styling away and then
//! guess it again.
//!
//! Every row of a line chart is laid out against one [`Geometry`]: a shared x
//! scale and left gutter. That is what makes a `dual-axes` spec truthful — the
//! volume bar for 14:35 sits under the price point for 14:35.
//!
//! A chart is one block inside a scrolling transcript, so the output is a
//! deterministic number of lines and never wider than the `width` given: the
//! caller wraps anything wider, and a wrapped braille row is a shredded plot.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::utils::text::{display_width, pad_display, strip_control_chars, truncate_width};

/// The price/value line.
const PLOT: Color = Color::Cyan;

/// One colour per series, in the order the spec lists them.
///
/// A comparison chart draws several securities on one canvas, and with a single
/// colour the curves were indistinguishable — the whole point of the chart is
/// telling them apart. Cyan stays first so a single-series chart looks unchanged.
const SERIES_COLORS: [Color; 6] = [
    PLOT,
    Color::Yellow,
    Color::Magenta,
    Color::Green,
    Color::Red,
    Color::Blue,
];

/// The colour of the `i`th series, cycling if a spec has more than the palette.
fn series_color(i: usize) -> Color {
    SERIES_COLORS[i % SERIES_COLORS.len()]
}
/// The volume histogram under it.
const VOLUME: Color = Color::Blue;
/// Axes, ticks and legend.
const AXIS: Color = Color::DarkGray;

/// Braille rows given to the line plot and to the volume histogram below it.
const LINE_ROWS: usize = 6;
const COL_ROWS: usize = 2;
/// A plot narrower than this cannot carry a readable curve, so the spec falls
/// back to a table.
const MIN_PLOT_W: usize = 12;
/// Cap so a very wide terminal does not stretch a 20-point series across 300
/// columns of mostly-empty canvas.
const MAX_PLOT_W: usize = 120;

const BRAILLE_DOTS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

/// One extracted numeric series from a vis-chart spec.
pub(crate) struct ChartSeries {
    /// `"line"` or `"column"`; decides which canvas it lands on.
    kind: String,
    label: String,
    values: Vec<f64>,
}

/// Draw a vis-chart spec as at most a screenful of styled lines.
pub fn render(spec: &Value, width: usize) -> Vec<Line<'static>> {
    let chart_type = spec.get("type").and_then(Value::as_str).unwrap_or_default();
    let title = strip_control_chars(
        spec.get("title")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    let (categories, series) = chart_series(spec);
    let mut out = Vec::new();
    if !title.is_empty() {
        out.push(Line::from(Span::styled(
            format!("  {}", truncate_width(&title, width.saturating_sub(2))),
            Style::default().fg(AXIS),
        )));
    }
    out.extend(match chart_type {
        "line" | "area" | "dual-axes" => line_chart(&categories, &series, width),
        "column" | "bar" => bar_chart(&categories, &series, width),
        "pie" => pie_chart(&categories, &series, width),
        _ => table_chart(&categories, &series),
    });
    out
}

/// Normalize the observed vis-chart data shapes into (categories, series):
/// - `{categories: [...], series: [{type, data: [...], axisYTitle}]}` (dual-axes)
/// - `{data: [{category|time|name|x, value, group?}]}` (column / pie / line)
pub(crate) fn chart_series(spec: &Value) -> (Vec<String>, Vec<ChartSeries>) {
    // Every string pulled out of the spec is server/LLM-controlled and ends up
    // on the terminal verbatim, so it is stripped of control characters here,
    // once, at the single point where the spec is decoded.
    if let Some(series) = spec.get("series").and_then(Value::as_array) {
        let categories = spec
            .get("categories")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|v| strip_control_chars(v.as_str().unwrap_or_default()))
                    .collect()
            })
            .unwrap_or_default();
        let series = series
            .iter()
            .map(|s| ChartSeries {
                kind: s
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("line")
                    .to_string(),
                // A comparison names each series; `axisYTitle` is the axis's
                // name and stands in only for a dual-axes spec, which has one
                // series per axis and no series names. Without this a
                // multi-series chart drew several colours with nothing to say
                // which security was which.
                label: strip_control_chars(
                    ["name", "label", "axisYTitle"]
                        .iter()
                        .find_map(|k| s.get(*k).and_then(Value::as_str))
                        .unwrap_or_default(),
                ),
                values: s
                    .get("data")
                    .and_then(Value::as_array)
                    // Keep positional alignment with `categories`: a
                    // non-numeric/`null` point becomes 0.0 rather than being
                    // dropped, otherwise every later value would shift onto
                    // the wrong category.
                    .map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect())
                    .unwrap_or_default(),
            })
            .collect();
        return (categories, series);
    }
    if let Some(data) = spec.get("data").and_then(Value::as_array) {
        // The row's x field is named for the axis it represents, so a time
        // series says `time` where a categorical one says `category`. Reading
        // only `category` left a `{time, value}` line chart with no labels and
        // no y scale, drawn as an anonymous block histogram.
        let category_of = |row: &Value| {
            ["category", "time", "name", "label", "x"]
                .iter()
                .find_map(|k| row.get(*k).and_then(Value::as_str))
                .map(strip_control_chars)
                .unwrap_or_default()
        };
        // The series kind comes from the spec, not from this shape: a `line`
        // spec written this way is still a line.
        let kind = match spec.get("type").and_then(Value::as_str) {
            Some("column" | "bar" | "pie") => "column",
            _ => "line",
        };
        let mut categories: Vec<String> = Vec::new();
        let mut groups: Vec<(String, Vec<f64>)> = Vec::new();
        for row in data {
            let cat = category_of(row);
            if !categories.contains(&cat) {
                categories.push(cat);
            }
            let group =
                strip_control_chars(row.get("group").and_then(Value::as_str).unwrap_or_default());
            let value = row.get("value").and_then(Value::as_f64).unwrap_or(0.0);
            match groups.iter_mut().find(|(g, _)| *g == group) {
                Some((_, vals)) => vals.push(value),
                None => groups.push((group, vec![value])),
            }
        }
        let series = groups
            .into_iter()
            .map(|(label, values)| ChartSeries {
                kind: kind.to_string(),
                label,
                values,
            })
            .collect();
        return (categories, series);
    }
    (Vec::new(), Vec::new())
}

/// The one x scale and left gutter every row of a line chart is drawn against.
///
/// Before this existed the line canvas and the volume row each invented their
/// own width and indent, so the two series of a `dual-axes` spec did not line up
/// in x at all — a volume spike appeared under the wrong point in time.
struct Geometry {
    /// Width of the y-label gutter, in columns.
    gutter: usize,
    /// Width of the plot area, in cells; each cell is 2 braille dot columns.
    plot_w: usize,
}

impl Geometry {
    /// Dot column holding data point `i` of `n`, spread edge to edge.
    fn x_dot(&self, i: usize, n: usize) -> usize {
        if n <= 1 {
            0
        } else {
            i * (self.plot_w * 2 - 1) / (n - 1)
        }
    }

    /// The gutter as blanks, for rows that carry no y label.
    fn blank_gutter(&self) -> String {
        " ".repeat(self.gutter)
    }
}

/// A braille line plot, optionally with a volume histogram sharing its x axis.
fn line_chart(categories: &[String], series: &[ChartSeries], width: usize) -> Vec<Line<'static>> {
    // Series with no data points are dropped up front: an empty line series has
    // no min/max and would otherwise poison the shared canvas (division by a
    // zero span, garbage plotting positions).
    let lines: Vec<&ChartSeries> = series
        .iter()
        .filter(|s| s.kind != "column" && !s.values.is_empty())
        .collect();
    let columns: Vec<&ChartSeries> = series
        .iter()
        .filter(|s| s.kind == "column" && !s.values.is_empty())
        .collect();
    if lines.is_empty() && columns.is_empty() {
        return Vec::new();
    }

    // Y labels come first: they set the gutter, which sets the plot width.
    let scale = (!lines.is_empty()).then(|| y_scale(&lines));
    let labels = scale
        .as_ref()
        .map(|(min, max)| y_labels(*min, *max))
        .unwrap_or_default();
    let gutter = labels.iter().map(String::len).max().unwrap_or(0);
    // `width` is a hard ceiling. Too narrow to draw is a table, not a squeezed
    // chart — the old code clamped the plot width *up* to a floor of 16, which
    // could exceed the space available and get wrapped in half by the caller.
    let Some(plot_w) = width
        .checked_sub(gutter + 1)
        .filter(|w| *w >= MIN_PLOT_W)
        .map(|w| w.min(MAX_PLOT_W))
    else {
        return table_chart(categories, series);
    };
    let geo = Geometry { gutter, plot_w };
    let axis = Style::default().fg(AXIS);

    let mut out = Vec::new();
    if let Some((min, max)) = scale {
        for (r, row) in line_canvas(&lines, min, max, &geo).iter().enumerate() {
            let label = labels.get(r).cloned().unwrap_or_default();
            let mut spans = vec![
                Span::styled(format!("{label:>gutter$}"), axis),
                Span::styled("┤", axis),
            ];
            spans.extend(colored_row(row));
            out.push(Line::from(spans));
        }
    }
    for (ci, s) in columns.iter().enumerate() {
        let color = column_color(lines.len(), ci, columns.len());
        for row in column_canvas(s, &geo) {
            out.push(Line::from(vec![
                Span::styled(geo.blank_gutter(), axis),
                Span::styled("┤", axis),
                Span::styled(braille_row(&row), Style::default().fg(color)),
            ]));
        }
    }
    // The axis rule anchors both plots to the same baseline before the ticks.
    out.push(Line::from(vec![
        Span::styled(geo.blank_gutter(), axis),
        Span::styled(format!("└{}", "─".repeat(plot_w)), axis),
    ]));
    let n = series.iter().map(|s| s.values.len()).max().unwrap_or(0);
    let ticks = x_axis_row(categories, &geo, n);
    if !ticks.trim().is_empty() {
        out.push(Line::from(Span::styled(
            format!("{} {ticks}", geo.blank_gutter()),
            axis,
        )));
    }
    out.extend(legend_lines(&lines, &columns, &geo, width));
    out
}

/// Render one canvas row as braille glyphs.
fn braille_row(row: &[u8]) -> String {
    row.iter().map(|&bits| braille(bits)).collect()
}

/// Render one canvas row as braille glyphs, in each cell's owning colour.
///
/// Adjacent cells of the same colour are merged, so a row is a handful of spans
/// rather than one per column.
fn colored_row(row: &[Cell]) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for cell in row {
        let color = series_color(cell.owner);
        let glyph = braille(cell.bits);
        match spans.last_mut() {
            // A blank cell has no owner worth honouring, so it joins whatever run
            // it sits in rather than breaking it.
            Some(last) if cell.bits == 0 || last.style.fg == Some(color) => {
                let mut content = last.content.to_string();
                content.push(glyph);
                last.content = content.into();
            }
            _ => spans.push(Span::styled(glyph.to_string(), Style::default().fg(color))),
        }
    }
    spans
}

/// The colour of a column series.
///
/// A lone column series keeps [`VOLUME`]: the overwhelmingly common shape is a
/// price line over a volume histogram, where blue bars under a cyan line is the
/// reading every other chart in the terminal uses. Several column series instead
/// continue the line series' palette, because then telling them apart matters
/// more than the convention.
fn column_color(lines: usize, index: usize, columns: usize) -> Color {
    if columns == 1 {
        VOLUME
    } else {
        series_color(lines + index)
    }
}

/// The (min, max) covering every line series.
fn y_scale(lines: &[&ChartSeries]) -> (f64, f64) {
    lines
        .iter()
        .flat_map(|s| s.values.iter().copied())
        .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)))
}

/// One label per plot row, evenly spaced from `max` down to `min`.
///
/// Only the top and bottom rows used to be labelled, leaving four bare `┤` in
/// between and no way to read a value off the middle of the curve. Precision
/// follows the span so a narrow range does not collapse into repeated labels.
#[allow(clippy::cast_precision_loss)]
fn y_labels(min: f64, max: f64) -> Vec<String> {
    let span = max - min;
    let prec = if span >= 10.0 {
        1
    } else if span >= 1.0 {
        2
    } else {
        3
    };
    (0..LINE_ROWS)
        .map(|r| {
            let v = max - span * r as f64 / (LINE_ROWS - 1) as f64;
            format!("{v:.prec$}")
        })
        .collect()
}

/// Plot every line series onto one braille canvas.
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
fn line_canvas(lines: &[&ChartSeries], min: f64, max: f64, geo: &Geometry) -> Vec<Vec<Cell>> {
    let span = if (max - min).abs() < f64::EPSILON {
        1.0
    } else {
        max - min
    };
    let mut canvas = vec![vec![Cell::default(); geo.plot_w]; LINE_ROWS];
    let n = lines.iter().map(|s| s.values.len()).max().unwrap_or(0);
    let last_dot_row = (LINE_ROWS * 4 - 1) as f64;
    for (si, s) in lines.iter().enumerate() {
        // Each series is drawn onto its own layer, then merged into the shared
        // canvas: bits accumulate so no curve disappears where two cross, but the
        // owner is recorded so the cell can be painted in that series' colour.
        let mut layer = vec![vec![0u8; geo.plot_w]; LINE_ROWS];
        let mut prev: Option<(usize, usize)> = None;
        for (i, &v) in s.values.iter().enumerate() {
            let x = geo.x_dot(i, n);
            let y = ((max - v) / span * last_dot_row).round() as usize;
            match prev {
                // Draw the actual segment. Filling a single column at the
                // midpoint left the dot columns between points empty, so a
                // 22-point series over 80 columns read as scatter, not a trend.
                Some(from) => plot_segment(&mut layer, from, (x, y)),
                None => plot(&mut layer, x, y),
            }
            prev = Some((x, y));
        }
        for (row, layer_row) in canvas.iter_mut().zip(&layer) {
            for (cell, &bits) in row.iter_mut().zip(layer_row) {
                cell.add(bits, si);
            }
        }
    }
    canvas
}

/// One braille cell: which dots are lit, and which series lit the most of them.
///
/// Two curves can cross inside one cell, and a cell has one colour. The series
/// contributing the most dots owns it, earliest series winning a tie — so a
/// crossing tints toward whichever curve is actually denser there instead of
/// whichever happened to be drawn last.
#[derive(Clone, Default)]
struct Cell {
    bits: u8,
    owner: usize,
    owner_dots: u32,
}

impl Cell {
    fn add(&mut self, bits: u8, series: usize) {
        if bits == 0 {
            return;
        }
        let dots = bits.count_ones();
        if self.bits == 0 || dots > self.owner_dots {
            self.owner = series;
            self.owner_dots = dots;
        }
        self.bits |= bits;
    }
}

/// Draw one column series as bottom-anchored braille bars on the shared x scale,
/// so a bar sits under the point of the line it belongs to.
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
fn column_canvas(s: &ChartSeries, geo: &Geometry) -> Vec<Vec<u8>> {
    let mut canvas = vec![vec![0u8; geo.plot_w]; COL_ROWS];
    let max = s.values.iter().copied().fold(f64::MIN, f64::max).max(1.0);
    let dots = geo.plot_w * 2;
    let height = COL_ROWS * 4;
    let n = s.values.len();
    for (i, &v) in s.values.iter().enumerate() {
        let bar = ((v / max) * height as f64)
            .round()
            .clamp(1.0, height as f64) as usize;
        let from = geo.x_dot(i, n);
        // A bar is as wide as the gap to the next point, less a dot of air once
        // there is room for one, so neighbouring bars stay distinguishable.
        let mut to = if i + 1 < n {
            geo.x_dot(i + 1, n).max(from + 1)
        } else {
            dots
        };
        if to.saturating_sub(from) >= 3 {
            to -= 1;
        }
        for x in from..to.min(dots) {
            for k in 0..bar {
                plot(&mut canvas, x, height - 1 - k);
            }
        }
    }
    canvas
}

/// Lay out category labels at their true x offsets.
///
/// This replaced a bare `first … last`, which told the reader nothing about
/// where in the series anything happened. Labels are measured in display columns
/// so CJK categories do not drift, and any tick that would touch its neighbour
/// is dropped rather than overlapped.
fn x_axis_row(categories: &[String], geo: &Geometry, n: usize) -> String {
    if categories.is_empty() || n == 0 {
        return String::new();
    }
    let count = (geo.plot_w / 12).clamp(2, 6).min(categories.len());
    let mut row = String::new();
    let mut cursor = 0usize;
    for k in 0..count {
        let i = if count <= 1 {
            0
        } else {
            k * (n - 1) / (count - 1)
        };
        let Some(label) = categories.get(i) else {
            continue;
        };
        let w = display_width(label);
        let mut x = geo.x_dot(i, n) / 2;
        // Keep the rightmost tick inside the plot instead of running past it.
        if x + w > geo.plot_w {
            x = geo.plot_w.saturating_sub(w);
        }
        // A tick that would touch its neighbour is dropped, not overlapped.
        if x < cursor || (cursor > 0 && x < cursor + 1) {
            continue;
        }
        row.push_str(&" ".repeat(x - cursor));
        row.push_str(label);
        cursor = x + w;
    }
    row
}

/// A legend naming each series behind its marker, wrapped to `width`.
///
/// Series names are axis titles straight from the spec — `成交量 ( 万股 )` and
/// friends — so on a narrow terminal they have to wrap or be clipped rather than
/// run off the edge and be folded by the caller.
fn legend_lines(
    lines: &[&ChartSeries],
    columns: &[&ChartSeries],
    geo: &Geometry,
    width: usize,
) -> Vec<Line<'static>> {
    let axis = Style::default().fg(AXIS);
    let indent = geo.gutter + 1;
    let avail = width.saturating_sub(indent);
    // A marker plus a space is the minimum an entry occupies.
    if avail < 3 {
        return Vec::new();
    }
    // The legend is the key to the colours, so it has to derive them the same way
    // the canvas does — index in, index out.
    let items: Vec<(&str, Color, String)> = lines
        .iter()
        .enumerate()
        .map(|(i, s)| ("⣿", series_color(i), s.label.clone()))
        .chain(columns.iter().enumerate().map(|(i, s)| {
            (
                "▇",
                column_color(lines.len(), i, columns.len()),
                s.label.clone(),
            )
        }))
        .filter(|(_, _, label)| !label.is_empty())
        .collect();

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for (marker, color, label) in items {
        let text = truncate_width(&label, avail - 2);
        let item_w = 2 + display_width(&text);
        let gap = if used == 0 { 0 } else { 3 };
        if used > 0 && used + gap + item_w > avail {
            out.push(Line::from(std::mem::take(&mut spans)));
            used = 0;
        }
        if used == 0 {
            spans.push(Span::styled(" ".repeat(indent), axis));
        } else {
            spans.push(Span::styled("   ", axis));
            used += 3;
        }
        spans.push(Span::styled(marker, Style::default().fg(color)));
        spans.push(Span::styled(format!(" {text}"), axis));
        used += item_w;
    }
    if !spans.is_empty() {
        out.push(Line::from(spans));
    }
    out
}

/// Plot the straight segment between two dot positions (Bresenham), so
/// consecutive data points read as a connected line.
///
/// Bresenham needs signed error terms. The casts cannot wrap or lose a sign in
/// practice: every coordinate is a canvas offset, bounded by
/// `MAX_PLOT_W * 2` dot columns and `LINE_ROWS * 4` dot rows, and the walk only
/// ever moves between the two given points.
#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
fn plot_segment(canvas: &mut [Vec<u8>], (x0, y0): (usize, usize), (x1, y1): (usize, usize)) {
    let (mut x, mut y) = (x0 as isize, y0 as isize);
    let (tx, ty) = (x1 as isize, y1 as isize);
    let dx = (tx - x).abs();
    let dy = -(ty - y).abs();
    let sx = if x < tx { 1 } else { -1 };
    let sy = if y < ty { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        plot(canvas, x as usize, y as usize);
        if x == tx && y == ty {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

fn plot(canvas: &mut [Vec<u8>], dot_x: usize, dot_y: usize) {
    let (cx, cy) = (dot_x / 2, dot_y / 4);
    if let Some(cell) = canvas.get_mut(cy).and_then(|row| row.get_mut(cx)) {
        *cell |= BRAILLE_DOTS[dot_y % 4][dot_x % 2];
    }
}

fn braille(bits: u8) -> char {
    char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ')
}

/// Horizontal ▓ bars, one per (category, group) pair.
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
fn bar_chart(categories: &[String], series: &[ChartSeries], width: usize) -> Vec<Line<'static>> {
    let max = series
        .iter()
        .flat_map(|s| s.values.iter().copied())
        .fold(f64::MIN, f64::max)
        .max(f64::EPSILON);
    let label_w = categories
        .iter()
        .map(|c| display_width(c))
        .max()
        .unwrap_or(0)
        + series
            .iter()
            .map(|s| display_width(&s.label))
            .max()
            .unwrap_or(0)
        + 1;
    let bar_w = width.saturating_sub(label_w + 12).clamp(10, 60);
    let mut out = Vec::new();
    for (ci, cat) in categories.iter().enumerate() {
        for s in series {
            let Some(&v) = s.values.get(ci) else { continue };
            let n = ((v / max) * bar_w as f64).round().max(1.0) as usize;
            let label = if s.label.is_empty() {
                cat.clone()
            } else {
                format!("{cat} {}", s.label)
            };
            out.push(Line::from(vec![
                Span::styled(
                    format!("  {} ", pad_display(&label, label_w)),
                    Style::default().fg(AXIS),
                ),
                Span::styled("▓".repeat(n), Style::default().fg(PLOT)),
                Span::styled(format!(" {v}"), Style::default().fg(AXIS)),
            ]));
        }
    }
    out
}

/// Proportion bars with percentages.
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
fn pie_chart(categories: &[String], series: &[ChartSeries], width: usize) -> Vec<Line<'static>> {
    let Some(s) = series.first() else {
        return Vec::new();
    };
    let total: f64 = s.values.iter().sum();
    if total <= 0.0 {
        return Vec::new();
    }
    let label_w = categories
        .iter()
        .map(|c| display_width(c))
        .max()
        .unwrap_or(0);
    let bar_w = width.saturating_sub(label_w + 12).clamp(10, 40);
    let mut out = Vec::new();
    for (ci, cat) in categories.iter().enumerate() {
        let Some(&v) = s.values.get(ci) else { continue };
        let pct = v / total * 100.0;
        // Clamp the bar length to `bar_w`: with mixed-sign values a single
        // slice's `pct` can exceed 100 (e.g. data [100, -50] → total 50 →
        // 200%), which would otherwise print a bar wider than the terminal.
        let n = ((pct / 100.0) * bar_w as f64)
            .round()
            .clamp(1.0, bar_w as f64) as usize;
        out.push(Line::from(vec![
            Span::styled(
                format!("  {} ", pad_display(cat, label_w)),
                Style::default().fg(AXIS),
            ),
            Span::styled("▓".repeat(n), Style::default().fg(PLOT)),
            Span::styled(format!(" {pct:.1}%"), Style::default().fg(AXIS)),
        ]));
    }
    out
}

/// Fallback: plain aligned table of category/value pairs per series. Also what a
/// terminal too narrow for a plot gets.
fn table_chart(categories: &[String], series: &[ChartSeries]) -> Vec<Line<'static>> {
    let label_w = categories
        .iter()
        .map(|c| display_width(c))
        .max()
        .unwrap_or(0);
    categories
        .iter()
        .enumerate()
        .map(|(ci, cat)| {
            let values: Vec<String> = series
                .iter()
                .filter_map(|s| s.values.get(ci).map(|v| format!("{v}")))
                .collect();
            Line::from(Span::styled(
                format!("  {}  {}", pad_display(cat, label_w), values.join("  ")),
                Style::default().fg(AXIS),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{render, LINE_ROWS, MIN_PLOT_W};
    use crate::utils::text::display_width;

    /// The transcript's real dual-axes shape: a price line plus a volume column
    /// series over shared categories.
    fn dual_axes() -> serde_json::Value {
        serde_json::json!({
            "type": "dual-axes",
            "categories": ["13:30", "13:35", "13:40", "13:45", "13:50", "13:55"],
            "title": "SPCX",
            "series": [
                {"type": "line", "data": [141.64, 140.18, 138.40, 138.95, 137.41, 137.58],
                 "axisYTitle": "股价 ($)"},
                {"type": "column", "data": [232.0, 191.0, 269.0, 185.0, 227.0, 112.0],
                 "axisYTitle": "成交量 ( 万股 )"}
            ]
        })
    }

    /// Plain text of each rendered line.
    fn text(spec: &serde_json::Value, width: usize) -> Vec<String> {
        render(spec, width)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
            .collect()
    }

    /// A chart is one block of a scrolling transcript: the caller wraps anything
    /// wider than `width`, and a wrapped braille row is a shredded plot.
    #[test]
    fn never_exceeds_the_given_width() {
        for width in [20usize, 30, 45, 60, 88, 120, 200] {
            for line in text(&dual_axes(), width) {
                assert!(
                    display_width(&line) <= width,
                    "at width {width}, line of {} cols: {line}",
                    display_width(&line)
                );
            }
        }
    }

    /// The braille rows of both series must be the same width and start at the
    /// same column, or a volume bar sits under the wrong point in time.
    #[test]
    fn dual_axes_series_share_one_x_scale() {
        let lines = render(&dual_axes(), 60);
        // Plot rows are the ones whose second span is the axis tick.
        let plots: Vec<&ratatui::text::Line> = lines
            .iter()
            .filter(|l| l.spans.get(1).is_some_and(|s| s.content == "┤"))
            .collect();
        assert_eq!(
            plots.len(),
            LINE_ROWS + super::COL_ROWS,
            "expected line rows plus volume rows"
        );
        let gutters: Vec<usize> = plots
            .iter()
            .map(|l| display_width(&l.spans[0].content))
            .collect();
        let plot_ws: Vec<usize> = plots
            .iter()
            .map(|l| l.spans[2].content.chars().count())
            .collect();
        assert!(
            gutters.iter().all(|g| *g == gutters[0]),
            "gutters differ: {gutters:?}"
        );
        assert!(
            plot_ws.iter().all(|w| *w == plot_ws[0]),
            "plot widths differ: {plot_ws:?}"
        );
    }

    /// Every plot row carries a value, not just the top and bottom ones.
    #[test]
    fn every_line_row_is_labelled() {
        let labels: Vec<String> = render(&dual_axes(), 60)
            .iter()
            .filter(|l| l.spans.get(1).is_some_and(|s| s.content == "┤"))
            .take(LINE_ROWS)
            .map(|l| l.spans[0].content.trim().to_string())
            .collect();
        assert_eq!(labels.len(), LINE_ROWS);
        assert!(
            labels.iter().all(|l| l.parse::<f64>().is_ok()),
            "every row needs a numeric label, got {labels:?}"
        );
        // Top is the maximum, bottom the minimum, and they descend in between.
        let nums: Vec<f64> = labels.iter().map(|l| l.parse().unwrap()).collect();
        assert!(
            (nums[0] - 141.64).abs() < 0.05,
            "top should be max: {nums:?}"
        );
        assert!(
            (nums[LINE_ROWS - 1] - 137.41).abs() < 0.05,
            "bottom should be min: {nums:?}"
        );
        assert!(
            nums.windows(2).all(|w| w[0] > w[1]),
            "not descending: {nums:?}"
        );
    }

    /// Consecutive points are joined, so a series reads as a trend rather than
    /// as scatter. Every cell column of the plot must be covered by some row.
    #[test]
    fn the_line_is_continuous_across_the_plot() {
        let spec = serde_json::json!({
            "type": "line",
            "categories": ["a", "b", "c", "d", "e"],
            "series": [{"type": "line", "data": [1.0, 2.0, 3.0, 4.0, 5.0]}]
        });
        let rows: Vec<Vec<char>> = render(&spec, 60)
            .iter()
            .filter(|l| l.spans.get(1).is_some_and(|s| s.content == "┤"))
            .map(|l| l.spans[2].content.chars().collect())
            .collect();
        assert!(!rows.is_empty());
        let plot_w = rows[0].len();
        for col in 0..plot_w {
            assert!(
                rows.iter()
                    .any(|r| r.get(col).is_some_and(|c| *c != '\u{2800}')),
                "column {col} of {plot_w} is empty — the line is not continuous"
            );
        }
    }

    /// The x axis names positions along the series, not just its two ends.
    #[test]
    fn x_axis_has_interior_ticks() {
        let rendered = text(&dual_axes(), 60);
        let ticks = rendered
            .iter()
            .find(|l| l.contains("13:30") && l.contains("13:55"))
            .expect("an axis row naming both ends");
        let named = ["13:30", "13:35", "13:40", "13:45", "13:50", "13:55"]
            .iter()
            .filter(|t| ticks.contains(**t))
            .count();
        assert!(
            named >= 3,
            "expected interior ticks, got {named} in {ticks:?}"
        );
    }

    /// `{time, value}` rows are a line series written the other way round. Read
    /// as categorical columns they lost their labels and their y scale entirely.
    #[test]
    fn time_value_rows_render_as_a_line() {
        let spec = serde_json::json!({
            "type": "line",
            "title": "daily",
            "data": [
                {"time": "8/3", "value": 104.83},
                {"time": "8/4", "value": 125.33},
                {"time": "8/5", "value": 108.27}
            ]
        });
        let lines = render(&spec, 60);
        let labels: Vec<String> = lines
            .iter()
            .filter(|l| l.spans.get(1).is_some_and(|s| s.content == "┤"))
            .map(|l| l.spans[0].content.trim().to_string())
            .collect();
        assert_eq!(labels.len(), LINE_ROWS, "a line chart needs a y axis");
        assert!(
            labels.first().is_some_and(|l| l.starts_with("125")),
            "top label should be the max, got {labels:?}"
        );
        let rendered = text(&spec, 60);
        assert!(
            rendered.iter().any(|l| l.contains("8/3")),
            "the time labels should reach the axis: {rendered:?}"
        );
    }

    /// Too narrow for a curve is a table, not a chart squeezed past the width.
    #[test]
    fn a_narrow_terminal_falls_back_to_a_table() {
        let width = MIN_PLOT_W; // gutter + tick leave less than MIN_PLOT_W over
        let rendered = text(&dual_axes(), width);
        assert!(
            rendered.iter().all(|l| !l.contains('┤')),
            "expected no plot at width {width}: {rendered:?}"
        );
        assert!(
            rendered.iter().any(|l| l.contains("13:30")),
            "the table should still carry the data: {rendered:?}"
        );
    }

    /// A comparison draws several securities on one canvas, so each needs its own
    /// colour and a legend entry naming it — one colour for all of them made the
    /// curves impossible to tell apart.
    #[test]
    fn each_series_gets_its_own_colour_and_legend_entry() {
        let spec = serde_json::json!({
            "type": "line",
            "categories": ["1/2", "1/9", "1/16"],
            "series": [
                {"type": "line", "name": "TSLA.US", "data": [100.0, 130.0, 90.0]},
                {"type": "line", "name": "NVDA.US", "data": [100.0, 90.0, 130.0]},
                {"type": "line", "name": "AAPL.US", "data": [100.0, 101.0, 102.0]}
            ]
        });
        let lines = render(&spec, 64);
        let colors: std::collections::HashSet<String> = lines
            .iter()
            .flat_map(|l| &l.spans)
            // Braille cells only: the gutter and axes are always dim.
            .filter(|s| {
                s.content
                    .chars()
                    .any(|c| ('\u{2801}'..='\u{28FF}').contains(&c))
            })
            .map(|s| format!("{:?}", s.style.fg))
            .collect();
        assert!(
            colors.len() >= 3,
            "three series should draw in three colours, got {colors:?}"
        );
        // The legend is the key to those colours, so every series has to be named
        // in it — the label comes from `name`, not just `axisYTitle`.
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        for symbol in ["TSLA.US", "NVDA.US", "AAPL.US"] {
            assert!(
                text.iter().any(|l| l.contains(symbol)),
                "{symbol} missing from the legend: {text:?}"
            );
        }
    }

    /// A price line over a volume histogram is the common shape, and blue bars
    /// under a cyan line is how every other chart in the terminal reads.
    #[test]
    fn a_lone_volume_series_keeps_its_blue() {
        assert_eq!(super::column_color(1, 0, 1), super::VOLUME);
        // Several column series need telling apart more than they need the
        // convention.
        assert_ne!(super::column_color(1, 0, 2), super::column_color(1, 1, 2));
    }

    /// Where two curves cross in one cell, both sets of dots survive and the
    /// denser series owns the colour.
    #[test]
    fn a_crossing_keeps_both_curves() {
        let mut cell = super::Cell::default();
        cell.add(0b0000_0011, 0);
        cell.add(0b0001_1100, 1);
        assert_eq!(cell.bits, 0b0001_1111, "both curves' dots are kept");
        assert_eq!(cell.owner, 1, "the denser series owns the cell");
    }
}
