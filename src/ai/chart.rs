//! Chart drawing for `vis-chart` specs, the agent's own answer dialect.
//!
//! This is the single place charts are drawn. It produces `Vec<Line<'static>>`
//! for the TUI, and [`crate::ai::stdout::render_vis_chart`] flattens
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

use ratatui::style::{Color, Modifier, Style};
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
/// Height, in text rows, of a vertical column chart's plot area.
const V_ROWS: usize = 8;
/// Eighth-height block glyphs, for the fractional top of a vertical bar.
const V_BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// Eighth-width block glyphs, for the fractional end of a horizontal bar.
const H_BLOCKS: [char; 9] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
/// A plot narrower than this cannot carry a readable curve, so the spec falls
/// back to a table.
const MIN_PLOT_W: usize = 12;
/// Most rows a single chart may occupy: the data is LLM-supplied, so a runaway
/// `data` array is truncated rather than allowed to flood the transcript.
const MAX_ROWS: usize = 240;
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
        "area" => line_chart(&categories, &series, width, true),
        "line" | "dual-axes" => line_chart(&categories, &series, width, false),
        "scatter" => scatter_chart(spec, width),
        "histogram" => histogram_chart(spec, width),
        // A single-series `column` stands up as vertical bars, like the web's; a
        // grouped one, or an explicit `bar`, stays horizontal where the group
        // labels have room to read.
        "column" if series.len() == 1 => vertical_bar_chart(&categories, &series, width),
        "column" | "bar" => bar_chart(&categories, &series, width),
        "pie" => pie_chart(&categories, &series, width),
        // A share of the whole, like a pie: the biggest tile is the biggest share.
        "treemap" => share_chart(&categories, &series, width),
        // A funnel is monotonically narrowing stages; centre the bars so the taper
        // reads as a funnel and annotate the drop-off between stages.
        "funnel" => funnel_chart(&categories, &series, width),
        // A radar is several axes scored on one scale; a bar per axis, each scaled
        // to the shared max, is that comparison without the polygon a terminal
        // cannot draw.
        "radar" => radar_chart(&categories, &series, width),
        "boxplot" | "box" => boxplot_chart(spec, width),
        // Everything else (sankey, wordcloud, network, mind-map, flow, …) has no
        // faithful ASCII form, but its data is still worth reading: list it rather
        // than drop it to a bare "0".
        _ => structured_chart(spec, &categories, &series, width),
    });
    // A chart is one block of a scrolling transcript, and the data comes from an
    // LLM: cap the rows so a spec with a runaway `data` array (tens of thousands
    // of points) cannot balloon the transcript. A real chart is far smaller; the
    // elision line says how much was dropped.
    if out.len() > MAX_ROWS {
        let hidden = out.len() - MAX_ROWS;
        out.truncate(MAX_ROWS);
        out.push(Line::from(Span::styled(
            format!("  ⋯ +{hidden}"),
            Style::default().fg(AXIS),
        )));
    }
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
///
/// `fill` shades the region below each line down to the baseline, which is what
/// turns a line chart into an area chart.
fn line_chart(
    categories: &[String],
    series: &[ChartSeries],
    width: usize,
    fill: bool,
) -> Vec<Line<'static>> {
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
        for (r, row) in line_canvas(&lines, min, max, &geo, fill).iter().enumerate() {
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
    // Large finance magnitudes (volume, turnover, market cap) are abbreviated so
    // an axis label stays a few columns wide instead of a run of raw digits.
    let abbrev = max.abs().max(min.abs()) >= 10_000.0;
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
            if abbrev {
                abbreviate(v)
            } else {
                format!("{v:.prec$}")
            }
        })
        .collect()
}

/// A compact K/M/B form of a large axis value, keeping its sign.
fn abbreviate(v: f64) -> String {
    let a = v.abs();
    let (scaled, suffix) = if a >= 1e9 {
        (v / 1e9, "B")
    } else if a >= 1e6 {
        (v / 1e6, "M")
    } else if a >= 1e3 {
        (v / 1e3, "K")
    } else {
        return format!("{v:.0}");
    };
    format!("{scaled:.1}{suffix}")
}

/// Plot every line series onto one braille canvas.
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
fn line_canvas(
    lines: &[&ChartSeries],
    min: f64,
    max: f64,
    geo: &Geometry,
    fill: bool,
) -> Vec<Vec<Cell>> {
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
        if fill {
            fill_below(&mut layer);
        }
        for (row, layer_row) in canvas.iter_mut().zip(&layer) {
            for (cell, &bits) in row.iter_mut().zip(layer_row) {
                cell.add(bits, si);
            }
        }
    }
    canvas
}

