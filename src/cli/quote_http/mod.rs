//! HTTP-REST backend for one-shot quote **commands**.
//!
//! The CLI's interactive TUI keeps the real-time `QuoteContext` WebSocket, but
//! one-shot commands (`longbridge quote`, `depth`, `trades`, …) do not need a
//! persistent WS subscription — a single round-trip is cheaper and avoids the
//! connect/subscribe/teardown cost. This module mirrors the Longbridge-MCP
//! WS→HTTP migration (longbridge-mcp#162): it POSTs the gateway's WS→HTTP shim
//! endpoints (e.g. `/quote/quotes`) and reshapes the raw proto-JSON back into
//! the exact shape each SDK struct's `Deserialize` expects, then
//! `serde_json::from_value`s into the typed struct the command formatters
//! already consume.
//!
//! ## Why a reshape layer
//!
//! The shim returns the gateway's proto-JSON, which differs from the SDK types
//! in a few mechanical ways this module normalizes:
//!
//! * containers are wrapped in a single proto field (`secu_quote`, `lines`, …) —
//!   [`unwrap`] lifts them out;
//! * `int64` fields arrive as JSON **strings** (`"20422500"`) — [`numify_paths`]
//!   turns them back into numbers for the SDK's `i64`/`i32` fields (the SDK's
//!   `Decimal` fields already accept strings, so they are left alone);
//! * unix-seconds timestamps need converting to the RFC3339 the SDK expects —
//!   [`convert_unix_paths`];
//! * enums arrive as wire ints and must be re-encoded through the SDK enum so
//!   they match its `Serialize`/`Deserialize` form — [`map_enum_path`];
//! * a handful of fields are renamed or dropped — [`rename_keys`]/[`drop_keys`].
//!
//! Unlike the MCP layer (whose output is human-facing JSON), the goal here is
//! round-tripping into the SDK struct, so every integer field must be numified
//! and every enum re-encoded through its SDK type — otherwise `from_value`
//! rejects the record.

