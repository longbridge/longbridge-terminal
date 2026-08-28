use std::{
    collections::{HashMap, HashSet},
    sync::{Mutex, RwLock},
};

use crate::data::{AdjustType, Counter, Kline, KlineType, Klines};
use rust_decimal::Decimal;

pub static KLINES: std::sync::LazyLock<KlineStore> = std::sync::LazyLock::new(KlineStore::new);

type StoreKey = (Counter, KlineType, AdjustType);

#[derive(Debug)]
pub struct KlineStore {
    inner: RwLock<HashMap<StoreKey, (bool /* no more history */, Klines)>>,
    /// Series with a request already on the wire.
    ///
    /// `by_pagination` is called from inside the render closure, so a series
    /// that is not in the store yet is asked for again on every frame — thirty
    /// times a second for as long as the fetch takes. The quote SDK runs one
    /// command at a time and awaits each round trip, so those duplicates do not
    /// merely waste quota: they take the single request slot away from the
    /// quote, trades and static-info calls the same stock switch is waiting on.
    /// One request per series at a time; the next frame re-asks if it is still
    /// missing when the first one lands.
    inflight: Mutex<HashSet<StoreKey>>,
}

/// Clears the in-flight mark when the request task ends, including when the
/// task is dropped part-way through.
struct InflightGuard(StoreKey);

impl Drop for InflightGuard {
    fn drop(&mut self) {
        KLINES.inflight.lock().expect("poison").remove(&self.0);
    }
}

impl KlineStore {
    fn new() -> Self {
        Self {
            inner: RwLock::default(),
            inflight: Mutex::default(),
        }
    }

    /// The key a series is stored under: below the daily period every adjust
    /// type shares one entry, because the adjustment is applied on read.
    fn key(counter: &Counter, kline_type: KlineType, adjust_type: AdjustType) -> StoreKey {
        (
            counter.clone(),
            kline_type,
            Self::normalize(kline_type).unwrap_or(adjust_type),
        )
    }

    /// Marks a series as being fetched, or reports that it already is.
    fn begin_request(&self, key: &StoreKey) -> Option<InflightGuard> {
        self.inflight
            .lock()
            .expect("poison")
            .insert(key.clone())
            .then(|| InflightGuard(key.clone()))
    }

    pub fn by_pagination(
        &self,
        counter: Counter,
        kline_type: KlineType,
        adjust_type: AdjustType,
        page: usize,
        page_size: usize,
    ) -> Klines {
        let key = Self::key(&counter, kline_type, adjust_type);
        let store = self.inner.read().expect("poison");
        let Some((has_more, entries)) = store.get(&key) else {
            self.spawn_request(
                key,
                counter,
                kline_type,
                adjust_type,
                (page + 1) * page_size,
            );
            return Klines::default();
        };

        let tmp: Klines;
        let results = if let Some(offset) = entries.len().checked_sub(page * page_size) {
            &entries[offset.saturating_sub(page_size)..offset]
        } else {
            tmp = vec![];
            &tmp
        };

        if *has_more && results.len() < page_size {
            self.spawn_request(key, counter, kline_type, adjust_type, page_size);
        }

        // Fix forward adjust
        if kline_type <= KlineType::PerDay && adjust_type == AdjustType::ForwardAdjust {
            results
                .iter()
                .map(|e| {
                    let (a, b) = (e.factor_a, e.factor_b);
                    Kline {
                        open: e.open * a + b,
                        close: e.close * a + b,
                        high: e.high * a + b,
                        low: e.low * a + b,
                        amount: e.amount,
                        balance: e.balance,
                        timestamp: e.timestamp,
                        factor_a: a,
                        factor_b: b,
                        total: e.total,
                    }
                })
                .collect()
        } else {
            results.to_vec()
        }
    }

    /// Fetch a series the caller is waiting on, unless it is already stored or
    /// already on the wire.
    ///
    /// The difference from [`spawn_request`] is that this one is awaited, so a
    /// caller can put the chart ahead of its own later requests. The quote SDK
    /// runs one command at a time, and the chart is by far the largest thing on
    /// the screen that is blank until its answer arrives, so it goes first.
    pub async fn ensure(
        &self,
        counter: &Counter,
        kline_type: KlineType,
        adjust_type: AdjustType,
        count: usize,
    ) {
        let key = Self::key(counter, kline_type, adjust_type);
        if self.inner.read().expect("poison").contains_key(&key) {
            return;
        }
        let Some(guard) = self.begin_request(&key) else {
            return;
        };
        Self::request(counter.clone(), kline_type, adjust_type, count).await;
        drop(guard);
    }

