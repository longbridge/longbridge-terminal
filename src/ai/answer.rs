//! The shape of a Longbridge AI answer.
//!
//! An answer is Markdown with three things of the agent's own mixed in:
//! ```` ```vis-chart ```` specs, `widget://…` references, and `[stock …]` /
//! `[citation N]` inline markers. This module is the one place that knows how to
//! take an answer apart; both the `ai` TUI and `agent chat`'s stdout renderer
//! consume the result rather than each re-scanning the text.
//!
//! The authority on what the agent may emit is the server's allowlist
//! (`portai/backend/config/default/mcp_resources.yaml`), which is what fills the
//! `{widget_templates}` slot in its prompt — not the web client's `COMPONENTS_MAP`
//! registry, which both lacks kinds the model is offered (`trade/order/*`) and
//! keeps kinds no server serves. Where the two disagree, follow the allowlist:
//! anything the model can emit has to render as something.

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
    /// A ```` ``` ```` code fence. A `vis-chart` one becomes a chart; any other
    /// is opaque text — its content is not scanned for widgets, so a `widget://`
    /// or `<x-widget` written *inside* a code example is shown, not extracted.
    Fence,
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
    /// A pre-filled order ticket. Actionable in the app; here it is read out so
    /// the reader can see what the agent proposed.
    OrderTicket(OrderTicket),
    /// A reference to one of the reader's own orders.
    OrderDetail { order_id: String },
    /// A kind we have no special rendering for; carries its path so the
    /// reference is still named rather than shown as a URL.
    Other { path: String },
}

/// The fields of an order ticket worth reading back.
///
/// A ticket carries fifteen parameters; these are the ones that say what the
/// order *is*. The rest (trigger price, trailing offsets, expiry, remark) qualify
/// it and are left to the app, which can actually place it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OrderTicket {
    /// For a multi-leg order this is the underlying, not a leg of its own.
    pub symbol: String,
    /// `Buy` / `Sell`, or empty when the value is not one we recognize — a
    /// direction is the one field it would be dangerous to guess at.
    pub side: String,
    /// `LO`, `MO`, … as the agent wrote it.
    pub order_type: String,
    pub quantity: String,
    pub price: String,
}