/// Light every dot below the topmost lit dot of each column, turning a plotted
/// line into a filled area down to the baseline.
fn fill_below(layer: &mut [Vec<u8>]) {
    let rows = layer.len();
    let cells_w = layer.first().map_or(0, Vec::len);
    let dot_h = rows * 4;
    let dot_w = cells_w * 2;
    for dx in 0..dot_w {
        let lit = |layer: &[Vec<u8>], dy: usize| {
            layer[dy / 4][dx / 2] & BRAILLE_DOTS[dy % 4][dx % 2] != 0
        };
        let Some(top) = (0..dot_h).find(|&dy| lit(layer, dy)) else {
            continue;
        };
        for dy in top..dot_h {
            layer[dy / 4][dx / 2] |= BRAILLE_DOTS[dy % 4][dx % 2];
        }
    }
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
    // Scale by the largest magnitude, not the largest value: a series that is all
    // negative (net capital flow, say) would otherwise have every bar collapse to
    // the minimum height, since `v / max` is negative and clamps to 1. The bar
    // shows magnitude; the line series and labels carry the sign.
    let max = s
        .values
        .iter()
        .map(|v| v.abs())
        .fold(0.0_f64, f64::max)
        .max(f64::EPSILON);
    let dots = geo.plot_w * 2;
    let height = COL_ROWS * 4;
    let n = s.values.len();
    for (i, &v) in s.values.iter().enumerate() {
        let bar = ((v.abs() / max) * height as f64)
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

/// Set one braille dot on a colour-owning canvas, crediting `owner` for the cell.
fn plot_colored(canvas: &mut [Vec<Cell>], dot_x: usize, dot_y: usize, owner: usize) {
    let (cx, cy) = (dot_x / 2, dot_y / 4);
    if let Some(cell) = canvas.get_mut(cy).and_then(|row| row.get_mut(cx)) {
        cell.add(BRAILLE_DOTS[dot_y % 4][dot_x % 2], owner);
    }
}

/// A straight braille line between two dots on a colour-owning canvas.
#[allow(clippy::cast_sign_loss)]
fn segment_colored(
    canvas: &mut [Vec<Cell>],
    (x0, y0): (isize, isize),
    (x1, y1): (isize, isize),
    owner: usize,
) {
    let (mut x, mut y) = (x0, y0);
    let (dx, dy) = ((x1 - x).abs(), -(y1 - y).abs());
    let (sx, sy) = (if x < x1 { 1 } else { -1 }, if y < y1 { 1 } else { -1 });
    let mut err = dx + dy;
    loop {
        if x >= 0 && y >= 0 {
            plot_colored(canvas, x as usize, y as usize, owner);
        }
        if x == x1 && y == y1 {
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

fn braille(bits: u8) -> char {
    char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ')
}

/// Horizontal ▓ bars, one per (category, group) pair.
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
fn bar_chart(categories: &[String], series: &[ChartSeries], width: usize) -> Vec<Line<'static>> {
    // Scale by the largest magnitude, so an all-negative series (e.g. drawdowns)
    // still shows relative size instead of every bar collapsing to one cell.
    let max = series
        .iter()
        .flat_map(|s| s.values.iter().map(|v| v.abs()))
        .fold(0.0_f64, f64::max)
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
    let bar_w = width.saturating_sub(label_w + 15).clamp(10, 60);
    // A single series keeps the plot colour; a grouped chart gives each group its
    // own colour so the groups can be told apart down the page.
    let multi = series.len() > 1;
    let mut out = Vec::new();
    for (ci, cat) in categories.iter().enumerate() {
        // A blank row sets a grouped chart's clusters apart; single bars stay tight,
        // where the wider columns and the bars themselves carry the rhythm and a
        // blank between every row would only read as loose.
        if multi && ci > 0 {
            out.push(Line::from(String::new()));
        }
        for (si, s) in series.iter().enumerate() {
            let Some(&v) = s.values.get(ci) else { continue };
            let label = if s.label.is_empty() {
                cat.clone()
            } else {
                format!("{cat} {}", s.label)
            };
            // A single-series bar takes the category's own palette colour, like the
            // web's columns; a grouped chart colours by group instead.
            let color = if multi {
                series_color(si)
            } else {
                series_color(ci)
            };
            out.push(Line::from(vec![
                Span::styled(
                    format!("  {}   ", pad_display(&label, label_w)),
                    Style::default().fg(AXIS),
                ),
                Span::styled(bar_blocks(v.abs() / max, bar_w), Style::default().fg(color)),
                Span::styled(format!("  {}", num(v)), Style::default().fg(AXIS)),
            ]));
        }
    }
    out
}

/// A horizontal bar `fraction` of `cells` wide, drawn to eighth-cell precision so
/// two close values are distinguishable even when they round to the same cell.
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
fn bar_blocks(fraction: f64, cells: usize) -> String {
    let eighths = (fraction.clamp(0.0, 1.0) * (cells * 8) as f64).round() as usize;
    let mut out = "█".repeat(eighths / 8);
    let rem = eighths % 8;
    if rem > 0 {
        out.push(H_BLOCKS[rem]);
    }
    if out.is_empty() {
        out.push(H_BLOCKS[1]);
    }
    out
}

/// A round upper bound at or above `max` (1 / 2 / 2.5 / 5 × 10ⁿ), so a value
/// axis ends on a number worth labelling.
fn nice_max(max: f64) -> f64 {
    if !max.is_finite() || max <= 0.0 {
        return 1.0;
    }
    let power = 10f64.powf(max.log10().floor());
    for step in [1.0, 2.0, 2.5, 5.0] {
        if step * power >= max * (1.0 - 1e-9) {
            return step * power;
        }
    }
    10.0 * power
}

/// A vertical column chart: an upward block bar per category over a labelled value
/// axis, the terminal's read on the web's vertical columns. Single-series only —
/// a grouped `column` spec stays with the horizontal [`bar_chart`], which already
/// colours and labels each group.
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
fn vertical_bar_chart(
    categories: &[String],
    series: &[ChartSeries],
    width: usize,
) -> Vec<Line<'static>> {
    let Some(s) = series.first() else {
        return Vec::new();
    };
    if s.values.is_empty() {
        return Vec::new();
    }
    let raw_max = s
        .values
        .iter()
        .map(|v| v.abs())
        .fold(0.0_f64, f64::max)
        .max(f64::EPSILON);
    // A round tick step, four of them to the top, so every labelled row lands on a
    // whole number (step, 2·step, …) instead of an eighth of the raw maximum.
    let step = nice_max(raw_max / 4.0);
    let max = step * 4.0;
    // Label every second row (there are eight), so the four ticks read step, 2·, 3·
    // and 4·step from the baseline; the rows between carry a bare axis.
    let abbrev = max >= 10_000.0;
    let labels: Vec<String> = (0..V_ROWS)
        .map(|r| {
            if !r.is_multiple_of(2) {
                return String::new();
            }
            let v = max * (V_ROWS - r) as f64 / V_ROWS as f64;
            if abbrev {
                abbreviate(v)
            } else {
                format!("{v:.0}")
            }
        })
        .collect();
    let gutter = labels.iter().map(String::len).max().unwrap_or(0);
    let n = s.values.len();
    // Fall back to the horizontal bars when there is no room for one glyph and a
    // gap per category.
    let avail = width.saturating_sub(gutter + 1);
    if avail < n * 2 {
        return bar_chart(categories, series, width);
    }
    let gap = 1usize;
    let bar_w = ((avail + gap) / n).saturating_sub(gap).clamp(1, 10);
    let plot_w = n * bar_w + (n - 1) * gap;
    let fills: Vec<usize> = s
        .values
        .iter()
        .map(|v| {
            ((v.abs() / max) * (V_ROWS * 8) as f64)
                .round()
                .clamp(1.0, (V_ROWS * 8) as f64) as usize
        })
        .collect();
    let axis = Style::default().fg(AXIS);
    let mut out = Vec::new();
    for (r, label) in labels.iter().enumerate() {
        let base = (V_ROWS - 1 - r) * 8;
        let mut spans = vec![
            Span::styled(format!("{label:>gutter$}"), axis),
            Span::styled("┤", axis),
        ];
        for (i, &fill) in fills.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" ".repeat(gap)));
            }
            let glyph = if fill >= base + 8 {
                '█'
            } else if fill <= base {
                ' '
            } else {
                V_BLOCKS[fill - base]
            };
            spans.push(Span::styled(
                glyph.to_string().repeat(bar_w),
                Style::default().fg(series_color(i)),
            ));
        }
        out.push(Line::from(spans));
    }
    out.push(Line::from(Span::styled(
        format!("{}└{}", " ".repeat(gutter), "─".repeat(plot_w)),
        axis,
    )));
    // Category labels, each centred under its bar; one that would touch its
    // neighbour is dropped rather than overlapped.
    let mut label_row = String::new();
    let mut cursor = 0usize;
    for (i, cat) in categories.iter().enumerate().take(n) {
        let w = display_width(cat);
        let target = i * (bar_w + gap) + bar_w.saturating_sub(w) / 2;
        if target < cursor {
            continue;
        }
        label_row.push_str(&" ".repeat(target - cursor));
        label_row.push_str(cat);
        cursor = target + w;
    }
    if !label_row.trim().is_empty() {
        out.push(Line::from(Span::styled(
            format!("{} {label_row}", " ".repeat(gutter)),
            axis,
        )));
    }
    out
}

