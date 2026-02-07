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
        }
    }

    /// Check if has quote permission
    pub fn quoting(&self) -> bool {
        // Simplified implementation: assume always has permission
        true
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
        self.trade_status = match quote.trade_status {
            longport::quote::TradeStatus::Normal => TradeStatus::Normal,
            longport::quote::TradeStatus::Halted => TradeStatus::Halted,
            longport::quote::TradeStatus::Delisted => TradeStatus::Delisted,
            _ => {
                // For other statuses (closed, pre/post market, etc.), map to STOP/UsStop
                let market = self.counter.region();
                match market {
                    super::types::Market::US => TradeStatus::UsStop,
                    _ => TradeStatus::STOP,
                }
            }
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