impl WidgetRef {
    /// The securities this widget is about, for fetching live quotes.
    pub fn symbols(&self) -> &[String] {
        match self {
            WidgetRef::Comparison { symbols } | WidgetRef::StockList { symbols } => symbols,
            WidgetRef::Quote { symbol } => std::slice::from_ref(symbol),
            WidgetRef::OrderTicket(ticket) => std::slice::from_ref(&ticket.symbol),
            WidgetRef::Cta { .. } | WidgetRef::OrderDetail { .. } | WidgetRef::Other { .. } => &[],
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
    // The path is answer markup and an unrecognized one is echoed back to name
    // the reference, so it is sanitized here — the same as the query values —
    // rather than at each of the two renderers.
    let path = strip_control_chars(path.trim_end_matches('/'));
    let path = path.as_str();
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
        "trade/order/submit" => WidgetRef::OrderTicket(OrderTicket {
            symbol: symbol.unwrap_or_default(),
            side: order_side(&query_values(query, "side")),
            order_type: first(query, "order_type"),
            quantity: first(query, "submitted_quantity"),
            price: first(query, "submitted_price"),
        }),
        "trade/order/detail" => WidgetRef::OrderDetail {
            order_id: first(query, "order_id"),
        },
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

/// The order's direction, or empty when the value is not one we recognize.
///
/// The parameter is numeric where the trading API is textual. `1` is `Buy` in
/// every captured ticket, and the SDK's `OrderSide` is declared `Unknown, Buy,
/// Sell`, which fixes `2` as `Sell`. Anything else is left blank: showing no
/// direction costs the reader a detail, showing the wrong one could cost them
/// money.
fn order_side(values: &[String]) -> String {
    match values.first().map(String::as_str) {
        Some("1") => rust_i18n::t!("Trade.Buy").to_string(),
        Some("2") => rust_i18n::t!("Trade.Sell").to_string(),
        _ => String::new(),
    }
}

/// The first value of `key`, or empty.
fn first(query: &str, key: &str) -> String {
    query_values(query, key)
        .into_iter()
        .next()
        .unwrap_or_default()
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

/// Market suffixes a symbol may carry.
///
/// Deliberately the closed set the platform trades rather than "two or three
/// capitals": the scanner runs over prose, and every false positive turns an
/// ordinary word into something that looks clickable and answers nothing.
const MARKETS: [&str; 5] = ["HK", "US", "SG", "SH", "SZ"];

/// Byte ranges of the securities named in `text`, in order.
///
/// A symbol is `CODE.MARKET` — `700.HK`, `AAPL.US`, and the leading-dot index
/// form `.DJI.US` — bounded by non-alphanumerics so `AAPL.USA` and `x700.HK` are
/// not matches.
pub fn symbol_spans(text: &str) -> Vec<std::ops::Range<usize>> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(dot) = text[at..].find('.') {
        let dot = at + dot;
        at = dot + 1;
        let Some(market) = MARKETS
            .iter()
            .find(|m| text[dot + 1..].starts_with(**m))
            .copied()
        else {
            continue;
        };
        let end = dot + 1 + market.len();
        // The suffix has to end the word, or `AAPL.USA` reads as a US ticker.
        if bytes
            .get(end)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'.')
        {
            continue;
        }
        // Walk back over the code, allowing one leading dot for an index.
        let mut start = dot;
        while start > 0 && bytes[start - 1].is_ascii_alphanumeric() {
            start -= 1;
        }
        if start == dot {
            continue; // a bare `.HK`
        }
        if start > 0 && bytes[start - 1] == b'.' {
            start -= 1;
        }
        // And the whole thing has to start a word.
        if start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'.') {
            continue;
        }
        // A code is short and upper-case: prose like `see.US` should not match.
        let code = &text[start..dot];
        let plausible = code.len() <= 8
            && !code.is_empty()
            && code
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase() || c == '.');
        if plausible {
            out.push(start..end);
            at = end;
        }
    }
    out
}

/// Words that look like tickers but are not, in a chat about markets.
///
/// Every one of these is either finance jargon or a unit, and several are also
/// real tickers — `AI`, `ET`, `AM`, `PT` among them. In this context the jargon
/// reading is the overwhelmingly likely one, and a link that opens the wrong
/// security's quote is worse than no link at all, so they are never candidates.
const NOT_TICKERS: [&str; 62] = [
    "AI", "AM", "PM", "ET", "PT", "UTC", "API", "APP", "CEO", "CFO", "COO", "CTO", "IPO", "ETF",
    "SEC", "FED", "FOMC", "GDP", "CPI", "PPI", "PMI", "YOY", "QOQ", "MOM", "TTM", "YTD", "MTD",
    "QTD", "EPS", "PE", "PB", "PS", "PEG", "ROE", "ROA", "ROI", "EBIT", "DCF", "WACC", "NAV",
    "AUM", "SPAC", "ITM", "OTM", "ATM", "IV", "HV", "OI", "MACD", "RSI", "KDJ", "BOLL", "CCI",
    "BIAS", "DIF", "DEA", "EMA", "SMA", "VWAP", "GTC", "LO", "MO",
];

/// Bare letter-bearing tokens in `text` that could be a ticker: `SPCX`, `TSLA`.
///
/// Candidates only. An answer about options is full of words shaped like tickers
/// — `ITM`, `MACD`, `BOLL` — so nothing here is linked until something confirms
/// it is a security: an explicit `[stock …]` marker, a widget, or the server
/// recognising it (see [`crate::ai::quotes::resolve_symbols`]).
///
/// A purely numeric token (a bare HK code like `9988`, but also every price and
/// year in the prose) is not a candidate: on its own a number is too ambiguous
/// to probe. Such a code is still linked when it carries its market (`9988.HK`,
/// via [`symbol_spans`]) or is named by a widget.
pub fn ticker_candidates(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for range in token_spans(text) {
        let token = &text[range];
        // At least one letter: a bare year or price is not a security.
        if token.chars().any(char::is_alphabetic)
            && !NOT_TICKERS.contains(&token)
            && !out.iter().any(|t| t == token)
        {
            out.push(token.to_string());
        }
    }
    out
}

