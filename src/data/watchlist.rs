use super::{Counter, TradeSession};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Watchlist group
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WatchlistGroup {
    pub id: u64,
    pub name: String,
}

/// Watchlist
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Watchlist {
    pub group_id: Option<u64>,
    pub counters: Vec<Counter>,
    pub groups: Vec<WatchlistGroup>,
    pub hidden: bool,
    pub sort_by: (u8, u8, bool), // (sort_mode, sort_by, reverse)
}

impl Watchlist {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_group_id(&mut self, id: u64) {
        self.group_id = Some(id);
    }

    pub fn set_counters(&mut self, counters: Vec<Counter>) {
        self.counters = counters;
    }

    pub fn counters(&self) -> &[Counter] {
        &self.counters
    }

    /// Full load (including holdings)
    pub fn full_load(&mut self, watchlist_counters: Vec<Counter>, holdings: Vec<Counter>) {
        // Use HashSet to deduplicate and merge watchlist and holdings
        let mut seen = HashSet::new();
        let mut all = Vec::new();

        for counter in watchlist_counters.into_iter().chain(holdings) {
            if seen.insert(counter.clone()) {
                all.push(counter);
            }
        }

        self.counters = all;
    }

    /// Load watchlist
    pub fn load(&mut self, counters: Vec<Counter>) {
        // Use HashSet to deduplicate
        let mut seen = HashSet::new();
        let mut deduped = Vec::new();

        for counter in counters {
            if seen.insert(counter.clone()) {
                deduped.push(counter);
            }
        }

        self.counters = deduped;
    }

    /// Set hidden state
    pub fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
    }

    /// Set sort by
    pub fn set_sortby(&mut self, sortby: (u8, u8, bool)) {
        self.sort_by = sortby;
    }

    /// Refresh (re-apply sorting, etc.)
    pub fn refresh(&mut self) {
        fn market_key(market: &str) -> &str {
            match market {
                "SH" | "SZ" => "CN",
                market => market,
            }
        }

        fn market_priority(market: &str) -> u8 {
            match market {
                "US" => 0,
                "HK" => 1,
                "SH" | "SZ" => 2,
                "SG" => 3,
                _ => 99,
            }
        }

        fn session_priority(session: TradeSession) -> u8 {
            match session {
                TradeSession::Intraday => 0,
                TradeSession::Pre => 1,
                TradeSession::Post | TradeSession::Overnight => 2,
            }
        }

        // Session is a market-level property for sorting. A few indices or
        // synthetic counters can report Intraday while most securities in the
        // same market are Pre, so use the market's most common session class.
        let mut session_counts: HashMap<&str, [usize; 3]> = HashMap::new();
        for counter in &self.counters {
            let Some(stock) = super::STOCKS.get(counter) else {
                continue;
            };
            if stock
                .quote
                .last_done
                .is_none_or(|price| price <= Decimal::ZERO)
            {
                continue;
            }
            session_counts
                .entry(market_key(counter.market()))
                .or_default()[usize::from(session_priority(stock.trade_session))] += 1;
        }
        let market_sessions: HashMap<&str, u8> = session_counts
            .into_iter()
            .map(|(market, counts)| {
                let mut best_rank = 0;
                let mut best_count = counts[0];
                for (rank, count) in counts.into_iter().enumerate().skip(1) {
                    if count > best_count {
                        best_rank = rank as u8;
                        best_count = count;
                    }
                }
                (market, best_rank)
            })
            .collect();

        // Snapshot sort keys before sorting. STOCKS is updated concurrently,
        // so reading it inside sort_by can yield different results for the same
        // pair on successive calls, violating total order and causing a panic.
        let keys: Vec<(u8, u8)> = self
            .counters
            .iter()
            .map(|counter| {
                (
                    market_sessions
                        .get(market_key(counter.market()))
                        .copied()
                        .unwrap_or(2),
                    market_priority(counter.market()),
                )
            })
            .collect();

        let mut indices: Vec<usize> = (0..self.counters.len()).collect();
        // Stable sort: equal keys preserve the original API order, giving the user
        // a predictable layout without an arbitrary alphabetical tiebreaker.
        indices.sort_by(|&i, &j| keys[i].cmp(&keys[j]));
        let sorted = indices
            .into_iter()
            .map(|i| self.counters[i].clone())
            .collect();
        self.counters = sorted;
    }

    /// Get group list
    pub fn groups(&self) -> &[WatchlistGroup] {
        &self.groups
    }

    /// Set group list
    pub fn set_groups(&mut self, groups: Vec<WatchlistGroup>) {
        self.groups = groups;
    }

    /// Get current group
    pub fn group(&self) -> Option<&WatchlistGroup> {
        let group_id = self.group_id?;
        self.groups.iter().find(|g| g.id == group_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{Stock, TradeSession, STOCKS};
    use rust_decimal::Decimal;

    fn counter_with_session(symbol: &str, session: TradeSession, has_quote: bool) -> Counter {
        let counter = Counter::new(symbol);
        let mut stock = Stock::new(counter.clone());
        stock.trade_session = session;
        stock.quote.last_done = has_quote.then_some(Decimal::ONE);
        STOCKS.insert(stock);
        counter
    }

    #[test]
    fn refresh_sorts_by_market_session_before_market_priority() {
        let counters = vec![
            counter_with_session(".SPX_SORT_TEST.US", TradeSession::Intraday, true),
            counter_with_session("AAPL_SORT_TEST.US", TradeSession::Pre, true),
            counter_with_session("MSFT_SORT_TEST.US", TradeSession::Pre, true),
            counter_with_session("AMD_SORT_TEST.US", TradeSession::Pre, true),
            counter_with_session("700_SORT_TEST.HK", TradeSession::Intraday, true),
            counter_with_session("300001_SORT_TEST.SZ", TradeSession::Intraday, true),
            counter_with_session("600000_SORT_TEST.SH", TradeSession::Intraday, true),
            counter_with_session("300002_SORT_TEST.SZ", TradeSession::Intraday, true),
            counter_with_session("D05_SORT_TEST.SG", TradeSession::Intraday, false),
        ];
        let mut watchlist = Watchlist::new();
        watchlist.set_counters(counters.clone());

        watchlist.refresh();

        let symbols: Vec<_> = watchlist.counters().iter().map(Counter::as_str).collect();
        assert_eq!(
            symbols,
            [
                "700_SORT_TEST.HK",
                "300001_SORT_TEST.SZ",
                "600000_SORT_TEST.SH",
                "300002_SORT_TEST.SZ",
                ".SPX_SORT_TEST.US",
                "AAPL_SORT_TEST.US",
                "MSFT_SORT_TEST.US",
                "AMD_SORT_TEST.US",
                "D05_SORT_TEST.SG",
            ]
        );

        for counter in counters {
            STOCKS.remove(&counter);
        }
    }
}