    /// Fetch a series, unless a request for it is already on the wire or the
    /// reader has already moved off the stock it belongs to.
    fn spawn_request(
        &self,
        key: StoreKey,
        counter: Counter,
        kline_type: KlineType,
        adjust_type: AdjustType,
        count: usize,
    ) {
        if !crate::tui::systems::settled_on(&counter) {
            return;
        }
        let Some(guard) = self.begin_request(&key) else {
            return;
        };
        crate::tui::app::RT.get().unwrap().spawn(async move {
            Self::request(counter, kline_type, adjust_type, count).await;
            drop(guard);
        });
    }

    /// Update candlestick data
    pub fn update(
        &self,
        counter: Counter,
        kline_type: KlineType,
        adjust_type: AdjustType,
        data: Klines,
        more: bool,
    ) {
        let key = (
            counter,
            kline_type,
            Self::normalize(kline_type).unwrap_or(adjust_type),
        );

        let mut store = self.inner.write().expect("poison");
        let entry = store.entry(key).or_insert((true, vec![]));
        entry.0 = more;

        // Merge candlestick data (simplified implementation)
        for kline in data {
            // Check if already exists
            if let Some(existing) = entry.1.iter_mut().find(|k| k.timestamp == kline.timestamp) {
                *existing = kline;
            } else {
                entry.1.push(kline);
            }
        }

        // Sort by timestamp
        entry.1.sort_by_key(|k| k.timestamp);
    }

    fn normalize(kline_type: KlineType) -> Option<AdjustType> {
        if kline_type <= KlineType::PerDay {
            Some(AdjustType::NoAdjust)
        } else {
            None
        }
    }

    async fn request(
        counter: Counter,
        kline_type: KlineType,
        adjust_type: AdjustType,
        count: usize,
    ) {
        // Use Longbridge SDK to request candlestick data
        let ctx = crate::openapi::quote();

        // Convert KlineType to Longbridge Period
        let period = match kline_type {
            KlineType::PerMinute => longbridge::quote::Period::OneMinute,
            KlineType::PerFiveMinutes => longbridge::quote::Period::FiveMinute,
            KlineType::PerFifteenMinutes => longbridge::quote::Period::FifteenMinute,
            KlineType::PerThirtyMinutes => longbridge::quote::Period::ThirtyMinute,
            KlineType::PerHour => longbridge::quote::Period::SixtyMinute,
            KlineType::PerDay => longbridge::quote::Period::Day,
            KlineType::PerWeek => longbridge::quote::Period::Week,
            KlineType::PerMonth => longbridge::quote::Period::Month,
            KlineType::PerYear => longbridge::quote::Period::Year,
        };

        let adjust = adjust_type;

        // Select appropriate trading session based on period type
        // For all periods, use All to get complete data
        let trade_session = longbridge::quote::TradeSessions::All;

        tracing::info!(
            "Requesting candlestick data: counter={}, period={:?}, count={}, adjust={:?}",
            counter,
            period,
            count,
            adjust
        );

        match ctx
            .candlesticks(counter.as_str(), period, count, adjust, trade_session)
            .await
        {
            Ok(candlesticks) => {
                tracing::info!(
                    "Successfully fetched candlestick data: counter={}, count={}",
                    counter,
                    candlesticks.len()
                );

                // Convert to internal format
                let klines: Vec<Kline> = candlesticks
                    .iter()
                    .map(|c| Kline {
                        timestamp: c.timestamp.unix_timestamp(),
                        open: c.open,
                        high: c.high,
                        low: c.low,
                        close: c.close,
                        amount: c.volume.unsigned_abs(),
                        balance: c.turnover,
                        factor_a: Decimal::ONE,
                        factor_b: Decimal::ZERO,
                        total: 0,
                    })
                    .collect();

                if !klines.is_empty() {
                    tracing::debug!(
                        "First candlestick: open={}, high={}, low={}, close={}, volume={}",
                        klines[0].open,
                        klines[0].high,
                        klines[0].low,
                        klines[0].close,
                        klines[0].amount
                    );
                }

                let has_more = klines.len() == count;
                KLINES.update(counter, kline_type, adjust_type, klines, has_more);
            }
            Err(e) => {
                tracing::error!(
                    "Failed to request candlestick data: counter={}, error={}",
                    counter,
                    e
                );
            }
        }
    }
}
