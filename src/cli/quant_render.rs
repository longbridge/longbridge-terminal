use std::io::Write;

use crossterm::style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor};
use serde_json::Value;

const SERIES_COLORS: &[Color] = &[
    Color::Cyan,
    Color::Yellow,
    Color::Green,
    Color::Red,
    Color::Magenta,
    Color::Blue,
    Color::DarkYellow,
    Color::DarkCyan,
    Color::DarkGreen,
    Color::DarkRed,
    Color::DarkMagenta,
    Color::DarkBlue,
    Color::White,
    Color::DarkGrey,
];

// ── Terminal helpers ──────────────────────────────────────────────────────────

fn term_width() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(120)
}

type Out<'a> = std::io::StdoutLock<'a>;

fn write_colored(out: &mut Out<'_>, color: Color, text: &str) {
    let _ = crossterm::execute!(
        out,
        SetForegroundColor(color),
        crossterm::style::Print(text),
        ResetColor,
    );
}

fn write_bold(out: &mut Out<'_>, text: &str) {
    let _ = crossterm::execute!(
        out,
        SetAttribute(Attribute::Bold),
        crossterm::style::Print(text),
        SetAttribute(Attribute::Reset),
    );
}

fn write_dim(out: &mut Out<'_>, text: &str) {
    let _ = crossterm::execute!(
        out,
        SetAttribute(Attribute::Dim),
        crossterm::style::Print(text),
        SetAttribute(Attribute::Reset),
    );
}

// ── Data extraction ───────────────────────────────────────────────────────────

struct Series {
    title: String,
    values: Vec<Option<f64>>,
}

fn extract_series(chart_json_raw: &Value) -> Vec<Series> {
    let chart_json = if chart_json_raw.is_string() {
        let s = chart_json_raw.as_str().unwrap_or("{}");
        serde_json::from_str(s).unwrap_or(Value::Null)
    } else {
        chart_json_raw.clone()
    };

    let Some(Value::Object(graphs)) = chart_json.get("series_graphs") else {
        return vec![];
    };

    let mut keys: Vec<u64> = graphs.keys().filter_map(|k| k.parse().ok()).collect();
    keys.sort_unstable();

    keys.into_iter()
        .filter_map(|k| {
            let node = graphs.get(&k.to_string())?;
            let plot = node.get("Plot").unwrap_or(node);
            let title = plot
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Series")
                .to_string();
            let raw = plot.get("series").and_then(Value::as_array)?;
            let values = raw
                .iter()
                .map(|item| match item {
                    Value::Number(n) => n.as_f64(),
                    Value::Object(m) => m
                        .get("Close")
                        .or_else(|| m.get("close"))
                        .or_else(|| m.get("value"))
                        .and_then(Value::as_f64),
                    _ => None,
                })
                .collect();
            Some(Series { title, values })
        })
        .collect()
}

fn fmt_val(v: Option<f64>) -> String {
    match v {
        None => "—".to_string(),
        Some(f) if f.abs() >= 1000.0 => format!("{f:+.0}"),
        Some(f) => format!("{f:+.2}"),
    }
}

// ── Braille sparkline ─────────────────────────────────────────────────────────
//
// Single-row filled-bar braille sparkline: each char covers 2 time slots × 4
// dot rows.  Bars are filled from the bottom up to the value level, giving
// much higher visual fidelity than block chars in a single text row.

#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
fn braille_spark(values: &[Option<f64>], width: usize) -> String {
    let clean: Vec<f64> = values.iter().filter_map(|v| *v).collect();
    if clean.is_empty() {
        return "·".repeat(width);
    }
    let lo = clean.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = clean.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = (hi - lo).max(1e-9);

    let px_w = width * 2;
    let step = values.len() as f64 / px_w as f64;
    let mut bits = vec![0u8; width];

    for px_x in 0..px_w {
        let i = ((px_x as f64 * step) as usize).min(values.len() - 1);
        let Some(v) = values[i] else { continue };
        let norm = (v - lo) / span;
        let px_y = (norm * 3.0).round() as usize; // 0=bottom, 3=top
        let col = px_x / 2;
        let dx = px_x % 2;
        for fill_y in 0..=px_y {
            bits[col] |= dot_bit(dx, 3 - fill_y); // dy=3 is bottom, dy=0 is top
        }
    }

    bits.iter().map(|&b| braille_char(b)).collect()
}

// ── Braille chart ─────────────────────────────────────────────────────────────
//
// Each Unicode braille character is a 2-column × 4-row dot grid.
// This gives 4× the vertical resolution of plain block characters.
//
// Dot-to-bit mapping (standard Unicode braille, U+2800 base):
//   col 0  col 1
//    dot1   dot4   row 0   bits 0x01  0x08
//    dot2   dot5   row 1   bits 0x02  0x10
//    dot3   dot6   row 2   bits 0x04  0x20
//    dot7   dot8   row 3   bits 0x40  0x80