/// A braille donut, each slice in its own colour, with a legend of names and
/// percentages beside it.
///
/// A ring rather than the row of proportion bars it replaced: a pie is a shape,
/// and a terminal can draw a recognisable one. Values are clamped non-negative —
/// a slice is a share of the whole and a negative share has no arc — and a
/// too-narrow terminal falls back to the labelled bars via [`share_chart`].
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
fn pie_chart(categories: &[String], series: &[ChartSeries], width: usize) -> Vec<Line<'static>> {
    use std::f64::consts::{FRAC_PI_2, TAU};

    let Some(s) = series.first() else {
        return Vec::new();
    };
    let values: Vec<f64> = s.values.iter().map(|v| v.max(0.0)).collect();
    let total: f64 = values.iter().sum();
    if total <= 0.0 {
        return Vec::new();
    }
    // The widest legend line, so the donut and the legend beside it fit `width`.
    let legend_w = categories
        .iter()
        .map(|c| display_width(c))
        .max()
        .unwrap_or(0)
        + 10;
    let outer = 20isize;
    let inner = 12isize;
    let cols = outer as usize + 1;
    let rows = outer as usize / 2 + 1;
    if width < cols + 3 + legend_w {
        return share_chart(categories, series, width);
    }
    let (cxd, cyd) = (outer, outer);
    let mut canvas = vec![vec![Cell::default(); cols]; rows];
    let mut angle = -FRAC_PI_2;
    let step = 0.5 / outer as f64;
    for (i, &v) in values.iter().enumerate() {
        let end = angle + v / total * TAU;
        let mut t = angle;
        while t < end {
            for r in inner..=outer {
                let dx = (cxd as f64 + r as f64 * t.cos()).round() as isize;
                let dy = (cyd as f64 + r as f64 * t.sin()).round() as isize;
                if dx >= 0 && dy >= 0 {
                    plot_colored(&mut canvas, dx as usize, dy as usize, i);
                }
            }
            t += step;
        }
        angle = end;
    }
    // The donut rows, each with its slice of the legend to the right.
    let axis = Style::default().fg(AXIS);
    let mut out = Vec::new();
    for (r, row) in canvas.iter().enumerate() {
        let mut spans = colored_row(row);
        if let Some(cat) = categories.get(r) {
            if let Some(&v) = values.get(r) {
                spans.push(Span::styled(
                    "   ██ ".to_string(),
                    Style::default().fg(series_color(r)),
                ));
                spans.push(Span::styled(
                    format!("{cat}  {:.1}%", v / total * 100.0),
                    axis,
                ));
            }
        }
        out.push(Line::from(spans));
    }
    // Any legend entries past the donut's height continue below it.
    for (i, cat) in categories.iter().enumerate().skip(rows) {
        let Some(&v) = values.get(i) else { continue };
        out.push(Line::from(vec![
            Span::styled(
                format!("{}   ██ ", " ".repeat(cols)),
                Style::default().fg(series_color(i)),
            ),
            Span::styled(format!("{cat}  {:.1}%", v / total * 100.0), axis),
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

/// A number formatted for a label: large magnitudes abbreviated (`1.2M`), small
/// ones trimmed of trailing zeros (`3.5`, not `3.50`).
fn num(v: f64) -> String {
    if v.abs() >= 10_000.0 {
        abbreviate(v)
    } else {
        let s = format!("{v:.2}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        s.to_string()
    }
}

/// One labelled bar row: `  label  ▓▓▓  value`, the bar `frac` (0..=1) of `bar_w`
/// wide. A positive fraction always draws at least one cell so a small-but-real
/// value is never invisible.
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
fn bar_row(
    label: &str,
    label_w: usize,
    frac: f64,
    bar_w: usize,
    value: &str,
    color: Color,
) -> Line<'static> {
    let frac = if frac.is_finite() {
        frac.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let bar = if frac > 0.0 {
        bar_blocks(frac, bar_w)
    } else {
        String::new()
    };
    Line::from(vec![
        Span::styled(
            format!("  {}   ", pad_display(label, label_w)),
            Style::default().fg(AXIS),
        ),
        Span::styled(bar, Style::default().fg(color)),
        Span::styled(format!("  {value}"), Style::default().fg(AXIS)),
    ])
}

/// Proportional bars by share of the total, largest first — a terminal reading of
/// a treemap, where the biggest tile is simply the longest bar.
#[allow(clippy::cast_precision_loss)]
fn share_chart(categories: &[String], series: &[ChartSeries], width: usize) -> Vec<Line<'static>> {
    let Some(s) = series.first() else {
        return Vec::new();
    };
    let total: f64 = s.values.iter().map(|v| v.abs()).sum();
    if s.values.is_empty() || total <= 0.0 {
        return table_chart(categories, series);
    }
    let mut items: Vec<(String, f64)> = categories
        .iter()
        .cloned()
        .zip(s.values.iter().copied())
        .collect();
    items.sort_by(|a, b| {
        b.1.abs()
            .partial_cmp(&a.1.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let label_w = items
        .iter()
        .map(|(c, _)| display_width(c))
        .max()
        .unwrap_or(0)
        .min(24);
    let bar_w = width.saturating_sub(label_w + 18).clamp(8, 48);
    items
        .iter()
        .enumerate()
        .map(|(i, (cat, v))| {
            let pct = v.abs() / total * 100.0;
            bar_row(
                cat,
                label_w,
                v.abs() / total,
                bar_w,
                &format!("{} ({pct:.1}%)", num(*v)),
                // Each tile its own colour, biggest first, so the ranking reads at
                // a glance rather than as one wall of cyan.
                series_color(i),
            )
        })
        .collect()
}

/// Funnel stages, each bar centred and scaled to the first (widest) stage, with
/// the stage-to-stage conversion rate called out.
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
fn funnel_chart(categories: &[String], series: &[ChartSeries], width: usize) -> Vec<Line<'static>> {
    let Some(s) = series.first() else {
        return Vec::new();
    };
    if s.values.is_empty() {
        return Vec::new();
    }
    let max = s
        .values
        .iter()
        .copied()
        .fold(f64::MIN, f64::max)
        .max(f64::EPSILON);
    let label_w = categories
        .iter()
        .map(|c| display_width(c))
        .max()
        .unwrap_or(0)
        .min(20);
    let bar_w = width.saturating_sub(label_w + 16).clamp(8, 48);
    let mut out = Vec::new();
    let mut prev: Option<f64> = None;
    for (ci, v) in s.values.iter().copied().enumerate() {
        let cat = categories.get(ci).cloned().unwrap_or_default();
        let n = ((v / max) * bar_w as f64).round().clamp(1.0, bar_w as f64) as usize;
        let pad = (bar_w - n) / 2;
        let conv = match prev {
            Some(p) if p > 0.0 => format!("  ↓{:.0}%", v / p * 100.0),
            _ => String::new(),
        };
        out.push(Line::from(vec![
            Span::styled(
                format!("  {} ", pad_display(&cat, label_w)),
                Style::default().fg(AXIS),
            ),
            Span::raw(" ".repeat(pad)),
            Span::styled("█".repeat(n), Style::default().fg(PLOT)),
            Span::styled(format!(" {}{conv}", num(v)), Style::default().fg(AXIS)),
        ]));
        prev = Some(v);
    }
    out
}

/// A braille radar: each series a coloured polygon over a grey grid of spokes and
/// an outer ring, with the axes and their values listed below.
///
/// The old row-of-bars form is kept as [`radar_bars`], the fallback for fewer than
/// three axes (no polygon to close) or a terminal too narrow for the ring.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
fn radar_chart(categories: &[String], series: &[ChartSeries], width: usize) -> Vec<Line<'static>> {
    use std::f64::consts::{FRAC_PI_2, TAU};

    let n = categories.len();
    let max = series
        .iter()
        .flat_map(|s| s.values.iter().map(|v| v.abs()))
        .fold(0.0_f64, f64::max)
        .max(f64::EPSILON);
    let radius = 20isize;
    let cols = radius as usize + 1;
    let rows = radius as usize / 2 + 1;
    if n < 3 || !max.is_finite() || width < cols + 2 {
        return radar_bars(categories, series, width);
    }
    let (cx, cy) = (radius, radius);
    let angle = |i: usize| -FRAC_PI_2 + i as f64 * TAU / n as f64;
    let vertex = |i: usize, r: f64| {
        let a = angle(i);
        (
            (cx as f64 + r * a.cos()).round() as isize,
            (cy as f64 + r * a.sin()).round() as isize,
        )
    };
    // Grey grid on its own monochrome canvas; the coloured data polygons on
    // another, so a filled cell can prefer a series colour over the grid.
    let mut grid = vec![vec![0u8; cols]; rows];
    let mut data = vec![vec![Cell::default(); cols]; rows];
    for i in 0..n {
        let a = vertex(i, radius as f64);
        let b = vertex((i + 1) % n, radius as f64);
        let clamp = |(x, y): (isize, isize)| (x.max(0) as usize, y.max(0) as usize);
        plot_segment(&mut grid, clamp(a), clamp(b));
        plot_segment(&mut grid, (cx as usize, cy as usize), clamp(a));
    }
    for (si, s) in series.iter().enumerate() {
        let points: Vec<(isize, isize)> = (0..n)
            .map(|i| {
                vertex(
                    i,
                    s.values.get(i).copied().unwrap_or(0.0).abs() / max * radius as f64,
                )
            })
            .collect();
        for i in 0..n {
            segment_colored(&mut data, points[i], points[(i + 1) % n], si);
        }
    }
    let mut out = Vec::new();
    for r in 0..rows {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut run = String::new();
        let mut run_color: Option<Color> = None;
        for c in 0..cols {
            let cell = &data[r][c];
            let color = if cell.bits != 0 {
                series_color(cell.owner)
            } else {
                AXIS
            };
            let glyph = braille(cell.bits | grid[r][c]);
            if run_color == Some(color) {
                run.push(glyph);
            } else {
                if let Some(prev) = run_color {
                    spans.push(Span::styled(
                        std::mem::take(&mut run),
                        Style::default().fg(prev),
                    ));
                }
                run.push(glyph);
                run_color = Some(color);
            }
        }
        if let Some(prev) = run_color {
            spans.push(Span::styled(run, Style::default().fg(prev)));
        }
        out.push(Line::from(spans));
    }
    // The axes and their values, one line per series so each polygon can be read.
    for (si, s) in series.iter().enumerate() {
        let listing = categories
            .iter()
            .enumerate()
            .map(|(i, cat)| format!("{cat} {}", num(s.values.get(i).copied().unwrap_or(0.0))))
            .collect::<Vec<_>>()
            .join("  ·  ");
        let color = if series.len() > 1 {
            series_color(si)
        } else {
            AXIS
        };
        out.push(Line::from(Span::styled(
            format!("  {}", truncate_width(&listing, width.saturating_sub(2))),
            Style::default().fg(color),
        )));
    }
    out
}

/// One bar per axis, each scaled to the shared maximum — the radar fallback for a
/// terminal that cannot hold the ring.
fn radar_bars(categories: &[String], series: &[ChartSeries], width: usize) -> Vec<Line<'static>> {
    let max = series
        .iter()
        .flat_map(|s| s.values.iter().copied())
        .fold(f64::MIN, f64::max)
        .max(f64::EPSILON);
    if !max.is_finite() {
        return table_chart(categories, series);
    }
    let label_w = categories
        .iter()
        .map(|c| display_width(c))
        .max()
        .unwrap_or(0)
        .min(20);
    let bar_w = width.saturating_sub(label_w + 15).clamp(8, 48);
    let multi = series.len() > 1;
    let mut out = Vec::new();
    for (si, s) in series.iter().enumerate() {
        if multi && !s.label.is_empty() {
            out.push(Line::from(Span::styled(
                format!("  {}", s.label),
                Style::default()
                    .fg(series_color(si))
                    .add_modifier(Modifier::BOLD),
            )));
        }
        for (ci, v) in s.values.iter().copied().enumerate() {
            let cat = categories.get(ci).cloned().unwrap_or_default();
            out.push(bar_row(
                &cat,
                label_w,
                v / max,
                bar_w,
                &num(v),
                series_color(si),
            ));
        }
    }
    out
}

/// The value at quantile `q` (0…1) of a sorted slice, linearly interpolated.
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let pos = q.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

/// The five-number summary (min, Q1, median, Q3, max) of a sample set. A set of
/// exactly five already-summarised values reproduces itself. `None` when empty.
fn five_number(mut samples: Vec<f64>) -> Option<(f64, f64, f64, f64, f64)> {
    samples.retain(|v| v.is_finite());
    if samples.is_empty() {
        return None;
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some((
        samples[0],
        quantile(&samples, 0.25),
        quantile(&samples, 0.5),
        quantile(&samples, 0.75),
        samples[samples.len() - 1],
    ))
}

/// A box-and-whisker row per category: whiskers `├──┤` from min to max, a `▓` box
/// spanning the quartiles, and a `┃` at the median, all on one shared scale.
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
fn boxplot_chart(spec: &Value, width: usize) -> Vec<Line<'static>> {
    struct Box {
        label: String,
        lo: f64,
        q1: f64,
        med: f64,
        q3: f64,
        hi: f64,
    }
    let Some(data) = spec.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut boxes = Vec::new();
    // Raw samples that arrived one scalar per row, keyed by their group so a
    // `[{group, value}, …]` distribution can be summarised rather than dropped.
    let mut grouped: Vec<(String, Vec<f64>)> = Vec::new();
    for row in data {
        let label = ["group", "category", "name", "x", "label"]
            .iter()
            .find_map(|k| row.get(*k).and_then(Value::as_str))
            .map(strip_control_chars)
            .unwrap_or_default();
        if let Some(arr) = row.get("value").and_then(Value::as_array) {
            // A `value` array is a sample set (or a five-number summary, which its
            // own quartiles reproduce); summarise it either way.
            let samples: Vec<f64> = arr.iter().filter_map(Value::as_f64).collect();
            if let Some((lo, q1, med, q3, hi)) = five_number(samples) {
                boxes.push(Box {
                    label,
                    lo,
                    q1,
                    med,
                    q3,
                    hi,
                });
            }
        } else if let Some(nums) = ["min", "q1", "median", "q3", "max"]
            .iter()
            .map(|k| row.get(*k).and_then(Value::as_f64))
            .collect::<Option<Vec<_>>>()
        {
            boxes.push(Box {
                label,
                lo: nums[0],
                q1: nums[1],
                med: nums[2],
                q3: nums[3],
                hi: nums[4],
            });
        } else if let Some(v) = row.get("value").and_then(Value::as_f64) {
            match grouped.iter_mut().find(|(g, _)| *g == label) {
                Some((_, vs)) => vs.push(v),
                None => grouped.push((label, vec![v])),
            }
        }
    }
    for (label, samples) in grouped {
        if let Some((lo, q1, med, q3, hi)) = five_number(samples) {
            boxes.push(Box {
                label,
                lo,
                q1,
                med,
                q3,
                hi,
            });
        }
    }
    if boxes.is_empty() {
        return Vec::new();
    }
    let gl = boxes.iter().map(|b| b.lo).fold(f64::MAX, f64::min);
    let gh = boxes.iter().map(|b| b.hi).fold(f64::MIN, f64::max);
    let span = if (gh - gl).abs() < f64::EPSILON {
        1.0
    } else {
        gh - gl
    };
    let label_w = boxes
        .iter()
        .map(|b| display_width(&b.label))
        .max()
        .unwrap_or(0)
        .min(16);
    // The exact five numbers are spelled out after the box — the box shows the
    // shape, but a terminal reader wants the values too, not just the median.
    let stats = |b: &Box| {
        format!(
            "{} · Q1 {} · 中 {} · Q3 {} · {}",
            num(b.lo),
            num(b.q1),
            num(b.med),
            num(b.q3),
            num(b.hi)
        )
    };
    let stats_w = boxes
        .iter()
        .map(|b| display_width(&stats(b)))
        .max()
        .unwrap_or(0);
    let room = width.saturating_sub(label_w + stats_w + 6);
    // On a narrow terminal the numbers matter more than the shape: drop the box
    // and show just the label and the five-number summary rather than overflow.
    if room < 10 {
        return boxes
            .iter()
            .map(|b| {
                Line::from(vec![
                    Span::styled(
                        format!("  {}   ", pad_display(&b.label, label_w)),
                        Style::default().fg(AXIS),
                    ),
                    Span::styled(stats(b), Style::default().fg(AXIS)),
                ])
            })
            .collect();
    }
    let bar_w = room.clamp(10, 48);
    let col = |v: f64| {
        (((v - gl) / span) * (bar_w - 1) as f64)
            .round()
            .clamp(0.0, (bar_w - 1) as f64) as usize
    };
    boxes
        .iter()
        .map(|b| {
            let (c_lo, c_q1, c_med, c_q3, c_hi) =
                (col(b.lo), col(b.q1), col(b.med), col(b.q3), col(b.hi));
            let mut cells = vec![' '; bar_w];
            cells[c_lo.min(c_hi)..=c_lo.max(c_hi)].fill('─');
            cells[c_q1.min(c_q3)..=c_q1.max(c_q3)].fill('▓');
            cells[c_lo.min(bar_w - 1)] = '├';
            cells[c_hi.min(bar_w - 1)] = '┤';
            cells[c_med.min(bar_w - 1)] = '┃';
            Line::from(vec![
                Span::styled(
                    format!("  {} ", pad_display(&b.label, label_w)),
                    Style::default().fg(AXIS),
                ),
                Span::styled(
                    cells.into_iter().collect::<String>(),
                    Style::default().fg(PLOT),
                ),
                Span::styled(format!("  {}", stats(b)), Style::default().fg(AXIS)),
            ])
        })
        .collect()
}

/// A braille scatter of `{x, y}` points on numeric axes.
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
fn scatter_chart(spec: &Value, width: usize) -> Vec<Line<'static>> {
    let Some(data) = spec.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    let pts: Vec<(f64, f64)> = data
        .iter()
        .filter_map(|row| {
            let x = row.get("x").and_then(Value::as_f64)?;
            let y = ["y", "value"]
                .iter()
                .find_map(|k| row.get(*k).and_then(Value::as_f64))?;
            Some((x, y))
        })
        .collect();
    if pts.len() < 2 {
        return Vec::new();
    }
    let (xmin, xmax) = pts.iter().fold((f64::MAX, f64::MIN), |(a, b), (x, _)| {
        (a.min(*x), b.max(*x))
    });
    let (ymin, ymax) = pts.iter().fold((f64::MAX, f64::MIN), |(a, b), (_, y)| {
        (a.min(*y), b.max(*y))
    });
    let labels = y_labels(ymin, ymax);
    let gutter = labels.iter().map(String::len).max().unwrap_or(0);
    let Some(plot_w) = width
        .checked_sub(gutter + 1)
        .filter(|w| *w >= MIN_PLOT_W)
        .map(|w| w.min(MAX_PLOT_W))
    else {
        return Vec::new();
    };
    let xspan = if (xmax - xmin).abs() < f64::EPSILON {
        1.0
    } else {
        xmax - xmin
    };
    let yspan = if (ymax - ymin).abs() < f64::EPSILON {
        1.0
    } else {
        ymax - ymin
    };
    let mut canvas = vec![vec![0u8; plot_w]; LINE_ROWS];
    for (x, y) in &pts {
        let dx = ((x - xmin) / xspan * (plot_w * 2 - 1) as f64).round() as usize;
        let dy = ((ymax - y) / yspan * (LINE_ROWS * 4 - 1) as f64).round() as usize;
        plot(&mut canvas, dx, dy);
    }
    let axis = Style::default().fg(AXIS);
    let mut out = Vec::new();
    for (r, row) in canvas.iter().enumerate() {
        let label = labels.get(r).cloned().unwrap_or_default();
        out.push(Line::from(vec![
            Span::styled(format!("{label:>gutter$}"), axis),
            Span::styled("┤", axis),
            Span::styled(braille_row(row), Style::default().fg(PLOT)),
        ]));
    }
    out.push(Line::from(vec![
        Span::styled(" ".repeat(gutter), axis),
        Span::styled(format!("└{}", "─".repeat(plot_w)), axis),
    ]));
    let (lo, hi) = (num(xmin), num(xmax));
    let gap = plot_w.saturating_sub(display_width(&lo) + display_width(&hi));
    out.push(Line::from(Span::styled(
        format!("{} {lo}{}{hi}", " ".repeat(gutter), " ".repeat(gap)),
        axis,
    )));
    out
}

/// A histogram: bin the samples (bare numbers or `{value}` rows) and draw one
/// labelled bar per bin, the bar scaled to the fullest bin.
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
fn histogram_chart(spec: &Value, width: usize) -> Vec<Line<'static>> {
    let Some(data) = spec.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    let samples: Vec<f64> = data
        .iter()
        .filter_map(|v| {
            v.as_f64()
                .or_else(|| v.get("value").and_then(Value::as_f64))
        })
        .collect();
    if samples.len() < 2 {
        return Vec::new();
    }
    let lo = samples.iter().copied().fold(f64::MAX, f64::min);
    let hi = samples.iter().copied().fold(f64::MIN, f64::max);
    let span = if (hi - lo).abs() < f64::EPSILON {
        1.0
    } else {
        hi - lo
    };
    let bins = samples.len().clamp(1, 10);
    let mut counts = vec![0usize; bins];
    for &v in &samples {
        let b = (((v - lo) / span) * bins as f64) as usize;
        counts[b.min(bins - 1)] += 1;
    }
    let maxc = counts.iter().copied().max().unwrap_or(1).max(1);
    let labels: Vec<String> = (0..bins)
        .map(|b| {
            let a = lo + span * b as f64 / bins as f64;
            let z = lo + span * (b + 1) as f64 / bins as f64;
            format!("{}–{}", num(a), num(z))
        })
        .collect();
    let lw = labels.iter().map(|l| display_width(l)).max().unwrap_or(0);
    let bar_w = width.saturating_sub(lw + 15).clamp(8, 48);
    counts
        .iter()
        .enumerate()
        .map(|(b, &c)| {
            bar_row(
                &labels[b],
                lw,
                c as f64 / maxc as f64,
                bar_w,
                &c.to_string(),
                PLOT,
            )
        })
        .collect()
}

/// The display name of a graph node or tree node, tolerating the various keys the
/// specs use (and a bare string node).
fn node_label(node: &Value) -> String {
    if let Some(s) = node.as_str() {
        return strip_control_chars(s);
    }
    ["name", "label", "title", "id", "text"]
        .iter()
        .find_map(|k| node.get(*k).and_then(Value::as_str))
        .map(strip_control_chars)
        .unwrap_or_default()
}

/// One end of an edge, which may be a name or a numeric node index.
fn edge_end(edge: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| {
        edge.get(*k).and_then(|v| {
            v.as_str()
                .map(strip_control_chars)
                .or_else(|| v.as_i64().map(|n| n.to_string()))
        })
    })
}

/// Draw a `{name, children}` hierarchy as an indented tree with box-drawing
/// connectors — a terminal reading of a mind-map, org chart, fishbone or any
/// tree-shaped flow. Bounded so a pathological tree cannot flood the transcript.
fn push_tree_node(
    node: &Value,
    prefix: &str,
    is_root: bool,
    is_last: bool,
    width: usize,
    out: &mut Vec<Line<'static>>,
) {
    if out.len() >= 120 {
        return;
    }
    let label = node_label(node);
    if is_root {
        out.push(Line::from(Span::styled(
            format!("  {}", truncate_width(&label, width.saturating_sub(2))),
            Style::default().fg(PLOT).add_modifier(Modifier::BOLD),
        )));
    } else {
        let branch = if is_last { "└─ " } else { "├─ " };
        out.push(Line::from(Span::styled(
            truncate_width(&format!("  {prefix}{branch}{label}"), width),
            Style::default().fg(AXIS),
        )));
    }
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        let child_prefix = if is_root {
            String::new()
        } else {
            format!("{prefix}{}", if is_last { "   " } else { "│  " })
        };
        for (i, child) in children.iter().enumerate() {
            push_tree_node(
                child,
                &child_prefix,
                false,
                i + 1 == children.len(),
                width,
                out,
            );
        }
    }
}

