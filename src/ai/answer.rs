//! The shape of a Longbridge AI answer.
//!
//! An answer is Markdown with three things of the agent's own mixed in:
//! ```` ```vis-chart ```` specs, `widget://…` references, and `[stock …]` /
//! `[citation N]` inline markers. This module is the one place that knows how to
//! take an answer apart; both the `ai` TUI and `agent chat`'s stdout renderer
//! consume the result rather than each re-scanning the text.
//!
//! Widget kinds are taken from the web client's own registry
//! (`portai/frontend/web/src/features/x-widget/index.tsx` `COMPONENTS_MAP`),
//! which is the authority on what the agent may emit.

use serde_json::Value;

use crate::utils::text::strip_control_chars;

/// URL scheme the agent uses to reference an embeddable widget.
pub const WIDGET_SCHEME: &str = "widget://";

/// A piece of an answer, in source order.
#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    Text(String),
    VisChart(Value),
    /// A widget reference, still in its URL form.
    XWidget(String),
}

/// What the answer scanner found next.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Marker {
    /// A ```` ```vis-chart ```` fence.
    Chart,
    /// An `<x-widget src="…">` tag.
    Tag,
    /// A `widget://…` URL sitting in the prose on its own.
    BareUrl,
}

/// A widget reference, resolved to what it actually shows.
///
/// A terminal cannot embed the web widget, so the point of naming the kinds is
/// to say something true about each one instead of printing its URL.
#[derive(Debug, Clone, PartialEq)]
pub enum WidgetRef {
    /// One security's quote and chart.
    Quote { symbol: String },
    /// Several securities compared on one chart.
    Comparison { symbols: Vec<String> },
    /// A list of securities.
    StockList { symbols: Vec<String> },
    /// A prompt to open an account, fund it, or complete a profile — actionable
    /// in the app, only nameable here.
    Cta { action: String },
    /// A kind we have no special rendering for; carries its path so the
    /// reference is still named rather than shown as a URL.
    Other { path: String },
}

impl WidgetRef {
    /// The securities this widget is about, for fetching live quotes.
    pub fn symbols(&self) -> &[String] {
        match self {
            WidgetRef::Comparison { symbols } | WidgetRef::StockList { symbols } => symbols,
            WidgetRef::Quote { symbol } => std::slice::from_ref(symbol),
            WidgetRef::Cta { .. } | WidgetRef::Other { .. } => &[],
        }
    }
}

/// Parse a `widget://…` URL into what it references.
///
/// Returns `None` for anything that is not a widget URL at all. Unknown paths
/// become [`WidgetRef::Other`] rather than `None`, so a widget kind added
/// server-side still renders as a named reference instead of leaking its URL.
pub fn parse_widget(src: &str) -> Option<WidgetRef> {
    let rest = src.strip_prefix(WIDGET_SCHEME)?;
    // The `#widget` fragment some URLs carry comes after the query, so it has to
    // go before the split — otherwise it rides along inside the last parameter's
    // value and turns `AAPL.US` into `AAPL.US#widget`.
    let rest = rest.split('#').next().unwrap_or(rest);
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let path = path.trim_end_matches('/');
    let symbols = query_values(query, "symbols");
    let symbol = query_values(query, "symbol").into_iter().next();
    Some(match path {
        "quote/security/detail" | "stock/quote" | "stock/overview" | "stock/financials" => {
            match symbol {
                Some(symbol) => WidgetRef::Quote { symbol },
                None => WidgetRef::Other {
                    path: path.to_string(),
                },
            }
        }
        "quote/security/comparison" => WidgetRef::Comparison { symbols },
        "stock/list" => WidgetRef::StockList { symbols },
        _ => match path.strip_prefix("cta/") {
            Some(action) => WidgetRef::Cta {
                action: action.to_string(),
            },
            None => WidgetRef::Other {
                path: path.to_string(),
            },
        },
    })
}

/// Every value of `key` in a query string, in order.
///
/// A repeated key is how the agent passes a list: `symbols=A&symbols=B`. Taking
/// only the first would silently reduce a comparison to one security.
fn query_values(query: &str, key: &str) -> Vec<String> {
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .filter(|(k, _)| *k == key)
        .map(|(_, v)| strip_control_chars(v))
        .filter(|v| !v.is_empty())
        .collect()
}