const fn dot_bit(dx: usize, dy: usize) -> u8 {
    match (dx, dy) {
        (0, 0) => 0x01,
        (0, 1) => 0x02,
        (0, 2) => 0x04,
        (0, 3) => 0x40,
        (1, 0) => 0x08,
        (1, 1) => 0x10,
        (1, 2) => 0x20,
        (1, 3) => 0x80,
        _ => 0,
    }
}

fn braille_char(bits: u8) -> char {
    char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ')
}

struct BrailleCanvas {
    /// Bit mask per cell (`char_height` rows × `char_width` cols).
    bits: Vec<Vec<u8>>,
    /// Foreground color assigned to each cell (last writer wins).
    colors: Vec<Vec<Color>>,
    char_width: usize,
    char_height: usize,
}

impl BrailleCanvas {
    fn new(char_width: usize, char_height: usize) -> Self {
        Self {
            bits: vec![vec![0u8; char_width]; char_height],
            colors: vec![vec![Color::DarkGrey; char_width]; char_height],
            char_width,
            char_height,
        }
    }

    /// Plot one series onto the canvas.  Values outside [lo, hi] are clamped.
    /// Adjacent pixels are connected with a vertical stroke so there are no gaps.
    #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
    fn plot(&mut self, values: &[Option<f64>], lo: f64, hi: f64, color: Color) {
        if values.is_empty() {
            return;
        }
        let span = (hi - lo).max(1e-9);
        let px_w = self.char_width * 2;
        let px_h = self.char_height * 4;
        let step = values.len() as f64 / px_w as f64;

        let px_y = |v: f64| -> usize {
            let norm = (v.clamp(lo, hi) - lo) / span;
            (norm * (px_h - 1) as f64).round() as usize
        };

        for px_x in 0..px_w {
            let i0 = ((px_x as f64 * step) as usize).min(values.len() - 1);
            let i1 = (((px_x + 1) as f64 * step) as usize).min(values.len() - 1);

            let Some(v0) = values[i0] else { continue };
            let y0 = px_y(v0);
            let y1 = values[i1].map_or(y0, &px_y);

            let y_lo = y0.min(y1);
            let y_hi = y0.max(y1);
            for y in y_lo..=y_hi {
                let row = (px_h - 1 - y) / 4;
                let dy = (px_h - 1 - y) % 4;
                let col = px_x / 2;
                let dx = px_x % 2;
                if row < self.char_height {
                    self.bits[row][col] |= dot_bit(dx, dy);
                    self.colors[row][col] = color;
                }
            }
        }
    }

    /// Draw a horizontal reference line at value `v` (dim color, does not overwrite series dots).
    #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
    fn hline(&mut self, v: f64, lo: f64, hi: f64) {
        let span = (hi - lo).max(1e-9);
        if v < lo || v > hi {
            return;
        }
        let px_h = self.char_height * 4;
        let y = ((v - lo) / span * (px_h - 1) as f64).round() as usize;
        let row = (px_h - 1 - y) / 4;
        let dy = (px_h - 1 - y) % 4;
        for col in 0..self.char_width {
            // Use both dot columns for a solid line, but don't overwrite series color.
            for dx in 0..2usize {
                if self.bits[row][col] & dot_bit(dx, dy) == 0 {
                    self.bits[row][col] |= dot_bit(dx, dy);
                    // leave color as DarkGrey (dim)
                }
            }
        }
    }
}

const LABEL_W: usize = 10; // chars for y-axis label + separator

#[allow(clippy::cast_precision_loss)]
fn draw_braille_chart(
    out: &mut Out<'_>,
    canvas: &BrailleCanvas,
    lo: f64,
    hi: f64,
    label_rows: &[usize], // which char rows get a y-axis label
) {
    let h = canvas.char_height;
    let span = hi - lo;

    for row in 0..h {
        // Y-axis label (only on selected rows, blank otherwise)
        if label_rows.contains(&row) {
            let v = hi - (row as f64 / (h - 1).max(1) as f64) * span;
            write_dim(out, &format!("{v:>8.2} │"));
        } else {
            write_dim(out, &format!("{:>8} │", ""));
        }

        // Braille characters
        for (col, (&bits, &color)) in canvas.bits[row]
            .iter()
            .zip(canvas.colors[row].iter())
            .enumerate()
        {
            let ch = braille_char(bits).to_string();
            if bits == 0 || (col < canvas.char_width && color == Color::DarkGrey) {
                write_dim(out, &ch);
            } else {
                write_colored(out, color, &ch);
            }
        }
        let _ = writeln!(out);
    }
    // LABEL_W-1 spaces so └ falls directly below the │ separator (│ is at col 9)
    write_dim(
        out,
        &format!(
            "{}└{}\n",
            " ".repeat(LABEL_W - 1),
            "─".repeat(canvas.char_width)
        ),
    );
}

