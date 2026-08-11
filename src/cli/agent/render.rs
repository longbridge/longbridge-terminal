//! Terminal rendering for agent answers: markdown, vis-chart blocks,
//! x-widget references, and inline entity markers.

use serde_json::Value;
use std::fmt::Write;
use unicode_width::UnicodeWidthStr;

/// Terminal display width (CJK / fullwidth glyphs count as 2 columns), as
/// opposed to `chars().count()` which undercounts wide glyphs and would
/// misalign labels, bars, and boxes containing Chinese/Japanese/Korean text.
fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Right-pad `s` with spaces until it reaches `width` display columns.
fn pad_display(s: &str, width: usize) -> String {
    let w = display_width(s);
    if w >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - w))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    Text(String),
    VisChart(Value),
    XWidget(String),
}

/// Split an answer into text / chart / widget segments, preserving order.
/// Malformed vis-chart JSON keeps the fenced block as text so no content
/// is silently dropped.
pub fn segment_answer(answer: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut text_acc = String::new();
    let mut rest = answer;

    loop {
        let chart_pos = rest.find("```vis-chart");
        let widget_pos = rest.find("<x-widget");
        let (pos, is_chart) = match (chart_pos, widget_pos) {
            (Some(c), Some(w)) if c <= w => (c, true),
            (Some(c), None) => (c, true),
            (_, Some(w)) => (w, false),
            (None, None) => break,
        };
        text_acc.push_str(&rest[..pos]);
        rest = &rest[pos..];

        if is_chart {
            let after = &rest["```vis-chart".len()..];
            let Some(end) = after.find("```") else {
                break; // unterminated fence: emit as text
            };
            match serde_json::from_str::<Value>(after[..end].trim()) {
                Ok(spec) => {
                    flush_text(&mut segments, &mut text_acc);
                    segments.push(Segment::VisChart(spec));
                }
                Err(_) => text_acc.push_str(&rest[.."```vis-chart".len() + end + 3]),
            }
            rest = &after[end + 3..];
        } else {
            // Find the opening tag's end
            let Some(tag_end_pos) = rest.find('>') else {
                break;
            };
            let tag_end = tag_end_pos + 1;
            let opening_tag = &rest[..tag_end];

            // Check if tag is self-closed (ends with />)
            let is_self_closed = opening_tag.trim_end().ends_with("/>");

            let end = if is_self_closed {
                tag_end
            } else {
                // Look for </x-widget> after the opening tag
                rest[tag_end..]
                    .find("</x-widget>")
                    .map_or(tag_end, |p| tag_end + p + "</x-widget>".len())
            };

            let consumed = &rest[..end];
            if let Some(src) = opening_tag
                .find("src=\"")
                .map(|p| &opening_tag[p + 5..])
                .and_then(|s| s.find('"').map(|e| s[..e].to_string()))
            {
                flush_text(&mut segments, &mut text_acc);
                segments.push(Segment::XWidget(src));
            } else {
                text_acc.push_str(consumed);
            }
            rest = &rest[end..];
        }
    }
    text_acc.push_str(rest);
    flush_text(&mut segments, &mut text_acc);
    segments
}

fn flush_text(segments: &mut Vec<Segment>, acc: &mut String) {
    if acc.trim().is_empty() {
        acc.clear();
    } else {
        segments.push(Segment::Text(std::mem::take(acc)));
    }
}

/// Strip control characters from server-originated text before it reaches
/// the terminal. C0 controls are removed except `\n` and `\t` (needed for
/// readable formatting); `ESC` (0x1b, the entry point for ANSI/OSC escape
/// sequences) and `DEL` (0x7f) are removed too. Without this, a
/// malicious/buggy answer, question, tool name, or reference field could
/// smuggle terminal escape sequences (e.g. an OSC title-bar rewrite or an
/// SGR color reset) into stdout/stderr.
pub fn strip_control_chars(s: &str) -> String {
    s.chars()
        .filter(|&c| c == '\n' || c == '\t' || (!c.is_control()))
        .collect()
}

