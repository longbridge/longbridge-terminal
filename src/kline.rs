use std::{collections::HashMap, sync::RwLock};

use crate::data::{AdjustType, Counter, Kline, KlineType, Klines, Market};
use once_cell::sync::Lazy;
use rust_decimal::Decimal;

pub static KLINES: Lazy<KlineStore> = Lazy::new(KlineStore::new);

type StoreKey = (Counter, KlineType, AdjustType);

#[derive(Debug)]
pub struct KlineStore {
    inner: RwLock<HashMap<StoreKey, (bool /* no more history */, Klines)>>,
}

impl KlineStore {
    fn new() -> Self {
        Self {
            inner: RwLock::default(),
        }
    }

    pub fn by_pagination(
        &self,
        counter: Counter,
        kline_type: KlineType,
        adjust_type: AdjustType,
        page: usize,
        page_size: usize,
    ) -> Klines {
        let store = self.inner.read().expect("poison");
        let Some((has_more, entries)) = store.get(&(
            counter.clone(),
            kline_type,
            Self::normalize(kline_type).unwrap_or(adjust_type),
        )) else {
            crate::app::RT.get().unwrap().spawn(Self::request(
                counter,
                kline_type,
                adjust_type,
                0,
                (page + 1) * page_size,
            ));
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
            crate::app::RT.get().unwrap().spawn(Self::request(
                counter,
                kline_type,
                adjust_type,
                entries.first().map(|e| e.timestamp).unwrap_or_default(),
                page_size,
            ));
        }

        // 修正前复权
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

    pub fn clear(&self) {
        // Clear candlestick cache
        let mut store = self.inner.write().expect("poison");
        store.clear();
    }

    /// Daily rotation (at market close)
    pub fn daily_rotate(&self, _market: Market) {
        // TODO: Implement daily rotation logic
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
        _before: i64,
        count: usize,
    ) {
        // Use longport SDK to request candlestick data
        let ctx = crate::openapi::quote();

        // Convert KlineType to longport Period
        let period = match kline_type {
            KlineType::PerMinute => longport::quote::Period::OneMinute,
            KlineType::PerFiveMinutes => longport::quote::Period::FiveMinute,
            KlineType::PerFifteenMinutes => longport::quote::Period::FifteenMinute,
            KlineType::PerThirtyMinutes => longport::quote::Period::ThirtyMinute,
            KlineType::PerHour => longport::quote::Period::SixtyMinute,
            KlineType::PerDay => longport::quote::Period::Day,
            KlineType::PerWeek => longport::quote::Period::Week,
            KlineType::PerMonth => longport::quote::Period::Month,
            KlineType::PerYear => longport::quote::Period::Year,
        };

        // Convert AdjustType to longport AdjustType
        let adjust = match adjust_type {
            AdjustType::NoAdjust => longport::quote::AdjustType::NoAdjust,
            AdjustType::ForwardAdjust => longport::quote::AdjustType::ForwardAdjust,
        };

        // Select appropriate trading session based on period type
        // For all periods, use All to get complete data
        let trade_session = longport::quote::TradeSessions::All;

        tracing::info!(
            "Requesting candlestick data: counter={}, period={:?}, count={}, adjust={:?}",
            counter, period, count, adjust
        );

        match ctx.candlesticks(counter.as_str(), period, count, adjust, trade_session).await {
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
                        amount: c.volume as u64,
                        balance: c.turnover,
                        factor_a: Decimal::ONE,
                        factor_b: Decimal::ZERO,
                        total: 0,
                    })
                    .collect();

                if !klines.is_empty() {
                    tracing::debug!(
                        "First candlestick: open={}, high={}, low={}, close={}, volume={}",
                        klines[0].open, klines[0].high, klines[0].low, klines[0].close, klines[0].amount
                    );
                }

                let has_more = klines.len() == count;
                KLINES.update(counter, kline_type, adjust_type, klines, has_more);
            }
            Err(e) => {
                tracing::error!("Failed to request candlestick data: counter={}, error={}", counter, e);
            }
        }
    }
}
