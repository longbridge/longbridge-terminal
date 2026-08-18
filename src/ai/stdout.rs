//! Rendering an answer to stdout, as ANSI text.
//!
//! The counterpart of [`crate::ai::tui`]'s renderer: both consume the same
//! answer model ([`crate::ai::answer`]) and the same chart drawing
//! ([`crate::ai::chart`]), one producing ratatui lines and this one producing
//! ANSI text for `longbridge agent chat`. It lives here, with the rest of the
//! answer rendering, rather than under the `agent` command that happens to be
//! its only caller — `src/cli/agent` is that command's own plumbing.

use serde_json::Value;
use std::fmt::Write;

use ratatui::style::Color;

use super::answer::{
    parse_quote_widget_symbol, parse_widget, replace_inline_markers, segment_answer, Segment,
    WidgetRef,
};
use super::quotes::QuoteCardData;
use crate::utils::text::{display_width, pad_display, strip_control_chars};

/// Render a vis-chart spec for stdout.
///
/// The drawing lives in [`crate::ai::chart`], which produces styled lines; this
/// flattens them to ANSI (or to bare text when `color` is off). Charts are drawn
/// once, in one place, so `agent chat` and the `ai` TUI cannot drift apart.
pub fn render_vis_chart(spec: &Value, width: usize, color: bool) -> String {
    let mut out = String::new();
    for line in super::chart::render(spec, width) {
        for span in &line.spans {
            match span.style.fg.filter(|_| color) {
                Some(fg) => {
                    let _ = write!(out, "\x1b[{}m{}\x1b[0m", sgr(fg), span.content);
                }
                None => out.push_str(&span.content),
            }
        }
        out.push('\n');
    }
    out
}

/// SGR foreground code for the colors the chart renderer uses.
fn sgr(color: Color) -> u8 {
    match color {
        Color::Cyan => 36,
        Color::Blue => 34,
        Color::DarkGray => 90,
        Color::Red => 31,
        Color::Green => 32,
        Color::Yellow => 33,
        Color::Magenta => 35,
        _ => 39, // default foreground
    }
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
        // Respect the reader's red-up/green-up preference rather than hardcoding
        // the Western convention — a rising price is red for a mainland reader.
        let c = crate::tui::ui::styles::up_color(card.direction.cmp(&0));
        format!("\x1b[{}m{body}\x1b[0m", sgr(c))
    } else {
        body.clone()
    };
    let border = "─".repeat(inner_w);
    let head_line = pad_display(&head, inner_w);
    let body_pad = " ".repeat(inner_w.saturating_sub(display_width(&body)));
    format!("┌─{border}─┐\n│ {head_line} │\n│ {colored_body}{body_pad} │\n└─{border}─┘\n")
}

