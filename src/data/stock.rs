use serde::{Deserialize, Serialize};

use super::types::*;

/// Stock data (simplified)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stock {
    pub counter: Counter,
    pub name: String,
    pub currency: Currency,
    pub trade_status: TradeStatus,
    pub quote: QuoteData,
    pub depth: DepthData,
    pub static_info: Option<StaticInfo>,  // Static info (market cap, shares, etc.)
    pub trades: Vec<TradeData>,           // Recent trades
}

impl Default for Stock {
    fn default() -> Self {
        Self {
            counter: Counter::default(),
            name: String::new(),
            currency: Currency::default(),
            trade_status: TradeStatus::default(),
            quote: QuoteData::default(),
            depth: DepthData::default(),
            static_info: None,
            trades: Vec::new(),
        }
    }
}

impl Stock {
    pub fn new(counter: Counter) -> Self {
        Self {
            counter,
            name: String::new(),
            currency: Currency::default(),
            trade_status: TradeStatus::default(),
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

    /// Update quote data (from longport SDK)
    pub fn update_from_quote(&mut self, quote: &longport::quote::RealtimeQuote) {
        self.quote.last_done = Some(quote.last_done);
        self.quote.open = Some(quote.open);
        self.quote.high = Some(quote.high);
        self.quote.low = Some(quote.low);
        self.quote.volume = quote.volume as u64;
        self.quote.turnover = quote.turnover;
        self.quote.timestamp = quote.timestamp.unix_timestamp();

        // Update trade_status from quote
        // Note: Longport SDK only provides simplified status (Normal/Halted/Delisted)
        // We map these to our detailed status codes
        self.trade_status = match quote.trade_status {
            longport::quote::TradeStatus::Normal => TradeStatus::TRADING,
            longport::quote::TradeStatus::Halted => TradeStatus::TRADING_HALT,
            longport::quote::TradeStatus::Delisted => TradeStatus::DELIST,
            longport::quote::TradeStatus::Fuse => TradeStatus::STOP,
            longport::quote::TradeStatus::SuspendTrade => TradeStatus::STOP,
            _ => TradeStatus::UNKNOWN,
        };
    }

    /// Update depth data (from longport SDK)
    pub fn update_from_depth(&mut self, depth: &longport::quote::SecurityDepth) {
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

    /// Update trades data (from longport SDK)
    pub fn update_from_trades(&mut self, trades: &[longport::quote::Trade]) {
        self.trades = trades
            .iter()
            .map(|t| TradeData {
                price: t.price,
                volume: t.volume,
                timestamp: t.timestamp.unix_timestamp(),
                trade_type: t.trade_type.clone(),
                direction: match t.direction {
                    longport::quote::TradeDirection::Neutral => super::types::TradeDirection::Neutral,
                    longport::quote::TradeDirection::Down => super::types::TradeDirection::Down,
                    longport::quote::TradeDirection::Up => super::types::TradeDirection::Up,
                },
            })
            .collect();
    }

    /// Update static info (from longport SDK)
    pub fn update_from_static_info(&mut self, info: &longport::quote::SecurityStaticInfo) {
        self.static_info = Some(StaticInfo {
            symbol: info.symbol.clone(),
            name_cn: info.name_cn.clone(),
            name_en: info.name_en.clone(),
            name_hk: info.name_hk.clone(),
            exchange: info.exchange.clone(),
            currency: info.currency.clone(),
            lot_size: info.lot_size,
            total_shares: info.total_shares,
            circulating_shares: info.circulating_shares,
            hk_shares: info.hk_shares,
            eps: Some(info.eps),
            eps_ttm: Some(info.eps_ttm),
            bps: Some(info.bps),
            dividend_yield: Some(info.dividend_yield),
            stock_derivatives: vec![], // Simplified for now, no derivative type conversion
            board: format!("{:?}", info.board), // Convert to string
        });
    }
}