/// A readable listing for chart kinds a terminal cannot draw faithfully (sankey,
/// word cloud, network, mind-map, flow, fishbone, org chart). The shape is lost
/// but the data is not: a hierarchy becomes an indented tree, a graph its nodes
/// and links, flows `source → target`, and weighted words a ranked list.
fn structured_chart(
    spec: &Value,
    categories: &[String],
    series: &[ChartSeries],
    width: usize,
) -> Vec<Line<'static>> {
    let axis = Style::default().fg(AXIS);
    // The graph/tree kinds nest their payload under `data` as an object; the
    // flow/word kinds put an array there. Resolve whichever is present.
    let payload = spec.get("data").unwrap_or(spec);

    // Hierarchy: {name, children} → an indented tree (mind-map, org chart,
    // fishbone, tree-shaped flow).
    if payload
        .get("children")
        .and_then(Value::as_array)
        .is_some_and(|c| !c.is_empty())
    {
        let mut out = Vec::new();
        push_tree_node(payload, "", true, true, width, &mut out);
        if !out.is_empty() {
            return out;
        }
    }

    // Node/edge graph: {nodes, edges|links} → the nodes, then the links (network
    // graph, flow diagram).
    if let Some(nodes) = payload.get("nodes").and_then(Value::as_array) {
        let mut out = Vec::new();
        for n in nodes {
            let name = node_label(n);
            if !name.is_empty() {
                out.push(Line::from(Span::styled(
                    format!("  • {}", truncate_width(&name, width.saturating_sub(4))),
                    axis,
                )));
            }
        }
        if let Some(edges) = payload
            .get("edges")
            .or_else(|| payload.get("links"))
            .and_then(Value::as_array)
        {
            for e in edges {
                if let (Some(s), Some(t)) = (
                    edge_end(e, &["source", "from"]),
                    edge_end(e, &["target", "to"]),
                ) {
                    out.push(Line::from(Span::styled(
                        truncate_width(&format!("  {s} → {t}"), width),
                        axis,
                    )));
                }
            }
        }
        if !out.is_empty() {
            return out;
        }
    }

    if let Some(data) = spec.get("data").and_then(Value::as_array) {
        // Flows: {source, target, value}.
        let mut flows = Vec::new();
        for row in data {
            let src = ["source", "from"]
                .iter()
                .find_map(|k| row.get(*k).and_then(Value::as_str));
            let tgt = ["target", "to"]
                .iter()
                .find_map(|k| row.get(*k).and_then(Value::as_str));
            if let (Some(s), Some(t)) = (src, tgt) {
                let v = row
                    .get("value")
                    .and_then(Value::as_f64)
                    .map(|v| format!("   {}", num(v)))
                    .unwrap_or_default();
                flows.push(Line::from(Span::styled(
                    format!(
                        "  {} → {}{v}",
                        strip_control_chars(s),
                        strip_control_chars(t)
                    ),
                    axis,
                )));
            }
        }
        if !flows.is_empty() {
            return flows;
        }
        // Weighted words: {text|word|name, value|weight|count}.
        let mut words: Vec<(String, f64)> = data
            .iter()
            .filter_map(|row| {
                let w = ["text", "word", "name", "category"]
                    .iter()
                    .find_map(|k| row.get(*k).and_then(Value::as_str))?;
                let v = ["value", "weight", "count"]
                    .iter()
                    .find_map(|k| row.get(*k).and_then(Value::as_f64))
                    .unwrap_or(0.0);
                Some((strip_control_chars(w), v))
            })
            .collect();
        if !words.is_empty() {
            words.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            return words
                .iter()
                .map(|(w, v)| {
                    let text = if *v > 0.0 {
                        format!("  {}   {}", w, num(*v))
                    } else {
                        format!("  {w}")
                    };
                    Line::from(Span::styled(text, axis))
                })
                .collect();
        }
    }
    // Last resort: the normalized table, if there is one.
    if categories.is_empty() {
        Vec::new()
    } else {
        table_chart(categories, series)
    }
}

