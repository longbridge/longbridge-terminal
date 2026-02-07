use super::Counter;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

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

        for counter in watchlist_counters.into_iter().chain(holdings.into_iter()) {
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
        // Get market sort priority (considering trading hours and fixed order)
        fn market_priority(market: &str, is_trading: bool) -> u8 {
            // Base priority: US=0, HK=1, SH/CN=2, SZ=3, SG=4
            let base = match market {
                "US" => 0,
                "HK" => 1,
                "SH" | "SZ" => 2,
                "SG" => 3,
                _ => 99,
            };

            if is_trading {
                // Markets in trading session: priority 0-4
                base
            } else {
                // Markets not trading: priority 10-14 (after trading markets)
                base + 10
            }
        }

        // Sort by market trading hours and default order
        self.counters.sort_by(|a, b| {
            let a_market_str = a.market();
            let b_market_str = b.market();
            let a_market = a.region();
            let b_market = b.region();
            let a_trading = a_market.is_trading();
            let b_trading = b_market.is_trading();

            // First sort by market priority (trading markets first)
            let a_priority = market_priority(a_market_str, a_trading);
            let b_priority = market_priority(b_market_str, b_trading);
            let market_cmp = a_priority.cmp(&b_priority);
            if market_cmp != std::cmp::Ordering::Equal {
                return market_cmp;
            }

            // Within same market, sort by code
            a.as_str().cmp(b.as_str())
        });
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
