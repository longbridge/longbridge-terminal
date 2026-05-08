use anyhow::{bail, Result};
use longbridge::quote::{
    AdjustType, CalcIndex, Period, PrePostQuote, SecurityListCategory, TradeSession, TradeSessions,
};
use longbridge::Market;
use time::Date;

use serde_json::Value;

use super::{
    api::{http_get, LbQuoteApi, QuoteApi},
    output::{
        fmt_date, fmt_datetime, fmt_dec, fmt_decimal, fmt_decimal_div100, fmt_decimal_div252,
        parse_date, print_table,
    },
    OutputFormat,
};
use crate::utils::counter::symbol_to_counter_id;

/// Return the locale-appropriate display name for a security.
///
/// Uses the Chinese name for zh-CN and zh-HK locales, English name otherwise.
fn locale_name<'a>(en: &'a str, zh: &'a str) -> &'a str {
    match crate::locale::get() {
        "zh-CN" | "zh-HK" => zh,
        _ => en,
    }
}

fn parse_period(s: &str) -> Result<Period> {
    match s {
        "1m" | "minute" => Ok(Period::OneMinute),
        "5m" => Ok(Period::FiveMinute),
        "15m" => Ok(Period::FifteenMinute),
        "30m" => Ok(Period::ThirtyMinute),
        "1h" | "hour" => Ok(Period::SixtyMinute),
        "day" | "d" | "1d" => Ok(Period::Day),
        "week" | "w" => Ok(Period::Week),
        "month" | "m" | "1mo" => Ok(Period::Month),
        "year" | "y" => Ok(Period::Year),
        _ => bail!("Unknown period '{s}'. Use: 1m 5m 15m 30m 1h day week month year"),
    }
}

fn parse_adjust(s: &str) -> Result<AdjustType> {
    match s {
        "none" | "no_adjust" => Ok(AdjustType::NoAdjust),
        "forward" | "forward_adjust" => Ok(AdjustType::ForwardAdjust),
        _ => bail!("Unknown adjust type '{s}'. Use: none forward"),
    }
}

fn parse_trade_sessions(s: &str) -> Result<TradeSessions> {
    match s {
        "intraday" => Ok(TradeSessions::Intraday),
        "all" => Ok(TradeSessions::All),
        _ => bail!("Unknown session '{s}'. Use: intraday all"),
    }
}

fn fmt_trade_session(s: TradeSession) -> &'static str {
    match s {
        TradeSession::Intraday => "Intraday",
        TradeSession::Pre => "Pre",
        TradeSession::Post => "Post",
        TradeSession::Overnight => "Overnight",
    }
}

/// Calculate percentage change: `(last - prev_close) / prev_close * 100`.
/// Returns `None` when `prev_close` is zero.
fn change_pct(last: rust_decimal::Decimal, prev_close: rust_decimal::Decimal) -> Option<String> {
    if prev_close.is_zero() {
        return None;
    }
    let pct = (last - prev_close) / prev_close * rust_decimal::Decimal::ONE_HUNDRED;
    Some(format!("{pct:+.2}%"))
}

fn change_val(last: rust_decimal::Decimal, prev_close: rust_decimal::Decimal) -> String {
    let val = last - prev_close;
    format!("{val:+}")
}

fn pre_post_quote_to_json(q: &PrePostQuote) -> serde_json::Value {
    serde_json::json!({
        "last": q.last_done.to_string(),
        "timestamp": crate::utils::datetime::format_datetime(q.timestamp),
        "high": q.high.to_string(),
        "low": q.low.to_string(),
        "volume": q.volume,
        "turnover": q.turnover.to_string(),
        "prev_close": q.prev_close.to_string(),
    })
}

type CalcIndexExtractor = fn(&longbridge::quote::SecurityCalcIndex) -> String;

fn calc_index_column(key: &str) -> Option<(&'static str, CalcIndexExtractor)> {
    match key {
        "last_done" => Some(("Last Done", |r| fmt_decimal(&r.last_done))),
        "change_value" => Some(("Change Value", |r| fmt_decimal(&r.change_value))),
        "change_rate" => Some(("Change Rate", |r| fmt_decimal(&r.change_rate))),
        "volume" | "vol" => Some(("Volume", |r| {
            r.volume.map_or_else(|| "-".to_string(), |v| v.to_string())
        })),
        "turnover" => Some(("Turnover", |r| fmt_decimal(&r.turnover))),
        "ytd_change_rate" => Some(("YTD Change Rate", |r| fmt_decimal(&r.ytd_change_rate))),
        "turnover_rate" => Some(("Turnover Rate", |r| fmt_decimal(&r.turnover_rate))),
        "total_market_value" | "mktcap" => {
            Some(("Total Market Value", |r| fmt_decimal(&r.total_market_value)))
        }
        "capital_flow" => Some(("Capital Flow", |r| fmt_decimal(&r.capital_flow))),
        "amplitude" => Some(("Amplitude", |r| fmt_decimal(&r.amplitude))),
        "volume_ratio" => Some(("Volume Ratio", |r| fmt_decimal(&r.volume_ratio))),
        "pe" | "pe_ttm" => Some(("PE TTM", |r| fmt_decimal(&r.pe_ttm_ratio))),
        "pb" => Some(("PB", |r| fmt_decimal(&r.pb_ratio))),
        "dps_rate" | "dividend_yield" => Some(("DPS Rate", |r| {
            r.dividend_ratio_ttm
                .map_or_else(|| "-".to_string(), |d| d.round_dp(2).to_string())
        })),
        "five_day_change_rate" => Some(("5D Chg Rate", |r| fmt_decimal(&r.five_day_change_rate))),
        "ten_day_change_rate" => Some(("10D Chg Rate", |r| fmt_decimal(&r.ten_day_change_rate))),
        "half_year_change_rate" => Some(("6M Chg Rate", |r| fmt_decimal(&r.half_year_change_rate))),
        "five_minutes_change_rate" => Some(("5Min Chg Rate", |r| {
            fmt_decimal(&r.five_minutes_change_rate)
        })),
        "implied_volatility" | "iv" => Some(("Impl. Vol.", |r| {
            r.implied_volatility
                .map_or_else(|| "-".to_string(), |d| format!("{d:.2}%"))
        })),
        "delta" => Some(("Delta", |r| fmt_decimal(&r.delta))),
        "gamma" => Some(("Gamma", |r| fmt_decimal(&r.gamma))),
        "theta" => Some(("Theta", |r| fmt_decimal_div252(&r.theta))),
        "vega" => Some(("Vega", |r| fmt_decimal_div100(&r.vega))),
        "rho" => Some(("Rho", |r| fmt_decimal_div100(&r.rho))),
        "open_interest" | "oi" => Some(("Open Interest", |r| {
            r.open_interest
                .map_or_else(|| "-".to_string(), |v| v.to_string())
        })),
        "expiry_date" | "exp" => Some(("Expiry Date", |r| {
            r.expiry_date
                .map_or_else(|| "-".to_string(), |d| d.to_string())
        })),
        "strike_price" | "strike" => Some(("Strike Price", |r| fmt_decimal(&r.strike_price))),
        "upper_strike_price" => Some(("Upper Strike", |r| fmt_decimal(&r.upper_strike_price))),
        "lower_strike_price" => Some(("Lower Strike", |r| fmt_decimal(&r.lower_strike_price))),
        "outstanding_qty" => Some(("Outst. Qty", |r| {
            r.outstanding_qty
                .map_or_else(|| "-".to_string(), |v| v.to_string())
        })),
        "outstanding_ratio" => Some(("Outstanding Ratio", |r| fmt_decimal(&r.outstanding_ratio))),
        "premium" => Some(("Premium", |r| fmt_decimal(&r.premium))),
        "itm_otm" => Some(("ITM/OTM", |r| fmt_decimal(&r.itm_otm))),
        "warrant_delta" => Some(("Warrant Delta", |r| fmt_decimal(&r.warrant_delta))),
        "call_price" => Some(("Call Price", |r| fmt_decimal(&r.call_price))),
        "to_call_price" => Some(("To Call Price", |r| fmt_decimal(&r.to_call_price))),
        "effective_leverage" => {
            Some(("Effective Leverage", |r| fmt_decimal(&r.effective_leverage)))
        }
        "leverage_ratio" => Some(("Leverage Ratio", |r| fmt_decimal(&r.leverage_ratio))),
        "conversion_ratio" => Some(("Conversion Ratio", |r| fmt_decimal(&r.conversion_ratio))),
        "balance_point" => Some(("Balance Point", |r| fmt_decimal(&r.balance_point))),
        _ => None,
    }
}

fn parse_calc_indexes(indexes: &[String]) -> Vec<CalcIndex> {
    indexes
        .iter()
        .filter_map(|s| match s.as_str() {
            "last_done" => Some(CalcIndex::LastDone),
            "change_value" => Some(CalcIndex::ChangeValue),
            "change_rate" => Some(CalcIndex::ChangeRate),
            "volume" | "vol" => Some(CalcIndex::Volume),
            "turnover" => Some(CalcIndex::Turnover),
            "ytd_change_rate" => Some(CalcIndex::YtdChangeRate),
            "turnover_rate" => Some(CalcIndex::TurnoverRate),
            "total_market_value" | "mktcap" => Some(CalcIndex::TotalMarketValue),
            "capital_flow" => Some(CalcIndex::CapitalFlow),
            "amplitude" => Some(CalcIndex::Amplitude),
            "volume_ratio" => Some(CalcIndex::VolumeRatio),
            "pe" | "pe_ttm" => Some(CalcIndex::PeTtmRatio),
            "pb" => Some(CalcIndex::PbRatio),
            "dps_rate" | "dividend_yield" => Some(CalcIndex::DividendRatioTtm),
            "five_day_change_rate" => Some(CalcIndex::FiveDayChangeRate),
            "ten_day_change_rate" => Some(CalcIndex::TenDayChangeRate),
            "half_year_change_rate" => Some(CalcIndex::HalfYearChangeRate),
            "five_minutes_change_rate" => Some(CalcIndex::FiveMinutesChangeRate),
            "expiry_date" | "exp" => Some(CalcIndex::ExpiryDate),
            "strike_price" | "strike" => Some(CalcIndex::StrikePrice),
            "upper_strike_price" => Some(CalcIndex::UpperStrikePrice),
            "lower_strike_price" => Some(CalcIndex::LowerStrikePrice),
            "outstanding_qty" => Some(CalcIndex::OutstandingQty),
            "outstanding_ratio" => Some(CalcIndex::OutstandingRatio),
            "premium" => Some(CalcIndex::Premium),
            "itm_otm" => Some(CalcIndex::ItmOtm),
            "implied_volatility" | "iv" => Some(CalcIndex::ImpliedVolatility),
            "warrant_delta" => Some(CalcIndex::WarrantDelta),
            "call_price" => Some(CalcIndex::CallPrice),
            "to_call_price" => Some(CalcIndex::ToCallPrice),
            "effective_leverage" => Some(CalcIndex::EffectiveLeverage),
            "leverage_ratio" => Some(CalcIndex::LeverageRatio),
            "conversion_ratio" => Some(CalcIndex::ConversionRatio),
            "balance_point" => Some(CalcIndex::BalancePoint),
            "open_interest" | "oi" => Some(CalcIndex::OpenInterest),
            "delta" => Some(CalcIndex::Delta),
            "gamma" => Some(CalcIndex::Gamma),
            "theta" => Some(CalcIndex::Theta),
            "vega" => Some(CalcIndex::Vega),
            "rho" => Some(CalcIndex::Rho),
            _ => None,
        })
        .collect()
}