#[cfg(test)]
mod tests {
    use super::{abbreviate, render, LINE_ROWS, MIN_PLOT_W};
    use crate::utils::text::display_width;

    #[test]
    fn large_axis_values_are_abbreviated() {
        assert_eq!(abbreviate(2_500_000_000.0), "2.5B");
        assert_eq!(abbreviate(1_500_000.0), "1.5M");
        assert_eq!(abbreviate(-12_000.0), "-12.0K");
        assert_eq!(abbreviate(500.0), "500");
    }

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

    /// A single-series column stands up as vertical block bars over a labelled
    /// baseline, rather than the horizontal row-per-bar form.
    #[test]
    fn a_single_series_column_is_vertical() {
        let spec = serde_json::json!({
            "type": "column",
            "data": [
                {"category": "Q1", "value": 120.0},
                {"category": "Q2", "value": 260.0},
            ],
        });
        let rows = text(&spec, 60);
        let joined = rows.join("\n");
        assert!(joined.contains('█'), "vertical bars are drawn:\n{joined}");
        assert!(
            rows.iter().any(|r| r.contains('└')),
            "a baseline axis anchors the bars:\n{joined}"
        );
        // The value axis is labelled with round ticks, not left off.
        assert!(joined.contains("200"), "a value tick:\n{joined}");
        // Categories sit on the foot, not inline with the bars as in the row form.
        assert!(joined.contains("Q1") && joined.contains("Q2"));
        // The taller bar reaches a higher row: the first row carrying a full block
        // belongs to Q2's column, left of nothing to Q1's left.
        let q2_top = rows.iter().position(|r| r.contains('█')).unwrap();
        assert!(
            !rows[q2_top].trim_start().starts_with("Q"),
            "the top block row is plot, not labels"
        );
    }

