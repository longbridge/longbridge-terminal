//! Live quotes for the securities an answer references.
//!
//! When an answer embeds a `widget://` reference to one or more securities, the
//! web client renders a chart. A terminal cannot, but it can show what the
//! reference is actually about: the current quote. This module turns the symbols
//! out of [`super::answer::WidgetRef`] into [`QuoteCardData`], which both the
//! `ai` TUI and `agent chat` draw as a card.

use std::collections::{HashMap, HashSet};

use longbridge::quote::SecurityQuote;
use rust_decimal::Decimal;

use super::answer::parse_widget;
use crate::cli::agent::events::Widget;
use crate::utils::number::format_volume;

/// A security's quote, pre-formatted for display.
///
/// Formatted rather than numeric because the card is drawn in two places — the
/// TUI and `agent chat`'s stdout — and both want the same rounding, signs and
/// abbreviation, so the decision is made once, here.
pub struct QuoteCardData {
    pub symbol: String,
    pub name: String,
    /// Previous close, kept numeric because a streamed push carries only the
    /// latest price — the change has to be recomputed against this.
    pub prev_close: Decimal,
    pub last: String,
    /// Absolute change, signed.
    pub change: String,
    /// Percentage change, signed.
    pub change_pct: String,
    /// 1 = up, -1 = down, 0 = flat.
    pub direction: i8,
    pub open: String,
    pub high: String,
    pub low: String,
    /// Volume, abbreviated.
    pub volume: String,
    /// Turnover, abbreviated.
    pub turnover: String,
    /// Time of the latest price, as `HH:MM`.
    pub at: String,
}

impl QuoteCardData {
    /// Build a card from a quote, naming it if a name is known.
    fn from_quote(quote: &SecurityQuote, name: String) -> Self {
        let (prev, last) = (quote.prev_close, quote.last_done);
        let change = last - prev;
        let pct = if prev.is_zero() {
            Decimal::ZERO
        } else {
            change / prev * Decimal::ONE_HUNDRED
        };
        let direction = match last.cmp(&prev) {
            std::cmp::Ordering::Greater => 1,
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
        };
        Self {
            symbol: quote.symbol.clone(),
            name,
            prev_close: prev,
            last: price(last),
            change: signed(change),
            change_pct: format!("{}{:.2}%", sign_of(pct), pct),
            direction,
            open: price(quote.open),
            high: price(quote.high),
            low: price(quote.low),
            volume: format_volume(u64::try_from(quote.volume).unwrap_or(0)),
            turnover: format_volume(u64::try_from(quote.turnover.trunc()).unwrap_or(0)),
            at: format!(
                "{:02}:{:02}",
                quote.timestamp.hour(),
                quote.timestamp.minute()
            ),
        }
    }
}

impl QuoteCardData {
    /// Fold a streamed quote into the card.
    ///
    /// The push has no previous close, so the change is recomputed against the
    /// one the first fetch established — which is why it is kept numeric.
    pub fn apply_push(&mut self, q: &longbridge::quote::PushQuote) {
        let change = q.last_done - self.prev_close;
        let pct = if self.prev_close.is_zero() {
            Decimal::ZERO
        } else {
            change / self.prev_close * Decimal::ONE_HUNDRED
        };
        self.direction = match q.last_done.cmp(&self.prev_close) {
            std::cmp::Ordering::Greater => 1,
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
        };
        self.last = price(q.last_done);
        self.change = signed(change);
        self.change_pct = format!("{}{:.2}%", sign_of(pct), pct);
        self.open = price(q.open);
        self.high = price(q.high);
        self.low = price(q.low);
        self.volume = format_volume(u64::try_from(q.volume).unwrap_or(0));
        self.turnover = format_volume(u64::try_from(q.turnover.trunc()).unwrap_or(0));
        self.at = format!(
            "{:02}:{:02}:{:02}",
            q.timestamp.hour(),
            q.timestamp.minute(),
            q.timestamp.second()
        );
    }
}

/// A price, trimmed of trailing zeros so `182.400` reads as `182.4`.
fn price(value: Decimal) -> String {
    value.round_dp(3).normalize().to_string()
}

/// A change, always carrying its sign so a gain is unambiguous.
fn signed(value: Decimal) -> String {
    format!("{}{}", sign_of(value), price(value))
}

fn sign_of(value: Decimal) -> &'static str {
    if value.is_sign_positive() && !value.is_zero() {
        "+"
    } else {
        ""
    }
}

/// Fetch a card for every security the answer's widgets reference.
///
/// All widget kinds contribute, not just the single-quote one: a comparison or a
/// stock list names several securities and each deserves its quote. Symbols are
/// deduped, so one batched quote request covers the whole answer however many
/// widgets repeat a ticker.
pub async fn fetch_cards(widgets: &[Widget]) -> HashMap<String, QuoteCardData> {
    let mut seen = HashSet::new();
    let symbols: Vec<String> = widgets
        .iter()
        .filter_map(|w| match w {
            Widget::XWidget { src } => parse_widget(src),
            Widget::VisChart { .. } => None,
        })
        .flat_map(|widget| widget.symbols().to_vec())
        .filter(|s| seen.insert(s.clone()))
        .collect();
    fetch_cards_for(&symbols).await
}