fn parse_market(s: &str) -> Result<longbridge::Market> {
    match s.to_uppercase().as_str() {
        "HK" => Ok(longbridge::Market::HK),
        "US" => Ok(longbridge::Market::US),
        "CN" | "SH" | "SZ" => Ok(longbridge::Market::CN),
        "SG" => Ok(longbridge::Market::SG),
        _ => bail!("Unknown market '{s}'. Use: HK US CN SG"),
    }
}

fn parse_security_list_category(_s: &str) -> SecurityListCategory {
    // Currently only Overnight is supported; expand as the SDK exposes more variants
    SecurityListCategory::Overnight
}

pub async fn cmd_quote(symbols: Vec<String>, format: &OutputFormat) -> Result<()> {
    if symbols.is_empty() {
        bail!("At least one symbol is required");
    }
    let ctx = crate::openapi::quote();
    let input = symbols.clone();
    let quotes = ctx.quote(symbols).await?;

    match format {
        OutputFormat::Json => {
            let records: Vec<serde_json::Value> = quotes
                .iter()
                .map(|q| {
                    serde_json::json!({
                        "symbol": q.symbol,
                        "last": q.last_done.to_string(),
                        "change_value": (q.last_done - q.prev_close).to_string(),
                        "change_percentage": if q.prev_close.is_zero() { None } else {
                            let pct = (q.last_done - q.prev_close) / q.prev_close * rust_decimal::Decimal::ONE_HUNDRED;
                            Some(pct.round_dp(2).to_string())
                        },
                        "prev_close": q.prev_close.to_string(),
                        "open": q.open.to_string(),
                        "high": q.high.to_string(),
                        "low": q.low.to_string(),
                        "volume": q.volume,
                        "turnover": q.turnover.to_string(),
                        "status": format!("{:?}", q.trade_status),
                        "pre_market_quote": q.pre_market_quote.as_ref().map(pre_post_quote_to_json),
                        "post_market_quote": q.post_market_quote.as_ref().map(pre_post_quote_to_json),
                        "overnight_quote": q.overnight_quote.as_ref().map(pre_post_quote_to_json),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&records)?);
        }
        OutputFormat::Pretty => {
            let headers = &[
                "Symbol",
                "Last",
                "Chg",
                "Chg%",
                "Prev Close",
                "Open",
                "High",
                "Low",
                "Volume",
                "Turnover",
                "Status",
            ];
            let rows = quotes
                .iter()
                .map(|q| {
                    vec![
                        q.symbol.clone(),
                        fmt_dec(q.last_done),
                        change_val(q.last_done, q.prev_close),
                        change_pct(q.last_done, q.prev_close).unwrap_or_default(),
                        fmt_dec(q.prev_close),
                        fmt_dec(q.open),
                        fmt_dec(q.high),
                        fmt_dec(q.low),
                        q.volume.to_string(),
                        crate::utils::number::format_financial_value(
                            &q.turnover.to_string(),
                            false,
                        ),
                        format!("{:?}", q.trade_status),
                    ]
                })
                .collect();
            print_table(headers, rows, format);

            // Show extended-hours rows when available
            let ext_rows: Vec<Vec<String>> = quotes
                .iter()
                .flat_map(|q| {
                    let sessions: &[(&str, &Option<PrePostQuote>)] = &[
                        ("Pre", &q.pre_market_quote),
                        ("Post", &q.post_market_quote),
                        ("Overnight", &q.overnight_quote),
                    ];
                    sessions
                        .iter()
                        .filter_map(|(label, opt)| {
                            opt.as_ref().map(|pmq| {
                                vec![
                                    q.symbol.clone(),
                                    label.to_string(),
                                    fmt_dec(pmq.last_done),
                                    change_val(pmq.last_done, pmq.prev_close),
                                    change_pct(pmq.last_done, pmq.prev_close).unwrap_or_default(),
                                    fmt_dec(pmq.high),
                                    fmt_dec(pmq.low),
                                    pmq.volume.to_string(),
                                    fmt_dec(pmq.prev_close),
                                    crate::utils::datetime::format_datetime(pmq.timestamp),
                                ]
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
            if !ext_rows.is_empty() {
                println!("\nExtended Hours:");
                print_table(
                    &[
                        "Symbol",
                        "Session",
                        "Last",
                        "Chg",
                        "Chg%",
                        "High",
                        "Low",
                        "Volume",
                        "Prev Close",
                        "Time",
                    ],
                    ext_rows,
                    format,
                );
            }
        }
    }
    let found: Vec<&str> = quotes.iter().map(|q| q.symbol.as_str()).collect();
    hint_symbols_do_you_mean(&input, &found);
    Ok(())
}

pub async fn cmd_depth(symbol: String, format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::quote();
    let depth = ctx.depth(symbol.clone()).await?;
    if depth.asks.is_empty() && depth.bids.is_empty() {
        hint_symbol_do_you_mean(&symbol);
    }

    match format {
        OutputFormat::Json => {
            let val = serde_json::json!({
                "symbol": symbol,
                "asks": depth.asks.iter().map(|d| serde_json::json!({
                    "position": d.position,
                    "price": d.price.map(|p| p.to_string()).unwrap_or_default(),
                    "volume": d.volume,
                    "order_num": d.order_num,
                })).collect::<Vec<_>>(),
                "bids": depth.bids.iter().map(|d| serde_json::json!({
                    "position": d.position,
                    "price": d.price.map(|p| p.to_string()).unwrap_or_default(),
                    "volume": d.volume,
                    "order_num": d.order_num,
                })).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Pretty => {
            println!("Symbol: {symbol}");
            println!("\nAsks (Sell):");
            let headers = &["Position", "Price", "Volume", "Orders"];
            let rows: Vec<Vec<String>> = depth
                .asks
                .iter()
                .map(|d| {
                    vec![
                        d.position.to_string(),
                        fmt_decimal(&d.price),
                        d.volume.to_string(),
                        d.order_num.to_string(),
                    ]
                })
                .collect();
            print_table(headers, rows, &OutputFormat::Pretty);

            println!("\nBids (Buy):");
            let rows: Vec<Vec<String>> = depth
                .bids
                .iter()
                .map(|d| {
                    vec![
                        d.position.to_string(),
                        fmt_decimal(&d.price),
                        d.volume.to_string(),
                        d.order_num.to_string(),
                    ]
                })
                .collect();
            print_table(headers, rows, &OutputFormat::Pretty);
        }
    }
    Ok(())
}

pub async fn cmd_brokers(symbol: String, format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::quote();
    let brokers = ctx.brokers(symbol.clone()).await?;

    match format {
        OutputFormat::Json => {
            let val = serde_json::json!({
                "symbol": symbol,
                "asks": brokers.ask_brokers.iter().map(|b| serde_json::json!({
                    "position": b.position,
                    "broker_ids": b.broker_ids,
                })).collect::<Vec<_>>(),
                "bids": brokers.bid_brokers.iter().map(|b| serde_json::json!({
                    "position": b.position,
                    "broker_ids": b.broker_ids,
                })).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Pretty => {
            println!("Symbol: {symbol}");
            println!("\nAsk Brokers:");
            let headers = &["Position", "Broker IDs"];
            let rows: Vec<Vec<String>> = brokers
                .ask_brokers
                .iter()
                .map(|b| {
                    vec![
                        b.position.to_string(),
                        b.broker_ids
                            .iter()
                            .map(std::string::ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", "),
                    ]
                })
                .collect();
            print_table(headers, rows, &OutputFormat::Pretty);

            println!("\nBid Brokers:");
            let rows: Vec<Vec<String>> = brokers
                .bid_brokers
                .iter()
                .map(|b| {
                    vec![
                        b.position.to_string(),
                        b.broker_ids
                            .iter()
                            .map(std::string::ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", "),
                    ]
                })
                .collect();
            print_table(headers, rows, &OutputFormat::Pretty);
        }
    }
    Ok(())
}

pub async fn cmd_trades(symbol: String, count: usize, format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::quote();
    let trades = ctx.trades(symbol.clone(), count).await?;

    let headers = &["Time", "Price", "Volume", "Direction", "Type"];
    let rows = trades
        .iter()
        .map(|t| {
            vec![
                fmt_datetime(t.timestamp),
                fmt_dec(t.price),
                t.volume.to_string(),
                format!("{:?}", t.direction),
                t.trade_type.clone(),
            ]
        })
        .collect();

    print_table(headers, rows, format);
    if trades.is_empty() {
        hint_symbol_do_you_mean(&symbol);
    }
    Ok(())
}

pub async fn cmd_intraday(symbol: String, session: &str, format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::quote();
    let trade_sessions = parse_trade_sessions(session)?;
    let lines = ctx.intraday(symbol.clone(), trade_sessions).await?;

    let headers = &["Time", "Price", "Volume", "Turnover", "Avg Price"];
    let rows = lines
        .iter()
        .map(|l| {
            vec![
                fmt_datetime(l.timestamp),
                fmt_dec(l.price),
                l.volume.to_string(),
                fmt_dec(l.turnover),
                fmt_dec(l.avg_price),
            ]
        })
        .collect();

    print_table(headers, rows, format);
    if lines.is_empty() {
        hint_symbol_do_you_mean(&symbol);
    }
    Ok(())
}

pub async fn cmd_kline(
    symbol: String,
    period: &str,
    count: usize,
    adjust: &str,
    session: &str,
    format: &OutputFormat,
) -> Result<()> {
    let ctx = crate::openapi::quote();
    let p = parse_period(period)?;
    let adj = parse_adjust(adjust)?;
    let trade_sessions = parse_trade_sessions(session)?;
    let candles = ctx
        .candlesticks(symbol.clone(), p, count, adj, trade_sessions)
        .await?;

    let show_session = matches!(trade_sessions, TradeSessions::All);
    if show_session {
        let headers = &[
            "Time", "Session", "Open", "High", "Low", "Close", "Volume", "Turnover",
        ];
        let rows = candles
            .iter()
            .map(|c| {
                vec![
                    fmt_datetime(c.timestamp),
                    fmt_trade_session(c.trade_session).to_string(),
                    fmt_dec(c.open),
                    fmt_dec(c.high),
                    fmt_dec(c.low),
                    fmt_dec(c.close),
                    c.volume.to_string(),
                    fmt_dec(c.turnover),
                ]
            })
            .collect();
        print_table(headers, rows, format);
    } else {
        let headers = &["Time", "Open", "High", "Low", "Close", "Volume", "Turnover"];
        let rows = candles
            .iter()
            .map(|c| {
                vec![
                    fmt_datetime(c.timestamp),
                    fmt_dec(c.open),
                    fmt_dec(c.high),
                    fmt_dec(c.low),
                    fmt_dec(c.close),
                    c.volume.to_string(),
                    fmt_dec(c.turnover),
                ]
            })
            .collect();
        print_table(headers, rows, format);
    }
    if candles.is_empty() {
        hint_symbol_do_you_mean(&symbol);
    }
    Ok(())
}

pub async fn cmd_kline_history(
    symbol: String,
    period: &str,
    start: Option<String>,
    end: Option<String>,
    adjust: &str,
    session: &str,
    format: &OutputFormat,
) -> Result<()> {
    let ctx = crate::openapi::quote();
    let p = parse_period(period)?;
    let adj = parse_adjust(adjust)?;
    let trade_sessions = parse_trade_sessions(session)?;
    let sym = symbol.clone();

    let candles = if let (Some(s), Some(e)) = (start, end) {
        let start_date = parse_date(&s)?;
        let end_date = parse_date(&e)?;
        ctx.history_candlesticks_by_date(
            symbol,
            p,
            adj,
            Some(start_date),
            Some(end_date),
            trade_sessions,
        )
        .await?
    } else {
        ctx.history_candlesticks_by_offset(symbol, p, adj, false, None, 100, trade_sessions)
            .await?
    };

    let show_session = matches!(trade_sessions, TradeSessions::All);
    if show_session {
        let headers = &[
            "Time", "Session", "Open", "High", "Low", "Close", "Volume", "Turnover",
        ];
        let rows = candles
            .iter()
            .map(|c| {
                vec![
                    fmt_datetime(c.timestamp),
                    fmt_trade_session(c.trade_session).to_string(),
                    fmt_dec(c.open),
                    fmt_dec(c.high),
                    fmt_dec(c.low),
                    fmt_dec(c.close),
                    c.volume.to_string(),
                    fmt_dec(c.turnover),
                ]
            })
            .collect();
        print_table(headers, rows, format);
    } else {
        let headers = &["Time", "Open", "High", "Low", "Close", "Volume", "Turnover"];
        let rows = candles
            .iter()
            .map(|c| {
                vec![
                    fmt_datetime(c.timestamp),
                    fmt_dec(c.open),
                    fmt_dec(c.high),
                    fmt_dec(c.low),
                    fmt_dec(c.close),
                    c.volume.to_string(),
                    fmt_dec(c.turnover),
                ]
            })
            .collect();
        print_table(headers, rows, format);
    }
    if candles.is_empty() {
        hint_symbol_do_you_mean(&sym);
    }
    Ok(())
}

pub async fn cmd_static(symbols: Vec<String>, format: &OutputFormat) -> Result<()> {
    if symbols.is_empty() {
        bail!("At least one symbol is required");
    }
    let ctx = crate::openapi::quote();
    let input = symbols.clone();
    let infos = ctx.static_info(symbols).await?;

    let headers = &[
        "Symbol",
        "Name",
        "Exchange",
        "Currency",
        "Lot Size",
        "Total Shares",
        "Circ. Shares",
        "EPS",
        "EPS TTM",
        "BPS",
        "Dividend",
    ];
    let rows = infos
        .iter()
        .map(|i| {
            vec![
                i.symbol.clone(),
                locale_name(&i.name_en, &i.name_cn).to_owned(),
                i.exchange.clone(),
                i.currency.clone(),
                i.lot_size.to_string(),
                i.total_shares.to_string(),
                i.circulating_shares.to_string(),
                fmt_dec(i.eps),
                fmt_dec(i.eps_ttm),
                fmt_dec(i.bps),
                fmt_dec(i.dividend_yield),
            ]
        })
        .collect();

    print_table(headers, rows, format);
    let found: Vec<&str> = infos.iter().map(|i| i.symbol.as_str()).collect();
    hint_symbols_do_you_mean(&input, &found);
    Ok(())
}

const STOCK_DEFAULT_FIELDS: &[&str] = &[
    "pe",
    "pb",
    "dps_rate",
    "turnover_rate",
    "total_market_value",
];
const OPTION_DEFAULT_FIELDS: &[&str] = &[
    "delta",
    "vega",
    "gamma",
    "theta",
    "rho",
    "implied_volatility",
    "open_interest",
];

pub async fn cmd_calc_index(
    symbols: Vec<String>,
    index: Vec<String>,
    format: &OutputFormat,
) -> Result<()> {
    if symbols.is_empty() {
        bail!("At least one symbol is required");
    }
    let ctx = crate::openapi::quote();

    // Check if using stock defaults; if results are all empty, retry with option fields
    let is_stock_default =
        index.iter().map(String::as_str).collect::<Vec<_>>() == STOCK_DEFAULT_FIELDS;

    let indexes = parse_calc_indexes(&index);
    let results = ctx.calc_indexes(symbols.clone(), indexes).await?;

    let all_empty = is_stock_default
        && results.iter().all(|r| {
            r.pe_ttm_ratio.is_none()
                && r.pb_ratio.is_none()
                && r.dividend_ratio_ttm.is_none()
                && r.turnover_rate.is_none()
                && r.total_market_value.is_none()
        });

    let (index, results) = if all_empty {
        let option_index: Vec<String> = OPTION_DEFAULT_FIELDS
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let option_indexes = parse_calc_indexes(&option_index);
        let results = ctx.calc_indexes(symbols.clone(), option_indexes).await?;
        (option_index, results)
    } else {
        (index, results)
    };

    // Deduplicate columns (e.g. "pe" and "pe_ttm" map to the same field)
    let columns: Vec<(&str, &str, CalcIndexExtractor)> = {
        let mut seen = std::collections::HashSet::new();
        index
            .iter()
            .filter_map(|key| {
                calc_index_column(key).map(|(header, extract)| (key.as_str(), header, extract))
            })
            .filter(|(_, header, _)| seen.insert(*header))
            .collect()
    };

    match format {
        OutputFormat::Json => {
            let records: Vec<serde_json::Value> = results
                .iter()
                .map(|r| {
                    let mut map = serde_json::Map::new();
                    map.insert(
                        "symbol".to_string(),
                        serde_json::Value::String(r.symbol.clone()),
                    );
                    for (key, _, extract) in &columns {
                        map.insert(key.to_string(), serde_json::Value::String(extract(r)));
                    }
                    serde_json::Value::Object(map)
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&records)?);
        }
        OutputFormat::Pretty => {
            let mut headers = vec!["Symbol"];
            headers.extend(columns.iter().map(|(_, h, _)| *h));
            let rows = results
                .iter()
                .map(|r| {
                    let mut row = vec![r.symbol.clone()];
                    row.extend(columns.iter().map(|(_, _, extract)| extract(r)));
                    row
                })
                .collect();
            print_table(&headers, rows, format);
        }
    }
    Ok(())
}

pub async fn cmd_capital_flow(symbol: String, format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::quote();
    let flows = ctx.capital_flow(symbol).await?;

    let headers = &["Time", "Inflow"];
    let rows = flows
        .iter()
        .map(|f| vec![fmt_datetime(f.timestamp), fmt_dec(f.inflow)])
        .collect();

    print_table(headers, rows, format);
    Ok(())
}

pub async fn cmd_capital_dist(symbol: String, format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::quote();
    let dist = ctx.capital_distribution(symbol.clone()).await?;

    match format {
        OutputFormat::Json => {
            let val = serde_json::json!({
                "symbol": symbol,
                "timestamp": crate::utils::datetime::format_datetime(dist.timestamp),
                "capital_in": {
                    "large": dist.capital_in.large.to_string(),
                    "medium": dist.capital_in.medium.to_string(),
                    "small": dist.capital_in.small.to_string(),
                },
                "capital_out": {
                    "large": dist.capital_out.large.to_string(),
                    "medium": dist.capital_out.medium.to_string(),
                    "small": dist.capital_out.small.to_string(),
                },
            });
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Pretty => {
            println!("Symbol: {}  Time: {}", symbol, fmt_datetime(dist.timestamp));
            let headers = &["Direction", "Large", "Medium", "Small"];
            let rows = vec![
                vec![
                    "Inflow".to_string(),
                    fmt_dec(dist.capital_in.large),
                    fmt_dec(dist.capital_in.medium),
                    fmt_dec(dist.capital_in.small),
                ],
                vec![
                    "Outflow".to_string(),
                    fmt_dec(dist.capital_out.large),
                    fmt_dec(dist.capital_out.medium),
                    fmt_dec(dist.capital_out.small),
                ],
            ];
            print_table(headers, rows, &OutputFormat::Pretty);
        }
    }
    Ok(())
}

pub async fn cmd_market_temp(
    market: &str,
    history: bool,
    start: Option<String>,
    end: Option<String>,
    _granularity: &str,
    format: &OutputFormat,
) -> Result<()> {
    let ctx = crate::openapi::quote();
    let m = parse_market(market)?;

    if history {
        let now = time::OffsetDateTime::now_utc().date();
        let start_date = start.as_deref().map(parse_date).transpose()?.unwrap_or(now);
        let end_date = end.as_deref().map(parse_date).transpose()?.unwrap_or(now);
        let resp = ctx
            .history_market_temperature(m, start_date, end_date)
            .await?;
        let headers = &[
            "Time",
            "Temperature",
            "Valuation",
            "Sentiment",
            "Description",
        ];
        let rows = resp
            .records
            .iter()
            .map(|t| {
                vec![
                    fmt_datetime(t.timestamp),
                    t.temperature.to_string(),
                    t.valuation.to_string(),
                    t.sentiment.to_string(),
                    t.description.clone(),
                ]
            })
            .collect();
        print_table(headers, rows, format);
    } else {
        let temp = ctx.market_temperature(m).await?;
        let headers = &["Field", "Value"];
        let rows = vec![
            vec!["Market".to_string(), market.to_uppercase()],
            vec!["Temperature".to_string(), temp.temperature.to_string()],
            vec!["Description".to_string(), temp.description.clone()],
            vec!["Valuation".to_string(), temp.valuation.to_string()],
            vec!["Sentiment".to_string(), temp.sentiment.to_string()],
        ];
        print_table(headers, rows, format);
    }
    Ok(())
}

pub async fn cmd_trading_session(format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::quote();
    let sessions = ctx.trading_session().await?;

    match format {
        OutputFormat::Json => {
            let val: Vec<_> = sessions
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "market": format!("{:?}", s.market),
                        "sessions": s.trade_sessions.iter().map(|ts| serde_json::json!({
                            "open": ts.begin_time.to_string(),
                            "close": ts.end_time.to_string(),
                            "session": format!("{:?}", ts.trade_session),
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Pretty => {
            let headers = &["Market", "Session", "Open", "Close"];
            let mut rows = vec![];
            for s in &sessions {
                for ts in &s.trade_sessions {
                    rows.push(vec![
                        format!("{:?}", s.market),
                        format!("{:?}", ts.trade_session),
                        ts.begin_time.to_string(),
                        ts.end_time.to_string(),
                    ]);
                }
            }
            print_table(headers, rows, &OutputFormat::Pretty);
        }
    }
    Ok(())
}

pub async fn cmd_trading_days(
    market: &str,
    start: Option<String>,
    end: Option<String>,
    format: &OutputFormat,
) -> Result<()> {
    let ctx = crate::openapi::quote();
    let m = parse_market(market)?;

    let now = time::OffsetDateTime::now_utc().date();
    let start_date = start.as_deref().map(parse_date).transpose()?.unwrap_or(now);
    let end_date = end
        .as_deref()
        .map(parse_date)
        .transpose()?
        .unwrap_or_else(|| {
            start_date
                .checked_add(time::Duration::days(30))
                .unwrap_or(start_date)
        });

    let days = ctx.trading_days(m, start_date, end_date).await?;

    match format {
        OutputFormat::Json => {
            let val = serde_json::json!({
                "trading_days": days.trading_days.iter().map(|d| fmt_date(*d)).collect::<Vec<_>>(),
                "half_trading_days": days.half_trading_days.iter().map(|d| fmt_date(*d)).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Pretty => {
            println!("Trading days:");
            for chunk in days.trading_days.chunks(7) {
                println!(
                    "  {}",
                    chunk
                        .iter()
                        .map(|d| fmt_date(*d))
                        .collect::<Vec<_>>()
                        .join("  ")
                );
            }
            if !days.half_trading_days.is_empty() {
                println!("\nHalf trading days:");
                for d in &days.half_trading_days {
                    println!("  {}", fmt_date(*d));
                }
            }
        }
    }
    Ok(())
}

pub async fn cmd_security_list(market: &str, category: &str, format: &OutputFormat) -> Result<()> {
    if !matches!(market.to_uppercase().as_str(), "US") {
        bail!("Only US market is supported for security-list (Longbridge API only exposes the Overnight category)");
    }
    let ctx = crate::openapi::quote();
    let m = parse_market(market)?;
    let cat = parse_security_list_category(category);
    let securities = ctx.security_list(m, cat).await?;

    let headers = &["Symbol", "Name"];
    let rows = securities
        .iter()
        .map(|s| {
            vec![
                s.symbol.clone(),
                locale_name(&s.name_en, &s.name_cn).to_owned(),
            ]
        })
        .collect();

    print_table(headers, rows, format);
    Ok(())
}

pub async fn cmd_participants(format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::quote();
    let participants = ctx.participants().await?;

    let headers = &["Broker ID", "Name EN", "Name CN"];
    let rows = participants
        .iter()
        .map(|p| {
            vec![
                p.broker_ids
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
                p.name_en.clone(),
                p.name_cn.clone(),
            ]
        })
        .collect();

    print_table(headers, rows, format);
    Ok(())
}

pub async fn cmd_subscriptions(format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::quote();
    let subs = ctx.subscriptions().await?;

    let headers = &["Symbol", "Sub Types", "Candlestick Periods"];
    let rows = subs
        .iter()
        .map(|s| {
            vec![
                s.symbol.clone(),
                format!("{:?}", s.sub_types),
                s.candlesticks
                    .iter()
                    .map(|p| format!("{p:?}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ]
        })
        .collect();

    print_table(headers, rows, format);
    Ok(())
}

pub async fn cmd_option_quote(symbols: Vec<String>, format: &OutputFormat) -> Result<()> {
    if symbols.is_empty() {
        bail!("At least one symbol is required");
    }
    let ctx = crate::openapi::quote();
    let quotes = ctx.option_quote(symbols).await?;

    match format {
        OutputFormat::Json => {
            let records: Vec<serde_json::Value> = quotes
                .iter()
                .map(|q| {
                    serde_json::json!({
                        "symbol": q.symbol,
                        "last": q.last_done.to_string(),
                        "prev_close": q.prev_close.to_string(),
                        "open": q.open.to_string(),
                        "high": q.high.to_string(),
                        "low": q.low.to_string(),
                        "timestamp": crate::utils::datetime::format_datetime(q.timestamp),
                        "volume": q.volume,
                        "turnover": q.turnover.to_string(),
                        "trade_status": format!("{:?}", q.trade_status),
                        "implied_volatility": q.implied_volatility.to_string(),
                        "open_interest": q.open_interest,
                        "expiry_date": fmt_date(q.expiry_date),
                        "strike_price": q.strike_price.to_string(),
                        "contract_multiplier": q.contract_multiplier.to_string(),
                        "contract_type": format!("{:?}", q.contract_type),
                        "contract_size": q.contract_size.to_string(),
                        "direction": format!("{:?}", q.direction),
                        "historical_volatility": q.historical_volatility.to_string(),
                        "underlying_symbol": q.underlying_symbol,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&records)?);
        }
        OutputFormat::Pretty => {
            let headers = &[
                "Symbol",
                "Last",
                "Prev Close",
                "Open",
                "High",
                "Low",
                "Volume",
                "Turnover",
                "Impl Vol",
                "Hist Vol",
                "OI",
                "Strike",
                "Expiry",
                "Type",
                "Direction",
                "Underlying",
            ];
            let rows = quotes
                .iter()
                .map(|q| {
                    vec![
                        q.symbol.clone(),
                        fmt_dec(q.last_done),
                        fmt_dec(q.prev_close),
                        fmt_dec(q.open),
                        fmt_dec(q.high),
                        fmt_dec(q.low),
                        q.volume.to_string(),
                        fmt_dec(q.turnover),
                        fmt_dec(q.implied_volatility),
                        fmt_dec(q.historical_volatility),
                        q.open_interest.to_string(),
                        fmt_dec(q.strike_price),
                        fmt_date(q.expiry_date),
                        format!("{:?}", q.contract_type),
                        format!("{:?}", q.direction),
                        q.underlying_symbol.clone(),
                    ]
                })
                .collect();
            print_table(headers, rows, format);
        }
    }
    Ok(())
}

pub async fn cmd_option_chain(
    symbol: String,
    date: Option<String>,
    format: &OutputFormat,
) -> Result<()> {
    let api = LbQuoteApi::new(crate::openapi::quote());
    run_option_chain(&api, symbol, date, format).await
}

pub async fn cmd_warrant_quote(symbols: Vec<String>, format: &OutputFormat) -> Result<()> {
    if symbols.is_empty() {
        bail!("At least one symbol is required");
    }
    let ctx = crate::openapi::quote();
    let quotes = ctx.warrant_quote(symbols).await?;

    let headers = &[
        "Symbol",
        "Last",
        "Prev Close",
        "Implied Vol",
        "Expiry",
        "Type",
    ];
    let rows = quotes
        .iter()
        .map(|q| {
            vec![
                q.symbol.clone(),
                fmt_dec(q.last_done),
                fmt_dec(q.prev_close),
                fmt_dec(q.implied_volatility),
                fmt_date(q.expiry_date),
                format!("{:?}", q.category),
            ]
        })
        .collect();

    print_table(headers, rows, format);
    Ok(())
}

pub async fn cmd_warrant_list(symbol: String, format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::quote();
    let warrants = ctx
        .warrant_list(
            symbol,
            longbridge::quote::WarrantSortBy::LastDone,
            longbridge::quote::SortOrderType::Descending,
            None,
            None,
            None,
            None,
            None,
        )
        .await?;

    let headers = &["Symbol", "Name", "Last", "Leverage Ratio", "Expiry", "Type"];
    let rows = warrants
        .iter()
        .map(|w| {
            vec![
                w.symbol.clone(),
                w.name.clone(),
                fmt_dec(w.last_done),
                fmt_dec(w.leverage_ratio),
                fmt_date(w.expiry_date),
                format!("{:?}", w.warrant_type),
            ]
        })
        .collect();

    print_table(headers, rows, format);
    Ok(())
}

pub async fn cmd_warrant_issuers(format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::quote();
    let issuers = ctx.warrant_issuers().await?;

    let headers = &["ID", "Name EN", "Name CN"];
    let rows = issuers
        .iter()
        .map(|i| {
            vec![
                i.issuer_id.to_string(),
                i.name_en.clone(),
                i.name_cn.clone(),
            ]
        })
        .collect();

    print_table(headers, rows, format);
    Ok(())
}

// ─── Testable run_* functions ─────────────────────────────────────────────────

/// Print a "did you mean …?" hint to stderr when a symbol returns no data.
///
/// Handles three cases:
/// - No market suffix at all (e.g. `TSLA`, `700`) — suggests adding `.US` or `.HK`.
/// - US symbol missing the leading dot for an index (e.g. `DJI.US`) — suggests `.DJI.US`.
/// - Other malformed symbols — generic format reminder.
fn hint_symbol_do_you_mean(symbol: &str) {
    eprintln!();
    if let Some((code, market)) = symbol.rsplit_once('.') {
        // Has a market suffix — check for missing leading dot on US indexes.
        if market.eq_ignore_ascii_case("US") && !code.starts_with('.') {
            eprintln!(
                "Hint: no data for \"{symbol}\". Did you mean \".{code}.{}\"? (US market indexes require a leading dot, e.g. .DJI.US, .VIX.US)",
                market.to_uppercase()
            );
        }
    } else {
        // No market suffix — guess from the code pattern.

        if symbol.chars().all(|c| c.is_ascii_digit()) {
            // All digits are almost always HK stocks (e.g. 700, 9988).
            eprintln!(
                "Hint: no data for \"{symbol}\". Did you mean \"{symbol}.HK\"? \n\
                Symbols require a market suffix, e.g. TSLA.US, 700.HK, .DJI.US"
            );
        } else {
            // Letters: could be a US stock (TSLA.US) or a US index (.VIX.US).
            eprintln!(
                "Hint: no data for \"{symbol}\". Did you mean \"{symbol}.US\" or \".{symbol}.US\" (if it's a US market index)? \n\
                Symbols require a market suffix, e.g. TSLA.US, 700.HK, .DJI.US"
            );
        }
    }
}

/// For multi-symbol queries: print hints for each input symbol that is absent
/// from the returned results.
fn hint_symbols_do_you_mean(queried: &[String], found_symbols: &[&str]) {
    let found: std::collections::HashSet<&str> = found_symbols.iter().copied().collect();
    for sym in queried {
        if !found.contains(sym.as_str()) {
            hint_symbol_do_you_mean(sym);
        }
    }
}

pub async fn run_quote(
    api: &dyn QuoteApi,
    symbols: Vec<String>,
    format: &OutputFormat,
) -> Result<()> {
    if symbols.is_empty() {
        bail!("At least one symbol is required");
    }
    let input = symbols.clone();
    let quotes = api.quote(symbols).await?;
    let headers = &[
        "Symbol",
        "Last",
        "Prev Close",
        "Open",
        "High",
        "Low",
        "Volume",
        "Turnover",
        "Status",
    ];
    let rows = quotes
        .iter()
        .map(|q| {
            vec![
                q.symbol.clone(),
                fmt_dec(q.last_done),
                fmt_dec(q.prev_close),
                fmt_dec(q.open),
                fmt_dec(q.high),
                fmt_dec(q.low),
                q.volume.to_string(),
                fmt_dec(q.turnover),
                format!("{:?}", q.trade_status),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    let found: Vec<&str> = quotes.iter().map(|q| q.symbol.as_str()).collect();
    hint_symbols_do_you_mean(&input, &found);
    Ok(())
}

pub async fn run_depth(api: &dyn QuoteApi, symbol: String, format: &OutputFormat) -> Result<()> {
    let depth = api.depth(symbol.clone()).await?;
    match format {
        OutputFormat::Json => {
            let val = serde_json::json!({
                "symbol": symbol,
                "asks": depth.asks.iter().map(|d| serde_json::json!({"position": d.position, "price": fmt_decimal(&d.price), "volume": d.volume})).collect::<Vec<_>>(),
                "bids": depth.bids.iter().map(|d| serde_json::json!({"position": d.position, "price": fmt_decimal(&d.price), "volume": d.volume})).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Pretty => {
            let headers = &["Position", "Price", "Volume", "Orders"];
            let ask_rows: Vec<Vec<String>> = depth
                .asks
                .iter()
                .map(|d| {
                    vec![
                        d.position.to_string(),
                        fmt_decimal(&d.price),
                        d.volume.to_string(),
                        d.order_num.to_string(),
                    ]
                })
                .collect();
            let bid_rows: Vec<Vec<String>> = depth
                .bids
                .iter()
                .map(|d| {
                    vec![
                        d.position.to_string(),
                        fmt_decimal(&d.price),
                        d.volume.to_string(),
                        d.order_num.to_string(),
                    ]
                })
                .collect();
            println!("Asks:");
            print_table(headers, ask_rows, &OutputFormat::Pretty);
            println!("Bids:");
            print_table(headers, bid_rows, &OutputFormat::Pretty);
        }
    }
    Ok(())
}

pub async fn run_brokers(api: &dyn QuoteApi, symbol: String, format: &OutputFormat) -> Result<()> {
    let brokers = api.brokers(symbol.clone()).await?;
    match format {
        OutputFormat::Json => {
            let val = serde_json::json!({
                "symbol": symbol,
                "asks": brokers.ask_brokers.iter().map(|b| serde_json::json!({"position": b.position, "broker_ids": b.broker_ids})).collect::<Vec<_>>(),
                "bids": brokers.bid_brokers.iter().map(|b| serde_json::json!({"position": b.position, "broker_ids": b.broker_ids})).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Pretty => {
            let headers = &["Position", "Broker IDs"];
            let ask_rows: Vec<Vec<String>> = brokers
                .ask_brokers
                .iter()
                .map(|b| {
                    vec![
                        b.position.to_string(),
                        b.broker_ids
                            .iter()
                            .map(std::string::ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", "),
                    ]
                })
                .collect();
            let bid_rows: Vec<Vec<String>> = brokers
                .bid_brokers
                .iter()
                .map(|b| {
                    vec![
                        b.position.to_string(),
                        b.broker_ids
                            .iter()
                            .map(std::string::ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", "),
                    ]
                })
                .collect();
            println!("Ask Brokers:");
            print_table(headers, ask_rows, &OutputFormat::Pretty);
            println!("Bid Brokers:");
            print_table(headers, bid_rows, &OutputFormat::Pretty);
        }
    }
    Ok(())
}

pub async fn run_trades(
    api: &dyn QuoteApi,
    symbol: String,
    count: usize,
    format: &OutputFormat,
) -> Result<()> {
    let trades = api.trades(symbol.clone(), count).await?;
    let headers = &["Time", "Price", "Volume", "Direction", "Type"];
    let rows = trades
        .iter()
        .map(|t| {
            vec![
                fmt_datetime(t.timestamp),
                fmt_dec(t.price),
                t.volume.to_string(),
                format!("{:?}", t.direction),
                t.trade_type.clone(),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    if trades.is_empty() {
        hint_symbol_do_you_mean(&symbol);
    }
    Ok(())
}

pub async fn run_intraday(api: &dyn QuoteApi, symbol: String, format: &OutputFormat) -> Result<()> {
    let lines = api.intraday(symbol.clone()).await?;
    let headers = &["Time", "Price", "Volume", "Turnover", "Avg Price"];
    let rows = lines
        .iter()
        .map(|l| {
            vec![
                fmt_datetime(l.timestamp),
                fmt_dec(l.price),
                l.volume.to_string(),
                fmt_dec(l.turnover),
                fmt_dec(l.avg_price),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    if lines.is_empty() {
        hint_symbol_do_you_mean(&symbol);
    }
    Ok(())
}

pub async fn run_kline(
    api: &dyn QuoteApi,
    symbol: String,
    period: Period,
    count: usize,
    adjust: AdjustType,
    format: &OutputFormat,
) -> Result<()> {
    let candles = api
        .candlesticks(symbol.clone(), period, count, adjust)
        .await?;
    let headers = &["Time", "Open", "High", "Low", "Close", "Volume", "Turnover"];
    let rows = candles
        .iter()
        .map(|c| {
            vec![
                fmt_datetime(c.timestamp),
                fmt_dec(c.open),
                fmt_dec(c.high),
                fmt_dec(c.low),
                fmt_dec(c.close),
                c.volume.to_string(),
                fmt_dec(c.turnover),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    if candles.is_empty() {
        hint_symbol_do_you_mean(&symbol);
    }
    Ok(())
}

pub async fn run_kline_history(
    api: &dyn QuoteApi,
    symbol: String,
    period: Period,
    adjust: AdjustType,
    start: Option<Date>,
    end: Option<Date>,
    format: &OutputFormat,
) -> Result<()> {
    let candles = if let (Some(s), Some(e)) = (start, end) {
        api.history_candlesticks_by_date(symbol.clone(), period, adjust, Some(s), Some(e))
            .await?
    } else {
        api.history_candlesticks_by_offset(symbol.clone(), period, adjust, 100)
            .await?
    };
    let headers = &["Time", "Open", "High", "Low", "Close", "Volume", "Turnover"];
    let rows = candles
        .iter()
        .map(|c| {
            vec![
                fmt_datetime(c.timestamp),
                fmt_dec(c.open),
                fmt_dec(c.high),
                fmt_dec(c.low),
                fmt_dec(c.close),
                c.volume.to_string(),
                fmt_dec(c.turnover),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    if candles.is_empty() {
        hint_symbol_do_you_mean(&symbol);
    }
    Ok(())
}

pub async fn run_static(
    api: &dyn QuoteApi,
    symbols: Vec<String>,
    format: &OutputFormat,
) -> Result<()> {
    if symbols.is_empty() {
        bail!("At least one symbol is required");
    }
    let input = symbols.clone();
    let infos = api.static_info(symbols).await?;
    let headers = &["Symbol", "Name", "Exchange", "Currency", "Lot Size"];
    let rows = infos
        .iter()
        .map(|i| {
            vec![
                i.symbol.clone(),
                locale_name(&i.name_en, &i.name_cn).to_owned(),
                i.exchange.clone(),
                i.currency.clone(),
                i.lot_size.to_string(),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    let found: Vec<&str> = infos.iter().map(|i| i.symbol.as_str()).collect();
    hint_symbols_do_you_mean(&input, &found);
    Ok(())
}

pub async fn run_calc_index(
    api: &dyn QuoteApi,
    symbols: Vec<String>,
    indexes: Vec<CalcIndex>,
    format: &OutputFormat,
) -> Result<()> {
    if symbols.is_empty() {
        bail!("At least one symbol is required");
    }
    let results = api.calc_indexes(symbols, indexes).await?;
    let headers = &["Symbol", "Last Done", "Change Rate", "PE TTM", "PB"];
    let rows = results
        .iter()
        .map(|r| {
            vec![
                r.symbol.clone(),
                fmt_decimal(&r.last_done),
                fmt_decimal(&r.change_rate),
                fmt_decimal(&r.pe_ttm_ratio),
                fmt_decimal(&r.pb_ratio),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_capital_flow(
    api: &dyn QuoteApi,
    symbol: String,
    format: &OutputFormat,
) -> Result<()> {
    let flows = api.capital_flow(symbol).await?;
    let headers = &["Time", "Inflow"];
    let rows = flows
        .iter()
        .map(|f| vec![fmt_datetime(f.timestamp), fmt_dec(f.inflow)])
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_capital_dist(
    api: &dyn QuoteApi,
    symbol: String,
    format: &OutputFormat,
) -> Result<()> {
    let dist = api.capital_distribution(symbol).await?;
    let headers = &["Direction", "Large", "Medium", "Small"];
    let rows = vec![
        vec![
            "Inflow".to_string(),
            fmt_dec(dist.capital_in.large),
            fmt_dec(dist.capital_in.medium),
            fmt_dec(dist.capital_in.small),
        ],
        vec![
            "Outflow".to_string(),
            fmt_dec(dist.capital_out.large),
            fmt_dec(dist.capital_out.medium),
            fmt_dec(dist.capital_out.small),
        ],
    ];
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_market_temp_current(
    api: &dyn QuoteApi,
    market: Market,
    format: &OutputFormat,
) -> Result<()> {
    let temp = api.market_temperature(market).await?;
    let headers = &["Field", "Value"];
    let rows = vec![
        vec!["Temperature".to_string(), temp.temperature.to_string()],
        vec!["Description".to_string(), temp.description.clone()],
        vec!["Valuation".to_string(), temp.valuation.to_string()],
        vec!["Sentiment".to_string(), temp.sentiment.to_string()],
    ];
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_market_temp_history(
    api: &dyn QuoteApi,
    market: Market,
    start: Date,
    end: Date,
    format: &OutputFormat,
) -> Result<()> {
    let resp = api.history_market_temperature(market, start, end).await?;
    let headers = &[
        "Time",
        "Temperature",
        "Valuation",
        "Sentiment",
        "Description",
    ];
    let rows = resp
        .records
        .iter()
        .map(|t| {
            vec![
                fmt_datetime(t.timestamp),
                t.temperature.to_string(),
                t.valuation.to_string(),
                t.sentiment.to_string(),
                t.description.clone(),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_trading_session(api: &dyn QuoteApi, format: &OutputFormat) -> Result<()> {
    let sessions = api.trading_session().await?;
    let headers = &["Market", "Session", "Open", "Close"];
    let mut rows = vec![];
    for s in &sessions {
        for ts in &s.trade_sessions {
            rows.push(vec![
                format!("{:?}", s.market),
                format!("{:?}", ts.trade_session),
                ts.begin_time.to_string(),
                ts.end_time.to_string(),
            ]);
        }
    }
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_trading_days(
    api: &dyn QuoteApi,
    market: Market,
    start: Date,
    end: Date,
    format: &OutputFormat,
) -> Result<()> {
    let days = api.trading_days(market, start, end).await?;
    let headers = &["Trading Days", "Half Trading Days"];
    let rows = vec![vec![
        days.trading_days
            .iter()
            .map(|d| fmt_date(*d))
            .collect::<Vec<_>>()
            .join(", "),
        days.half_trading_days
            .iter()
            .map(|d| fmt_date(*d))
            .collect::<Vec<_>>()
            .join(", "),
    ]];
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_security_list(
    api: &dyn QuoteApi,
    market: Market,
    format: &OutputFormat,
) -> Result<()> {
    let securities = api.security_list(market).await?;
    let headers = &["Symbol", "Name"];
    let rows = securities
        .iter()
        .map(|s| {
            vec![
                s.symbol.clone(),
                locale_name(&s.name_en, &s.name_cn).to_owned(),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_participants(api: &dyn QuoteApi, format: &OutputFormat) -> Result<()> {
    let participants = api.participants().await?;
    let headers = &["Broker ID", "Name EN", "Name CN"];
    let rows = participants
        .iter()
        .map(|p| {
            vec![
                p.broker_ids
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
                p.name_en.clone(),
                p.name_cn.clone(),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_subscriptions(api: &dyn QuoteApi, format: &OutputFormat) -> Result<()> {
    let subs = api.subscriptions().await?;
    let headers = &["Symbol", "Sub Types", "Candlestick Periods"];
    let rows = subs
        .iter()
        .map(|s| {
            vec![
                s.symbol.clone(),
                format!("{:?}", s.sub_types),
                s.candlesticks
                    .iter()
                    .map(|p| format!("{p:?}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_option_quote(
    api: &dyn QuoteApi,
    symbols: Vec<String>,
    format: &OutputFormat,
) -> Result<()> {
    if symbols.is_empty() {
        bail!("At least one symbol is required");
    }
    let quotes = api.option_quote(symbols).await?;
    let headers = &["Symbol", "Last", "Strike", "Expiry", "Type"];
    let rows = quotes
        .iter()
        .map(|q| {
            vec![
                q.symbol.clone(),
                fmt_dec(q.last_done),
                fmt_dec(q.strike_price),
                fmt_date(q.expiry_date),
                format!("{:?}", q.contract_type),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_option_chain(
    api: &dyn QuoteApi,
    symbol: String,
    date: Option<String>,
    format: &OutputFormat,
) -> Result<()> {
    match date {
        Some(date_str) => {
            run_option_chain_strikes(api, symbol, parse_date(&date_str)?, format).await
        }
        None => run_option_chain_dates(api, symbol, format).await,
    }
}

pub async fn run_option_chain_dates(
    api: &dyn QuoteApi,
    symbol: String,
    format: &OutputFormat,
) -> Result<()> {
    let dates = api.option_chain_expiry_date_list(symbol).await?;
    let headers = &["Expiry Date"];
    let rows = dates.iter().map(|d| vec![fmt_date(*d)]).collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_option_chain_strikes(
    api: &dyn QuoteApi,
    symbol: String,
    expiry_date: Date,
    format: &OutputFormat,
) -> Result<()> {
    let strikes = api.option_chain_info_by_date(symbol, expiry_date).await?;

    let all_symbols: Vec<String> = strikes
        .iter()
        .flat_map(|s| [s.call_symbol.clone(), s.put_symbol.clone()])
        .filter(|sym| !sym.is_empty())
        .collect();

    let quotes = if all_symbols.is_empty() {
        vec![]
    } else {
        api.option_quote(all_symbols).await.unwrap_or_default()
    };
    let quote_map: std::collections::HashMap<&str, &longbridge::quote::OptionQuote> =
        quotes.iter().map(|q| (q.symbol.as_str(), q)).collect();

    if quote_map.is_empty() {
        let headers = &["Strike", "Call Symbol", "Put Symbol", "Standard"];
        let rows = strikes
            .iter()
            .map(|s| {
                vec![
                    fmt_dec(s.price),
                    s.call_symbol.clone(),
                    s.put_symbol.clone(),
                    s.standard.to_string(),
                ]
            })
            .collect();
        print_table(headers, rows, format);
    } else {
        let headers = &[
            "Strike",
            "Call Last",
            "Call IV",
            "Call Vol",
            "Put Last",
            "Put IV",
            "Put Vol",
            "Standard",
        ];
        let rows = strikes
            .iter()
            .map(|s| {
                let call = quote_map.get(s.call_symbol.as_str());
                let put = quote_map.get(s.put_symbol.as_str());
                vec![
                    fmt_dec(s.price),
                    call.map_or_else(|| "-".to_string(), |q| fmt_dec(q.last_done)),
                    call.map_or_else(|| "-".to_string(), |q| fmt_dec(q.implied_volatility)),
                    call.map_or_else(|| "-".to_string(), |q| q.volume.to_string()),
                    put.map_or_else(|| "-".to_string(), |q| fmt_dec(q.last_done)),
                    put.map_or_else(|| "-".to_string(), |q| fmt_dec(q.implied_volatility)),
                    put.map_or_else(|| "-".to_string(), |q| q.volume.to_string()),
                    s.standard.to_string(),
                ]
            })
            .collect();
        print_table(headers, rows, format);
    }
    Ok(())
}

pub async fn run_warrant_quote(
    api: &dyn QuoteApi,
    symbols: Vec<String>,
    format: &OutputFormat,
) -> Result<()> {
    if symbols.is_empty() {
        bail!("At least one symbol is required");
    }
    let quotes = api.warrant_quote(symbols).await?;
    let headers = &[
        "Symbol",
        "Last",
        "Prev Close",
        "Implied Vol",
        "Expiry",
        "Type",
    ];
    let rows = quotes
        .iter()
        .map(|q| {
            vec![
                q.symbol.clone(),
                fmt_dec(q.last_done),
                fmt_dec(q.prev_close),
                fmt_dec(q.implied_volatility),
                fmt_date(q.expiry_date),
                format!("{:?}", q.category),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_warrant_list(
    api: &dyn QuoteApi,
    symbol: String,
    format: &OutputFormat,
) -> Result<()> {
    let warrants = api.warrant_list(symbol).await?;
    let headers = &["Symbol", "Name", "Last", "Leverage Ratio", "Expiry", "Type"];
    let rows = warrants
        .iter()
        .map(|w| {
            vec![
                w.symbol.clone(),
                w.name.clone(),
                fmt_dec(w.last_done),
                fmt_dec(w.leverage_ratio),
                fmt_date(w.expiry_date),
                format!("{:?}", w.warrant_type),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_warrant_issuers(api: &dyn QuoteApi, format: &OutputFormat) -> Result<()> {
    let issuers = api.warrant_issuers().await?;
    let headers = &["ID", "Name EN", "Name CN"];
    let rows = issuers
        .iter()
        .map(|i| {
            vec![
                i.issuer_id.to_string(),
                i.name_en.clone(),
                i.name_cn.clone(),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

// ── Pending commands ─────────────────────────────────────────────────────────

/// Format share count with thousands separator (no sign)
fn fmt_shares(raw: &str) -> String {
    let v: f64 = raw.parse().unwrap_or(0.0);
    if v == 0.0 {
        return "0".to_string();
    }
    format_with_commas(v.abs() as i64)
}

/// Format share change with sign and thousands separator
fn fmt_shares_chg(raw: &str) -> String {
    let v: f64 = raw.parse().unwrap_or(0.0);
    if v == 0.0 {
        return "0".to_string();
    }
    let formatted = format_with_commas(v.abs() as i64);
    if v > 0.0 {
        format!("+{formatted}")
    } else {
        format!("-{formatted}")
    }
}

fn format_with_commas(n: i64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

/// Format unix timestamp string to ISO 8601 date
fn fmt_ts(raw: &str) -> String {
    raw.parse::<i64>()
        .map_or_else(|_| raw.to_string(), crate::utils::datetime::format_date)
}

/// Format unix timestamp string to ISO 8601 datetime (HH:MM)
fn fmt_ts_time(raw: &str) -> String {
    raw.parse::<i64>().map_or_else(
        |_| raw.to_string(),
        |ts| {
            use time::OffsetDateTime;
            match OffsetDateTime::from_unix_timestamp(ts) {
                Ok(dt) => format!(
                    "{:04}-{:02}-{:02}T{:02}:{:02}Z",
                    dt.year(),
                    dt.month() as u8,
                    dt.day(),
                    dt.hour(),
                    dt.minute()
                ),
                Err(_) => ts.to_string(),
            }
        },
    )
}

/// Format premium rate: -0.182435 → -18.24%
fn fmt_premium(raw: &str) -> String {
    raw.parse::<f64>()
        .map_or_else(|_| raw.to_string(), |v| format!("{:.2}%", v * 100.0))
}

fn val_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "-".to_owned(),
        other => other.to_string(),
    }
}

fn print_json(data: &Value) {
    println!("{}", serde_json::to_string_pretty(data).unwrap_or_default());
}

/// Convert index symbol to IX/ prefix `counter_id` (e.g. `HSI.HK` → `IX/HK/HSI`,
/// `.DJI.US` → `IX/US/DJI`).
fn index_symbol_to_counter_id(symbol: &str) -> String {
    if let Some((code, market)) = symbol.rsplit_once('.') {
        let market = market.to_uppercase();
        // Strip leading dot from code (e.g. `.DJI.US` → code part is `.DJI`, strip to `DJI`)
        let code = code.trim_start_matches('.');
        format!("IX/{market}/{code}")
    } else {
        symbol.to_string()
    }
}

pub async fn cmd_history_intraday(
    symbol: String,
    session: &str,
    hist_date: &str,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let trade_session = match session {
        "all" => "100",
        "pre" | "post" => "101",
        _ => "0",
    };
    let data = http_get(
        "/v1/quote/history-timeshares",
        &[
            ("counter_id", cid.as_str()),
            ("date", hist_date),
            ("trade_session", trade_session),
            ("adjust_type", "0"),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            let mut found = false;
            if let Some(timeshares) = data.get("timeshares").and_then(|v| v.as_array()) {
                for ts in timeshares {
                    if let Some(minutes) = ts.get("minutes").and_then(|v| v.as_array()) {
                        if !minutes.is_empty() {
                            found = true;
                        }
                        let headers = ["timestamp", "price", "volume", "turnover", "avg_price"];
                        let rows: Vec<Vec<String>> = minutes
                            .iter()
                            .map(|m| {
                                vec![
                                    val_str(&m["timestamp"]),
                                    val_str(&m["price"]),
                                    val_str(&m["amount"]),
                                    val_str(&m["balance"]),
                                    val_str(&m["avg_price"]),
                                ]
                            })
                            .collect();
                        print_table(&headers, rows, format);
                    }
                }
            }
            if !found {
                println!("No history intraday data found for this date.");
            }
        }
    }
    Ok(())
}

pub async fn cmd_constituent(
    symbol: String,
    limit: i32,
    sort: &str,
    order: &str,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = index_symbol_to_counter_id(&symbol);
    let indicator = match sort {
        "price" => "2",
        "turnover" => "3",
        "inflow" => "4",
        "turnover_rate" => "5",
        "market_cap" => "6",
        _ => "1", // change% default
    };
    let order_val = if order == "asc" { "1" } else { "0" };
    let limit_str = limit.to_string();
    let data = http_get(
        "/v1/quote/index-constituents",
        &[
            ("counter_id", cid.as_str()),
            ("offset", "0"),
            ("limit", limit_str.as_str()),
            ("indicator", indicator),
            ("order", order_val),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            let total = data["total"].as_i64().unwrap_or(0);
            let rise = data["rise_num"].as_i64().unwrap_or(0);
            let fall = data["fall_num"].as_i64().unwrap_or(0);
            let flat = data["flat_num"].as_i64().unwrap_or(0);
            println!("Constituents ({total} total)  Rise: {rise}  Fall: {fall}  Flat: {flat}\n");
            let items = match data.get("stocks").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a,
                _ => {
                    println!("No constituent data found.");
                    return Ok(());
                }
            };
            let headers = [
                "symbol",
                "name",
                "price",
                "prev_close",
                "change%",
                "volume",
                "turnover",
            ];
            let rows: Vec<Vec<String>> = items
                .iter()
                .map(|item| {
                    vec![
                        crate::utils::counter::counter_id_to_symbol(&val_str(&item["counter_id"])),
                        val_str(&item["name"]),
                        val_str(&item["last_done"]),
                        val_str(&item["prev_close"]),
                        val_str(&item["chg"]),
                        val_str(&item["amount"]),
                        val_str(&item["balance"]),
                    ]
                })
                .collect();
            print_table(&headers, rows, format);
        }
    }
    Ok(())
}

fn market_trade_status_label(code: i64) -> &'static str {
    match code {
        101 => "Pre-Open",
        102 | 103 | 105 | 202 | 203 => "Trading",
        104 => "Lunch Break",
        106 => "Post-Trading",
        108 => "Closed",
        201 => "Pre-Market",
        204 => "Post-Market",
        _ => "Unknown",
    }
}

pub async fn cmd_market_status(format: &OutputFormat, verbose: bool) -> Result<()> {
    let data = http_get("/v1/quote/market-status", &[], verbose).await?;
    let list = data
        .get("market_time")
        .or_else(|| data.get("list"))
        .and_then(|v| v.as_array());
    let items = match list {
        Some(a) if !a.is_empty() => a,
        _ => {
            println!("No market status data found.");
            return Ok(());
        }
    };
    match format {
        OutputFormat::Json => {
            let out: Vec<Value> = items
                .iter()
                .map(|item| {
                    let code = item["trade_status"].as_i64().unwrap_or(0);
                    serde_json::json!({
                        "market": val_str(&item["market"]),
                        "status": market_trade_status_label(code),
                    })
                })
                .collect();
            print_json(&serde_json::json!(out));
        }
        OutputFormat::Pretty => {
            let headers = ["market", "status"];
            let rows: Vec<Vec<String>> = items
                .iter()
                .map(|item| {
                    let code = item["trade_status"].as_i64().unwrap_or(0);
                    vec![
                        val_str(&item["market"]),
                        market_trade_status_label(code).to_string(),
                    ]
                })
                .collect();
            print_table(&headers, rows, format);
        }
    }
    Ok(())
}

pub async fn cmd_broker_holding_top(
    symbol: String,
    period: &str,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/quote/broker-holding",
        &[("counter_id", cid.as_str()), ("type", period)],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            let updated = val_str(&data["updated_at"]);
            println!("Broker Holding Top (updated: {updated})\n");

            for (label, key) in [("Buy", "buy"), ("Sell", "sell")] {
                let raw = val_str(&data[key]);
                if raw == "-" || raw.is_empty() {
                    continue;
                }
                if let Ok(items) = serde_json::from_str::<Vec<Value>>(&raw) {
                    println!("{label}:");
                    let headers = ["broker", "parti_no", "change(shares)"];
                    let rows: Vec<Vec<String>> = items
                        .iter()
                        .map(|item| {
                            vec![
                                val_str(&item["name"]),
                                val_str(&item["parti_number"]),
                                fmt_shares_chg(&val_str(&item["chg"])),
                            ]
                        })
                        .collect();
                    print_table(&headers, rows, &OutputFormat::Pretty);
                    println!();
                }
            }
        }
    }
    Ok(())
}

pub async fn cmd_broker_holding_detail(
    symbol: String,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/quote/broker-holding/detail",
        &[("counter_id", cid.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            let items = match data.get("list").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a.clone(),
                _ => {
                    // Fallback: list might be a JSON string
                    let raw = val_str(&data["list"]);
                    if raw == "-" || raw.is_empty() {
                        println!("No broker holding detail found.");
                        return Ok(());
                    }
                    serde_json::from_str::<Vec<Value>>(&raw).unwrap_or_default()
                }
            };
            if items.is_empty() {
                println!("No broker holding detail found.");
                return Ok(());
            }
            let headers = ["broker", "parti_no", "ratio%", "shares", "chg_1d", "chg_5d"];
            let rows: Vec<Vec<String>> = items
                .iter()
                .map(|item| {
                    let ratio = item.get("ratio").unwrap_or(&Value::Null);
                    let shares = item.get("shares").unwrap_or(&Value::Null);
                    vec![
                        val_str(&item["name"]),
                        val_str(&item["parti_number"]),
                        val_str(&ratio["value"]),
                        fmt_shares(&val_str(&shares["value"])),
                        fmt_shares_chg(&val_str(&shares["chg_1"])),
                        fmt_shares_chg(&val_str(&shares["chg_5"])),
                    ]
                })
                .collect();
            print_table(&headers, rows, format);
        }
    }
    Ok(())
}

pub async fn cmd_broker_holding_daily(
    symbol: String,
    broker: &str,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/quote/broker-holding/daily",
        &[("counter_id", cid.as_str()), ("parti_number", broker)],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            let items = match data.get("list").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a.clone(),
                _ => {
                    let raw = val_str(&data["list"]);
                    if raw == "-" || raw.is_empty() {
                        println!("No daily holding data found.");
                        return Ok(());
                    }
                    serde_json::from_str::<Vec<Value>>(&raw).unwrap_or_default()
                }
            };
            if items.is_empty() {
                println!("No daily holding data found.");
                return Ok(());
            }
            let headers = ["date", "holding(shares)", "ratio%", "change(shares)"];
            let rows: Vec<Vec<String>> = items
                .iter()
                .map(|item| {
                    vec![
                        val_str(&item["date"]),
                        fmt_shares(&val_str(&item["holding"])),
                        val_str(&item["ratio"]),
                        fmt_shares_chg(&val_str(&item["chg"])),
                    ]
                })
                .collect();
            print_table(&headers, rows, format);
        }
    }
    Ok(())
}

pub async fn cmd_ah_premium_kline(
    symbol: String,
    kline_type: &str,
    count: i32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let line_type = match kline_type {
        "1m" => "1",
        "5m" => "5",
        "15m" => "15",
        "30m" => "30",
        "60m" => "60",
        "week" => "2000",
        "month" => "3000",
        "year" => "4000",
        _ => "1000", // day
    };
    let count_str = count.to_string();
    let data = http_get(
        "/v1/quote/ahpremium/klines",
        &[
            ("counter_id", cid.as_str()),
            ("line_num", count_str.as_str()),
            ("line_type", line_type),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            let items = match data.get("klines").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a,
                _ => {
                    println!("No AH premium data found.");
                    return Ok(());
                }
            };
            let headers = ["date", "A-share(CNY)", "H-share(HKD)", "premium", "fx_rate"];
            let rows: Vec<Vec<String>> = items
                .iter()
                .map(|item| {
                    vec![
                        fmt_ts(&val_str(&item["timestamp"])),
                        val_str(&item["aprice"]),
                        val_str(&item["hprice"]),
                        fmt_premium(&val_str(&item["ahpremium_rate"])),
                        val_str(&item["currency_rate"]),
                    ]
                })
                .collect();
            print_table(&headers, rows, format);
        }
    }
    Ok(())
}

pub async fn cmd_ah_premium_intraday(
    symbol: String,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/quote/ahpremium/timeshares",
        &[("counter_id", cid.as_str()), ("days", "1")],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            let items = match data
                .get("klines")
                .or_else(|| data.get("minutes"))
                .and_then(|v| v.as_array())
            {
                Some(a) if !a.is_empty() => a,
                _ => {
                    println!("No AH premium intraday data found.");
                    return Ok(());
                }
            };
            let headers = ["time", "A-share(CNY)", "H-share(HKD)", "premium", "fx_rate"];
            let rows: Vec<Vec<String>> = items
                .iter()
                .map(|item| {
                    vec![
                        fmt_ts_time(&val_str(&item["timestamp"])),
                        val_str(&item["aprice"]),
                        val_str(&item["hprice"]),
                        fmt_premium(&val_str(&item["ahpremium_rate"])),
                        val_str(&item["currency_rate"]),
                    ]
                })
                .collect();
            print_table(&headers, rows, format);
        }
    }
    Ok(())
}

pub async fn cmd_trade_stats(symbol: String, format: &OutputFormat, verbose: bool) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/quote/trades-statistics",
        &[("counter_id", cid.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            // Summary stats
            if let Some(stats) = data
                .get("statistics")
                .or_else(|| data.get("tradestatistics"))
            {
                let total: f64 = val_str(&stats["total_amount"]).parse().unwrap_or(0.0);
                let buy: f64 = val_str(&stats["buy"]).parse().unwrap_or(0.0);
                let sell: f64 = val_str(&stats["sell"]).parse().unwrap_or(0.0);
                let neutral: f64 = val_str(&stats["neutral"]).parse().unwrap_or(0.0);
                let buy_pct = if total > 0.0 {
                    buy / total * 100.0
                } else {
                    0.0
                };
                let sell_pct = if total > 0.0 {
                    sell / total * 100.0
                } else {
                    0.0
                };
                let neutral_pct = if total > 0.0 {
                    neutral / total * 100.0
                } else {
                    0.0
                };

                println!(
                    "Prev Close: {}  Avg Price: {}  Trades: {}  Updated: {}",
                    val_str(&stats["preclose"]),
                    val_str(&stats["avgprice"]),
                    val_str(&stats["trades_count"]),
                    fmt_ts_time(&val_str(&stats["timestamp"])),
                );
                println!(
                    "Volume: {}  Buy: {} ({:.1}%)  Sell: {} ({:.1}%)  Neutral: {} ({:.1}%)",
                    fmt_shares(&total.to_string()),
                    fmt_shares(&buy.to_string()),
                    buy_pct,
                    fmt_shares(&sell.to_string()),
                    sell_pct,
                    fmt_shares(&neutral.to_string()),
                    neutral_pct,
                );
                println!();
            }
            // Price distribution
            if let Some(items) = data
                .get("trades")
                .or_else(|| data.get("pricetrades"))
                .and_then(|v| v.as_array())
            {
                if !items.is_empty() {
                    let headers = ["price", "buy(shares)", "sell(shares)", "neutral(shares)"];
                    let rows: Vec<Vec<String>> = items
                        .iter()
                        .map(|item| {
                            vec![
                                val_str(&item["price"]),
                                fmt_shares(&val_str(&item["buy_amount"])),
                                fmt_shares(&val_str(&item["sell_amount"])),
                                fmt_shares(&val_str(&item["neutral_amount"])),
                            ]
                        })
                        .collect();
                    print_table(&headers, rows, &OutputFormat::Pretty);
                }
            }
        }
    }
    Ok(())
}

pub async fn cmd_anomaly(
    market: &str,
    symbol: Option<String>,
    count: i32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let count_str = count.to_string();
    let market_upper = market.to_uppercase();
    let mut params = vec![
        ("category", "0"),
        ("size", count_str.as_str()),
        ("market", market_upper.as_str()),
    ];
    let cid;
    if let Some(ref sym) = symbol {
        cid = symbol_to_counter_id(sym);
        params.push(("counter_id", cid.as_str()));
    }
    let data = http_get("/v1/quote/changes", &params, verbose).await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            let items = match data.get("changes").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a,
                _ => {
                    println!("No anomalies found.");
                    return Ok(());
                }
            };
            let headers = ["time", "symbol", "name", "alert", "emotion"];
            let rows: Vec<Vec<String>> = items
                .iter()
                .map(|item| {
                    let emotion = match item["emotion"].as_i64() {
                        Some(1) => "Bull",
                        Some(2) => "Bear",
                        _ => "-",
                    };
                    vec![
                        val_str(&item["alert_time"]),
                        crate::utils::counter::counter_id_to_symbol(&val_str(&item["counter_id"])),
                        val_str(&item["name"]),
                        val_str(&item["alert_name"]),
                        emotion.to_string(),
                    ]
                })
                .collect();
            print_table(&headers, rows, format);
        }
    }
    Ok(())
}

pub async fn cmd_option_volume_stats(
    symbol: String,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/quote/option-volume-stats",
        &[("underlying_counter_id", cid.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            let call: i64 = val_str(&data["c"]).parse().unwrap_or(0);
            let put: i64 = val_str(&data["p"]).parse().unwrap_or(0);
            let pc_ratio = if call > 0 {
                #[allow(clippy::cast_precision_loss)]
                let ratio = put as f64 / call as f64;
                format!("{ratio:.4}")
            } else {
                "-".to_string()
            };
            println!("Option Volume Stats — {symbol}\n");
            print_table(
                &["call_vol", "put_vol", "pc_ratio"],
                vec![vec![
                    format_with_commas(call),
                    format_with_commas(put),
                    pc_ratio,
                ]],
                format,
            );
        }
    }
    Ok(())
}

pub async fn cmd_option_volume_daily(
    symbol: String,
    count: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();
    let count_str = count.to_string();
    let data = http_get(
        "/v1/quote/option-volume-stats/daily",
        &[
            ("counter_id", cid.as_str()),
            ("timestamp", now.as_str()),
            ("line_num", count_str.as_str()),
            ("direction", "1"),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            let mut items = match data.get("stats").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a.clone(),
                _ => {
                    println!("No option volume data found for {symbol}.");
                    return Ok(());
                }
            };
            items.reverse();
            println!("Option Volume Daily — {symbol}\n");
            let headers = [
                "date",
                "total_vol",
                "call_vol",
                "put_vol",
                "pc_vol",
                "call_oi",
                "put_oi",
                "pc_oi",
            ];
            let rows: Vec<Vec<String>> = items
                .iter()
                .map(|item| {
                    let total_vol: i64 = val_str(&item["total_volume"]).parse().unwrap_or(0);
                    let call_vol: i64 = val_str(&item["total_call_volume"]).parse().unwrap_or(0);
                    let put_vol: i64 = val_str(&item["total_put_volume"]).parse().unwrap_or(0);
                    let call_oi: i64 = val_str(&item["total_call_open_interest"])
                        .parse()
                        .unwrap_or(0);
                    let put_oi: i64 = val_str(&item["total_put_open_interest"])
                        .parse()
                        .unwrap_or(0);
                    vec![
                        fmt_ts(&val_str(&item["timestamp"])),
                        format_with_commas(total_vol),
                        format_with_commas(call_vol),
                        format_with_commas(put_vol),
                        val_str(&item["put_call_volume_ratio"]),
                        format_with_commas(call_oi),
                        format_with_commas(put_oi),
                        val_str(&item["put_call_open_interest_ratio"]),
                    ]
                })
                .collect();
            print_table(&headers, rows, format);
        }
    }
    Ok(())
}

pub async fn cmd_short_positions(
    symbol: String,
    count: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();
    let count_str = count.clamp(1, 100).to_string();
    let data = http_get(
        "/v1/quote/short-positions/us",
        &[
            ("counter_id", cid.as_str()),
            ("last_timestamp", now.as_str()),
            ("page_size", count_str.as_str()),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            let mut items = match data.get("data").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a.clone(),
                _ => {
                    println!("No short selling data found for {symbol}.");
                    return Ok(());
                }
            };
            items.reverse();
            println!("Short Selling Data — {symbol}\n");
            let headers = [
                "date",
                "rate%",
                "short_shares",
                "avg_daily_vol",
                "days_cover",
                "close",
            ];
            let rows: Vec<Vec<String>> = items
                .iter()
                .map(|item| {
                    let ts = val_str(&item["timestamp"]);
                    let rate = val_str(&item["rate"])
                        .parse::<f64>()
                        .map_or_else(|_| val_str(&item["rate"]), |v| format!("{:.2}%", v * 100.0));
                    let short_shares: i64 =
                        val_str(&item["current_shares_short"]).parse().unwrap_or(0);
                    let avg_vol: i64 = val_str(&item["avg_daily_share_volume"])
                        .parse()
                        .unwrap_or(0);
                    let days = val_str(&item["days_to_cover"]);
                    let close = val_str(&item["close"]);
                    vec![
                        fmt_ts(&ts),
                        rate,
                        format_with_commas(short_shares),
                        format_with_commas(avg_vol),
                        days,
                        close,
                    ]
                })
                .collect();
            print_table(&headers, rows, format);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::api::MockQuoteApi;

    #[tokio::test]
    async fn test_run_quote_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_quote()
            .with(mockall::predicate::eq(vec!["TSLA.US".to_string()]))
            .times(1)
            .returning(|_| Ok(vec![]));
        run_quote(&mock, vec!["TSLA.US".to_string()], &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_quote_empty_symbols_errors() {
        let mock = MockQuoteApi::new();
        let result = run_quote(&mock, vec![], &OutputFormat::Pretty).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_run_depth_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_depth()
            .with(mockall::predicate::eq("700.HK".to_string()))
            .times(1)
            .returning(|_| {
                Ok(longbridge::quote::SecurityDepth {
                    asks: vec![],
                    bids: vec![],
                })
            });
        run_depth(&mock, "700.HK".to_string(), &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_brokers_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_brokers()
            .with(mockall::predicate::eq("700.HK".to_string()))
            .times(1)
            .returning(|_| {
                Ok(longbridge::quote::SecurityBrokers {
                    ask_brokers: vec![],
                    bid_brokers: vec![],
                })
            });
        run_brokers(&mock, "700.HK".to_string(), &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_trades_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_trades()
            .with(
                mockall::predicate::eq("AAPL.US".to_string()),
                mockall::predicate::eq(20_usize),
            )
            .times(1)
            .returning(|_, _| Ok(vec![]));
        run_trades(&mock, "AAPL.US".to_string(), 20, &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_intraday_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_intraday()
            .with(mockall::predicate::eq("TSLA.US".to_string()))
            .times(1)
            .returning(|_| Ok(vec![]));
        run_intraday(&mock, "TSLA.US".to_string(), &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_kline_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_candlesticks()
            .with(
                mockall::predicate::eq("TSLA.US".to_string()),
                mockall::predicate::eq(Period::Day),
                mockall::predicate::eq(100_usize),
                mockall::predicate::eq(AdjustType::NoAdjust),
            )
            .times(1)
            .returning(|_, _, _, _| Ok(vec![]));
        run_kline(
            &mock,
            "TSLA.US".to_string(),
            Period::Day,
            100,
            AdjustType::NoAdjust,
            &OutputFormat::Pretty,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_kline_history_by_date() {
        let mut mock = MockQuoteApi::new();
        let start = time::macros::date!(2024 - 01 - 01);
        let end = time::macros::date!(2024 - 12 - 31);
        mock.expect_history_candlesticks_by_date()
            .times(1)
            .returning(|_, _, _, _, _| Ok(vec![]));
        run_kline_history(
            &mock,
            "TSLA.US".to_string(),
            Period::Day,
            AdjustType::NoAdjust,
            Some(start),
            Some(end),
            &OutputFormat::Pretty,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_kline_history_by_offset() {
        let mut mock = MockQuoteApi::new();
        mock.expect_history_candlesticks_by_offset()
            .times(1)
            .returning(|_, _, _, _| Ok(vec![]));
        run_kline_history(
            &mock,
            "TSLA.US".to_string(),
            Period::Day,
            AdjustType::NoAdjust,
            None,
            None,
            &OutputFormat::Pretty,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_static_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_static_info()
            .with(mockall::predicate::eq(vec!["TSLA.US".to_string()]))
            .times(1)
            .returning(|_| Ok(vec![]));
        run_static(&mock, vec!["TSLA.US".to_string()], &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_capital_flow_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_capital_flow()
            .with(mockall::predicate::eq("TSLA.US".to_string()))
            .times(1)
            .returning(|_| Ok(vec![]));
        run_capital_flow(&mock, "TSLA.US".to_string(), &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_capital_dist_dispatches() {
        use rust_decimal::Decimal;
        let mut mock = MockQuoteApi::new();
        mock.expect_capital_distribution()
            .with(mockall::predicate::eq("TSLA.US".to_string()))
            .times(1)
            .returning(|_| {
                Ok(longbridge::quote::CapitalDistributionResponse {
                    timestamp: time::OffsetDateTime::UNIX_EPOCH,
                    capital_in: longbridge::quote::CapitalDistribution {
                        large: Decimal::ZERO,
                        medium: Decimal::ZERO,
                        small: Decimal::ZERO,
                    },
                    capital_out: longbridge::quote::CapitalDistribution {
                        large: Decimal::ZERO,
                        medium: Decimal::ZERO,
                        small: Decimal::ZERO,
                    },
                })
            });
        run_capital_dist(&mock, "TSLA.US".to_string(), &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_trading_session_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_trading_session()
            .times(1)
            .returning(|| Ok(vec![]));
        run_trading_session(&mock, &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_security_list_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_security_list()
            .with(mockall::predicate::eq(longbridge::Market::HK))
            .times(1)
            .returning(|_| Ok(vec![]));
        run_security_list(&mock, longbridge::Market::HK, &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_participants_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_participants().times(1).returning(|| Ok(vec![]));
        run_participants(&mock, &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_subscriptions_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_subscriptions()
            .times(1)
            .returning(|| Ok(vec![]));
        run_subscriptions(&mock, &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_option_quote_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_option_quote()
            .with(mockall::predicate::eq(
                vec!["AAPL240119C190000".to_string()],
            ))
            .times(1)
            .returning(|_| Ok(vec![]));
        run_option_quote(
            &mock,
            vec!["AAPL240119C190000".to_string()],
            &OutputFormat::Pretty,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_option_chain_dates_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_option_chain_expiry_date_list()
            .with(mockall::predicate::eq("AAPL.US".to_string()))
            .times(1)
            .returning(|_| Ok(vec![]));
        run_option_chain_dates(&mock, "AAPL.US".to_string(), &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_option_chain_dispatches_dates_without_date_arg() {
        let mut mock = MockQuoteApi::new();
        mock.expect_option_chain_expiry_date_list()
            .with(mockall::predicate::eq("AAPL.US".to_string()))
            .times(1)
            .returning(|_| Ok(vec![]));
        mock.expect_option_chain_info_by_date().times(0);

        run_option_chain(&mock, "AAPL.US".to_string(), None, &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_option_chain_dispatches_strikes_with_date_arg() {
        let mut mock = MockQuoteApi::new();
        let date = time::macros::date!(2024 - 01 - 19);
        mock.expect_option_chain_expiry_date_list().times(0);
        mock.expect_option_chain_info_by_date()
            .with(
                mockall::predicate::eq("AAPL.US".to_string()),
                mockall::predicate::eq(date),
            )
            .times(1)
            .returning(|_, _| Ok(vec![]));

        run_option_chain(
            &mock,
            "AAPL.US".to_string(),
            Some("2024-01-19".to_string()),
            &OutputFormat::Pretty,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_option_chain_strikes_dispatches() {
        let mut mock = MockQuoteApi::new();
        let date = time::macros::date!(2024 - 01 - 19);
        mock.expect_option_chain_info_by_date()
            .with(
                mockall::predicate::eq("AAPL.US".to_string()),
                mockall::predicate::eq(date),
            )
            .times(1)
            .returning(|_, _| Ok(vec![]));
        run_option_chain_strikes(&mock, "AAPL.US".to_string(), date, &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_warrant_quote_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_warrant_quote()
            .times(1)
            .returning(|_| Ok(vec![]));
        run_warrant_quote(&mock, vec!["12345.HK".to_string()], &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_warrant_list_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_warrant_list()
            .with(mockall::predicate::eq("700.HK".to_string()))
            .times(1)
            .returning(|_| Ok(vec![]));
        run_warrant_list(&mock, "700.HK".to_string(), &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_warrant_issuers_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_warrant_issuers()
            .times(1)
            .returning(|| Ok(vec![]));
        run_warrant_issuers(&mock, &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[test]
    fn test_parse_period_valid() {
        assert!(parse_period("day").is_ok());
        assert!(parse_period("1m").is_ok());
        assert!(parse_period("1h").is_ok());
        assert!(parse_period("week").is_ok());
        assert!(parse_period("month").is_ok());
    }

    #[test]
    fn test_parse_period_invalid() {
        assert!(parse_period("invalid").is_err());
        assert!(parse_period("2h").is_err());
    }

    #[test]
    fn test_parse_adjust_valid() {
        assert!(parse_adjust("no_adjust").is_ok());
        assert!(parse_adjust("none").is_ok());
        assert!(parse_adjust("forward_adjust").is_ok());
        assert!(parse_adjust("forward").is_ok());
    }

    #[test]
    fn test_parse_adjust_invalid() {
        assert!(parse_adjust("backward").is_err());
    }

    #[test]
    fn test_parse_market_valid() {
        assert!(parse_market("HK").is_ok());
        assert!(parse_market("hk").is_ok());
        assert!(parse_market("US").is_ok());
        assert!(parse_market("CN").is_ok());
        assert!(parse_market("SH").is_ok());
        assert!(parse_market("SZ").is_ok());
    }

    #[test]
    fn test_parse_market_invalid() {
        assert!(parse_market("XX").is_err());
    }
}