    /// An area chart fills below the curve — dense braille the plain line lacks —
    /// so it reads as area, not as a bare line.
    #[test]
    fn an_area_chart_fills_below_the_curve() {
        let spec = serde_json::json!({
            "type":"area",
            "data":[{"time":"1","value":50.0},{"time":"2","value":80.0},{"time":"3","value":110.0}]
        });
        let joined = text(&spec, 72).join("\n");
        assert!(
            joined.contains('⣿'),
            "the area should be filled solid somewhere: {joined}"
        );
    }

    /// A funnel narrows stage by stage, its bars centred, and names the conversion
    /// from each stage to the next.
    #[test]
    fn a_funnel_tapers_and_shows_conversion() {
        let spec = serde_json::json!({
            "type":"funnel",
            "data":[{"category":"访问","value":1000.0},{"category":"下单","value":300.0}]
        });
        let rows = text(&spec, 72);
        assert!(
            rows.iter().any(|r| r.contains('↓')),
            "conversion shown: {rows:?}"
        );
        let bars = |r: &str| r.chars().filter(|&c| c == '█').count();
        let first = rows
            .iter()
            .find(|r| r.contains("访问"))
            .map_or(0, |r| bars(r));
        let last = rows
            .iter()
            .find(|r| r.contains("下单"))
            .map_or(0, |r| bars(r));
        assert!(first > last, "the funnel narrows: {first} then {last}");
    }