use anyhow::Result;
use longbridge::httpclient::{Json, Method};
use longbridge::quote::{
    Candlestick, CapitalDistributionResponse, CapitalFlowLine, IntradayLine, IssuerInfo,
    MarketTradingDays, MarketTradingSession, OptionChainContract, OptionQuote, ParticipantInfo,
    SecurityBrokers, SecurityCalcIndex, SecurityDepth, SecurityQuote, SecurityStaticInfo, Trade,
    WarrantInfo, WarrantQuote,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

mod reshape;
#[allow(clippy::wildcard_imports)]
use reshape::*;

// ── HTTP transport ───────────────────────────────────────────────────────────

/// POST the given shim `path` with `body`, returning the raw gateway JSON.
///
/// A rate-limit rejection (`429002`) is retried with exponential backoff — the
/// gateway's limit is per second, so a short wait clears it. Any other error is
/// returned immediately.
///
/// An endpoint served by one data center only ([`endpoint_region`]) is
/// restricted to it: a session in another region gets the SDK's
/// `DcRegionRestricted` error before any request is sent, the same short-circuit
/// the WS `QuoteContext` applies.
async fn http_post(path: &str, body: Value) -> Result<Value> {
    const MAX_ATTEMPTS: u32 = 4;
    const BASE_DELAY_MS: u64 = 250;

    let region = endpoint_region(path);
    let client = crate::openapi::http_client();
    let mut attempt = 1;
    loop {
        let mut request = client.request(Method::POST, path).body(Json(body.clone()));
        if let Some(region) = region {
            request = request.dc_restrict(region);
        }
        let result = request
            .response::<Json<Value>>()
            .send()
            .await
            .map_err(to_cli_error);
        match result {
            Ok(resp) => return Ok(resp.0),
            Err(err) if attempt < MAX_ATTEMPTS && is_rate_limited(&err) => {
                let delay = BASE_DELAY_MS << (attempt - 1);
                tracing::debug!("{path}: rate limited, retry {attempt} in {delay}ms");
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

/// The only data center that serves `path`, if it is restricted to one. The
/// broker queue is AP-only, as `QuoteContext::brokers` enforces on the WS path.
fn endpoint_region(path: &str) -> Option<longbridge::DcRegion> {
    match path {
        "/quote/brokers" => Some(longbridge::DcRegion::Ap),
        _ => None,
    }
}

/// Whether `err` is the gateway's rate-limit rejection, business code
/// `429002`. A bare HTTP 429 is not matched: the SDK's `send()` already retries
/// that itself, and retrying it here too would nest the two loops.
fn is_rate_limited(err: &anyhow::Error) -> bool {
    use longbridge::httpclient::HttpClientError;
    const RATE_LIMIT_CODE: i32 = 429_002;
    matches!(
        err.downcast_ref::<longbridge::Error>(),
        Some(longbridge::Error::HttpClient(HttpClientError::OpenApi { code, .. }))
            if *code == RATE_LIMIT_CODE
    )
}

/// `/quote/warrants` page size: the shim's `sort_count` cap valid on every
/// gateway (see [`fetch_warrant_pages`]).
const WARRANT_PAGE: usize = 100;
/// Runaway guard against a bogus `total_count`.
const WARRANT_MAX_PAGES: usize = 200;
/// The most rows [`fetch_warrant_pages`] will return.
const WARRANT_MAX_ROWS: usize = WARRANT_PAGE * WARRANT_MAX_PAGES;

/// Fetch every page of `/quote/warrants` for `symbol`, sorted by the static
/// expiry-date key. Returns `(total_count, raw rows)`.
///
/// The shim caps `sort_count` per request — 100 on the test gateway (anything
/// above is `301607 requst too many sort count`) and 500 on production — so the
/// page size is one valid on both. The first page reports `total_count`; the
/// rest are fetched concurrently, bounded so a large list does not burst the
/// gateway's rate limit, and kept in request order.
async fn fetch_warrant_pages(symbol: &str) -> Result<(usize, Vec<Value>)> {
    use futures::{StreamExt, TryStreamExt};

    const CONCURRENCY: usize = 3;
    // `WarrantSortBy::ExpiryDate` / `SortOrderType::Descending`. The order is
    // replaced locally afterwards; the key only has to be stable while paging.
    // Many warrants share an expiry, so the gateway's tiebreak still decides
    // the order inside a group — `warrant_list` merges a second pass when the
    // first comes up short.
    const SORT_BY_EXPIRY_DATE: i32 = 5;
    const SORT_DESCENDING: i32 = 1;

    let language = crate::openapi::api_language() as i32;
    let body = |offset: usize| {
        serde_json::json!({
            "symbol": symbol,
            "filter_config": {
                "sort_by": SORT_BY_EXPIRY_DATE, "sort_order": SORT_DESCENDING,
                "sort_offset": offset, "sort_count": WARRANT_PAGE,
            },
            "language": language,
        })
    };
    let page_rows = |resp: &Value| {
        resp.get("warrant_list")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };

    let first = http_post("/quote/warrants", body(0)).await?;
    let total = first
        .get("total_count")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let mut rows = page_rows(&first);
    let rest: Vec<Value> = futures::stream::iter(
        page_offsets(total, WARRANT_PAGE, WARRANT_MAX_PAGES)
            .into_iter()
            .map(|offset| http_post("/quote/warrants", body(offset))),
    )
    .buffered(CONCURRENCY)
    .try_collect()
    .await?;
    for resp in &rest {
        rows.extend(page_rows(resp));
    }
    Ok((total, rows))
}

/// Order raw warrant rows by `last_done` descending — the order the WS
/// `warrant_list` returns — comparing numerically (string order would put
/// "9.5" above "10"). Equal prices are ordered by symbol so the output does not
/// depend on the gateway's paging tiebreak. A missing/empty price sorts as 0.
fn order_by_last_done_desc(mut rows: Vec<Value>) -> Vec<Value> {
    let price = |row: &Value| {
        row.get("last_done")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<rust_decimal::Decimal>().ok())
            .unwrap_or_default()
    };
    let symbol = |row: &Value| {
        row.get("symbol")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    rows.sort_by_cached_key(|row| (std::cmp::Reverse(price(row)), symbol(row)));
    rows
}

/// Union of two warrant fetches, keyed by symbol (first occurrence wins), so a
/// row either pass missed at a page boundary is recovered.
fn merge_warrant_passes(mut first: Vec<Value>, second: Vec<Value>) -> Vec<Value> {
    first.extend(second);
    dedupe_by_symbol(first)
}

/// `sort_offset`s for the pages after the first: `page, 2·page, …` below
/// `total`, at most `max_pages - 1` of them (the first page is fetched
/// separately to learn `total`).
fn page_offsets(total: usize, page: usize, max_pages: usize) -> Vec<usize> {
    (1..max_pages)
        .map(|i| i * page)
        .take_while(|&offset| offset < total)
        .collect()
}

/// Wrap the SDK HTTP error as a `longbridge::Error` before it enters `anyhow`.
/// `main.rs` renders errors by downcasting to `longbridge::Error`; a bare
/// `HttpClientError` misses that match and falls through to the generic
/// renderer, losing the `API error (code …)` form, the trace id, and the
/// option-quote OPRA permission guidance.
fn to_cli_error(err: longbridge::httpclient::HttpClientError) -> anyhow::Error {
    anyhow::Error::from(longbridge::Error::from(err))
}

// ── typed API ────────────────────────────────────────────────────────────────

/// HTTP-REST implementation of the quote commands.
pub struct HttpQuoteApi;

impl HttpQuoteApi {
    /// Reshape the raw `/quote/quotes` response into `Vec<SecurityQuote>`.
    fn reshape_quotes(mut value: Value) -> Result<Vec<SecurityQuote>> {
        unwrap(&mut value, "secu_quote");
        rename_keys(&mut value, &[("over_night_quote", "overnight_quote")]);
        drop_keys(&mut value, &["volume_str"]);
        convert_unix_paths(
            &mut value,
            &[
                "*.timestamp",
                "*.pre_market_quote.timestamp",
                "*.post_market_quote.timestamp",
                "*.overnight_quote.timestamp",
            ],
        );
        // Illiquid/halted names come back with "" price/volume fields; the SDK's
        // non-`Option` `Decimal`/`i64`s need a real 0 (matching the WS
        // `unwrap_or_default`). Runs before `numify` so "" → "0" → number.
        // `symbol` is the only string field; timestamps are already converted.
        zero_empty_numeric(&mut value, QUOTE_NUMERIC);
        numify_paths(
            &mut value,
            &[
                "*.volume",
                "*.pre_market_quote.volume",
                "*.post_market_quote.volume",
                "*.overnight_quote.volume",
            ],
        );
        map_int_enum(&mut value, &["*.trade_status"], &trade_status);
        from_value(value)
    }

    pub async fn quote(&self, symbols: Vec<String>) -> Result<Vec<SecurityQuote>> {
        let value = http_post("/quote/quotes", serde_json::json!({ "symbol": symbols })).await?;
        Self::reshape_quotes(value)
    }

    /// Reshape `/quote/depth` into `SecurityDepth` (`ask`/`bid` → `asks`/`bids`;
    /// numify `order_num`/`volume`).
    fn reshape_depth(mut value: Value) -> Result<SecurityDepth> {
        rename_keys(&mut value, &[("ask", "asks"), ("bid", "bids")]);
        drop_keys(&mut value, &["symbol", "volume_str"]);
        // An empty depth level comes back with `price: ""` (and "0" counts);
        // `Depth::price` is `Option<Decimal>` so "" must become null (not "0",
        // which would decode to `Some(0)` — a real 0 price). The non-`Option`
        // `position`/`volume`/`order_num` are zeroed then numified.
        set_empty_at(&mut value, &["asks.*.price", "bids.*.price"], &Value::Null);
        set_empty_at(
            &mut value,
            &[
                "asks.*.order_num",
                "asks.*.volume",
                "asks.*.position",
                "bids.*.order_num",
                "bids.*.volume",
                "bids.*.position",
            ],
            &Value::String("0".into()),
        );
        numify_paths(
            &mut value,
            &[
                "asks.*.order_num",
                "asks.*.volume",
                "asks.*.position",
                "bids.*.order_num",
                "bids.*.volume",
                "bids.*.position",
            ],
        );
        from_value(value)
    }

    pub async fn depth(&self, symbol: String) -> Result<SecurityDepth> {
        let value = http_post("/quote/depth", serde_json::json!({ "symbol": symbol })).await?;
        Self::reshape_depth(value)
    }

    /// Reshape `/quote/brokers` into `SecurityBrokers` (drop the echoed
    /// `symbol`; `ask_brokers`/`bid_brokers` already match the SDK).
    fn reshape_brokers(mut value: Value) -> Result<SecurityBrokers> {
        drop_keys(&mut value, &["symbol"]);
        numify_paths(
            &mut value,
            &["ask_brokers.*.position", "bid_brokers.*.position"],
        );
        from_value(value)
    }

    pub async fn brokers(&self, symbol: String) -> Result<SecurityBrokers> {
        let value = http_post("/quote/brokers", serde_json::json!({ "symbol": symbol })).await?;
        Self::reshape_brokers(value)
    }

    /// Reshape `/quote/trades` into `Vec<Trade>` (unwrap `trades`; numify
    /// `volume`; unix `timestamp`; map `direction`/`trade_session`).
    fn reshape_trades(mut value: Value) -> Result<Vec<Trade>> {
        unwrap(&mut value, "trades");
        drop_keys(&mut value, &["volume_str"]);
        convert_unix_paths(&mut value, &["*.timestamp"]);
        // `price` is the only non-`Option` `Decimal`; `trade_type` "" is a
        // meaningful value (automatch normal) so it is preserved. Runs before
        // `numify`/enum-map (which operate on ints, never "").
        zero_empty_numeric(&mut value, TRADE_NUMERIC);
        numify_paths(&mut value, &["*.volume"]);
        map_int_enum(&mut value, &["*.direction"], &trade_direction);
        map_int_enum(&mut value, &["*.trade_session"], &trade_session);
        from_value(value)
    }

    pub async fn trades(&self, symbol: String, count: usize) -> Result<Vec<Trade>> {
        let value = http_post(
            "/quote/trades",
            serde_json::json!({ "symbol": symbol, "count": count }),
        )
        .await?;
        Self::reshape_trades(value)
    }

    /// Reshape `/quote/intraday` into `Vec<IntradayLine>` (unwrap `lines`;
    /// numify `volume`; unix `timestamp`).
    fn reshape_intraday(mut value: Value) -> Result<Vec<IntradayLine>> {
        unwrap(&mut value, "lines");
        drop_keys(&mut value, &["volume_str"]);
        convert_unix_paths(&mut value, &["*.timestamp"]);
        zero_empty_numeric(&mut value, INTRADAY_NUMERIC);
        numify_paths(&mut value, &["*.volume"]);
        from_value(value)
    }

    pub async fn intraday(
        &self,
        symbol: String,
        sessions: longbridge::quote::TradeSessions,
    ) -> Result<Vec<IntradayLine>> {
        let value = http_post(
            "/quote/intraday",
            serde_json::json!({ "symbol": symbol, "trade_session": sessions as i32 }),
        )
        .await?;
        Self::reshape_intraday(value)
    }

    /// Reshape `/quote/participants` into `Vec<ParticipantInfo>` (unwrap
    /// `participant_broker_numbers`; rename `participant_name_*` → `name_*`).
    fn reshape_participants(mut value: Value) -> Result<Vec<ParticipantInfo>> {
        unwrap(&mut value, "participant_broker_numbers");
        rename_keys(
            &mut value,
            &[
                ("participant_name_cn", "name_cn"),
                ("participant_name_en", "name_en"),
                ("participant_name_hk", "name_hk"),
            ],
        );
        from_value(value)
    }

    pub async fn participants(&self) -> Result<Vec<ParticipantInfo>> {
        let value = http_post("/quote/participants", serde_json::json!({})).await?;
        Self::reshape_participants(value)
    }

    /// Reshape `/quote/candlesticks` (or `/quote/history-candlesticks`) into
    /// `Vec<Candlestick>` (unwrap `candlesticks`; numify `volume`; unix
    /// `timestamp`; map `trade_session`; inject the gateway-omitted
    /// `open_updated`).
    fn reshape_candlesticks(mut value: Value) -> Result<Vec<Candlestick>> {
        unwrap(&mut value, "candlesticks");
        drop_keys(&mut value, &["volume_str"]);
        convert_unix_paths(&mut value, &["*.timestamp"]);
        map_int_enum(&mut value, &["*.trade_session"], &trade_session);
        zero_empty_numeric(&mut value, CANDLE_NUMERIC);
        numify_paths(&mut value, &["*.volume"]);
        // The REST payload omits the SDK's `open_updated` flag; the WS path's
        // `TryFrom<quote::Candlestick>` sets it `true` for every bar it returns,
        // so default it the same way.
        insert_missing(&mut value, "open_updated", &Value::Bool(true));
        from_value(value)
    }

    pub async fn candlesticks(
        &self,
        symbol: String,
        period: longbridge::quote::Period,
        count: usize,
        adjust: longbridge::quote::AdjustType,
        sessions: longbridge::quote::TradeSessions,
    ) -> Result<Vec<Candlestick>> {
        let value = http_post(
            "/quote/candlesticks",
            serde_json::json!({
                "symbol": symbol,
                "period": period as i32,
                "count": count,
                "adjust_type": adjust as i32,
                "trade_session": sessions as i32,
            }),
        )
        .await?;
        Self::reshape_candlesticks(value)
    }

    pub async fn history_candlesticks_by_date(
        &self,
        symbol: String,
        period: longbridge::quote::Period,
        adjust: longbridge::quote::AdjustType,
        start: Option<time::Date>,
        end: Option<time::Date>,
        sessions: longbridge::quote::TradeSessions,
    ) -> Result<Vec<Candlestick>> {
        let ymd = |d: time::Date| format!("{:04}{:02}{:02}", d.year(), d.month() as u8, d.day());
        let value = http_post(
            "/quote/history-candlesticks",
            serde_json::json!({
                "symbol": symbol,
                "period": period as i32,
                "adjust_type": adjust as i32,
                "query_type": 2,
                "date_request": {
                    "start_date": start.map(ymd).unwrap_or_default(),
                    "end_date": end.map(ymd).unwrap_or_default(),
                },
                "trade_session": sessions as i32,
            }),
        )
        .await?;
        Self::reshape_candlesticks(value)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn history_candlesticks_by_offset(
        &self,
        symbol: String,
        period: longbridge::quote::Period,
        adjust: longbridge::quote::AdjustType,
        forward: bool,
        time: Option<time::PrimitiveDateTime>,
        count: usize,
        sessions: longbridge::quote::TradeSessions,
    ) -> Result<Vec<Candlestick>> {
        let (date_s, minute_s) = match time {
            Some(t) => (
                format!("{:04}{:02}{:02}", t.year(), t.month() as u8, t.day()),
                format!("{:02}{:02}", t.hour(), t.minute()),
            ),
            None => (String::new(), String::new()),
        };
        let value = http_post(
            "/quote/history-candlesticks",
            serde_json::json!({
                "symbol": symbol,
                "period": period as i32,
                "adjust_type": adjust as i32,
                "query_type": 1,
                "offset_request": {
                    "direction": i32::from(forward),
                    "date": date_s,
                    "minute": minute_s,
                    "count": count,
                },
                "trade_session": sessions as i32,
            }),
        )
        .await?;
        Self::reshape_candlesticks(value)
    }

    /// Reshape `/quote/static-info` into `Vec<SecurityStaticInfo>` (unwrap
    /// `secu_static_info`; drop `listing_date`; numify share counts; fold the
    /// `stock_derivatives` int array into the `DerivativeType` bitflags).
    fn reshape_static_info(mut value: Value) -> Result<Vec<SecurityStaticInfo>> {
        unwrap(&mut value, "secu_static_info");
        drop_keys(&mut value, &["listing_date"]);
        // Securities without earnings data return "" for eps/bps/etc. and the
        // share counts; the SDK's non-`Option` `Decimal`/`i64`s need a real 0.
        // Runs before `numify` so "" → "0" → number.
        zero_empty_numeric(&mut value, STATIC_NUMERIC);
        numify_paths(
            &mut value,
            &[
                "*.lot_size",
                "*.total_shares",
                "*.circulating_shares",
                "*.hk_shares",
            ],
        );
        if let Some(items) = value.as_array_mut() {
            for it in items.iter_mut() {
                if let Some(sd) = it.get("stock_derivatives") {
                    let bits: u8 = sd.as_array().map_or(0u8, |a| {
                        a.iter()
                            .filter_map(Value::as_u64)
                            .fold(0u8, |acc, n| acc | (n as u8))
                    });
                    let dt = longbridge::quote::DerivativeType::from_bits_truncate(bits);
                    if let (Some(obj), Ok(v)) = (it.as_object_mut(), serde_json::to_value(dt)) {
                        obj.insert("stock_derivatives".to_string(), v);
                    }
                }
            }
        }
        // `SecurityBoard` has no serde default; an empty/unrecognized board would
        // fail the whole batch. The WS path degrades to `Unknown` (types.rs
        // `board.parse().unwrap_or(SecurityBoard::Unknown)`); match that.
        default_unparseable_enum::<longbridge::quote::SecurityBoard>(
            &mut value,
            &["*.board"],
            "Unknown",
        );
        from_value(value)
    }

    pub async fn static_info(&self, symbols: Vec<String>) -> Result<Vec<SecurityStaticInfo>> {
        let value = http_post(
            "/quote/static-info",
            serde_json::json!({ "symbol": symbols }),
        )
        .await?;
        Self::reshape_static_info(value)
    }

    /// Reshape `/quote/capital-flow` into `Vec<CapitalFlowLine>` (unwrap
    /// `capital_flow_lines`; drop echoed `symbol`; unix `timestamp`).
    fn reshape_capital_flow(mut value: Value) -> Result<Vec<CapitalFlowLine>> {
        unwrap(&mut value, "capital_flow_lines");
        drop_keys(&mut value, &["symbol"]);
        convert_unix_default_paths(&mut value, &["*.timestamp"]);
        zero_empty_numeric(&mut value, CAPITAL_FLOW_NUMERIC);
        from_value(value)
    }

    pub async fn capital_flow(&self, symbol: String) -> Result<Vec<CapitalFlowLine>> {
        let value = http_post(
            "/quote/capital-flow",
            serde_json::json!({ "symbol": symbol }),
        )
        .await?;
        Self::reshape_capital_flow(value)
    }

    /// Reshape `/quote/capital-distribution` into `CapitalDistributionResponse`
    /// (drop echoed `symbol`; unix `timestamp`; empty-string buckets → `"0"`).
    fn reshape_capital_distribution(mut value: Value) -> Result<CapitalDistributionResponse> {
        drop_keys(&mut value, &["symbol"]);
        convert_unix_default_paths(&mut value, &["timestamp"]);
        for group in ["capital_in", "capital_out"] {
            if let Some(obj) = value.get_mut(group).and_then(|g| g.as_object_mut()) {
                for k in ["large", "medium", "small"] {
                    if let Some(v) = obj.get_mut(k) {
                        if v.as_str() == Some("") {
                            *v = Value::String("0".to_string());
                        }
                    }
                }
            }
        }
        from_value(value)
    }

    pub async fn capital_distribution(
        &self,
        symbol: String,
    ) -> Result<CapitalDistributionResponse> {
        let value = http_post(
            "/quote/capital-distribution",
            serde_json::json!({ "symbol": symbol }),
        )
        .await?;
        Self::reshape_capital_distribution(value)
    }

    /// Reshape `/quote/markets/trading-days` into `MarketTradingDays` (rename
    /// `trade_day`/`half_trade_day` → plural; `YYYYMMDD` → `YYYY-MM-DD`).
    fn reshape_trading_days(mut value: Value) -> Result<MarketTradingDays> {
        rename_keys(
            &mut value,
            &[
                ("trade_day", "trading_days"),
                ("half_trade_day", "half_trading_days"),
            ],
        );
        // Both are non-`Option` `Vec<Date>` with no serde default; the gateway
        // may omit an empty repeated field (proto3 JSON), so ensure both keys
        // exist to avoid a `missing field` decode error.
        if let Some(obj) = value.as_object_mut() {
            for k in ["trading_days", "half_trading_days"] {
                obj.entry(k.to_string())
                    .or_insert_with(|| Value::Array(vec![]));
            }
        }
        reformat_ymd(&mut value, &["trading_days.*", "half_trading_days.*"]);
        from_value(value)
    }

    pub async fn trading_days(
        &self,
        market: longbridge::Market,
        begin: time::Date,
        end: time::Date,
    ) -> Result<MarketTradingDays> {
        let ymd = |d: time::Date| format!("{:04}{:02}{:02}", d.year(), d.month() as u8, d.day());
        let market = serde_json::to_value(market)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let value = http_post(
            "/quote/markets/trading-days",
            serde_json::json!({ "market": market, "beg_day": ymd(begin), "end_day": ymd(end) }),
        )
        .await?;
        Self::reshape_trading_days(value)
    }

    /// Reshape `/quote/markets/trading-sessions` into `Vec<MarketTradingSession>`
    /// (unwrap `market_trade_session`; rename each market's inner `trade_session`
    /// array to `trade_sessions`; per session `beg_time`→`begin_time` as time,
    /// `end_time` as time, and map the `trade_session` int enum).
    fn reshape_trading_sessions(mut value: Value) -> Result<Vec<MarketTradingSession>> {
        unwrap(&mut value, "market_trade_session");
        if let Some(markets) = value.as_array_mut() {
            for market in markets.iter_mut() {
                let Some(obj) = market.as_object_mut() else {
                    continue;
                };
                // Rename the inner array only (a blanket rename would also hit
                // each session's `trade_session` enum field).
                if let Some(sessions) = obj.remove("trade_session") {
                    obj.insert("trade_sessions".to_string(), sessions);
                }
                let Some(sessions) = obj.get_mut("trade_sessions").and_then(Value::as_array_mut)
                else {
                    continue;
                };
                for session in sessions.iter_mut() {
                    if let Some(so) = session.as_object_mut() {
                        if let Some(bt) = so.remove("beg_time") {
                            so.insert("begin_time".to_string(), hhmm_to_time(&bt));
                        }
                        if let Some(et) = so.get_mut("end_time") {
                            *et = hhmm_to_time(et);
                        }
                    }
                    map_int_enum(session, &["trade_session"], &trade_session);
                }
            }
        }
        // `Market` has no serde default; guard an unrecognized market string the
        // same way the WS path does (`market.parse().unwrap_or_default()`).
        default_unparseable_enum::<longbridge::Market>(&mut value, &["*.market"], "Unknown");
        from_value(value)
    }

    pub async fn trading_session(&self) -> Result<Vec<MarketTradingSession>> {
        let value = http_post("/quote/markets/trading-sessions", serde_json::json!({})).await?;
        Self::reshape_trading_sessions(value)
    }

    /// Reshape `/quote/options/expiry-dates` into `Vec<Date>` (unwrap
    /// `expiry_date`; `YYYYMMDD` → `YYYY-MM-DD`).
    fn reshape_expiry_dates(mut value: Value) -> Result<Vec<time::Date>> {
        unwrap(&mut value, "expiry_date");
        reformat_ymd(&mut value, &["*"]);
        from_value(value)
    }

    pub async fn option_chain_expiry_date_list(&self, symbol: String) -> Result<Vec<time::Date>> {
        let value = http_post(
            "/quote/options/expiry-dates",
            serde_json::json!({ "symbol": symbol }),
        )
        .await?;
        Self::reshape_expiry_dates(value)
    }

    /// Reshape `/quote/warrants/issuers` into `Vec<IssuerInfo>` (unwrap
    /// `issuer_info`; rename proto `id` → SDK `issuer_id`).
    fn reshape_warrant_issuers(mut value: Value) -> Result<Vec<IssuerInfo>> {
        unwrap(&mut value, "issuer_info");
        rename_keys(&mut value, &[("id", "issuer_id")]);
        from_value(value)
    }

    pub async fn warrant_issuers(&self) -> Result<Vec<IssuerInfo>> {
        let value = http_post("/quote/warrants/issuers", serde_json::json!({})).await?;
        Self::reshape_warrant_issuers(value)
    }

    /// Reshape `/quote/options/strikes` into `Vec<OptionChainContract>` (unwrap
    /// `list`; default empty/null fields; map the string-code enums;
    /// `YYYYMMDD` → `YYYY-MM-DD`).
    fn reshape_option_chain(mut value: Value) -> Result<Vec<OptionChainContract>> {
        unwrap_list(&mut value, "list");
        // Tolerate what the SDK's own option-chain decoder tolerates: `""`
        // strike → 0, and `null`/missing symbol, cycle type, standard attr and
        // days-to-expiry → their defaults, rather than failing the whole chain.
        set_empty_at(&mut value, &["*.strike_price"], &Value::String("0".into()));
        for key in ["symbol", "option_type", "standard_attr"] {
            default_null_or_missing(&mut value, key, &Value::String(String::new()));
        }
        default_null_or_missing(&mut value, "days_to_expiry", &Value::from(0));
        map_str_enum::<longbridge::quote::OptionDirection>(&mut value, &["*.direction"]);
        map_str_enum::<longbridge::quote::OptionExpiryCycleType>(&mut value, &["*.option_type"]);
        map_str_enum::<longbridge::quote::OptionStandardAttr>(&mut value, &["*.standard_attr"]);
        reformat_ymd(&mut value, &["*.expiry_date"]);
        from_value(value)
    }

    pub async fn option_chain_info_by_date(
        &self,
        symbol: String,
        expiry_date: time::Date,
    ) -> Result<Vec<OptionChainContract>> {
        let ymd = format!(
            "{:04}{:02}{:02}",
            expiry_date.year(),
            expiry_date.month() as u8,
            expiry_date.day()
        );
        let value = http_post(
            "/quote/options/strikes",
            serde_json::json!({ "symbol": symbol, "expiry_date": ymd, "standard_only": false }),
        )
        .await?;
        Self::reshape_option_chain(value)
    }

    /// Reshape `/quote/options/quotes` into `Vec<OptionQuote>` (unwrap
    /// `secu_quote`; lift `option_extend`; numify `volume`/`open_interest`; unix
    /// `timestamp`; map `trade_status`; the string-code option enums; dates).
    fn reshape_option_quote(mut value: Value) -> Result<Vec<OptionQuote>> {
        unwrap(&mut value, "secu_quote");
        drop_keys(&mut value, &["volume_str"]);
        lift_nested(&mut value, "option_extend");
        convert_unix_paths(&mut value, &["*.timestamp"]);
        // Untraded price fields come back as "" — the SDK's non-`Option`
        // `Decimal`/`i64`s need a real 0 (matching the WS `unwrap_or_default`).
        // Before numify so "" → "0" → number.
        zero_empty_numeric(&mut value, OPTION_QUOTE_NUMERIC);
        numify_paths(&mut value, &["*.volume", "*.open_interest"]);
        map_int_enum(&mut value, &["*.trade_status"], &trade_status);
        map_str_enum::<longbridge::quote::OptionType>(&mut value, &["*.contract_type"]);
        map_str_enum::<longbridge::quote::OptionDirection>(&mut value, &["*.direction"]);
        reformat_ymd(&mut value, &["*.expiry_date"]);
        from_value(value)
    }

    pub async fn option_quote(&self, symbols: Vec<String>) -> Result<Vec<OptionQuote>> {
        let value = http_post(
            "/quote/options/quotes",
            serde_json::json!({ "symbol": symbols }),
        )
        .await?;
        Self::reshape_option_quote(value)
    }

    /// Reshape `/quote/warrants/quotes` into `Vec<WarrantQuote>` (unwrap
    /// `secu_quote`; lift `warrant_extend`; numify `volume`/`outstanding_quantity`;
    /// unix `timestamp`; map `trade_status`/`category`; dates).
    fn reshape_warrant_quote(mut value: Value) -> Result<Vec<WarrantQuote>> {
        unwrap(&mut value, "secu_quote");
        drop_keys(&mut value, &["volume_str"]);
        lift_nested(&mut value, "warrant_extend");
        // The REST payload names it `outstanding_qty`; the SDK field is
        // `outstanding_quantity`.
        rename_keys(&mut value, &[("outstanding_qty", "outstanding_quantity")]);
        convert_unix_paths(&mut value, &["*.timestamp"]);
        // `category` already arrives as the SDK `WarrantType` name ("Bull"), so it
        // is not int-mapped (unlike `warrant_list`, whose `type` is an int); an
        // unknown or empty name degrades to `Unknown` like the WS path.
        default_unparseable_enum::<longbridge::quote::WarrantType>(
            &mut value,
            &["*.category"],
            "Unknown",
        );
        // Untraded price fields come back as "" — the SDK's non-`Option`
        // `Decimal`/`i64`s need a real 0 (matching the WS `unwrap_or_default`).
        // Before numify so "" → "0" → number.
        zero_empty_numeric(&mut value, WARRANT_QUOTE_NUMERIC);
        numify_paths(&mut value, &["*.volume", "*.outstanding_quantity"]);
        map_int_enum(&mut value, &["*.trade_status"], &trade_status);
        reformat_ymd(&mut value, &["*.expiry_date", "*.last_trade_date"]);
        from_value(value)
    }

    pub async fn warrant_quote(&self, symbols: Vec<String>) -> Result<Vec<WarrantQuote>> {
        let value = http_post(
            "/quote/warrants/quotes",
            serde_json::json!({ "symbol": symbols }),
        )
        .await?;
        Self::reshape_warrant_quote(value)
    }

    /// Reshape `/quote/warrants` into `Vec<WarrantInfo>` (unwrap `warrant_list`;
    /// rename `change_val`→`change_value` & `type`→`warrant_type`; map
    /// `warrant_type`/`status` enums; `""`→null; numify counts; dates).
    fn reshape_warrant_list(mut value: Value) -> Result<Vec<WarrantInfo>> {
        unwrap(&mut value, "warrant_list");
        rename_keys(
            &mut value,
            &[("change_val", "change_value"), ("type", "warrant_type")],
        );
        map_int_enum(&mut value, &["*.warrant_type"], &warrant_type);
        map_int_enum(&mut value, &["*.status"], &warrant_status);
        // `WarrantInfo` mixes non-`Option` numeric fields (zero when empty, as
        // the WS `unwrap_or_default`) with `Option` fields (null when empty).
        // Zero the former first, then null everything else still empty.
        empty_keys_to_zero(
            &mut value,
            &[
                "last_done",
                "change_rate",
                "change_value",
                "volume",
                "turnover",
                "outstanding_qty",
                "outstanding_ratio",
                "premium",
                "leverage_ratio",
            ],
        );
        numify_paths(&mut value, &["*.volume", "*.outstanding_qty"]);
        reformat_ymd(&mut value, &["*.expiry_date"]);
        empty_str_to_null(&mut value, &["symbol", "name"]);
        from_value(value)
    }

    pub async fn warrant_list(&self, symbol: String) -> Result<Vec<WarrantInfo>> {
        // The WS path returns the whole list in one consistent snapshot. Over
        // the shim it has to be paged, which needs a *static* sort key: paging
        // by last price (what WS sorts by) is unsafe because prices move
        // between page fetches and a warrant can slide across a page boundary —
        // fetched twice or skipped entirely (observed: 715 of 716). Expiry date
        // does not move intraday, so fetch by that, then order locally by last
        // price descending to match the WS output.
        let (total, rows) = fetch_warrant_pages(&symbol).await?;
        let mut rows = dedupe_by_symbol(rows);
        let expected = total.min(WARRANT_MAX_ROWS);
        if rows.len() < expected {
            // Expiry dates repeat, so inside one date the gateway's tiebreak
            // decides the order and a row can still slip across a page boundary.
            // A short read is a missing warrant: take a second pass and merge it.
            let (_, retry) = fetch_warrant_pages(&symbol).await?;
            rows = merge_warrant_passes(rows, retry);
            if rows.len() < expected {
                tracing::warn!(
                    "warrant list for {symbol}: {} of {expected} rows after two passes",
                    rows.len()
                );
                eprintln!(
                    "Warning: the gateway returned {} of {expected} warrants for {symbol}; \
                     the list may be incomplete.",
                    rows.len()
                );
            }
        }
        Self::reshape_warrant_list(
            serde_json::json!({ "warrant_list": order_by_last_done_desc(rows) }),
        )
    }

    /// Reshape `/quote/calc-indexes` into `Vec<SecurityCalcIndex>` (unwrap
    /// `security_calc_index`; rename `change_val`→`change_value`; drop
    /// `volume_str`; numify counts; dates; `""`→null; null the
    /// indexes that were not requested).
    fn reshape_calc_indexes(
        mut value: Value,
        requested: &[longbridge::quote::CalcIndex],
    ) -> Result<Vec<SecurityCalcIndex>> {
        unwrap_list(&mut value, "security_calc_index");
        rename_keys(&mut value, &[("change_val", "change_value")]);
        drop_keys(&mut value, &["volume_str"]);
        // WS reports a requested integer index as `Some(n)` even when the wire
        // value is empty (proto default 0); unrequested ones are nulled below.
        zero_empty_numeric(
            &mut value,
            &["*.volume", "*.outstanding_qty", "*.open_interest"],
        );
        numify_paths(
            &mut value,
            &["*.volume", "*.outstanding_qty", "*.open_interest"],
        );
        // proto-JSON may also omit a default-0 field or send `null`; WS still
        // reads it as 0 for a requested index.
        for key in ["volume", "outstanding_qty", "open_interest"] {
            default_null_or_missing(&mut value, key, &Value::from(0));
        }
        reformat_ymd(&mut value, &["*.expiry_date"]);
        empty_str_to_null(&mut value, &["symbol"]);
        keep_requested_indexes(&mut value, requested);
        from_value(value)
    }

    pub async fn calc_indexes(
        &self,
        symbols: Vec<String>,
        indexes: Vec<longbridge::quote::CalcIndex>,
    ) -> Result<Vec<SecurityCalcIndex>> {
        let index_ints: Vec<i32> = indexes.iter().map(|i| calc_index_wire(*i)).collect();
        let value = http_post(
            "/quote/calc-indexes",
            serde_json::json!({ "symbols": symbols, "calc_index": index_ints }),
        )
        .await?;
        Self::reshape_calc_indexes(value, &indexes)
    }
}

/// `from_value` with the error message preserved, so a reshape miss is
/// diagnosable (`serde_json` reports the missing/mistyped field).
fn from_value<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|e| anyhow::anyhow!("decode gateway response: {e}"))
}

/// The WS-backed implementation, for endpoints the gateway does not expose over
/// the WS→HTTP shim (`market_temperature`, `security_list`, `watchlist`,
/// `subscriptions`, `us_crypto_overview`, …) — mirrors longbridge-mcp#162, which
/// likewise keeps those on the `QuoteContext`. Uses the tracking `quote_cmd()`
/// accessor, as every `src/cli` access must (enforced by
/// `cli_uses_only_tracking_quote_accessor`); the one path that reaches it today,
/// `static` on a `.BKKT` symbol, therefore fires the beacon twice.
fn ws() -> crate::cli::api::LbQuoteApi {
    crate::cli::api::LbQuoteApi::new(crate::openapi::quote_cmd())
}

/// Full `QuoteApi` so the trait-object path (`run_static`, `run_option_chain`)
/// shares the same HTTP backend as the direct command entry points. Shim-migrated
/// methods hit HTTP; the rest delegate to [`ws`] — the trait requires them, though
/// today only `us_crypto_overview` (via `run_static`) is reached that way.
#[async_trait::async_trait]
impl crate::cli::api::QuoteApi for HttpQuoteApi {
    async fn quote(&self, symbols: Vec<String>) -> Result<Vec<SecurityQuote>> {
        HttpQuoteApi::quote(self, symbols).await
    }
    async fn depth(&self, symbol: String) -> Result<SecurityDepth> {
        HttpQuoteApi::depth(self, symbol).await
    }
    async fn brokers(&self, symbol: String) -> Result<SecurityBrokers> {
        HttpQuoteApi::brokers(self, symbol).await
    }
    async fn trades(&self, symbol: String, count: usize) -> Result<Vec<Trade>> {
        HttpQuoteApi::trades(self, symbol, count).await
    }
    async fn intraday(&self, symbol: String) -> Result<Vec<IntradayLine>> {
        HttpQuoteApi::intraday(self, symbol, longbridge::quote::TradeSessions::Intraday).await
    }
    async fn candlesticks(
        &self,
        symbol: String,
        period: longbridge::quote::Period,
        count: usize,
        adjust: longbridge::quote::AdjustType,
    ) -> Result<Vec<Candlestick>> {
        HttpQuoteApi::candlesticks(
            self,
            symbol,
            period,
            count,
            adjust,
            longbridge::quote::TradeSessions::Intraday,
        )
        .await
    }
    async fn history_candlesticks_by_date(
        &self,
        symbol: String,
        period: longbridge::quote::Period,
        adjust: longbridge::quote::AdjustType,
        start: Option<time::Date>,
        end: Option<time::Date>,
    ) -> Result<Vec<Candlestick>> {
        // The trait carries no session parameter (parity with `LbQuoteApi`,
        // which also queries the regular session only).
        HttpQuoteApi::history_candlesticks_by_date(
            self,
            symbol,
            period,
            adjust,
            start,
            end,
            longbridge::quote::TradeSessions::Intraday,
        )
        .await
    }
    async fn history_candlesticks_by_offset(
        &self,
        symbol: String,
        period: longbridge::quote::Period,
        adjust: longbridge::quote::AdjustType,
        count: usize,
    ) -> Result<Vec<Candlestick>> {
        HttpQuoteApi::history_candlesticks_by_offset(
            self,
            symbol,
            period,
            adjust,
            false,
            None,
            count,
            longbridge::quote::TradeSessions::Intraday,
        )
        .await
    }
    async fn static_info(&self, symbols: Vec<String>) -> Result<Vec<SecurityStaticInfo>> {
        HttpQuoteApi::static_info(self, symbols).await
    }
    async fn capital_flow(&self, symbol: String) -> Result<Vec<CapitalFlowLine>> {
        HttpQuoteApi::capital_flow(self, symbol).await
    }
    async fn capital_distribution(&self, symbol: String) -> Result<CapitalDistributionResponse> {
        HttpQuoteApi::capital_distribution(self, symbol).await
    }
    async fn trading_session(&self) -> Result<Vec<MarketTradingSession>> {
        HttpQuoteApi::trading_session(self).await
    }
    async fn trading_days(
        &self,
        market: longbridge::Market,
        begin: time::Date,
        end: time::Date,
    ) -> Result<MarketTradingDays> {
        HttpQuoteApi::trading_days(self, market, begin, end).await
    }
    async fn participants(&self) -> Result<Vec<ParticipantInfo>> {
        HttpQuoteApi::participants(self).await
    }
    async fn option_chain_expiry_date_list(&self, symbol: String) -> Result<Vec<time::Date>> {
        HttpQuoteApi::option_chain_expiry_date_list(self, symbol).await
    }
    async fn warrant_issuers(&self) -> Result<Vec<longbridge::quote::IssuerInfo>> {
        HttpQuoteApi::warrant_issuers(self).await
    }
    async fn calc_indexes(
        &self,
        symbols: Vec<String>,
        indexes: Vec<longbridge::quote::CalcIndex>,
    ) -> Result<Vec<SecurityCalcIndex>> {
        HttpQuoteApi::calc_indexes(self, symbols, indexes).await
    }
    async fn option_quote(&self, symbols: Vec<String>) -> Result<Vec<OptionQuote>> {
        HttpQuoteApi::option_quote(self, symbols).await
    }
    async fn option_chain_info_by_date(
        &self,
        symbol: String,
        expiry_date: time::Date,
    ) -> Result<Vec<OptionChainContract>> {
        HttpQuoteApi::option_chain_info_by_date(self, symbol, expiry_date).await
    }
    async fn warrant_quote(&self, symbols: Vec<String>) -> Result<Vec<WarrantQuote>> {
        HttpQuoteApi::warrant_quote(self, symbols).await
    }
    async fn warrant_list(&self, symbol: String) -> Result<Vec<WarrantInfo>> {
        HttpQuoteApi::warrant_list(self, symbol).await
    }

    // ── not exposed over the shim — stay on the WS `QuoteContext` ───────────
    async fn us_crypto_overview(&self, symbol: String) -> Result<Value> {
        ws().us_crypto_overview(symbol).await
    }
    async fn market_temperature(
        &self,
        market: longbridge::Market,
    ) -> Result<longbridge::quote::MarketTemperature> {
        ws().market_temperature(market).await
    }
    async fn history_market_temperature(
        &self,
        market: longbridge::Market,
        start: time::Date,
        end: time::Date,
    ) -> Result<longbridge::quote::HistoryMarketTemperatureResponse> {
        ws().history_market_temperature(market, start, end).await
    }
    async fn security_list(
        &self,
        market: longbridge::Market,
    ) -> Result<Vec<longbridge::quote::Security>> {
        ws().security_list(market).await
    }
    async fn subscriptions(&self) -> Result<Vec<longbridge::quote::Subscription>> {
        ws().subscriptions().await
    }
    async fn watchlist(&self) -> Result<Vec<longbridge::quote::WatchlistGroup>> {
        ws().watchlist().await
    }
    async fn create_watchlist_group(&self, name: String) -> Result<i64> {
        ws().create_watchlist_group(name).await
    }
    async fn delete_watchlist_group(&self, id: i64) -> Result<()> {
        ws().delete_watchlist_group(id).await
    }
    async fn update_watchlist_group(
        &self,
        req: longbridge::quote::RequestUpdateWatchlistGroup,
    ) -> Result<()> {
        ws().update_watchlist_group(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use longbridge::quote::{TradeSession, TradeStatus};

    /// A real `/quote/quotes` response captured from the test gateway
    /// (700.HK + AAPL.US), proving the reshape round-trips into the typed SDK
    /// struct: string `int64`s, unix timestamps, wire-int `trade_status`, and
    /// the `over_night_quote` rename.
    const QUOTES_FIXTURE: &str = r#"{
      "secu_quote": [
        {
          "high": "425.800", "last_done": "424.800", "low": "414.800",
          "open": "415.000", "over_night_quote": null, "post_market_quote": null,
          "pre_market_quote": null, "prev_close": "411.400", "symbol": "700.HK",
          "timestamp": "1791533288", "trade_status": 0,
          "turnover": "8610387928.000", "volume": "20422500", "volume_str": ""
        },
        {
          "high": "341.570", "last_done": "340.420", "low": "335.900",
          "open": "336.815",
          "over_night_quote": {
            "high": "340.820", "last_done": "337.000", "low": "336.230",
            "prev_close": "340.420", "timestamp": "1791532800",
            "turnover": "26803738.000", "volume": "79180"
          },
          "post_market_quote": {
            "high": "340.790", "last_done": "340.600", "low": "340.030",
            "prev_close": "340.420", "timestamp": "1791503997",
            "turnover": "1809749608.878", "volume": "5316207"
          },
          "pre_market_quote": {
            "high": "340.590", "last_done": "334.037", "low": "332.940",
            "prev_close": "340.420", "timestamp": "1791544639",
            "turnover": "147675771.321", "volume": "441532"
          },
          "prev_close": "336.670", "symbol": "AAPL.US", "timestamp": "1791504000",
          "trade_status": 0, "turnover": "11993652713.000",
          "volume": "35332449", "volume_str": ""
        }
      ]
    }"#;

    /// Real `/quote/depth` response (700.HK): `ask`/`bid` roots, string
    /// `order_num`/`volume`, padded decimal `price`.
    const DEPTH_FIXTURE: &str = r#"{
      "ask": [
        {"order_num": "272", "position": 1, "price": "425.000", "volume": "908200", "volume_str": ""},
        {"order_num": "62", "position": 2, "price": "425.200", "volume": "65300", "volume_str": ""}
      ],
      "bid": [
        {"order_num": "5", "position": 1, "price": "424.800", "volume": "1200", "volume_str": ""}
      ]
    }"#;

    /// Real `/quote/brokers` response (700.HK): numeric `broker_ids`.
    const BROKERS_FIXTURE: &str = r#"{
      "symbol": "700.HK",
      "ask_brokers": [{"position": 1, "broker_ids": [6999, 5999, 8914]}],
      "bid_brokers": [{"position": 1, "broker_ids": [2021, 2456]}]
    }"#;

    /// Real `/quote/options/quotes` response (test gateway): option-specific
    /// fields nested under `option_extend`, `""` price fields, `"A"`/`"C"` codes.
    const OPTION_QUOTE_FIXTURE: &str = r#"{
      "secu_quote": [{
        "high": "", "last_done": "208.50", "low": "", "open": "",
        "option_extend": {
          "contract_multiplier": "100", "contract_size": "100",
          "contract_type": "A", "direction": "C", "expiry_date": "20261016",
          "historical_volatility": "0.2027", "implied_volatility": "2.573",
          "open_interest": "37", "strike_price": "130", "underlying_symbol": "AAPL.US"
        },
        "prev_close": "208.50", "symbol": "AAPL261016C130000.US",
        "timestamp": "1791576900", "trade_status": 0, "turnover": "", "volume": "0"
      }]
    }"#;

    /// Real `/quote/warrants/quotes` response: fields under `warrant_extend`,
    /// `category` already the `WarrantType` name, `outstanding_qty` → rename.
    const WARRANT_QUOTE_FIXTURE: &str = r#"{
      "secu_quote": [{
        "high": "", "last_done": "0.010", "low": "", "open": "",
        "prev_close": "0.010", "symbol": "69926.HK", "timestamp": "1791455462",
        "trade_status": 10, "turnover": "", "volume": "0",
        "warrant_extend": {
          "call_price": "415", "category": "Bull", "conversion_ratio": "500",
          "expiry_date": "20270225", "implied_volatility": "0.000",
          "last_trade_date": "20261008", "lower_strike_price": "0",
          "outstanding_qty": "27800000", "outstanding_ratio": "0.1853",
          "strike_price": "412", "underlying_symbol": "700.HK", "upper_strike_price": "0"
        }
      }]
    }"#;

    /// Real `/quote/warrants` response: `type`/`status` ints, `change_val`
    /// rename, many `""` (Option) fields.
    const WARRANT_LIST_FIXTURE: &str = r#"{
      "total_count": 708,
      "warrant_list": [{
        "balance_point": "", "call_price": "", "change_rate": "0",
        "change_val": "0", "conversion_ratio": "", "delta": "0",
        "effective_leverage": "", "expiry_date": "", "implied_volatility": "0",
        "itm_otm": "", "last_done": "0.01", "leverage_ratio": "",
        "lower_strike_price": "", "name": "SG#TENCTRC2702B", "outstanding_qty": "",
        "outstanding_ratio": "", "premium": "", "status": 0, "strike_price": "",
        "symbol": "69926.HK", "to_call_price": "", "turnover": "0", "type": 0,
        "upper_strike_price": "", "volume": "0"
      }]
    }"#;

    /// Real `/quote/options/strikes` response: `C`/`P` direction codes,
    /// `YYYYMMDD` expiry, numeric `days_to_expiry`.
    const STRIKES_FIXTURE: &str = r#"{
      "list": [{
        "days_to_expiry": 7, "direction": "C", "expiry_date": "20261016",
        "option_type": "", "standard_attr": "", "strike_price": "130",
        "symbol": "AAPL261016C130000.US"
      }]
    }"#;

    /// Real `/quote/calc-indexes` response: `change_val` rename, many `""`
    /// (Option) fields, `change_rate` populated.
    const CALC_INDEX_FIXTURE: &str = r#"{
      "security_calc_index": [{
        "amplitude": "", "balance_point": "", "call_price": "",
        "capital_flow": "", "change_rate": "3.26", "change_val": "13.400",
        "conversion_ratio": "", "delta": "", "dividend_ratio_ttm": "1.25",
        "effective_leverage": "", "expiry_date": "", "gamma": "",
        "implied_volatility": "", "open_interest": "", "outstanding_qty": "",
        "pb_ratio": "2.95", "pe_ttm_ratio": "14.24", "rho": "", "symbol": "700.HK",
        "total_market_value": "3862538856756.000", "turnover_rate": "0.22",
        "vega": "", "volume": ""
      }]
    }"#;

    #[test]
    fn http_errors_reach_the_main_error_renderer() {
        // `main.rs` matches `longbridge::Error::HttpClient(HttpClientError::OpenApi
        // { .. })` to print `API error (code N)`, the trace id and the OPRA
        // guidance. The HTTP backend's errors must downcast to exactly that.
        use longbridge::httpclient::HttpClientError;
        let err = to_cli_error(HttpClientError::OpenApi {
            code: 301_604,
            message: "no quote access".into(),
            trace_id: "trace-1".into(),
        });
        match err.downcast_ref::<longbridge::Error>() {
            Some(longbridge::Error::HttpClient(HttpClientError::OpenApi {
                code,
                trace_id,
                ..
            })) => {
                assert_eq!(*code, 301_604);
                assert_eq!(trace_id, "trace-1");
            }
            other => {
                panic!("HTTP error did not downcast to longbridge::Error::HttpClient: {other:?}")
            }
        }
    }

    #[test]
    fn rate_limit_errors_are_detected() {
        use longbridge::httpclient::HttpClientError;
        let api = |code| {
            to_cli_error(HttpClientError::OpenApi {
                code,
                message: String::new(),
                trace_id: String::new(),
            })
        };
        assert!(is_rate_limited(&api(429_002)));
        // A bare HTTP 429 is retried inside the SDK's `send()`; matching it here
        // would nest the two retry loops.
        assert!(!is_rate_limited(&to_cli_error(HttpClientError::BadStatus(
            reqwest::StatusCode::TOO_MANY_REQUESTS
        ))));
        assert!(!is_rate_limited(&api(301_604)));
        assert!(!is_rate_limited(&to_cli_error(HttpClientError::BadStatus(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        ))));
        assert!(!is_rate_limited(&anyhow::anyhow!("other")));
    }

    #[test]
    fn order_by_last_done_desc_is_numeric_and_stable() {
        let rows = vec![
            serde_json::json!({"symbol": "A", "last_done": "9.5"}),
            serde_json::json!({"symbol": "B", "last_done": "10"}),
            serde_json::json!({"symbol": "C", "last_done": "0.01"}),
            serde_json::json!({"symbol": "D", "last_done": "0.01"}), // tie with C
            serde_json::json!({"symbol": "E", "last_done": ""}),     // empty → 0
        ];
        let syms: Vec<String> = order_by_last_done_desc(rows)
            .iter()
            .map(|r| r["symbol"].as_str().unwrap().to_owned())
            .collect();
        // Numeric, not lexicographic ("10" above "9.5"); C stays before D.
        assert_eq!(syms, ["B", "A", "C", "D", "E"]);
    }

    #[test]
    fn page_offsets_cover_total_and_respect_cap() {
        // 716 warrants at 100/page: page 1 + offsets 100..=700.
        assert_eq!(
            page_offsets(716, 100, 200),
            vec![100, 200, 300, 400, 500, 600, 700]
        );
        assert_eq!(page_offsets(100, 100, 200), Vec::<usize>::new()); // exactly one page
        assert_eq!(page_offsets(0, 100, 200), Vec::<usize>::new()); // empty list
        assert_eq!(page_offsets(1_000_000, 100, 200).len(), 199); // runaway cap
    }

    #[test]
    fn dedupe_by_symbol_keeps_first_and_order() {
        let rows = vec![
            serde_json::json!({"symbol": "A.HK", "n": 1}),
            serde_json::json!({"symbol": "B.HK", "n": 2}),
            serde_json::json!({"symbol": "A.HK", "n": 3}), // repeated page row
            serde_json::json!({"n": 4}),                   // no symbol: kept
        ];
        let out = dedupe_by_symbol(rows);
        let ns: Vec<i64> = out.iter().map(|r| r["n"].as_i64().unwrap()).collect();
        assert_eq!(ns, vec![1, 2, 4]);
    }

    #[test]
    fn calc_index_wire_matches_proto() {
        // Pin the SDK→proto `CalcIndex` wire mapping to the real proto
        // enumeration values (longbridge.quote.v1 CalcIndex). If the SDK enum is
        // ever reordered, this fails here instead of sending wrong indexes.
        use longbridge::quote::CalcIndex as C;
        for (idx, wire) in [
            (C::LastDone, 1),
            (C::ChangeValue, 2),
            (C::ChangeRate, 3),
            (C::Volume, 4),
            (C::Turnover, 5),
            (C::PeTtmRatio, 12),
            (C::PbRatio, 13),
            (C::DividendRatioTtm, 14),
            (C::ExpiryDate, 19),
            (C::OpenInterest, 35),
            (C::Delta, 36),
            (C::Rho, 40),
        ] {
            assert_eq!(
                calc_index_wire(idx),
                wire,
                "{idx:?} must map to proto {wire}"
            );
        }
    }

    #[test]
    fn reshape_static_info_degrades_unknown_board() {
        // A security with no earnings data ("" eps/shares) and an unrecognized
        // board must decode (eps→0, board→Unknown) rather than fail the batch.
        let raw: Value = serde_json::from_str(
            r#"{"secu_static_info":[{
                "board":"NonsenseBoard","bps":"","circulating_shares":"",
                "currency":"USD","dividend_yield":"","eps":"","eps_ttm":"",
                "exchange":"X","hk_shares":"","listing_date":"20200101",
                "lot_size":"1","name_cn":"a","name_en":"a","name_hk":"a",
                "stock_derivatives":[],"symbol":"T.US","total_shares":""
            }]}"#,
        )
        .unwrap();
        let infos = HttpQuoteApi::reshape_static_info(raw).expect("reshape static_info");
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].board, longbridge::quote::SecurityBoard::Unknown);
        assert_eq!(infos[0].eps.to_string(), "0"); // "" → 0
        assert_eq!(infos[0].total_shares, 0);
    }

    #[test]
    fn reshape_trading_days_defaults_missing_half_days() {
        // Gateway omits the empty `half_trade_day` array → must not fail.
        let raw: Value = serde_json::from_str(r#"{"trade_day":["20261012","20261013"]}"#).unwrap();
        let days = HttpQuoteApi::reshape_trading_days(raw).expect("reshape trading_days");
        assert_eq!(days.trading_days.len(), 2);
        assert_eq!(days.half_trading_days, Vec::<time::Date>::new());
    }

    #[test]
    fn reshape_option_quote_round_trips() {
        let raw: Value = serde_json::from_str(OPTION_QUOTE_FIXTURE).unwrap();
        let q = HttpQuoteApi::reshape_option_quote(raw).expect("reshape option_quote");
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].symbol, "AAPL261016C130000.US");
        assert_eq!(q[0].open_interest, 37);
        assert_eq!(q[0].strike_price.to_string(), "130");
        assert_eq!(q[0].high.to_string(), "0"); // "" → 0
        assert_eq!(q[0].contract_type, longbridge::quote::OptionType::American); // "A"
        assert_eq!(q[0].direction, longbridge::quote::OptionDirection::Call); // "C"
    }

    #[test]
    fn reshape_warrant_quote_round_trips() {
        let raw: Value = serde_json::from_str(WARRANT_QUOTE_FIXTURE).unwrap();
        let q = HttpQuoteApi::reshape_warrant_quote(raw).expect("reshape warrant_quote");
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].outstanding_quantity, 27_800_000);
        assert_eq!(q[0].category, longbridge::quote::WarrantType::Bull);
        assert_eq!(q[0].high.to_string(), "0"); // "" → 0
        assert_eq!(q[0].trade_status, TradeStatus::SuspendTrade); // int 10
    }

    #[test]
    fn reshape_warrant_list_round_trips() {
        let raw: Value = serde_json::from_str(WARRANT_LIST_FIXTURE).unwrap();
        let w = HttpQuoteApi::reshape_warrant_list(raw).expect("reshape warrant_list");
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].warrant_type, longbridge::quote::WarrantType::Call); // int 0
        assert_eq!(w[0].volume, 0);
        assert!(w[0].strike_price.is_none()); // "" → null → None
        assert!(w[0].expiry_date.is_none());
        assert_eq!(w[0].change_value.to_string(), "0"); // change_val rename
    }

    #[test]
    fn reshape_option_chain_round_trips() {
        let raw: Value = serde_json::from_str(STRIKES_FIXTURE).unwrap();
        let c = HttpQuoteApi::reshape_option_chain(raw).expect("reshape option_chain");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].direction, longbridge::quote::OptionDirection::Call); // "C"
        assert_eq!(c[0].days_to_expiry, 7);
        assert_eq!(c[0].expiry_date.to_string(), "2026-10-16"); // reformatted

        // "" cycle type / standard attr are the common case: Monthly / Normal.
        assert_eq!(
            c[0].option_type,
            longbridge::quote::OptionExpiryCycleType::Monthly
        );
        assert_eq!(
            c[0].standard_attr,
            longbridge::quote::OptionStandardAttr::Normal
        );
    }

    #[test]
    fn reshape_calc_indexes_round_trips() {
        use longbridge::quote::CalcIndex;
        let raw: Value = serde_json::from_str(CALC_INDEX_FIXTURE).unwrap();
        let requested = [
            CalcIndex::PeTtmRatio,
            CalcIndex::Volume,
            CalcIndex::Amplitude,
            CalcIndex::ChangeValue,
        ];
        let r = HttpQuoteApi::reshape_calc_indexes(raw, &requested).expect("reshape calc_indexes");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].pe_ttm_ratio.unwrap().to_string(), "14.24");
        // A requested integer index that is empty on the wire is `Some(0)`, as WS.
        assert_eq!(r[0].volume, Some(0));
        assert!(r[0].amplitude.is_none());
        assert_eq!(r[0].change_value.unwrap().to_string(), "13.400"); // change_val rename
                                                                      // Sent by the gateway but not requested → None, as on the WS path.
        assert!(r[0].change_rate.is_none());
    }

    #[test]
    fn reshape_depth_round_trips() {
        let raw: Value = serde_json::from_str(DEPTH_FIXTURE).unwrap();
        let depth = HttpQuoteApi::reshape_depth(raw).expect("reshape depth");
        assert_eq!(depth.asks.len(), 2);
        assert_eq!(depth.bids.len(), 1);
        assert_eq!(depth.asks[0].volume, 908_200);
        assert_eq!(depth.asks[0].order_num, 272);
        assert_eq!(depth.asks[0].price.unwrap().to_string(), "425.000");
    }

    #[test]
    fn reshape_depth_empty_level_nulls_price() {
        // An empty ask level (`price: ""`, "0" counts) must decode: price→None,
        // counts→0 (regression for the round-2 TSLA.US case).
        let raw: Value = serde_json::from_str(
            r#"{"ask":[{"order_num":"0","position":1,"price":"","volume":"0","volume_str":""}],"bid":[]}"#,
        )
        .unwrap();
        let d = HttpQuoteApi::reshape_depth(raw).expect("reshape depth empty");
        assert_eq!(d.asks.len(), 1);
        assert!(d.asks[0].price.is_none());
        assert_eq!(d.asks[0].volume, 0);
        assert_eq!(d.asks[0].order_num, 0);
    }

    #[test]
    fn reshape_brokers_round_trips() {
        let raw: Value = serde_json::from_str(BROKERS_FIXTURE).unwrap();
        let brokers = HttpQuoteApi::reshape_brokers(raw).expect("reshape brokers");
        assert_eq!(brokers.ask_brokers[0].position, 1);
        assert_eq!(brokers.ask_brokers[0].broker_ids, vec![6999, 5999, 8914]);
        assert_eq!(brokers.bid_brokers[0].broker_ids, vec![2021, 2456]);
    }

    #[test]
    fn reshape_quotes_round_trips_into_sdk_struct() {
        let raw: Value = serde_json::from_str(QUOTES_FIXTURE).unwrap();
        let quotes = HttpQuoteApi::reshape_quotes(raw).expect("reshape should decode");
        assert_eq!(quotes.len(), 2);

        let tencent = &quotes[0];
        assert_eq!(tencent.symbol, "700.HK");
        assert_eq!(tencent.volume, 20_422_500);
        assert_eq!(tencent.last_done.to_string(), "424.800");
        assert!(tencent.overnight_quote.is_none());

        let apple = &quotes[1];
        assert_eq!(apple.symbol, "AAPL.US");
        assert_eq!(apple.volume, 35_332_449);
        let overnight = apple
            .overnight_quote
            .as_ref()
            .expect("AAPL overnight present");
        assert_eq!(overnight.volume, 79_180);
        // trade_status wire int 0 → SDK `Normal`.
        assert_eq!(apple.trade_status, TradeStatus::Normal);
    }

    // ── reshape fixtures captured from the production gateway (TSLA.US) ──────

    fn ts(secs: i64) -> time::OffsetDateTime {
        time::OffsetDateTime::from_unix_timestamp(secs).unwrap()
    }

    #[test]
    fn reshape_trades_round_trips() {
        let raw = serde_json::json!({"symbol": "TSLA.US", "trades": [
            {"direction": 0, "price": "319.450", "timestamp": "1786046372", "trade_session": 0,
             "trade_type": "I", "volume": "20", "volume_str": ""},
            {"direction": 2, "price": "", "timestamp": "1786046373", "trade_session": 3,
             "trade_type": "", "volume": "", "volume_str": ""}
        ]});
        let t = HttpQuoteApi::reshape_trades(raw).expect("reshape trades");
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].price.to_string(), "319.450");
        assert_eq!(t[0].volume, 20);
        assert_eq!(t[0].timestamp, ts(1_786_046_372));
        assert_eq!(t[0].trade_type, "I");
        assert_eq!(t[0].trade_session, TradeSession::Intraday);
        // Untraded "" price/volume → 0; `trade_type` stays an empty string.
        assert!(t[1].price.is_zero());
        assert_eq!(t[1].volume, 0);
        assert_eq!(t[1].trade_type, "");
        assert_eq!(t[1].direction, longbridge::quote::TradeDirection::Up);
        assert_eq!(t[1].trade_session, TradeSession::Overnight);
    }

    #[test]
    fn reshape_intraday_round_trips() {
        let raw = serde_json::json!({"symbol": "TSLA.US", "lines": [
            {"avg_price": "382.063219", "price": "382.330", "timestamp": "1791552600",
             "turnover": "191223787.501", "volume": "500503", "volume_str": ""},
            {"avg_price": "", "price": "", "timestamp": "1791552660",
             "turnover": "", "volume": "", "volume_str": ""}
        ]});
        let l = HttpQuoteApi::reshape_intraday(raw).expect("reshape intraday");
        assert_eq!(l[0].price.to_string(), "382.330");
        assert_eq!(l[0].avg_price.to_string(), "382.063219");
        assert_eq!(l[0].volume, 500_503);
        assert_eq!(l[0].turnover.to_string(), "191223787.501");
        assert_eq!(l[0].timestamp, ts(1_791_552_600));
        assert!(l[1].price.is_zero() && l[1].avg_price.is_zero() && l[1].turnover.is_zero());
        assert_eq!(l[1].volume, 0);
    }

    #[test]
    fn reshape_candlesticks_round_trips() {
        let raw = serde_json::json!({"symbol": "TSLA.US", "candlesticks": [
            {"close": "377.810", "high": "382.350", "low": "374.430", "open": "378.445",
             "timestamp": "1791345600", "trade_session": 0, "turnover": "9646485110.000",
             "volume": "25594563", "volume_str": ""},
            {"close": "375.000", "high": "375.960", "low": "368.030", "open": "374.120",
             "timestamp": "1791432000", "trade_session": 1, "turnover": "10594751818.000",
             "volume": "28394945", "volume_str": ""}
        ]});
        let c = HttpQuoteApi::reshape_candlesticks(raw).expect("reshape candlesticks");
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].close.to_string(), "377.810");
        assert_eq!(c[0].open.to_string(), "378.445");
        assert_eq!(c[0].volume, 25_594_563);
        assert_eq!(c[0].timestamp, ts(1_791_345_600));
        assert_eq!(c[1].trade_session, TradeSession::Pre);
        // `open_updated` is absent on the wire; WS sets it `true` for every bar.
        let json = serde_json::to_value(c[0]).unwrap();
        assert_eq!(json["open_updated"], Value::Bool(true));
    }

    #[test]
    fn reshape_capital_flow_round_trips() {
        let raw = serde_json::json!({"symbol": "TSLA.US", "capital_flow_lines": [
            {"inflow": "-4950686.083", "timestamp": "1786023000"},
            {"inflow": "", "timestamp": "1786023060"}
        ]});
        let f = HttpQuoteApi::reshape_capital_flow(raw).expect("reshape capital_flow");
        assert_eq!(f[0].inflow.to_string(), "-4950686.083");
        assert_eq!(f[0].timestamp, ts(1_786_023_000));
        assert!(f[1].inflow.is_zero());
    }

    #[test]
    fn reshape_capital_distribution_round_trips() {
        let raw = serde_json::json!({
            "capital_in": {"large": "137206566.683", "medium": "316790323.900", "small": "591924592.784"},
            "capital_out": {"large": "80653650.205", "medium": "292432740.398", "small": "551508397.027"},
            "symbol": "TSLA.US", "timestamp": "1786046400"
        });
        let d =
            HttpQuoteApi::reshape_capital_distribution(raw).expect("reshape capital_distribution");
        assert_eq!(d.timestamp, ts(1_786_046_400));
        assert_eq!(d.capital_in.large.to_string(), "137206566.683");
        assert_eq!(d.capital_out.small.to_string(), "551508397.027");
    }

    #[test]
    fn reshape_capital_distribution_zero_timestamp() {
        // Outside trading hours the gateway sends all zeros, timestamp "0".
        let raw = serde_json::json!({
            "capital_in": {"large": "0", "medium": "0", "small": "0"},
            "capital_out": {"large": "0", "medium": "0", "small": "0"},
            "symbol": "700.HK", "timestamp": "0"
        });
        let d = HttpQuoteApi::reshape_capital_distribution(raw).expect("reshape zero distribution");
        assert_eq!(d.timestamp, ts(0));
        assert!(d.capital_in.large.is_zero());
    }

    #[test]
    fn reshape_trading_sessions_round_trips() {
        let raw = serde_json::json!({"market_trade_session": [
            {"market": "US", "trade_session": [
                {"beg_time": 400, "end_time": 930, "trade_session": 1},
                {"beg_time": 930, "end_time": 1600, "trade_session": 0},
                {"beg_time": 1600, "end_time": 2000, "trade_session": 2}]},
            {"market": "HK", "trade_session": [
                {"beg_time": 930, "end_time": 1200, "trade_session": 0}]},
            {"market": "XX", "trade_session": []}
        ]});
        let m = HttpQuoteApi::reshape_trading_sessions(raw).expect("reshape trading_sessions");
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].market, longbridge::Market::US);
        let pre = &m[0].trade_sessions[0];
        assert_eq!(pre.begin_time, time::macros::time!(4:00));
        assert_eq!(pre.end_time, time::macros::time!(9:30));
        assert_eq!(pre.trade_session, TradeSession::Pre);
        assert_eq!(m[0].trade_sessions[2].end_time, time::macros::time!(20:00));
        assert_eq!(m[1].market, longbridge::Market::HK);
        // An unknown market degrades instead of failing the batch.
        assert_eq!(m[2].market, longbridge::Market::Unknown);
    }

    #[test]
    fn reshape_participants_round_trips() {
        let raw = serde_json::json!({"participant_broker_numbers": [
            {"broker_ids": [4208], "participant_name_cn": "汇丰证券经纪亚洲",
             "participant_name_en": "HSBC Securities Brokers (Asia) Limited",
             "participant_name_hk": "匯豐証券經紀亞洲"},
            {"broker_ids": [3718, 3760], "participant_name_cn": "金利丰",
             "participant_name_en": "Kingston", "participant_name_hk": "金利豐"}
        ]});
        let p = HttpQuoteApi::reshape_participants(raw).expect("reshape participants");
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].broker_ids, vec![4208]);
        assert_eq!(p[0].name_en, "HSBC Securities Brokers (Asia) Limited");
        assert_eq!(p[1].name_cn, "金利丰");
        assert_eq!(p[1].name_hk, "金利豐");
    }

    #[test]
    fn reshape_warrant_issuers_round_trips() {
        let raw = serde_json::json!({"issuer_info": [
            {"id": 15, "name_cn": "瑞银", "name_en": "UB", "name_hk": "瑞銀"},
            {"id": 14, "name_cn": "汇丰", "name_en": "HS", "name_hk": "匯豐"}
        ]});
        let i = HttpQuoteApi::reshape_warrant_issuers(raw).expect("reshape warrant_issuers");
        assert_eq!(i[0].issuer_id, 15);
        assert_eq!(i[0].name_en, "UB");
        assert_eq!(i[1].name_hk, "匯豐");
    }

    #[test]
    fn reshape_expiry_dates_round_trips() {
        let raw = serde_json::json!({"expiry_date": ["20261007", "20261009", "20261012"]});
        let d = HttpQuoteApi::reshape_expiry_dates(raw).expect("reshape expiry_dates");
        assert_eq!(
            d,
            vec![
                time::macros::date!(2026 - 10 - 07),
                time::macros::date!(2026 - 10 - 09),
                time::macros::date!(2026 - 10 - 12),
            ]
        );
    }

    #[test]
    fn reshape_quotes_zeroes_empty_prices_in_every_session() {
        // A halted/illiquid name: "" prices in the regular and extended blocks.
        let raw = serde_json::json!({"secu_quote": [{
            "high": "", "last_done": "", "low": "", "open": "", "prev_close": "1.00",
            "symbol": "HALT.US", "timestamp": "1791590400", "trade_status": 0,
            "turnover": "", "volume": "", "volume_str": "",
            "pre_market_quote": {"high": "", "last_done": "", "low": "", "prev_close": "",
                                 "timestamp": "1791552600", "turnover": "", "volume": ""},
            "post_market_quote": null, "over_night_quote": null
        }]});
        let q = HttpQuoteApi::reshape_quotes(raw).expect("reshape empty quote");
        assert!(q[0].last_done.is_zero() && q[0].open.is_zero() && q[0].high.is_zero());
        assert_eq!(q[0].volume, 0);
        assert_eq!(q[0].prev_close.to_string(), "1.00");
        let pre = q[0].pre_market_quote.as_ref().expect("pre-market block");
        assert!(pre.last_done.is_zero() && pre.turnover.is_zero());
        assert_eq!(pre.volume, 0);
        assert_eq!(pre.timestamp, ts(1_791_552_600));
    }

    #[test]
    fn reshape_warrant_list_keeps_empty_name_and_degrades_unknown_enums() {
        // One row with an empty `name` must not fail the whole list (the String
        // field keeps "", only `Option` fields become None), and unknown
        // type/status ints degrade to `Unknown`.
        let raw = serde_json::json!({"warrant_list": [
            {"balance_point": "", "call_price": "", "change_rate": "", "change_val": "",
             "conversion_ratio": "", "delta": "", "effective_leverage": "", "expiry_date": "",
             "implied_volatility": "", "itm_otm": "", "last_done": "", "leverage_ratio": "",
             "lower_strike_price": "", "name": "", "outstanding_qty": "", "outstanding_ratio": "",
             "premium": "", "status": 99, "strike_price": "", "symbol": "12345.HK",
             "to_call_price": "", "turnover": "", "type": 99, "upper_strike_price": "", "volume": ""},
            {"balance_point": "463.2", "call_price": "820", "change_rate": "-0.0137",
             "change_val": "-0.01", "conversion_ratio": "500", "delta": "0",
             "effective_leverage": "0", "expiry_date": "20280802", "implied_volatility": "0",
             "itm_otm": "0.48", "last_done": "0.72", "leverage_ratio": "1.18",
             "lower_strike_price": "0", "name": "UB#TENCTRP2808D", "outstanding_qty": "0",
             "outstanding_ratio": "0", "premium": "-0.09", "status": 4, "strike_price": "823.2",
             "symbol": "61304.HK", "to_call_price": "-0.48", "turnover": "0", "type": 0,
             "upper_strike_price": "0", "volume": "0"}
        ]});
        let w =
            HttpQuoteApi::reshape_warrant_list(raw).expect("one empty name must not fail the list");
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].name, "");
        assert!(w[0].last_done.is_zero());
        assert!(w[0].expiry_date.is_none() && w[0].strike_price.is_none());
        assert_eq!(w[0].warrant_type, longbridge::quote::WarrantType::Unknown);
        assert_eq!(w[0].status, longbridge::quote::WarrantStatus::Unknown);
        assert_eq!(w[1].name, "UB#TENCTRP2808D");
        assert_eq!(w[1].expiry_date, Some(time::macros::date!(2028 - 08 - 02)));
        assert_eq!(w[1].call_price.unwrap().to_string(), "820");
    }

    #[test]
    fn reshape_option_enums_degrade_unknown_codes() {
        use longbridge::quote::{OptionDirection, OptionExpiryCycleType, OptionStandardAttr};
        // New/unknown string codes must degrade to `Unknown`, not fail the batch.
        let raw = serde_json::json!({"list": [{
            "days_to_expiry": 7, "direction": "X", "expiry_date": "20261016",
            "option_type": "M", "standard_attr": "new", "strike_price": "130",
            "symbol": "AAPL261016C130000.US"
        }]});
        let c = HttpQuoteApi::reshape_option_chain(raw).expect("unknown codes must not fail");
        assert_eq!(c[0].direction, OptionDirection::Unknown);
        assert_eq!(c[0].option_type, OptionExpiryCycleType::Unknown);
        assert_eq!(c[0].standard_attr, OptionStandardAttr::Unknown);
    }

    #[test]
    fn calc_index_greeks_pass_through_unscaled() {
        // Verified against v0.29.0 (WS) on the same contracts: the REST greeks
        // are already on the WS scale, so vega/rho must not be rescaled.
        use longbridge::quote::CalcIndex;
        let raw = serde_json::json!({"security_calc_index": [{
            "symbol": "AAPL261016C250000.US", "delta": "0.999", "gamma": "0.001",
            "theta": "-0.043", "vega": "0.001", "rho": "0.047",
            "implied_volatility": "70.31", "open_interest": "1256", "pe_ttm_ratio": "31.2"
        }]});
        let requested = [
            CalcIndex::Delta,
            CalcIndex::Gamma,
            CalcIndex::Theta,
            CalcIndex::Vega,
            CalcIndex::Rho,
            CalcIndex::OpenInterest,
        ];
        let r = HttpQuoteApi::reshape_calc_indexes(raw, &requested).expect("reshape greeks");
        assert_eq!(r[0].vega.unwrap().to_string(), "0.001");
        assert_eq!(r[0].rho.unwrap().to_string(), "0.047");
        assert_eq!(r[0].delta.unwrap().to_string(), "0.999");
        assert_eq!(r[0].open_interest, Some(1256));
        // Returned by the gateway but not requested.
        assert!(r[0].pe_ttm_ratio.is_none());
        assert!(r[0].implied_volatility.is_none());
    }

    #[test]
    fn calc_index_field_names_match_sdk_fields() {
        use longbridge::quote::CalcIndex;
        assert_eq!(calc_index_field(CalcIndex::LastDone), "last_done");
        assert_eq!(calc_index_field(CalcIndex::PeTtmRatio), "pe_ttm_ratio");
        assert_eq!(
            calc_index_field(CalcIndex::DividendRatioTtm),
            "dividend_ratio_ttm"
        );
        assert_eq!(calc_index_field(CalcIndex::ItmOtm), "itm_otm");
        assert_eq!(calc_index_field(CalcIndex::WarrantDelta), "warrant_delta");
        assert_eq!(
            calc_index_field(CalcIndex::FiveMinutesChangeRate),
            "five_minutes_change_rate"
        );
    }

    #[test]
    fn brokers_is_restricted_to_the_ap_data_center() {
        // Mirrors `QuoteContext::brokers`, which rejects non-AP sessions.
        assert_eq!(
            endpoint_region("/quote/brokers"),
            Some(longbridge::DcRegion::Ap)
        );
        assert_eq!(endpoint_region("/quote/depth"), None);
        assert_eq!(endpoint_region("/quote/quotes"), None);
    }

    #[test]
    fn omitted_or_null_lists_decode_as_empty() {
        // proto3 JSON drops an empty repeated field entirely.
        assert!(HttpQuoteApi::reshape_option_chain(serde_json::json!({}))
            .unwrap()
            .is_empty());
        let none: [longbridge::quote::CalcIndex; 0] = [];
        assert!(
            HttpQuoteApi::reshape_calc_indexes(serde_json::json!({}), &none)
                .unwrap()
                .is_empty()
        );
        assert!(HttpQuoteApi::reshape_calc_indexes(
            serde_json::json!({"security_calc_index": null}),
            &none
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn calc_index_requested_integer_missing_or_null_is_zero() {
        use longbridge::quote::CalcIndex;
        let raw = serde_json::json!({"security_calc_index": [
            {"symbol": "A.US", "volume": null},
            {"symbol": "B.US"}
        ]});
        let requested = [CalcIndex::Volume, CalcIndex::OpenInterest];
        let r = HttpQuoteApi::reshape_calc_indexes(raw, &requested).expect("reshape");
        assert_eq!(r[0].volume, Some(0));
        assert_eq!(r[1].volume, Some(0));
        assert_eq!(r[1].open_interest, Some(0));
        // Not requested → stays None even though it was defaulted first.
        assert!(r[1].outstanding_qty.is_none());
    }

    #[test]
    fn warrant_passes_merge_recovers_rows_missed_by_either() {
        let row = |s: &str| serde_json::json!({"symbol": s});
        let merged = merge_warrant_passes(
            vec![row("A"), row("B"), row("D")],
            vec![row("A"), row("C"), row("D")],
        );
        let syms: Vec<&str> = merged
            .iter()
            .map(|r| r["symbol"].as_str().unwrap())
            .collect();
        assert_eq!(syms, ["A", "B", "D", "C"]);
    }

    #[test]
    fn reshape_option_chain_tolerates_empty_and_null_fields() {
        // What the SDK's own decoder accepts: "" strike, null symbol and
        // days-to-expiry, missing cycle/attr — and a null list.
        let raw = serde_json::json!({"list": [
            {"days_to_expiry": null, "direction": "P", "expiry_date": "20261016",
             "strike_price": "", "symbol": null}
        ]});
        let c = HttpQuoteApi::reshape_option_chain(raw).expect("tolerant decode");
        assert!(c[0].strike_price.is_zero());
        assert_eq!(c[0].symbol, "");
        assert_eq!(c[0].days_to_expiry, 0);
        assert_eq!(
            c[0].option_type,
            longbridge::quote::OptionExpiryCycleType::Monthly
        );
        assert_eq!(
            c[0].standard_attr,
            longbridge::quote::OptionStandardAttr::Normal
        );
        let empty = HttpQuoteApi::reshape_option_chain(serde_json::json!({"list": null}))
            .expect("null list");
        assert!(empty.is_empty());
    }

    #[test]
    fn calc_index_field_covers_every_sdk_field() {
        // Every `CalcIndex` must name a real `SecurityCalcIndex` field, or
        // `keep_requested_indexes` would null a requested value.
        use longbridge::quote::CalcIndex::{self, *};
        // No wildcard arm: a variant added to the SDK fails to compile here,
        // forcing it into `all` below.
        #[allow(clippy::match_same_arms)]
        fn listed(index: CalcIndex) {
            match index {
                LastDone
                | ChangeValue
                | ChangeRate
                | Volume
                | Turnover
                | YtdChangeRate
                | TurnoverRate
                | TotalMarketValue
                | CapitalFlow
                | Amplitude
                | VolumeRatio
                | PeTtmRatio
                | PbRatio
                | DividendRatioTtm
                | FiveDayChangeRate
                | TenDayChangeRate
                | HalfYearChangeRate
                | FiveMinutesChangeRate
                | ExpiryDate
                | StrikePrice
                | UpperStrikePrice
                | LowerStrikePrice
                | OutstandingQty
                | OutstandingRatio
                | Premium
                | ItmOtm
                | ImpliedVolatility
                | WarrantDelta
                | CallPrice
                | ToCallPrice
                | EffectiveLeverage
                | LeverageRatio
                | ConversionRatio
                | BalancePoint
                | OpenInterest
                | Delta
                | Gamma
                | Theta
                | Vega
                | Rho => {}
            }
        }
        let blank: longbridge::quote::SecurityCalcIndex =
            serde_json::from_value(serde_json::json!({"symbol": "X"})).unwrap();
        let fields = serde_json::to_value(blank).unwrap();
        assert!(
            fields.get("pe_ttm_ratio").is_some(),
            "None fields must serialize as keys"
        );
        let all = [
            LastDone,
            ChangeValue,
            ChangeRate,
            Volume,
            Turnover,
            YtdChangeRate,
            TurnoverRate,
            TotalMarketValue,
            CapitalFlow,
            Amplitude,
            VolumeRatio,
            PeTtmRatio,
            PbRatio,
            DividendRatioTtm,
            FiveDayChangeRate,
            TenDayChangeRate,
            HalfYearChangeRate,
            FiveMinutesChangeRate,
            ExpiryDate,
            StrikePrice,
            UpperStrikePrice,
            LowerStrikePrice,
            OutstandingQty,
            OutstandingRatio,
            Premium,
            ItmOtm,
            ImpliedVolatility,
            WarrantDelta,
            CallPrice,
            ToCallPrice,
            EffectiveLeverage,
            LeverageRatio,
            ConversionRatio,
            BalancePoint,
            OpenInterest,
            Delta,
            Gamma,
            Theta,
            Vega,
            Rho,
        ];
        for index in all {
            listed(index);
            let field = calc_index_field(index);
            assert!(
                fields.get(&field).is_some(),
                "{index:?} → `{field}` is not a field"
            );
        }
    }

    #[test]
    fn warrant_order_breaks_price_ties_by_symbol() {
        let rows = vec![
            serde_json::json!({"symbol": "B", "last_done": "0.5"}),
            serde_json::json!({"symbol": "A", "last_done": "0.5"}),
            serde_json::json!({"symbol": "C", "last_done": "1"}),
        ];
        let syms: Vec<String> = order_by_last_done_desc(rows)
            .iter()
            .map(|r| r["symbol"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(syms, ["C", "A", "B"]);
    }
}