/// Replace `[stock Name]` and `[citation N]` inline markers.
pub fn replace_inline_markers(text: &str, color: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(end) = after.find(']') else {
            out.push_str(after);
            return out;
        };
        let inner = &after[1..end];
        if let Some(name) = inner.strip_prefix("stock ") {
            if color {
                let _ = write!(out, "\x1b[36m{name}\x1b[0m");
            } else {
                out.push_str(name);
            }
        } else if let Some(n) = inner.strip_prefix("citation ") {
            if color {
                let _ = write!(out, "\x1b[2m[{n}]\x1b[0m");
            } else {
                let _ = write!(out, "[{n}]");
            }
        } else {
            out.push_str(&after[..=end]);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Extract the symbol from a quote-detail widget URL.
pub fn parse_quote_widget_symbol(src: &str) -> Option<String> {
    if !src.starts_with("widget://quote/") {
        return None;
    }
    let query = src.split_once('?')?.1;
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix("symbol="))
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

pub struct QuoteCardData {
    pub symbol: String,
    pub name: String,
    pub last: String,
    pub change_pct: String,
    /// 1 = up, -1 = down, 0 = flat.
    pub direction: i8,
}

/// One extracted numeric series from a vis-chart spec.
struct ChartSeries {
    kind: String, // "line" | "column"
    label: String,
    values: Vec<f64>,
}

/// Normalize the two observed vis-chart data shapes into (categories, series):
/// - `{categories: [...], series: [{type, data: [...], axisYTitle}]}` (dual-axes)
/// - `{data: [{category, value, group?}]}` (column / pie / line)
fn chart_series(spec: &Value) -> (Vec<String>, Vec<ChartSeries>) {
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
                label: strip_control_chars(
                    s.get("axisYTitle")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                ),
                values: s
                    .get("data")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_f64).collect())
                    .unwrap_or_default(),
            })
            .collect();
        return (categories, series);
    }
    if let Some(data) = spec.get("data").and_then(Value::as_array) {
        // group rows by `group` (falling back to a single anonymous series)
        let mut categories: Vec<String> = Vec::new();
        let mut groups: Vec<(String, Vec<f64>)> = Vec::new();
        for row in data {
            let cat = strip_control_chars(
                row.get("category")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            if !categories.contains(&cat) {
                categories.push(cat.clone());
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
                kind: "column".to_string(),
                label,
                values,
            })
            .collect();
        return (categories, series);
    }
    (Vec::new(), Vec::new())
}

/// Render a vis-chart JSON spec as terminal text.
pub fn render_vis_chart(spec: &Value, width: usize, color: bool) -> String {
    let chart_type = spec.get("type").and_then(Value::as_str).unwrap_or_default();
    let title = strip_control_chars(
        spec.get("title")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    let (categories, series) = chart_series(spec);
    let mut out = String::new();
    if !title.is_empty() {
        let _ = writeln!(out, "  {title}");
    }
    let body = match chart_type {
        "line" | "area" | "dual-axes" => render_line_block(&categories, &series, width, color),
        "column" | "bar" => render_bar_block(&categories, &series, width),
        "pie" => render_pie_block(&categories, &series, width),
        _ => render_table_block(&categories, &series),
    };
    out.push_str(&body);
    out
}

const BRAILLE_DOTS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];