    /// A radar of three or more axes draws a braille polygon and lists each axis
    /// with its value below.
    #[test]
    fn a_radar_draws_a_polygon_and_lists_axes() {
        let spec = serde_json::json!({
            "type":"radar",
            "data":[{"name":"速度","value":8.0},{"name":"质量","value":9.0},{"name":"成本","value":3.0}]
        });
        let rows = text(&spec, 72);
        let joined = rows.join("\n");
        // The polygon and grid are braille; there is a plotted shape, not bars.
        assert!(
            joined
                .chars()
                .any(|c| ('\u{2800}'..='\u{28ff}').contains(&c)),
            "a braille polygon is drawn:\n{joined}"
        );
        assert!(!joined.contains('▓'), "no horizontal bars remain");
        // Every axis and its value are listed for reading.
        for axis in ["速度 8", "质量 9", "成本 3"] {
            assert!(joined.contains(axis), "missing {axis} in:\n{joined}");
        }
    }

    /// Fewer than three axes cannot close a polygon, so a radar falls back to the
    /// scaled bars.
    #[test]
    fn a_two_axis_radar_falls_back_to_bars() {
        let spec = serde_json::json!({
            "type":"radar",
            "data":[{"name":"速度","value":8.0},{"name":"质量","value":9.0}]
        });
        let joined = text(&spec, 72).join("\n");
        assert!(joined.contains('█'), "bars are drawn:\n{joined}");
    }

    /// A treemap lists shares largest-first with percentages.
    #[test]
    fn a_treemap_ranks_shares_with_percentages() {
        let spec = serde_json::json!({
            "type":"treemap",
            "data":[{"name":"家电","value":300.0},{"name":"电子","value":500.0},{"name":"服装","value":200.0}]
        });
        let rows = text(&spec, 72);
        // Largest share first despite the input order.
        let pos = |name: &str| {
            rows.iter()
                .position(|r| r.contains(name))
                .unwrap_or(usize::MAX)
        };
        assert!(
            pos("电子") < pos("家电") && pos("家电") < pos("服装"),
            "{rows:?}"
        );
        assert!(rows.iter().any(|r| r.contains("50.0%")));
    }

    /// A scatter of `{x, y}` points plots braille dots on numeric axes.
    #[test]
    fn a_scatter_plots_points() {
        let spec = serde_json::json!({
            "type":"scatter",
            "data":[{"x":1.0,"y":2.0},{"x":3.0,"y":6.0},{"x":5.0,"y":4.0}]
        });
        let joined = text(&spec, 72).join("\n");
        assert!(
            joined
                .chars()
                .any(|c| ('\u{2800}'..='\u{28FF}').contains(&c) && c != '\u{2800}'),
            "braille points drawn: {joined}"
        );
    }

    /// A histogram bins its samples and draws one labelled bar per bin — the range
    /// and the count spelled out, which reads more plainly in a terminal than a
    /// wall of touching columns.
    #[test]
    fn a_histogram_bins_samples() {
        let spec = serde_json::json!({
            "type":"histogram",
            "data":[1.0,2.0,2.0,3.0,3.0,3.0,4.0,4.0,5.0]
        });
        let rows = text(&spec, 72);
        assert!(
            rows.iter().any(|r| r.contains('–')),
            "bin ranges labelled: {rows:?}"
        );
        assert!(rows.iter().any(|r| r.contains('█')), "bars drawn");
    }

    /// A boxplot draws whiskers, a box and a median marker on a shared scale.
    #[test]
    fn a_boxplot_draws_box_and_whiskers() {
        let spec = serde_json::json!({
            "type":"boxplot",
            "data":[{"category":"A","value":[10.0,25.0,40.0,55.0,70.0]}]
        });
        let joined = text(&spec, 72).join("\n");
        assert!(
            joined.contains('├') && joined.contains('┤'),
            "whiskers: {joined}"
        );
        assert!(joined.contains('┃'), "median marker: {joined}");
        assert!(joined.contains('▓'), "the quartile box: {joined}");
    }

    /// Raw samples arriving one scalar per row, keyed by group, are summarised into
    /// a box each rather than dropped — the shape a distribution spec often takes.
    #[test]
    fn a_boxplot_summarises_grouped_raw_samples() {
        let spec = serde_json::json!({
            "type":"boxplot",
            "data":[
                {"group":"Tech","value":5.0},{"group":"Tech","value":-3.0},{"group":"Tech","value":8.0},
                {"group":"Tech","value":2.0},{"group":"Tech","value":11.0},
                {"group":"Staples","value":2.0},{"group":"Staples","value":1.0},{"group":"Staples","value":3.0},
                {"group":"Staples","value":-1.0},{"group":"Staples","value":0.5}
            ]
        });
        let rows = text(&spec, 72);
        let joined = rows.join("\n");
        // Both groups drew a box, rather than the whole chart coming back empty.
        assert!(
            joined.contains("Tech") && joined.contains("Staples"),
            "{joined}"
        );
        assert!(
            rows.iter().filter(|r| r.contains('▓')).count() >= 2,
            "a box per group:\n{joined}"
        );
    }

    /// A sankey (and any {source,target,value} flow) is listed as its flows rather
    /// than dropped to a bare 0.
    #[test]
    fn a_sankey_lists_its_flows() {
        let spec = serde_json::json!({
            "type":"sankey",
            "data":[{"source":"工资","target":"储蓄","value":400.0}]
        });
        let joined = text(&spec, 72).join("\n");
        assert!(
            joined.contains("工资 → 储蓄"),
            "the flow is named: {joined}"
        );
        assert!(joined.contains("400"), "with its magnitude");
    }

    /// A word cloud is listed as a ranked word/weight list.
    #[test]
    fn a_word_cloud_ranks_words() {
        let spec = serde_json::json!({
            "type":"word-cloud",
            "data":[{"text":"云计算","value":30.0},{"text":"AI","value":50.0}]
        });
        let rows = text(&spec, 72);
        let pos = |w: &str| {
            rows.iter()
                .position(|r| r.contains(w))
                .unwrap_or(usize::MAX)
        };
        assert!(pos("AI") < pos("云计算"), "ranked by weight: {rows:?}");
    }

