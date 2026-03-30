use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::types::{
    Counter, Currency, Depth, DepthData, QuoteData, TradeData, TradeSession, TradeStatus,
};

/// Stock data (simplified)
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Stock {
    pub counter: Counter,
    pub name: String,
    pub currency: Currency,
    pub trade_status: TradeStatus,
    pub trade_session: TradeSession,
    pub quote: QuoteData,
    pub depth: DepthData,
    pub static_info: Option<Arc<longbridge::quote::SecurityStaticInfo>>,
    pub trades: Vec<TradeData>, // Recent trades
}

impl Stock {
    pub fn new(counter: Counter) -> Self {
        Self {
            counter,
            name: String::new(),
            currency: Currency::default(),
            trade_status: TradeStatus::default(),
            trade_session: TradeSession::default(),
            quote: QuoteData::default(),
            depth: DepthData::default(),
            static_info: None,
            trades: Vec::new(),
        }
    }

    /// Check if has quote permission
    pub fn quoting(&self) -> bool {
        // Simplified implementation: assume always has permission
        true
    }

    /// Get display name, fallback to code if name is empty
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            self.counter.code()
        } else {
            &self.name
        }
    }

    /// Update quote data (from longbridge SDK `PushQuote`, for WebSocket push)
    pub fn update_from_push_quote(&mut self, quote: &longbridge::quote::PushQuote) {
        self.quote.last_done = Some(quote.last_done);
        self.quote.open = Some(quote.open);
        self.quote.high = Some(quote.high);
        self.quote.low = Some(quote.low);
        self.quote.volume = quote.volume.unsigned_abs();
        self.quote.turnover = quote.turnover;
        self.quote.timestamp = quote.timestamp.unix_timestamp();

        // Update trade_status and trade_session directly from PushQuote
        self.trade_status = quote.trade_status;
        self.trade_session = quote.trade_session;
    }

    /// Update from `SecurityQuote` (full quote data from API, includes `prev_close` but NO `trade_session`)
    pub fn update_from_security_quote(&mut self, quote: &longbridge::quote::SecurityQuote) {
        self.quote.last_done = Some(quote.last_done);
        self.quote.prev_close = Some(quote.prev_close);
        self.quote.open = Some(quote.open);
        self.quote.high = Some(quote.high);
        self.quote.low = Some(quote.low);
        self.quote.volume = quote.volume.unsigned_abs();
        self.quote.turnover = quote.turnover;
        self.quote.timestamp = quote.timestamp.unix_timestamp();

        // Update trade_status from SecurityQuote (Note: SecurityQuote does NOT have trade_session)
        self.trade_status = quote.trade_status;
        // trade_session will be updated from WebSocket PushQuote or calculated from market hours
    }

    /// Update depth data (from longbridge SDK)
    pub fn update_from_depth(&mut self, depth: &longbridge::quote::SecurityDepth) {
        self.depth.asks = depth
            .asks
            .iter()
            .map(|d| Depth {
                position: d.position,
                price: d.price.unwrap_or_default(),
                volume: d.volume,
                order_num: d.order_num,
            })
            .collect();

        self.depth.bids = depth
            .bids
            .iter()
            .map(|d| Depth {
                position: d.position,
                price: d.price.unwrap_or_default(),
                volume: d.volume,
                order_num: d.order_num,
            })
            .collect();
    }

    /// Update trades data (from longbridge SDK)
    pub fn update_from_trades(&mut self, trades: &[longbridge::quote::Trade]) {
        self.trades = trades
            .iter()
            .map(|t| TradeData {
                price: t.price,
                volume: t.volume,
                timestamp: t.timestamp.unix_timestamp(),
                trade_type: t.trade_type.clone(),
                direction: t.direction,
            })
            .collect();
    }

    /// Update static info (from longbridge SDK)
    pub fn update_from_static_info(&mut self, info: longbridge::quote::SecurityStaticInfo) {
        self.static_info = Some(Arc::new(info));
    }
}