/// The symbol of a single-quote widget, or `None` for any other kind.
pub fn parse_quote_widget_symbol(src: &str) -> Option<String> {
    match parse_widget(src)? {
        WidgetRef::Quote { symbol } => Some(symbol),
        _ => None,
    }
}

/// Length of the bare `widget://…` URL at the start of `s`.
///
/// A tagged widget is delimited by its quotes; a bare one has to be delimited by
/// hand. It runs to the first whitespace or bracket, less any sentence
/// punctuation that happened to follow it.
pub fn bare_widget_url_end(s: &str) -> usize {
    let end = s
        .find(|c: char| {
            c.is_whitespace() || matches!(c, ')' | ']' | '}' | '>' | '<' | '"' | '\'' | '，' | '。')
        })
        .unwrap_or(s.len());
    s[..end].trim_end_matches(['.', ',', ';', ':']).len()
}

/// Split an answer into text / chart / widget segments, preserving order.
/// Malformed vis-chart JSON keeps the fenced block as text so no content
/// is silently dropped.
pub fn segment_answer(answer: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut text_acc = String::new();
    let mut rest = answer;

    loop {
        // Whichever marker comes first wins. A tagged widget always starts
        // before the `widget://` inside it, so the bare-URL scan cannot steal a
        // tag out from under the tag branch.
        let found = [
            (rest.find("```vis-chart"), Marker::Chart),
            (rest.find("<x-widget"), Marker::Tag),
            (rest.find(WIDGET_SCHEME), Marker::BareUrl),
        ];
        let Some((pos, marker)) = found
            .into_iter()
            .filter_map(|(at, m)| at.map(|at| (at, m)))
            .min_by_key(|(at, _)| *at)
        else {
            break;
        };
        text_acc.push_str(&rest[..pos]);
        rest = &rest[pos..];

        if marker == Marker::BareUrl {
            // Some answers print the reference URL on its own line instead of
            // wrapping it in an `<x-widget>` tag. It means the same thing, so it
            // gets the same treatment rather than being left as a raw URL in the
            // prose.
            let end = bare_widget_url_end(rest);
            flush_text(&mut segments, &mut text_acc);
            segments.push(Segment::XWidget(rest[..end].to_string()));
            rest = &rest[end..];
        } else if marker == Marker::Chart {
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

/// Replace `[stock Name]` and `[citation N]` inline markers.
pub fn replace_inline_markers(text: &str, color: bool) -> String {
    use std::fmt::Write;

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

/// A security's live quote, for the chip an answer's quote widget becomes.
pub struct QuoteCardData {
    pub symbol: String,
    pub name: String,
    pub last: String,
    pub change_pct: String,
    /// 1 = up, -1 = down, 0 = flat.
    pub direction: i8,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kinds come from the web client's `COMPONENTS_MAP`; each has to resolve
    /// to something nameable rather than falling through to its URL.
    #[test]
    fn every_known_widget_kind_parses() {
        assert_eq!(
            parse_widget("widget://quote/security/detail?symbol=TSLA.US&time_range=1"),
            Some(WidgetRef::Quote {
                symbol: "TSLA.US".into()
            })
        );
        assert_eq!(
            parse_widget(
                "widget://quote/security/comparison?symbols=TSLA.US&symbols=NVDA.US&time_range=3"
            ),
            Some(WidgetRef::Comparison {
                symbols: vec!["TSLA.US".into(), "NVDA.US".into()]
            })
        );
        assert_eq!(
            parse_widget("widget://stock/list?symbols=MSFT.US&symbols=NVDA.US"),
            Some(WidgetRef::StockList {
                symbols: vec!["MSFT.US".into(), "NVDA.US".into()]
            })
        );
        assert_eq!(
            parse_widget("widget://cta/open_account"),
            Some(WidgetRef::Cta {
                action: "open_account".into()
            })
        );
        // A kind with no special rendering still names itself.
        assert_eq!(
            parse_widget("widget://quant/backtest_result?backtest_uuid=abc123"),
            Some(WidgetRef::Other {
                path: "quant/backtest_result".into()
            })
        );
        assert_eq!(parse_widget("https://example.com"), None);
    }

    /// A repeated key is how a list is passed; taking only the first value would
    /// quietly reduce a two-stock comparison to one.
    #[test]
    fn repeated_query_keys_are_all_collected() {
        let widget = parse_widget("widget://stock/list?symbols=A&symbols=B&symbols=C").unwrap();
        assert_eq!(widget.symbols(), ["A".to_string(), "B".into(), "C".into()]);
    }

    /// The `#widget` fragment some URLs carry is not part of the path.
    #[test]
    fn a_fragment_does_not_change_the_kind() {
        assert_eq!(
            parse_widget("widget://stock/overview?symbol=AAPL.US#widget"),
            Some(WidgetRef::Quote {
                symbol: "AAPL.US".into()
            })
        );
    }

    /// `symbols=` must not be read as `symbol=`, and vice versa.
    #[test]
    fn singular_and_plural_symbol_keys_do_not_collide() {
        assert_eq!(
            parse_widget("widget://quote/security/comparison?symbols=A&symbols=B")
                .unwrap()
                .symbols(),
            ["A".to_string(), "B".into()]
        );
        assert_eq!(
            parse_widget("widget://quote/security/detail?symbol=A")
                .unwrap()
                .symbols(),
            ["A".to_string()]
        );
    }

    #[test]
    fn only_a_single_quote_widget_yields_a_symbol() {
        assert_eq!(
            parse_quote_widget_symbol("widget://quote/security/detail?symbol=700.HK").as_deref(),
            Some("700.HK")
        );
        assert!(
            parse_quote_widget_symbol("widget://quote/security/comparison?symbols=A&symbols=B")
                .is_none(),
            "a comparison is not a single quote"
        );
    }

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

    /// Answers sometimes print the reference URL on its own instead of wrapping
    /// it in an `<x-widget>` tag. Left unrecognized it showed up as a raw URL in
    /// the middle of the prose.
    #[test]
    fn bare_widget_url_becomes_a_widget_segment() {
        let md = "## TSLA\n\nwidget://quote/security/detail?symbol=TSLA.US&time_range=1\n\ntail";
        let segs = segment_answer(md);
        let widget = segs
            .iter()
            .find_map(|s| match s {
                Segment::XWidget(src) => Some(src.clone()),
                _ => None,
            })
            .expect("the bare URL should become a widget segment");
        assert_eq!(
            parse_quote_widget_symbol(&widget).as_deref(),
            Some("TSLA.US")
        );
        // The URL must not also be left behind in the prose.
        for seg in &segs {
            if let Segment::Text(t) = seg {
                assert!(!t.contains("widget://"), "URL leaked into text: {t:?}");
            }
        }
    }

    /// The `widget://` inside a tag must not be picked up a second time.
    #[test]
    fn tagged_widget_is_not_double_counted() {
        let md = "<x-widget src=\"widget://quote/security/detail?symbol=700.HK\"></x-widget>";
        let widgets = segment_answer(md)
            .into_iter()
            .filter(|s| matches!(s, Segment::XWidget(_)))
            .count();
        assert_eq!(widgets, 1, "expected one widget segment, got {widgets}");
        let found = crate::cli::agent::events::extract_widgets(md);
        assert_eq!(found.len(), 1, "extract_widgets double-counted: {found:?}");
    }

    /// A URL that ends a sentence keeps the sentence punctuation out of the URL.
    #[test]
    fn bare_url_stops_at_punctuation_and_brackets() {
        for (input, want) in [
            ("widget://a?b=1 rest", "widget://a?b=1"),
            ("widget://a?b=1.", "widget://a?b=1"),
            ("widget://a?b=1)", "widget://a?b=1"),
            ("widget://a?b=1\nnext", "widget://a?b=1"),
        ] {
            let end = bare_widget_url_end(input);
            assert_eq!(&input[..end], want, "for {input:?}");
        }
    }
}