    /// A `{name, children}` hierarchy (org chart, mind-map, fishbone) draws as an
    /// indented tree with box-drawing connectors, not just a bare title.
    #[test]
    fn a_hierarchy_draws_as_an_indented_tree() {
        let spec = serde_json::json!({
            "type":"organization-chart",
            "data":{"name":"CEO","children":[
                {"name":"技术部","children":[{"name":"前端组"},{"name":"后端组"}]},
                {"name":"市场部"}
            ]}
        });
        let rows = text(&spec, 60);
        let joined = rows.join("\n");
        assert!(joined.contains("CEO") && joined.contains("前端组") && joined.contains("市场部"));
        assert!(
            joined.contains('├') && joined.contains('└'),
            "tree connectors: {joined}"
        );
        // A leaf under the first branch is indented past a top-level branch,
        // measured in display columns before the label (box chars are multibyte).
        let indent = |name: &str| {
            rows.iter()
                .find(|r| r.contains(name))
                .map_or(0, |r| display_width(&r[..r.find(name).unwrap()]))
        };
        assert!(
            indent("前端组") > indent("技术部"),
            "children indent deeper"
        );
    }

    /// A `{nodes, edges}` graph (network, flow) lists its nodes then its links,
    /// resolving numeric edge endpoints too.
    #[test]
    fn a_graph_lists_nodes_and_links() {
        let spec = serde_json::json!({
            "type":"network-graph",
            "data":{
                "nodes":[{"name":"Alice"},{"name":"Bob"}],
                "edges":[{"source":"Alice","target":"Bob"}]
            }
        });
        let joined = text(&spec, 60).join("\n");
        assert!(
            joined.contains("• Alice") && joined.contains("• Bob"),
            "nodes: {joined}"
        );
        assert!(joined.contains("Alice → Bob"), "links: {joined}");
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

    /// Every chart kind, not just the line plot, is bounded by the given width —
    /// a bar or box that ran past it would be folded by the caller and shredded.
    #[test]
    fn no_chart_kind_exceeds_the_given_width() {
        let specs = serde_json::json!([
            {"type":"area","data":[{"time":"一","value":50.0},{"time":"二","value":80.0},{"time":"三","value":110.0}]},
            {"type":"funnel","data":[{"category":"访问阶段","value":1000.0},{"category":"成交","value":150.0}]},
            {"type":"radar","data":[{"name":"速度","value":8.0},{"name":"质量","value":9.0}]},
            {"type":"treemap","data":[{"name":"电子产品类","value":500.0},{"name":"服装","value":200.0}]},
            {"type":"scatter","data":[{"x":1.0,"y":2.0},{"x":3.0,"y":6.0},{"x":5.0,"y":4.0}]},
            {"type":"histogram","data":[1.0,2.0,3.0,3.0,4.0,7.0,8.0,9.0]},
            {"type":"boxplot","data":[{"category":"甲","value":[10.0,25.0,40.0,55.0,70.0]}]},
            {"type":"sankey","data":[{"source":"工资收入","target":"储蓄账户","value":400.0}]},
            {"type":"word-cloud","data":[{"text":"云计算","value":30.0}]},
            {"type":"organization-chart","data":{"name":"总公司名称很长的根节点","children":[{"name":"技术研发中心部门"},{"name":"市场"}]}},
            {"type":"network-graph","data":{"nodes":[{"name":"节点甲"},{"name":"节点乙"}],"edges":[{"source":"节点甲","target":"节点乙"}]}},
        ]);
        for spec in specs.as_array().unwrap() {
            for width in [40usize, 60, 88, 120, 200] {
                for line in text(spec, width) {
                    assert!(
                        display_width(&line) <= width,
                        "{} at width {width}: {} cols: {line}",
                        spec["type"],
                        display_width(&line)
                    );
                }
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

    #[test]
    fn a_pie_chart_draws_a_donut_with_percentages() {
        let spec = serde_json::json!({
            "type": "pie",
            "data": [
                {"category": "HK", "value": 60.0},
                {"category": "US", "value": 40.0},
            ],
        });
        let joined = text(&spec, 60).join("\n");
        assert!(joined.contains("60.0%") && joined.contains("40.0%"));
        // The slices form a braille ring, with a legend swatch beside it.
        assert!(
            joined
                .chars()
                .any(|c| ('\u{2800}'..='\u{28ff}').contains(&c)),
            "a braille donut is drawn:\n{joined}"
        );
        assert!(joined.contains('█'), "the legend carries colour swatches");
    }

    #[test]
    fn a_pie_with_no_positive_total_draws_nothing() {
        let spec = serde_json::json!({
            "type": "pie",
            "data": [{"category": "A", "value": 0.0}, {"category": "B", "value": 0.0}],
        });
        assert!(render(&spec, 60).is_empty());
    }

    #[test]
    fn a_pie_slice_over_100_percent_stays_within_width() {
        // Mixed signs: [100, -50] → total 50 → the first slice is 200%. The bar
        // must be clamped rather than run off the terminal.
        let spec = serde_json::json!({
            "type": "pie",
            "data": [{"category": "A", "value": 100.0}, {"category": "B", "value": -50.0}],
        });
        for line in text(&spec, 40) {
            assert!(display_width(&line) <= 40, "line exceeds width: {line:?}");
        }
    }

    #[test]
    fn a_column_chart_draws_bars() {
        let spec = serde_json::json!({
            "type": "column",
            "data": [
                {"category": "Jan", "value": 3.0},
                {"category": "Feb", "value": 5.0},
            ],
        });
        let joined = text(&spec, 60).join("\n");
        assert!(joined.contains("Jan") && joined.contains("Feb"));
    }

    /// An all-negative bar chart (e.g. drawdowns) scales by magnitude, so the
    /// larger loss draws the longer bar rather than both collapsing to one cell.
    #[test]
    fn an_all_negative_bar_chart_scales_by_magnitude() {
        let spec = serde_json::json!({
            "type": "bar",
            "data": [
                {"category": "A", "value": -10.0},
                {"category": "B", "value": -100.0},
            ],
        });
        let rows = text(&spec, 60);
        let bars = |name: &str| {
            rows.iter()
                .find(|r| r.contains(name))
                .map_or(0, |r| r.chars().filter(|&c| c == '█').count())
        };
        assert!(bars("B") > bars("A"), "the larger loss is the longer bar");
        assert!(bars("A") > 0);
    }

    /// A runaway `data` array cannot balloon the transcript: the chart is capped
    /// and says how many rows it dropped.
    #[test]
    fn a_huge_chart_is_capped() {
        let data: Vec<_> = (0..5000)
            .map(|i| serde_json::json!({"category": i.to_string(), "value": 1.0}))
            .collect();
        let spec = serde_json::json!({ "type": "bar", "data": data });
        let rows = text(&spec, 60);
        assert!(rows.len() <= 241, "capped, got {} rows", rows.len());
        assert!(
            rows.last().is_some_and(|r| r.contains('⋯')),
            "an elision line reports the drop"
        );
    }

    /// A dual-axes column series that is all negative (net capital flow, say) must
    /// still scale by magnitude: the larger outflow draws the taller bar rather
    /// than every bar collapsing to the minimum height.
    #[test]
    fn a_negative_column_series_scales_by_magnitude() {
        let geo = super::Geometry {
            gutter: 0,
            plot_w: 20,
        };
        let series = super::ChartSeries {
            kind: "column".into(),
            label: String::new(),
            values: vec![-10.0, -100.0],
        };
        let canvas = super::column_canvas(&series, &geo);
        let rows_set = |cell: usize| canvas.iter().filter(|row| row[cell] != 0).count();
        // Rightmost cell carries the -100 bar; a mid cell carries the -10 bar.
        let big = rows_set(19);
        let small = rows_set(5);
        assert!(small > 0, "the smaller magnitude still draws a bar");
        assert!(
            big > small,
            "the larger magnitude draws a taller bar: big={big} small={small}"
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