/// Whether `text` is exactly one security symbol.
pub fn is_symbol(text: &str) -> bool {
    symbol_spans(text)
        .first()
        .is_some_and(|r| *r == (0..text.len()))
}

/// Every security named in `text`, as `(range, symbol)` in source order.
///
/// A dotted symbol names itself. A bare ticker names whatever `aliases` resolved
/// it to — and only a resolved one counts, which is what keeps `ITM` and `MACD`
/// out of the transcript's links.
pub fn security_spans(
    text: &str,
    aliases: &std::collections::HashMap<String, String>,
) -> Vec<(std::ops::Range<usize>, String)> {
    let mut out: Vec<(std::ops::Range<usize>, String)> = symbol_spans(text)
        .into_iter()
        .map(|r| {
            let symbol = text[r.clone()].to_string();
            (r, symbol)
        })
        .collect();
    if !aliases.is_empty() {
        for range in token_spans(text) {
            if let Some(symbol) = aliases.get(&text[range.clone()]) {
                out.push((range, symbol.clone()));
            }
        }
        out.sort_by_key(|(r, _)| r.start);
    }
    out
}

/// Ranges of every bare ticker-shaped token in `text`, in order, repeats included.
///
/// Separate from [`ticker_candidates`], which dedupes and drops jargon: this is
/// for finding the occurrences of a token already known to be a security.
fn token_spans(text: &str) -> Vec<std::ops::Range<usize>> {
    let taken = symbol_spans(text);
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < text.len() {
        if !text.is_char_boundary(i) || !bytes[i].is_ascii_uppercase() {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < text.len() && (bytes[j].is_ascii_uppercase() || bytes[j].is_ascii_digit()) {
            j += 1;
        }
        let bounded = |at: usize| -> bool {
            at == 0 || at >= text.len() || !bytes[at].is_ascii_alphanumeric()
        };
        let inside = taken.iter().any(|r| r.start < j && i < r.end);
        if (2..=6).contains(&(j - i)) && bounded(i.saturating_sub(1)) && bounded(j) && !inside {
            out.push(i..j);
        }
        i = j.max(i + 1);
    }
    out
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
            c.is_whitespace()
                || matches!(
                    c,
                    ')' | ']'
                        | '}'
                        | '>'
                        | '<'
                        | '"'
                        | '\''
                        // Full-width CJK punctuation that ends a clause. A bare URL
                        // in Chinese prose is far more often closed by one of these
                        // than by an ASCII mark, and pulling it into the URL
                        // corrupts the trailing parameter (e.g. the symbol).
                        | '，' | '。' | '、' | '；' | '：' | '？' | '！'
                        | '）' | '（' | '】' | '【' | '》' | '《' | '」' | '「'
                )
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
            (rest.find("```"), Marker::Fence),
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
        } else if marker == Marker::Fence {
            if rest.starts_with("```vis-chart") {
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
                // A plain code block is opaque: emit it whole (fences included) as
                // text so the markdown renderer draws it as code, and — crucially —
                // so a `widget://`/`<x-widget` written inside a code example is not
                // torn out of it. The closing fence is the next ```.
                let after = &rest[3..];
                match after.find("```") {
                    Some(end) => {
                        text_acc.push_str(&rest[..3 + end + 3]);
                        rest = &after[end + 3..];
                    }
                    None => break, // unterminated fence: emit the rest as text
                }
            }
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
///
/// The grammar follows the server's, which is looser than the web client's
/// regex: the keyword is case-insensitive and the separator may be a space, a
/// colon, or a full-width colon. Markers the server emits but the web client
/// fails to linkify still read correctly here.
///
/// A footnote-style `[^1]` is the same thing as `[citation 1]`, so it gets the
/// same treatment. A bare `[1]` is already in the form a citation renders as, so
/// it is left alone — rewriting it would also catch any bracketed number in the
/// prose.
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
        let mut consumed = end + 1;
        if let Some(name) = marker_body(inner, "stock") {
            // `[stock 特斯拉](TSLA.US)` is common enough that the web client
            // handles it specially — and throws the symbol away. A terminal
            // reader wants the ticker, so it is kept, set apart from the name.
            let symbol = link_target(&after[end + 1..]);
            if let Some(symbol) = &symbol {
                consumed += symbol.len() + 2;
            }
            if color {
                let _ = write!(out, "\x1b[36m{name}\x1b[0m");
            } else {
                out.push_str(name);
            }
            if let Some(symbol) = symbol {
                if color {
                    let _ = write!(out, " \x1b[2m({symbol})\x1b[0m");
                } else {
                    let _ = write!(out, " ({symbol})");
                }
            }
        } else if let Some(n) = marker_body(inner, "citation")
            .or_else(|| inner.strip_prefix('^'))
            // A bare `[^]` (or `[citation ]`) has no number; leaving it to fall
            // through keeps it as literal text rather than rendering an empty `[]`.
            .filter(|n| !n.is_empty())
        {
            if color {
                let _ = write!(out, "\x1b[2m[{n}]\x1b[0m");
            } else {
                let _ = write!(out, "[{n}]");
            }
        } else {
            out.push_str(&after[..=end]);
        }
        rest = &after[consumed..];
    }
    out.push_str(rest);
    out
}