/// Compute shared [lo, hi] range across multiple value slices.
fn combined_range(slices: &[&[Option<f64>]]) -> (f64, f64) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for s in slices {
        for &v in s.iter().flatten() {
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    if lo > hi {
        (0.0, 1.0)
    } else {
        (lo, hi)
    }
}

fn render_braille(out: &mut Out<'_>, series_list: &[Series], term_w: usize) {
    const CHAR_H: usize = 12; // 12 rows × 4 dots = 48 pixel rows
                              // chart_w: terminal width minus label column and 2-char left indent
    let chart_w = term_w.saturating_sub(LABEL_W + 2).max(20);

    // ── Chart 1: Composite (index 0) + Comp Signal (index 1) overlaid ─────────
    if let (Some(s0), Some(s1)) = (series_list.first(), series_list.get(1)) {
        let (lo, hi) = combined_range(&[&s0.values, &s1.values]);

        let mut canvas = BrailleCanvas::new(chart_w, CHAR_H);
        if lo < 0.0 && hi > 0.0 {
            canvas.hline(0.0, lo, hi);
        }
        // Draw signal first so composite renders on top
        canvas.plot(&s1.values, lo, hi, SERIES_COLORS[1]);
        canvas.plot(&s0.values, lo, hi, SERIES_COLORS[0]);

        // Title line with color legend
        let _ = write!(out, "  ");
        write_colored(out, SERIES_COLORS[0], &s0.title);
        write_dim(out, "  +  ");
        write_colored(out, SERIES_COLORS[1], &s1.title);
        let _ = writeln!(out);

        draw_braille_chart(out, &canvas, lo, hi, &[0, 3, 6, 9, CHAR_H - 1]);
        let _ = writeln!(out);
    }

    // ── Chart 2: Comp Hist (index 2) standalone ────────────────────────────────
    if let Some(s2) = series_list.get(2) {
        let (lo, hi) = combined_range(&[&s2.values]);

        let mut canvas = BrailleCanvas::new(chart_w, CHAR_H);
        if lo < 0.0 && hi > 0.0 {
            canvas.hline(0.0, lo, hi);
        }
        canvas.plot(&s2.values, lo, hi, SERIES_COLORS[2]);

        write_colored(out, SERIES_COLORS[2], &format!("  {}\n", s2.title));
        draw_braille_chart(out, &canvas, lo, hi, &[0, 3, 6, 9, CHAR_H - 1]);
        let _ = writeln!(out);
    }
}

// ── Table ─────────────────────────────────────────────────────────────────────

pub fn render_terminal(resp: &Value) {
    // Columns: name(22) | bars(6) | first(10) | last(10) | min(10) | max(10) | sparkline(rest)
    const C_NAME: usize = 22;
    const C_BARS: usize = 6;
    const C_NUM: usize = 10;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let w = term_width();

    let data = resp.get("data").unwrap_or(resp);
    let chart_json_raw = data.get("chart_json").unwrap_or(&Value::Null);
    let series_list = extract_series(chart_json_raw);

    if series_list.is_empty() {
        let _ = writeln!(out, "No chart data in response.");
        return;
    }

    // ── header separator ───────────────────────────────────────────────────────
    let _ = writeln!(out);
    write_dim(&mut out, &"─".repeat(w));
    let _ = writeln!(out);

    // ── table ──────────────────────────────────────────────────────────────────
    let spark_w = w.saturating_sub(C_NAME + C_BARS + C_NUM * 4 + 6).max(20);

    // header row
    write_bold(
        &mut out,
        &format!(
            "{:<C_NAME$}│{:>C_BARS$}│{:>C_NUM$}│{:>C_NUM$}│{:>C_NUM$}│{:>C_NUM$} {}\n",
            "Series", "Bars", "First", "Last", "Min", "Max", "Sparkline"
        ),
    );
    write_dim(&mut out, &format!("{}\n", "─".repeat(w)));

    for (i, s) in series_list.iter().enumerate() {
        let color = SERIES_COLORS[i % SERIES_COLORS.len()];
        let clean: Vec<f64> = s.values.iter().filter_map(|v| *v).collect();
        let bars = clean.len();
        let first = clean.first().copied();
        let last = clean.last().copied();
        let lo = clean.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = clean.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let (lo, hi) = if clean.is_empty() {
            (None, None)
        } else {
            (Some(lo), Some(hi))
        };

        let sp = braille_spark(&s.values, spark_w);
        let name = s.title.chars().take(C_NAME).collect::<String>();

        // name column colored
        write_colored(&mut out, color, &format!("{name:<C_NAME$}"));
        let _ = write!(
            out,
            "│{bars:>C_BARS$}│{:>C_NUM$}│{:>C_NUM$}│{:>C_NUM$}│{:>C_NUM$} ",
            fmt_val(first),
            fmt_val(last),
            fmt_val(lo),
            fmt_val(hi),
        );
        write_colored(&mut out, color, &sp);
        let _ = writeln!(out);
    }

    write_dim(&mut out, &format!("{}\n", "─".repeat(w)));
    let total_bars = series_list.first().map_or(0, |s| s.values.len());
    write_dim(
        &mut out,
        &format!("  {} series  ·  {} bars\n\n", series_list.len(), total_bars),
    );

    render_braille(&mut out, &series_list, w);
}