/// A widget reference as one line of text.
///
/// The terminal cannot embed the widget, so it says what the reference is about.
/// An order ticket is read back in full — it is the most consequential thing an
/// answer can carry — while the rest name themselves.
fn widget_summary(widget: &WidgetRef) -> String {
    match widget {
        WidgetRef::Quote { symbol } => symbol.clone(),
        WidgetRef::Comparison { symbols } => {
            format!("{}: {}", t!("Ai.WidgetComparison"), symbols.join(", "))
        }
        WidgetRef::StockList { symbols } => {
            format!("{}: {}", t!("Ai.WidgetStockList"), symbols.join(", "))
        }
        WidgetRef::Cta { action } => match action.as_str() {
            "open_account" => t!("Ai.WidgetCta.open_account").to_string(),
            "fund_account" => t!("Ai.WidgetCta.fund_account").to_string(),
            "complete_profile" => t!("Ai.WidgetCta.complete_profile").to_string(),
            other => other.replace('_', " "),
        },
        WidgetRef::OrderTicket(ticket) => {
            let mut parts = vec![
                ticket.side.as_str(),
                ticket.quantity.as_str(),
                ticket.symbol.as_str(),
                ticket.order_type.as_str(),
            ];
            parts.retain(|p| !p.is_empty());
            let mut summary = format!("{}  {}", t!("Ai.WidgetOrderTicket"), parts.join(" "));
            if !ticket.price.is_empty() {
                let _ = write!(summary, " @ {}", ticket.price);
            }
            summary
        }
        WidgetRef::OrderDetail { order_id } => {
            format!("{}  {order_id}", t!("Ai.WidgetOrderDetail"))
        }
        WidgetRef::Other { path } => path.clone(),
    }
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
                    // Naming the reference beats echoing its URL. Only something
                    // that is not a widget URL at all falls through to the raw
                    // text, and that is sanitized first — `src` is answer markup.
                    None => match parse_widget(&src) {
                        Some(widget) => {
                            let _ = writeln!(out, "→ {}", widget_summary(&widget));
                        }
                        None => {
                            let _ = writeln!(out, "→ {}", strip_control_chars(&src));
                        }
                    },
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

    /// A card with every field filled, so a test can override just the one it
    /// cares about rather than restating the whole quote.
    fn card(
        symbol: &str,
        name: &str,
        last: &str,
        change_pct: &str,
        direction: i8,
    ) -> QuoteCardData {
        QuoteCardData {
            prev_close: rust_decimal_macros::dec!(180.0),
            symbol: symbol.into(),
            name: name.into(),
            last: last.into(),
            change: "+1.00".into(),
            change_pct: change_pct.into(),
            direction,
            open: "179.2".into(),
            high: "183.5".into(),
            low: "178.9".into(),
            volume: "4212万".into(),
            turnover: "58.3亿".into(),
            at: "15:09".into(),
        }
    }
    use unicode_width::UnicodeWidthStr;

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
        assert!(out.contains('█'));
        assert!(out.contains("42.53"));
        // longest value owns the longest bar
        let bar_len = |needle: &str| {
            out.lines()
                .find(|l| l.contains(needle))
                .map_or(0, |l| l.chars().filter(|&c| c == '█').count())
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
    fn unknown_chart_type_falls_back_to_readable_text() {
        // A kind with no faithful ASCII form still lists its data rather than
        // dropping it — as text, not bars.
        let spec = serde_json::json!({
            "type": "sunburst",
            "data": [{"category": "growth", "value": 8.0}]
        });
        let out = render_vis_chart(&spec, 60, false);
        assert!(out.contains("growth") && out.contains('8'));
        assert!(!out.contains('█')); // listed as text, not drawn as bars
    }

    #[test]
    fn quote_card_renders_box() {
        let card = card("TSLA.US", "Tesla", "328.58", "+2.83%", 1);
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
            card("TSLA.US", "Tesla", "328.58", "+2.83%", 1),
        );
        let out = render_answer(md, &quotes, 60, false);
        assert!(out.contains("Tesla")); // marker replaced + card
        assert!(out.contains("100.0%")); // pie rendered
        assert!(out.contains("328.58")); // quote card
                                         // An unknown widget names its path; the URL itself is never echoed.
        assert!(out.contains("unknown/thing"));
        assert!(!out.contains("widget://"));
        assert!(!out.contains("[stock")); // no raw markers survive
        assert!(!out.contains("vis-chart")); // no raw fences survive
    }

    // -- fix round 1 regression tests --

    #[test]
    fn quote_card_lines_have_equal_char_count() {
        let card = card("TSLA.US", "Tesla", "328.58", "+2.83%", 1);
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
        // The column series still draws (as braille bars on the shared canvas).
        assert!(out.chars().any(|c| ('\u{2800}'..='\u{28FF}').contains(&c)));
        // A degenerate line series must not poison the scale: no NaN/inf labels,
        // and nothing wider than the width we asked for.
        assert!(
            !out.contains("NaN") && !out.contains("inf"),
            "degenerate scale leaked into labels:\n{out}"
        );
        for line in out.lines() {
            assert!(
                UnicodeWidthStr::width(line) <= 60,
                "line exceeds width: {line}"
            );
        }
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
            "type": "bar",
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
                l.find('█')
                    .map(|byte_idx| UnicodeWidthStr::width(&l[..byte_idx]))
            })
        };
        let cjk_col = bar_col("腾讯控股").expect("cjk line present");
        let ascii_col = bar_col("AAPL").expect("ascii line present");
        assert_eq!(cjk_col, ascii_col);
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

    /// An unrecognized widget is named by its path, and that path is answer
    /// markup: it must not carry an escape sequence to the terminal.
    #[test]
    fn unknown_widget_hint_is_sanitized() {
        let md = "<x-widget src=\"widget://evil\x1b]0;pwn\x07/thing\"></x-widget>";
        let out = render_answer(md, &std::collections::HashMap::new(), 60, false);
        assert!(
            out.contains("evil"),
            "the reference is still named: {out:?}"
        );
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
        let card = card("700\x1b[31m.HK", "Ten\x07cent", "320.00", "-1.20%", -1);
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
        let card = card("700.HK", "腾讯控股", "320.00", "-1.20%", -1);
        let plain = render_quote_card(&card, false);
        let widths: Vec<usize> = plain.lines().map(UnicodeWidthStr::width).collect();
        assert!(!widths.is_empty());
        assert!(
            widths.iter().all(|&w| w == widths[0]),
            "CJK card lines have unequal display width: {widths:?}\n{plain}"
        );
    }
}