/// Fetch a card per symbol. Split out so a live-quote refresh can reuse it.
pub async fn fetch_cards_for(symbols: &[String]) -> HashMap<String, QuoteCardData> {
    let mut cards = HashMap::new();
    if symbols.is_empty() {
        return cards;
    }
    let Ok(quotes) = crate::openapi::helpers::get_quotes(symbols.to_vec()).await else {
        return cards;
    };
    // Names come from a second batched call. A card without one is still useful,
    // so a failure here degrades the card rather than losing it.
    let names: HashMap<String, String> = crate::openapi::helpers::get_static_info(symbols.to_vec())
        .await
        .map(|infos| {
            infos
                .into_iter()
                .map(|i| (i.symbol, i.name_cn))
                .filter(|(_, name)| !name.is_empty())
                .collect()
        })
        .unwrap_or_default();
    for quote in &quotes {
        let name = names.get(&quote.symbol).cloned().unwrap_or_default();
        cards.insert(quote.symbol.clone(), QuoteCardData::from_quote(quote, name));
    }
    cards
}

/// Resolve bare tickers to real symbols by asking the server which ones exist.
///
/// A bare `SPCX` could be any market, or nothing at all — an answer about options
/// is full of words shaped like tickers. Rather than guess, every candidate is
/// offered to the static-info endpoint under each market suffix in one batched
/// call, and only what comes back with a name is a security. The server is the
/// authority; a candidate it does not know is left as plain text.
///
/// US first, then HK, then SG: for a letter ticker the US listing is what an
/// answer means far more often than not, and a digit code is Hong Kong.
pub async fn resolve_symbols(candidates: &[String]) -> HashMap<String, String> {
    let mut resolved = HashMap::new();
    if candidates.is_empty() {
        return resolved;
    }
    let mut probes: Vec<String> = Vec::new();
    for candidate in candidates {
        let markets: &[&str] = if candidate.chars().all(|c| c.is_ascii_digit()) {
            &["HK", "SH", "SZ"]
        } else {
            &["US", "HK", "SG"]
        };
        for market in markets {
            probes.push(format!("{candidate}.{market}"));
        }
    }
    let Ok(infos) = crate::openapi::helpers::get_static_info(probes).await else {
        return resolved;
    };
    for info in infos {
        // A listing the server knows carries a name; anything else is a miss it
        // echoed back.
        if info.name_cn.is_empty() && info.name_en.is_empty() {
            continue;
        }
        let Some((code, _)) = info.symbol.rsplit_once('.') else {
            continue;
        };
        // First market wins, which is the priority order the probes were built in.
        resolved
            .entry(code.to_string())
            .or_insert_with(|| info.symbol.clone());
    }
    resolved
}

/// Every security named by the widgets in `answer`, deduped, in source order.
///
/// Used to decide what to subscribe to for live updates.
pub fn symbols_in(answer: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    crate::cli::agent::events::extract_widgets(answer)
        .iter()
        .filter_map(|w| match w {
            Widget::XWidget { src } => parse_widget(src),
            Widget::VisChart { .. } => None,
        })
        .flat_map(|w| w.symbols().to_vec())
        .filter(|s| seen.insert(s.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every widget kind that names securities contributes them, and a ticker
    /// repeated across widgets is fetched once.
    #[test]
    fn symbols_come_from_every_widget_kind_and_are_deduped() {
        let answer = "\
            <x-widget src=\"widget://quote/security/detail?symbol=TSLA.US\"></x-widget>\n\
            widget://quote/security/comparison?symbols=TSLA.US&symbols=NVDA.US\n\
            <x-widget src=\"widget://stock/list?symbols=NVDA.US&symbols=AAPL.US\"></x-widget>\n\
            <x-widget src=\"widget://cta/open_account\"></x-widget>";
        assert_eq!(
            symbols_in(answer),
            vec!["TSLA.US".to_string(), "NVDA.US".into(), "AAPL.US".into()],
            "a comparison and a list name securities too, and duplicates collapse"
        );
    }

    /// A CTA names no security, so it must not drag an empty symbol into the
    /// request.
    #[test]
    fn a_widget_without_securities_contributes_nothing() {
        assert!(symbols_in("<x-widget src=\"widget://cta/fund_account\"></x-widget>").is_empty());
        assert!(symbols_in("no widgets here").is_empty());
    }

    #[test]
    fn prices_are_trimmed_and_changes_are_signed() {
        use rust_decimal_macros::dec;
        assert_eq!(price(dec!(182.400)), "182.4");
        assert_eq!(price(dec!(182)), "182");
        assert_eq!(signed(dec!(3.75)), "+3.75");
        assert_eq!(signed(dec!(-3.75)), "-3.75");
        assert_eq!(signed(dec!(0)), "0");
    }
}