/// Braille canvas for line series + block row for column series.
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
fn render_line_block(
    categories: &[String],
    series: &[ChartSeries],
    width: usize,
    color: bool,
) -> String {
    // Series with no data points are dropped up front: an empty line series
    // has no min/max and would otherwise poison the shared braille canvas
    // (division by a zero span, garbage plotting positions).
    let lines: Vec<&ChartSeries> = series
        .iter()
        .filter(|s| s.kind != "column" && !s.values.is_empty())
        .collect();
    let columns: Vec<&ChartSeries> = series
        .iter()
        .filter(|s| s.kind == "column" && !s.values.is_empty())
        .collect();
    if lines.is_empty() && columns.is_empty() {
        return String::new();
    }
    let mut out = String::new();

    if !lines.is_empty() {
        let all: Vec<f64> = lines
            .iter()
            .flat_map(|s| s.values.iter().copied())
            .collect();
        let (min, max) = all
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        let span = if (max - min).abs() < f64::EPSILON {
            1.0
        } else {
            max - min
        };
        let label_w = format!("{max:.1}").len().max(format!("{min:.1}").len());
        let chart_w = width.saturating_sub(label_w + 2).clamp(16, 120);
        let rows = 6usize; // 6 braille rows = 24 dot rows
        let mut canvas = vec![vec![0u8; chart_w]; rows];
        let n = lines.iter().map(|s| s.values.len()).max().unwrap_or(0);
        for s in &lines {
            let mut prev: Option<(usize, usize)> = None;
            for (i, &v) in s.values.iter().enumerate() {
                let x = if n <= 1 {
                    0
                } else {
                    i * (chart_w * 2 - 1) / (n - 1)
                };
                let y = ((max - v) / span * ((rows * 4 - 1) as f64)).round() as usize;
                if let Some((px, py)) = prev {
                    // vertical interpolation so lines look connected
                    let (from, to) = if py <= y { (py, y) } else { (y, py) };
                    for yy in from..=to {
                        plot(&mut canvas, usize::midpoint(px, x), yy);
                    }
                }
                plot(&mut canvas, x, y);
                prev = Some((x, y));
            }
        }
        for (r, row) in canvas.iter().enumerate() {
            let label = if r == 0 {
                format!("{max:>label_w$.1}")
            } else if r == rows - 1 {
                format!("{min:>label_w$.1}")
            } else {
                " ".repeat(label_w)
            };
            let cells: String = row.iter().map(|&bits| braille(bits)).collect();
            let _ = writeln!(out, "{label}┤{cells}");
        }
        let names: Vec<String> = lines
            .iter()
            .filter(|s| !s.label.is_empty())
            .map(|s| s.label.clone())
            .collect();
        if !names.is_empty() {
            let joined = names.join(" · ");
            if color {
                let _ = writeln!(out, "{}\x1b[2m⣿ {joined}\x1b[0m", " ".repeat(label_w + 1));
            } else {
                let _ = writeln!(out, "{}⣿ {joined}", " ".repeat(label_w + 1));
            }
        }
    }

    for s in &columns {
        let max = s.values.iter().copied().fold(f64::MIN, f64::max).max(1.0);
        let blocks: String = s
            .values
            .iter()
            .map(|&v| {
                let idx = ((v / max) * 7.0).round() as u32;
                char::from_u32(0x2581 + idx.min(7)).unwrap_or('▁')
            })
            .collect();
        let label = if s.label.is_empty() {
            "volume"
        } else {
            &s.label
        };
        let _ = writeln!(out, "  {blocks} {label}");
    }

    if !categories.is_empty() {
        let first = categories.first().map_or("", String::as_str);
        let last = categories.last().map_or("", String::as_str);
        let _ = writeln!(out, "  {first} … {last}");
    }
    out
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
fn render_bar_block(categories: &[String], series: &[ChartSeries], width: usize) -> String {
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
    let mut out = String::new();
    for (ci, cat) in categories.iter().enumerate() {
        for s in series {
            let Some(&v) = s.values.get(ci) else { continue };
            let n = ((v / max) * bar_w as f64).round().max(1.0) as usize;
            let label = if s.label.is_empty() {
                cat.clone()
            } else {
                format!("{cat} {}", s.label)
            };
            let bar = "▓".repeat(n);
            let padded = pad_display(&label, label_w);
            let _ = writeln!(out, "  {padded} {bar} {v}");
        }
    }
    out
}

/// Proportion bars with percentages.
#[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
fn render_pie_block(categories: &[String], series: &[ChartSeries], width: usize) -> String {
    let Some(s) = series.first() else {
        return String::new();
    };
    let total: f64 = s.values.iter().sum();
    if total <= 0.0 {
        return String::new();
    }
    let label_w = categories
        .iter()
        .map(|c| display_width(c))
        .max()
        .unwrap_or(0);
    let bar_w = width.saturating_sub(label_w + 12).clamp(10, 40);
    let mut out = String::new();
    for (ci, cat) in categories.iter().enumerate() {
        let Some(&v) = s.values.get(ci) else { continue };
        let pct = v / total * 100.0;
        let n = ((pct / 100.0) * bar_w as f64).round().max(1.0) as usize;
        let bar = "▓".repeat(n);
        let padded = pad_display(cat, label_w);
        let _ = writeln!(out, "  {padded} {bar} {pct:.1}%");
    }
    out
}

/// Fallback: plain aligned table of category/value pairs per series.
fn render_table_block(categories: &[String], series: &[ChartSeries]) -> String {
    let mut out = String::new();
    let label_w = categories
        .iter()
        .map(|c| display_width(c))
        .max()
        .unwrap_or(0);
    for (ci, cat) in categories.iter().enumerate() {
        let values: Vec<String> = series
            .iter()
            .filter_map(|s| s.values.get(ci).map(|v| format!("{v}")))
            .collect();
        let joined = values.join("  ");
        let padded = pad_display(cat, label_w);
        let _ = writeln!(out, "  {padded}  {joined}");
    }
    out
}

/// Boxed mini quote card. `head` and `body` sit on their own lines inside the
/// box (rather than embedded in the border) so the top/bottom border and both
/// content lines always share the same display width — including for CJK
/// names, where a naive `chars().count()` pad would misalign the box.
pub fn render_quote_card(card: &QuoteCardData, color: bool) -> String {
    // Card fields come from the quote API (symbol/name are server-controlled),
    // so they are sanitized before the widths are measured — stripping later
    // would leave the box borders misaligned.
    let head = format!(
        "{} · {}",
        strip_control_chars(&card.symbol),
        strip_control_chars(&card.name)
    );
    let body = format!(
        "{}  {}",
        strip_control_chars(&card.last),
        strip_control_chars(&card.change_pct)
    );
    let inner_w = display_width(&head).max(display_width(&body));
    let colored_body = if color {
        let sgr = match card.direction {
            1 => "\x1b[32m",
            -1 => "\x1b[31m",
            _ => "\x1b[0m",
        };
        format!("{sgr}{body}\x1b[0m")
    } else {
        body.clone()
    };
    let border = "─".repeat(inner_w);
    let head_line = pad_display(&head, inner_w);
    let body_pad = " ".repeat(inner_w.saturating_sub(display_width(&body)));
    format!("┌─{border}─┐\n│ {head_line} │\n│ {colored_body}{body_pad} │\n└─{border}─┘\n")
}

/// Full answer rendering pipeline for pretty output.
pub fn render_answer(
    answer: &str,
    quotes: &std::collections::HashMap<String, QuoteCardData>,
    width: usize,
    color: bool,
) -> String {
    let mut out = String::new();
    for segment in segment_answer(answer) {
        match segment {
            Segment::Text(text) => {
                let text = strip_control_chars(&text);
                let text = replace_inline_markers(&text, color);
                let skin = if color {
                    termimad::MadSkin::default()
                } else {
                    termimad::MadSkin::no_style()
                };
                let _ = write!(out, "{}", skin.text(&text, Some(width)));
                out.push('\n');
            }
            Segment::VisChart(spec) => {
                out.push_str(&render_vis_chart(&spec, width, color));
                out.push('\n');
            }
            Segment::XWidget(src) => {
                match parse_quote_widget_symbol(&src).and_then(|sym| quotes.get(&sym)) {
                    Some(card) => out.push_str(&render_quote_card(card, color)),
                    None => {
                        // `src` is raw answer markup: sanitize before echoing.
                        let _ = writeln!(out, "→ {}", strip_control_chars(&src));
                    }
                }
                out.push('\n');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_split_text_chart_widget() {
        let md = "intro\n```vis-chart\n{\"type\":\"column\"}\n```\nmiddle\n<x-widget src=\"widget://quote/security/detail?symbol=700.HK\"></x-widget>\ntail";
        let segs = segment_answer(md);
        assert_eq!(segs.len(), 5);
        assert!(matches!(&segs[0], Segment::Text(t) if t.contains("intro")));
        assert!(matches!(&segs[1], Segment::VisChart(v) if v["type"] == "column"));
        assert!(matches!(&segs[2], Segment::Text(t) if t.contains("middle")));
        assert!(matches!(&segs[3], Segment::XWidget(s) if s.contains("700.HK")));
        assert!(matches!(&segs[4], Segment::Text(t) if t.contains("tail")));
    }

    #[test]
    fn segments_malformed_chart_stays_text() {
        let md = "a\n```vis-chart\nnot json\n```\nb";
        let segs = segment_answer(md);
        // Malformed spec: keep the whole fenced block as text so nothing is lost
        assert!(segs.iter().all(|s| matches!(s, Segment::Text(_))));
        let joined: String = segs
            .iter()
            .map(|s| match s {
                Segment::Text(t) => t.as_str(),
                _ => "",
            })
            .collect();
        assert!(joined.contains("not json"));
    }

    #[test]
    fn plain_answer_is_single_text_segment() {
        assert_eq!(
            segment_answer("hello"),
            vec![Segment::Text("hello".to_string())]
        );
    }

    #[test]
    fn inline_stock_marker_colored_and_plain() {
        let s = replace_inline_markers("hold [stock Apple] today", true);
        assert!(s.contains("\x1b[36mApple\x1b[0m"));
        let plain = replace_inline_markers("hold [stock Apple] today", false);
        assert_eq!(plain, "hold Apple today");
    }

    #[test]
    fn inline_citation_marker() {
        let plain = replace_inline_markers("as reported [citation 3].", false);
        assert_eq!(plain, "as reported [3].");
        let colored = replace_inline_markers("as reported [citation 3].", true);
        assert!(colored.contains("\x1b[2m[3]\x1b[0m"));
    }

    #[test]
    fn cjk_text_passes_through_markers() {
        let plain = replace_inline_markers("对比 [stock 腾讯控股] 与 [stock Apple]", false);
        assert_eq!(plain, "对比 腾讯控股 与 Apple");
    }

    #[test]
    fn quote_widget_symbol_parses() {
        assert_eq!(
            parse_quote_widget_symbol("widget://quote/security/detail?symbol=TSLA.US&time_range=1"),
            Some("TSLA.US".to_string())
        );
        assert_eq!(
            parse_quote_widget_symbol("widget://portfolio/summary"),
            None
        );
        assert_eq!(
            parse_quote_widget_symbol("widget://quote/security/detail"),
            None
        );
    }

    #[test]
    fn self_closed_widget_does_not_swallow_later_content() {
        let md = "<x-widget src=\"a\"/> some content <x-widget src=\"b\"></x-widget> tail";
        let segs = segment_answer(md);
        // Should have: XWidget(a), Text(some content), XWidget(b), Text(tail)
        assert_eq!(segs.len(), 4);
        assert!(matches!(&segs[0], Segment::XWidget(s) if s == "a"));
        assert!(matches!(&segs[1], Segment::Text(t) if t.contains("some content")));
        assert!(matches!(&segs[2], Segment::XWidget(s) if s == "b"));
        assert!(matches!(&segs[3], Segment::Text(t) if t.contains("tail")));
    }

    #[test]
    fn unterminated_widget_preserves_trailing_content() {
        let md = "<x-widget src=\"a\">unclosed content here, no closing tag";
        let segs = segment_answer(md);
        // Should have: XWidget(a), Text(unclosed content here, no closing tag)
        assert_eq!(segs.len(), 2);
        assert!(matches!(&segs[0], Segment::XWidget(s) if s == "a"));
        assert!(matches!(&segs[1], Segment::Text(t) if t.contains("unclosed content")));
    }

    fn dual_axes_spec() -> serde_json::Value {
        serde_json::json!({
            "type": "dual-axes",
            "categories": ["7/10", "7/11", "7/12", "7/13"],
            "title": "TSLA close and volume",
            "series": [
                {"type": "line", "data": [407.7, 394.7, 396.1, 319.6], "axisYTitle": "close"},
                {"type": "column", "data": [3341.0, 3281.0, 2338.0, 11561.0], "axisYTitle": "volume"}
            ]
        })
    }

    #[test]
    fn dual_axes_renders_braille_and_volume_row() {
        let out = render_vis_chart(&dual_axes_spec(), 60, false);
        assert!(out.contains("TSLA close and volume"));
        // braille dots for the line series
        assert!(out.chars().any(|c| ('\u{2800}'..='\u{28FF}').contains(&c)));
        // block elements for the column series
        assert!(out.chars().any(|c| ('\u{2581}'..='\u{2588}').contains(&c)));
        // y-axis extremes labeled
        assert!(out.contains("407.7") && out.contains("319.6"));
    }

    #[test]
    fn grouped_column_renders_bars() {
        let spec = serde_json::json!({
            "type": "column",
            "data": [
                {"category": "PE", "value": 35.47, "group": "AAPL"},
                {"category": "PE", "value": 34.10, "group": "NVDA"},
                {"category": "PB", "value": 42.53, "group": "AAPL"},
                {"category": "PB", "value": 27.84, "group": "NVDA"}
            ],
            "group": true,
            "title": "Valuation"
        });
        let out = render_vis_chart(&spec, 60, false);
        assert!(out.contains("Valuation"));
        assert!(out.contains("AAPL") && out.contains("NVDA"));
        assert!(out.contains('▓'));
        assert!(out.contains("42.53"));
        // longest value owns the longest bar
        let bar_len = |needle: &str| {
            out.lines()
                .find(|l| l.contains(needle))
                .map_or(0, |l| l.chars().filter(|&c| c == '▓').count())
        };
        assert!(bar_len("42.53") > bar_len("27.84"));
    }

    #[test]
    fn pie_renders_percentages() {
        let spec = serde_json::json!({
            "type": "pie",
            "data": [
                {"category": "US", "value": 75.0},
                {"category": "HK", "value": 25.0}
            ]
        });
        let out = render_vis_chart(&spec, 60, false);
        assert!(out.contains("75.0%") && out.contains("25.0%"));
    }

    #[test]
    fn unknown_chart_type_falls_back_to_table() {
        let spec = serde_json::json!({
            "type": "radar",
            "data": [{"category": "growth", "value": 8.0}]
        });
        let out = render_vis_chart(&spec, 60, false);
        assert!(out.contains("growth") && out.contains('8'));
        assert!(!out.contains('▓')); // table, not bars
    }

    #[test]
    fn quote_card_renders_box() {
        let card = QuoteCardData {
            symbol: "TSLA.US".into(),
            name: "Tesla".into(),
            last: "328.58".into(),
            change_pct: "+2.83%".into(),
            direction: 1,
        };
        let plain = render_quote_card(&card, false);
        assert!(plain.contains("TSLA.US") && plain.contains("Tesla"));
        assert!(plain.contains("328.58") && plain.contains("+2.83%"));
        assert!(plain.contains('┌') && plain.contains('└'));
        let colored = render_quote_card(&card, true);
        assert!(colored.contains("\x1b[32m")); // up = green
    }

    #[test]
    fn render_answer_full_pipeline() {
        let md = "## Title\n\nsee [stock Tesla]\n\n```vis-chart\n{\"type\":\"pie\",\"data\":[{\"category\":\"a\",\"value\":1.0}]}\n```\n\n<x-widget src=\"widget://quote/security/detail?symbol=TSLA.US\"></x-widget>\n<x-widget src=\"widget://unknown/thing\"></x-widget>";
        let mut quotes = std::collections::HashMap::new();
        quotes.insert(
            "TSLA.US".to_string(),
            QuoteCardData {
                symbol: "TSLA.US".into(),
                name: "Tesla".into(),
                last: "328.58".into(),
                change_pct: "+2.83%".into(),
                direction: 1,
            },
        );
        let out = render_answer(md, &quotes, 60, false);
        assert!(out.contains("Tesla")); // marker replaced + card
        assert!(out.contains("100.0%")); // pie rendered
        assert!(out.contains("328.58")); // quote card
        assert!(out.contains("widget://unknown/thing")); // unknown widget hint
        assert!(!out.contains("[stock")); // no raw markers survive
        assert!(!out.contains("vis-chart")); // no raw fences survive
    }

    // -- fix round 1 regression tests --

    #[test]
    fn quote_card_lines_have_equal_char_count() {
        let card = QuoteCardData {
            symbol: "TSLA.US".into(),
            name: "Tesla".into(),
            last: "328.58".into(),
            change_pct: "+2.83%".into(),
            direction: 1,
        };
        let plain = render_quote_card(&card, false);
        let lens: Vec<usize> = plain.lines().map(|l| l.chars().count()).collect();
        assert!(!lens.is_empty());
        assert!(
            lens.iter().all(|&n| n == lens[0]),
            "border/body lines misaligned: {lens:?}\n{plain}"
        );
    }

    #[test]
    fn dual_axes_empty_line_series_skips_braille_without_panic() {
        // Line series has no data points; the column series still has some.
        let spec = serde_json::json!({
            "type": "dual-axes",
            "categories": ["7/10", "7/11"],
            "series": [
                {"type": "line", "data": [], "axisYTitle": "close"},
                {"type": "column", "data": [10.0, 20.0], "axisYTitle": "volume"}
            ]
        });
        let out = render_vis_chart(&spec, 60, false);
        // No braille garbage from an empty/degenerate line series.
        assert!(!out.chars().any(|c| ('\u{2800}'..='\u{28FF}').contains(&c)));
        // The column block row is still rendered.
        assert!(out.chars().any(|c| ('\u{2581}'..='\u{2588}').contains(&c)));
    }

    #[test]
    fn dual_axes_all_series_empty_does_not_panic() {
        let spec = serde_json::json!({
            "type": "dual-axes",
            "series": [
                {"type": "line", "data": []},
                {"type": "column", "data": []}
            ]
        });
        let out = render_vis_chart(&spec, 60, false);
        assert!(!out.chars().any(|c| ('\u{2800}'..='\u{28FF}').contains(&c)));
    }

    #[test]
    fn bar_labels_align_with_cjk_categories() {
        let spec = serde_json::json!({
            "type": "column",
            "data": [
                {"category": "腾讯控股", "value": 10.0},
                {"category": "AAPL", "value": 20.0}
            ]
        });
        let out = render_vis_chart(&spec, 60, false);
        // The `▓` bar must start at the same terminal column on both lines,
        // measured in display width (not byte or char count), regardless of
        // the CJK label being visually wider than its char count.
        let bar_col = |needle: &str| {
            out.lines().find(|l| l.contains(needle)).and_then(|l| {
                l.find('▓')
                    .map(|byte_idx| UnicodeWidthStr::width(&l[..byte_idx]))
            })
        };
        let cjk_col = bar_col("腾讯控股").expect("cjk line present");
        let ascii_col = bar_col("AAPL").expect("ascii line present");
        assert_eq!(cjk_col, ascii_col);
    }

    #[test]
    fn strip_control_chars_removes_escape_sequences() {
        let hostile = "\x1b]0;evil\x07 hello \x1b[31mred\x1b[0m";
        let clean = strip_control_chars(hostile);
        assert!(!clean.contains('\x1b'), "ESC survived: {clean:?}");
        assert!(!clean.contains('\x07'), "BEL survived: {clean:?}");
        assert!(clean.contains("hello"));
        assert!(clean.contains("red"));
    }

    #[test]
    fn strip_control_chars_keeps_newline_and_tab() {
        let s = "line1\n\tindented";
        assert_eq!(strip_control_chars(s), s);
    }

    #[test]
    fn render_answer_without_color_emits_no_ansi_escapes() {
        let md = "## Title\n\nSome *emphasized* text and a [link](http://example.com).";
        let out = render_answer(md, &std::collections::HashMap::new(), 60, false);
        assert!(
            !out.contains('\x1b'),
            "no-color render must not contain ANSI escapes: {out:?}"
        );
    }

    // -- fix round 3 regression tests: sanitize every server-derived string --

    #[test]
    fn chart_title_categories_and_labels_are_sanitized() {
        let spec = serde_json::json!({
            "type": "dual-axes",
            "title": "title\x1b[31m",
            "categories": ["7/10\x1b[31m", "7/11"],
            "series": [
                {"type": "line", "data": [1.0, 2.0], "axisYTitle": "close\x1b[2m"},
                {"type": "column", "data": [3.0, 4.0], "axisYTitle": "volume\x07"}
            ]
        });
        let out = render_vis_chart(&spec, 60, false);
        assert!(!out.contains('\x1b'), "ESC survived in chart: {out:?}");
        assert!(!out.contains('\x07'), "BEL survived in chart: {out:?}");
        assert!(out.contains("title") && out.contains("close") && out.contains("volume"));
    }

    #[test]
    fn bar_chart_categories_and_groups_are_sanitized() {
        let spec = serde_json::json!({
            "type": "column",
            "data": [
                {"category": "P\x1b[31mE", "value": 10.0, "group": "AA\x07PL"},
                {"category": "PB", "value": 20.0, "group": "AA\x07PL"}
            ]
        });
        let out = render_vis_chart(&spec, 60, false);
        assert!(!out.contains('\x1b') && !out.contains('\x07'), "{out:?}");
    }

    #[test]
    fn unknown_widget_hint_is_sanitized() {
        let md = "<x-widget src=\"widget://evil\x1b]0;pwn\x07/thing\"></x-widget>";
        let out = render_answer(md, &std::collections::HashMap::new(), 60, false);
        assert!(out.contains("widget://evil"));
        assert!(
            !out.contains('\x1b'),
            "ESC survived in widget hint: {out:?}"
        );
        assert!(
            !out.contains('\x07'),
            "BEL survived in widget hint: {out:?}"
        );
    }

    #[test]
    fn quote_card_fields_are_sanitized_and_stay_aligned() {
        let card = QuoteCardData {
            symbol: "700\x1b[31m.HK".into(),
            name: "Ten\x07cent".into(),
            last: "320.00".into(),
            change_pct: "-1.20%".into(),
            direction: -1,
        };
        let plain = render_quote_card(&card, false);
        assert!(
            !plain.contains('\x1b') && !plain.contains('\x07'),
            "{plain:?}"
        );
        let widths: Vec<usize> = plain.lines().map(UnicodeWidthStr::width).collect();
        assert!(
            widths.iter().all(|&w| w == widths[0]),
            "card misaligned after sanitization: {widths:?}\n{plain}"
        );
    }

    #[test]
    fn quote_card_aligns_with_cjk_name() {
        let card = QuoteCardData {
            symbol: "700.HK".into(),
            name: "腾讯控股".into(),
            last: "320.00".into(),
            change_pct: "-1.20%".into(),
            direction: -1,
        };
        let plain = render_quote_card(&card, false);
        let widths: Vec<usize> = plain.lines().map(UnicodeWidthStr::width).collect();
        assert!(!widths.is_empty());
        assert!(
            widths.iter().all(|&w| w == widths[0]),
            "CJK card lines have unequal display width: {widths:?}\n{plain}"
        );
    }
}
