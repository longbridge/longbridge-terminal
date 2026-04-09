use super::{Counter, TradeSessionExt};
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
        fn market_priority(market: &str) -> u8 {
            match market {
                "US" => 0,
                "HK" => 1,
                "SH" | "SZ" => 2,
                "SG" => 3,
                _ => 99,
            }
        }

        // Snapshot sort keys before sorting. STOCKS is updated concurrently,
        // so reading it inside sort_by can yield different results for the same
        // pair on successive calls, violating total order and causing a panic.
        let keys: Vec<(bool, u8)> = self
            .counters
            .iter()
            .map(|c| {
                let not_trading = !super::STOCKS
                    .get(c)
                    .is_some_and(|s| s.trade_session.is_normal_trading());
                (not_trading, market_priority(c.market()))
            })
            .collect();

        let mut indices: Vec<usize> = (0..self.counters.len()).collect();
        indices.sort_by(|&i, &j| {
            keys[i]
                .cmp(&keys[j])
                .then_with(|| self.counters[i].as_str().cmp(self.counters[j].as_str()))
        });
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
