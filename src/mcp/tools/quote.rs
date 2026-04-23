use serde_json::{json, Value};

use super::{api_err, opt_string, opt_u32, require_string, require_strings};
use crate::mcp::protocol::Tool;

pub fn tool_definitions() -> Vec<Tool> {
    vec![
        Tool {
            name: "quote",
            description: "Get real-time quotes for one or more symbols. \
                Symbol format: CODE.MARKET (e.g. 700.HK, TSLA.US, 600519.SH).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "symbols": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "List of symbols, e.g. [\"700.HK\", \"TSLA.US\"]",
                        "minItems": 1
                    }
                },
                "required": ["symbols"]
            }),
        },
        Tool {
            name: "depth",
            description: "Get Level 2 bid/ask order book for a symbol.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "symbol": {"type": "string", "description": "Symbol, e.g. \"700.HK\""}
                },
                "required": ["symbol"]
            }),
        },
        Tool {
            name: "trades",
            description: "Get recent tick-by-tick trades for a symbol.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "symbol": {"type": "string"},
                    "count": {"type": "integer", "description": "Number of trades (default 50, max 1000)", "default": 50}
                },
                "required": ["symbol"]
            }),
        },
        Tool {
            name: "intraday",
            description: "Get today's intraday minute-by-minute price and volume data.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "symbol": {"type": "string"}
                },
                "required": ["symbol"]
            }),
        },
        Tool {
            name: "kline",
            description: "Get OHLCV candlestick data. Periods: 1m, 5m, 15m, 30m, 1h, day, week, month, year.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "symbol": {"type": "string"},
                    "period": {
                        "type": "string",
                        "description": "Candlestick period: 1m 5m 15m 30m 1h day week month year",
                        "default": "day"
                    },
                    "count": {"type": "integer", "description": "Number of candles (default 100)", "default": 100},
                    "adjust": {
                        "type": "string",
                        "description": "Price adjustment: none or forward",
                        "default": "none"
                    }
                },
                "required": ["symbol"]
            }),
        },
        Tool {
            name: "static_info",
            description: "Get static reference info for symbols: name, exchange, currency, lot size, total shares.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "symbols": {
                        "type": "array",
                        "items": {"type": "string"},
                        "minItems": 1
                    }
                },
                "required": ["symbols"]
            }),
        },
    ]
}

pub async fn handle_quote(args: Value) -> Result<Value, (i64, String)> {
    let symbols = require_strings(&args, "symbols")?;
    let ctx = crate::openapi::quote();
    let quotes = ctx.quote(&symbols).await.map_err(api_err)?;

    let result: Vec<Value> = quotes
        .iter()
        .map(|q| {
            json!({
                "symbol": q.symbol,
                "last_done": q.last_done.to_string(),
                "prev_close": q.prev_close.to_string(),
                "open": q.open.to_string(),
                "high": q.high.to_string(),
                "low": q.low.to_string(),
                "volume": q.volume,
                "turnover": q.turnover.to_string(),
                "trade_status": format!("{:?}", q.trade_status),
            })
        })
        .collect();

    Ok(json!({ "quotes": result }))
}

pub async fn handle_depth(args: Value) -> Result<Value, (i64, String)> {
    let symbol = require_string(&args, "symbol")?;
    let ctx = crate::openapi::quote();
    let depth = ctx.depth(symbol.clone()).await.map_err(api_err)?;

    let map_levels = |levels: &[longbridge::quote::Depth]| -> Vec<Value> {
        levels
            .iter()
            .map(|d| {
                json!({
                    "position": d.position,
                    "price": d.price.map(|p| p.to_string()).unwrap_or_default(),
                    "volume": d.volume,
                    "order_num": d.order_num,
                })
            })
            .collect()
    };

    Ok(json!({
        "symbol": symbol,
        "asks": map_levels(&depth.asks),
        "bids": map_levels(&depth.bids),
    }))
}

pub async fn handle_trades(args: Value) -> Result<Value, (i64, String)> {
    let symbol = require_string(&args, "symbol")?;
    let count = opt_u32(&args, "count", 50) as usize;
    let ctx = crate::openapi::quote();
    let trades = ctx.trades(&symbol, count).await.map_err(api_err)?;

    let result: Vec<Value> = trades
        .iter()
        .map(|t| {
            json!({
                "price": t.price.to_string(),
                "volume": t.volume,
                "timestamp": t.timestamp.to_string(),
                "trade_type": t.trade_type,
                "direction": format!("{:?}", t.direction),
                "trade_session": format!("{:?}", t.trade_session),
            })
        })
        .collect();

    Ok(json!({ "trades": result }))
}

pub async fn handle_intraday(args: Value) -> Result<Value, (i64, String)> {
    use longbridge::quote::TradeSessions;

    let symbol = require_string(&args, "symbol")?;
    let ctx = crate::openapi::quote();
    let lines = ctx
        .intraday(symbol, TradeSessions::Intraday)
        .await
        .map_err(api_err)?;

    let result: Vec<Value> = lines
        .iter()
        .map(|l| {
            json!({
                "timestamp": l.timestamp.to_string(),
                "price": l.price.to_string(),
                "volume": l.volume,
                "turnover": l.turnover.to_string(),
                "avg_price": l.avg_price.to_string(),
            })
        })
        .collect();

    Ok(json!({ "intraday": result }))
}

pub async fn handle_kline(args: Value) -> Result<Value, (i64, String)> {
    use longbridge::quote::{AdjustType, Period, TradeSessions};

    let symbol = require_string(&args, "symbol")?;
    let count = opt_u32(&args, "count", 100) as usize;

    let period_str = opt_string(&args, "period").unwrap_or("day");
    let period = match period_str {
        "1m" | "minute" => Period::OneMinute,
        "5m" => Period::FiveMinute,
        "15m" => Period::FifteenMinute,
        "30m" => Period::ThirtyMinute,
        "1h" | "hour" => Period::SixtyMinute,
        "week" | "w" => Period::Week,
        "month" | "1mo" | "m" => Period::Month,
        "year" | "y" => Period::Year,
        _ => Period::Day,
    };

    let adjust_str = opt_string(&args, "adjust").unwrap_or("none");
    let adjust = match adjust_str {
        "forward" => AdjustType::ForwardAdjust,
        _ => AdjustType::NoAdjust,
    };

    let ctx = crate::openapi::quote();
    let klines = ctx
        .candlesticks(symbol, period, count, adjust, TradeSessions::Intraday)
        .await
        .map_err(api_err)?;

    let result: Vec<Value> = klines
        .iter()
        .map(|k| {
            json!({
                "timestamp": k.timestamp.to_string(),
                "open": k.open.to_string(),
                "high": k.high.to_string(),
                "low": k.low.to_string(),
                "close": k.close.to_string(),
                "volume": k.volume,
                "turnover": k.turnover.to_string(),
            })
        })
        .collect();

    Ok(json!({ "klines": result }))
}

pub async fn handle_static_info(args: Value) -> Result<Value, (i64, String)> {
    let symbols = require_strings(&args, "symbols")?;
    let ctx = crate::openapi::quote();
    let infos = ctx.static_info(&symbols).await.map_err(api_err)?;

    let result: Vec<Value> = infos
        .iter()
        .map(|s| {
            json!({
                "symbol": s.symbol,
                "name_cn": s.name_cn,
                "name_en": s.name_en,
                "exchange": s.exchange,
                "currency": s.currency,
                "lot_size": s.lot_size,
                "total_shares": s.total_shares,
                "circulating_shares": s.circulating_shares,
            })
        })
        .collect();

    Ok(json!({ "securities": result }))
}