/// The body of a `[keyword<sep>body]` marker, or `None` if `inner` is not one.
///
/// Separator and case follow the server's parser: `stock`, `STOCK`, `stock:` and
/// `stock：` all count. The length cap is the server's too, and keeps a long
/// bracketed passage that merely starts with the word from being swallowed.
fn marker_body<'a>(inner: &'a str, keyword: &str) -> Option<&'a str> {
    let (head, body) = inner.split_at_checked(keyword.len())?;
    if !head.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let body = body
        .strip_prefix([' ', ':', '：'])
        .map(str::trim)
        .filter(|b| !b.is_empty() && b.chars().count() <= 100)?;
    Some(body)
}

/// The target of a Markdown link that immediately follows a marker: the `X` of
/// `](X)`, if `s` starts with one and it looks like a symbol rather than prose.
fn link_target(s: &str) -> Option<&str> {
    let inner = s.strip_prefix('(')?.split_once(')')?.0;
    let plausible = !inner.is_empty()
        && inner.len() <= 24
        && inner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    plausible.then_some(inner)
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

    /// An order ticket is allowlisted to the model, so it can appear in an
    /// answer. Falling through to "named path" would throw away the one thing
    /// the reader needs: what the order actually is.
    #[test]
    fn an_order_ticket_is_read_back() {
        assert_eq!(
            parse_widget(
                "widget://trade/order/submit?symbol=AAPL.US&order_type=LO&side=1\
                 &submitted_price=83.50&submitted_quantity=100&time_in_force=0&outside_rth=0"
            ),
            Some(WidgetRef::OrderTicket(OrderTicket {
                symbol: "AAPL.US".into(),
                side: "Buy".into(),
                order_type: "LO".into(),
                quantity: "100".into(),
                price: "83.50".into(),
            }))
        );
        // The ticket's security is worth a live quote.
        assert_eq!(
            parse_widget("widget://trade/order/submit?symbol=700.HK&side=2")
                .unwrap()
                .symbols(),
            ["700.HK".to_string()]
        );
        assert_eq!(
            parse_widget("widget://trade/order/detail?order_id=901234567"),
            Some(WidgetRef::OrderDetail {
                order_id: "901234567".into()
            })
        );
    }

    /// A direction is the one field that must never be guessed: an unrecognized
    /// value leaves it blank rather than defaulting to Buy.
    #[test]
    fn an_unknown_order_side_is_left_blank() {
        for (side, want) in [("1", "Buy"), ("2", "Sell"), ("7", ""), ("", "")] {
            let src = format!("widget://trade/order/submit?symbol=A&side={side}");
            let Some(WidgetRef::OrderTicket(ticket)) = parse_widget(&src) else {
                panic!("not a ticket: {src}");
            };
            assert_eq!(ticket.side, want, "for side={side:?}");
        }
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

    /// A widget reference written inside a plain code block is an example, not a
    /// live widget: the block stays whole text so the renderer draws it as code,
    /// and the URL is not extracted into a card.
    #[test]
    fn a_widget_inside_a_code_block_is_not_extracted() {
        let md = "here is the syntax:\n```text\n<x-widget src=\"widget://quote/security/detail?symbol=AAPL.US\"></x-widget>\n```\ndone";
        let segs = segment_answer(md);
        assert!(
            segs.iter().all(|s| matches!(s, Segment::Text(_))),
            "no widget/chart should be split out: {segs:?}"
        );
        let joined: String = segs
            .iter()
            .map(|s| match s {
                Segment::Text(t) => t.as_str(),
                _ => "",
            })
            .collect();
        assert!(joined.contains("widget://quote/security/detail?symbol=AAPL.US"));
        assert!(joined.contains("done"));
    }

    /// A bare code fence before a real chart must not let the tag scanner reach
    /// into the code block: the block is text, the chart still draws.
    #[test]
    fn a_code_block_does_not_shadow_a_later_chart() {
        let md = "```json\n{\"note\": \"widget://quote/security/detail?symbol=X.US\"}\n```\n```vis-chart\n{\"type\":\"pie\"}\n```";
        let segs = segment_answer(md);
        assert!(matches!(&segs[0], Segment::Text(t) if t.contains("widget://")));
        assert!(segs.iter().any(|s| matches!(s, Segment::VisChart(_))));
        assert!(!segs.iter().any(|s| matches!(s, Segment::XWidget(_))));
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

    /// The server's marker grammar is looser than the web client's regex: the
    /// keyword is case-insensitive and the separator may be a colon, full-width
    /// or not. Markers the web app fails to linkify still read correctly here.
    #[test]
    fn marker_grammar_follows_the_server_not_the_web_regex() {
        for input in [
            "[stock:腾讯]",
            "[STOCK 腾讯]",
            "[stock：腾讯]",
            "[Stock 腾讯]",
        ] {
            assert_eq!(
                replace_inline_markers(input, false),
                "腾讯",
                "for {input:?}"
            );
        }
        assert_eq!(replace_inline_markers("[citation:3]", false), "[3]");
        // A footnote is the same thing as a citation.
        assert_eq!(
            replace_inline_markers("as reported[^12].", false),
            "as reported[12]."
        );
        // A bare bracketed number already reads as a citation, so it is left be.
        assert_eq!(
            replace_inline_markers("see [1] and [2]", false),
            "see [1] and [2]"
        );
        // Bracketed prose that merely starts with the word is not a marker.
        assert_eq!(
            replace_inline_markers("[stockholders meeting]", false),
            "[stockholders meeting]"
        );
        assert_eq!(replace_inline_markers("[stock]", false), "[stock]");
        // A footnote marker with no number is not a citation; it stays literal
        // rather than collapsing to an empty `[]`.
        assert_eq!(
            replace_inline_markers("see [^] here", false),
            "see [^] here"
        );
    }

    /// The link form is common, and the web client discards the target. In a
    /// terminal the ticker is worth keeping.
    #[test]
    fn the_link_form_keeps_the_symbol() {
        assert_eq!(
            replace_inline_markers("看 [stock 特斯拉](TSLA.US) 的走势", false),
            "看 特斯拉 (TSLA.US) 的走势"
        );
        // A following parenthesis that is ordinary prose stays where it is.
        assert_eq!(
            replace_inline_markers("[stock 特斯拉](见下文) 的走势", false),
            "特斯拉(见下文) 的走势"
        );
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
            // Chinese prose closes a clause with full-width punctuation; it must
            // not be pulled into the URL and corrupt the last parameter.
            (
                "widget://quote/security/detail?symbol=700.HK）",
                "widget://quote/security/detail?symbol=700.HK",
            ),
            (
                "widget://quote/security/detail?symbol=700.HK；后面",
                "widget://quote/security/detail?symbol=700.HK",
            ),
            (
                "widget://quote/security/detail?symbol=700.HK。",
                "widget://quote/security/detail?symbol=700.HK",
            ),
        ] {
            let end = bare_widget_url_end(input);
            assert_eq!(&input[..end], want, "for {input:?}");
        }
    }

    /// The scanner runs over prose, so a false positive turns an ordinary word
    /// into something that looks clickable and answers nothing.
    #[test]
    fn symbols_are_found_in_prose_and_nowhere_else() {
        let text = "看 700.HK 和 AAPL.US，还有 .DJI.US；但 AAPL.USA、x700.HK、see.us 不算。";
        let found: Vec<&str> = symbol_spans(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(found, ["700.HK", "AAPL.US", ".DJI.US"], "in {text:?}");
    }

    #[test]
    fn a_symbol_on_its_own_is_recognised() {
        for yes in ["700.HK", "AAPL.US", "9988.HK", "600519.SH", ".DJI.US"] {
            assert!(is_symbol(yes), "{yes} should be a symbol");
        }
        for no in ["AAPL", "AAPL.USA", "HK", "", "hello.us", "AAPL.US extra"] {
            assert!(!is_symbol(no), "{no:?} should not be a symbol");
        }
    }

    /// The scanner is what makes a symbol clickable, so it has to find the ones
    /// answers actually contain — including inside the link form's parentheses.
    #[test]
    fn symbols_are_found_where_answers_put_them() {
        let text = "特斯拉 (TSLA.US) 与 腾讯 (700.HK) 对比";
        let found: Vec<&str> = symbol_spans(text).into_iter().map(|r| &text[r]).collect();
        assert_eq!(found, ["TSLA.US", "700.HK"]);
    }

    /// The transcript this came from mentions `SPCX` eighteen times and never
    /// once with a market suffix — while also using `ITM`, `MACD`, `BOLL` and
    /// `AI`, every one of which is shaped like a ticker.
    #[test]
    fn candidates_are_tickers_and_not_jargon() {
        let text = "SPCX 卖 Put 已 ITM，MACD 与 BOLL 显示 TSLA 走弱，UTC 15:09，IV 偏高，AI 概念股";
        assert_eq!(
            ticker_candidates(text),
            vec!["SPCX".to_string(), "TSLA".into()],
            "only the two that are securities"
        );
        // A dotted symbol is already known, so its code is not offered again.
        assert!(ticker_candidates("AAPL.US 上涨").is_empty());
        // Numbers alone are not securities.
        assert!(ticker_candidates("2026 年 135 美元").is_empty());
    }

    /// Nothing is linked until something confirms it is a security: an unresolved
    /// candidate stays plain text.
    #[test]
    fn only_resolved_tickers_become_securities() {
        let text = "SPCX 与 ITM";
        assert!(
            security_spans(text, &std::collections::HashMap::new()).is_empty(),
            "unresolved, so nothing is a security"
        );
        let mut aliases = std::collections::HashMap::new();
        aliases.insert("SPCX".to_string(), "SPCX.US".to_string());
        let found = security_spans(text, &aliases);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(&text[found[0].0.clone()], "SPCX");
        assert_eq!(found[0].1, "SPCX.US", "the click target is the full symbol");
    }

    /// Both forms in one line, in source order.
    #[test]
    fn dotted_and_bare_securities_are_ordered_together() {
        let mut aliases = std::collections::HashMap::new();
        aliases.insert("TSLA".to_string(), "TSLA.US".to_string());
        let text = "TSLA 对比 700.HK 再看 TSLA";
        let found: Vec<(&str, String)> = security_spans(text, &aliases)
            .into_iter()
            .map(|(r, symbol)| (&text[r], symbol))
            .collect();
        assert_eq!(
            found,
            vec![
                ("TSLA", "TSLA.US".to_string()),
                ("700.HK", "700.HK".into()),
                ("TSLA", "TSLA.US".into()),
            ]
        );
    }
}
