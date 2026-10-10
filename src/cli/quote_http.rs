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
    TradeDirection, TradeSession, TradeStatus, WarrantInfo, WarrantQuote,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

// ── reshape primitives ───────────────────────────────────────────────────────

/// Lift the single container field (e.g. `secu_quote`) out of the gateway's
/// response object, so the root matches the SDK's array- or object-rooted shape.
fn unwrap(value: &mut Value, key: &str) {
    if let Some(inner) = value.get_mut(key).map(Value::take) {
        *value = inner;
    }
}

/// Apply `f` to every node reached by a `*`-aware dotted `path`.
fn walk(value: &mut Value, segs: &[&str], f: &dyn Fn(&mut Value)) {
    let Some((seg, rest)) = segs.split_first() else {
        f(value);
        return;
    };
    match value {
        Value::Array(arr) if *seg == "*" => arr.iter_mut().for_each(|v| walk(v, rest, f)),
        Value::Object(map) if *seg == "*" => map.values_mut().for_each(|v| walk(v, rest, f)),
        Value::Object(map) => {
            if let Some(v) = map.get_mut(*seg) {
                walk(v, rest, f);
            }
        }
        _ => {}
    }
}

/// Convert a unix-seconds string/number to an RFC3339 string at each path.
fn convert_unix_paths(value: &mut Value, paths: &[&str]) {
    for p in paths {
        let segs: Vec<&str> = p.split('.').collect();
        walk(value, &segs, &|v| {
            let ts = match v {
                Value::String(s) => s.parse::<i64>().ok(),
                Value::Number(n) => n.as_i64(),
                _ => None,
            };
            if let Some(ts) = ts {
                if let Ok(dt) = OffsetDateTime::from_unix_timestamp(ts) {
                    if let Ok(s) = dt.format(&Rfc3339) {
                        *v = Value::String(s);
                    }
                }
            }
        });
    }
}

/// Convert a unix-seconds string/number to the serialized form the SDK's
/// **bare** `OffsetDateTime` fields expect (those without an explicit
/// `#[serde(with = "...rfc3339")]`, e.g. `CapitalFlowLine::timestamp`). Rather
/// than guess `time`'s default format, round-trip through its own `Serialize`:
/// `unix → OffsetDateTime → to_value` yields exactly what its `Deserialize`
/// reads back.
fn convert_unix_default_paths(value: &mut Value, paths: &[&str]) {
    for p in paths {
        let segs: Vec<&str> = p.split('.').collect();
        walk(value, &segs, &|v| {
            let ts = match v {
                Value::String(s) => s.parse::<i64>().ok(),
                Value::Number(n) => n.as_i64(),
                _ => None,
            };
            if let Some(ts) = ts {
                if let Ok(dt) = OffsetDateTime::from_unix_timestamp(ts) {
                    if let Ok(mapped) = serde_json::to_value(dt) {
                        *v = mapped;
                    }
                }
            }
        });
    }
}

/// Replace an empty-string value with `repl` at each `*`-aware path. Used for
/// fields that need a targeted empty→null (`Option` columns) or empty→"0"
/// (non-`Option` columns) where a recursive pass would be too broad.
fn set_empty_at(value: &mut Value, paths: &[&str], repl: &Value) {
    for p in paths {
        let segs: Vec<&str> = p.split('.').collect();
        walk(value, &segs, &|v| {
            if v.as_str() == Some("") {
                *v = repl.clone();
            }
        });
    }
}

/// Convert a numeric string to a JSON integer at each path. The gateway sends
/// `int64` fields as strings; the SDK's integer fields need real numbers.
fn numify_paths(value: &mut Value, paths: &[&str]) {
    for p in paths {
        let segs: Vec<&str> = p.split('.').collect();
        walk(value, &segs, &|v| {
            if let Value::String(s) = v {
                if let Ok(n) = s.parse::<i64>() {
                    *v = Value::Number(n.into());
                }
            }
        });
    }
}

/// Replace the wire int at each path with `f(int)` (typically the SDK enum
/// re-serialized through its own `Serialize`, so the result matches the SDK
/// type's `Deserialize`). `f` returning `None` leaves the value unchanged.
fn map_int_enum(value: &mut Value, paths: &[&str], f: &dyn Fn(i64) -> Option<Value>) {
    for p in paths {
        let segs: Vec<&str> = p.split('.').collect();
        walk(value, &segs, &|v| {
            if let Some(n) = v.as_i64() {
                if let Some(mapped) = f(n) {
                    *v = mapped;
                }
            }
        });
    }
}

// The closures below mirror the WS path's `try_from(wire).unwrap_or(default)`:
// an unrecognized wire int falls back to the enum's default variant rather than
// leaving the raw int in place (which would fail the name-based `from_value`).

/// `trade_status` wire int → SDK `TradeStatus` serialized form (unknown →
/// `Normal`, the proto 0-variant / `unwrap_or_default` equivalent).
fn trade_status(n: i64) -> Option<Value> {
    let e = TradeStatus::try_from(n as i32).unwrap_or(TradeStatus::Normal);
    serde_json::to_value(e).ok()
}

/// `direction` wire int → SDK `TradeDirection` serialized form (`From<i32>` is
/// infallible, defaulting to `Neutral`).
fn trade_direction(n: i64) -> Option<Value> {
    serde_json::to_value(TradeDirection::from(n as i32)).ok()
}

/// `trade_session` wire int → SDK `TradeSession` serialized form. Mirrors the
/// SDK's `From<proto::TradeSession>` (0=Normal→Intraday, 1=Pre, 2=Post,
/// 3=Overnight) without taking a dependency on the proto crate; unknown →
/// `Intraday` (the SDK default).
fn trade_session(n: i64) -> Option<Value> {
    let s = match n {
        1 => TradeSession::Pre,
        2 => TradeSession::Post,
        3 => TradeSession::Overnight,
        _ => TradeSession::Intraday,
    };
    serde_json::to_value(s).ok()
}

/// `warrant_type` wire int → SDK `WarrantType` serialized form (unknown →
/// `Unknown`).
fn warrant_type(n: i64) -> Option<Value> {
    let e = longbridge::quote::WarrantType::try_from(n as i32)
        .unwrap_or(longbridge::quote::WarrantType::Unknown);
    serde_json::to_value(e).ok()
}

/// `status` wire int → SDK `WarrantStatus` serialized form (unknown →
/// `Unknown`).
fn warrant_status(n: i64) -> Option<Value> {
    let e = longbridge::quote::WarrantStatus::try_from(n as i32)
        .unwrap_or(longbridge::quote::WarrantStatus::Unknown);
    serde_json::to_value(e).ok()
}

/// Replace a string code at each path with the SDK enum `E`'s serialized name
/// via its `FromStr` (strum) parse — e.g. option `direction` `"C"` → `"Call"`,
/// `standard_attr` `""` → `"Normal"`. Leaves unparseable values unchanged.
fn map_str_enum<E>(value: &mut Value, paths: &[&str])
where
    E: std::str::FromStr + serde::Serialize,
{
    for p in paths {
        let segs: Vec<&str> = p.split('.').collect();
        walk(value, &segs, &|v| {
            if let Value::String(s) = v {
                if let Ok(e) = s.parse::<E>() {
                    if let Ok(mapped) = serde_json::to_value(e) {
                        *v = mapped;
                    }
                }
            }
        });
    }
}

/// Replace an empty-string value with `"0"` at the given (array-element) keys —
/// for structs that mix non-`Option` numeric fields (which need `0`) with
/// `Option` fields (which an [`empty_str_to_null`] pass then turns to `None`).
fn empty_keys_to_zero(value: &mut Value, keys: &[&str]) {
    match value {
        Value::Array(arr) => arr.iter_mut().for_each(|v| empty_keys_to_zero(v, keys)),
        Value::Object(map) => {
            for k in keys {
                if let Some(v) = map.get_mut(*k) {
                    if v.as_str() == Some("") {
                        *v = Value::String("0".to_string());
                    }
                }
            }
        }
        _ => {}
    }
}

/// At each path, if the string value does not parse as the SDK enum `E`, replace
/// it with `fallback` (a valid variant name). Valid values are left as-is (the
/// gateway already sends the serde variant name). Mirrors the WS path's
/// `parse().unwrap_or(Unknown)` for name-based enums (`SecurityBoard`, `Market`)
/// that derive plain serde `Deserialize` with no `#[serde(other)]`, so an
/// unrecognized/empty value would otherwise fail the whole batch decode.
fn default_unparseable_enum<E>(value: &mut Value, paths: &[&str], fallback: &str)
where
    E: std::str::FromStr,
{
    for p in paths {
        let segs: Vec<&str> = p.split('.').collect();
        walk(value, &segs, &|v| {
            if let Value::String(s) = v {
                if s.parse::<E>().is_err() {
                    *v = Value::String(fallback.to_string());
                }
            }
        });
    }
}

/// Recursively replace empty-string values with `null` (so the SDK's `Option`
/// fields deserialize as `None` rather than failing to parse `""` as a decimal).
fn empty_str_to_null(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for v in map.values_mut() {
                empty_str_to_null(v);
            }
        }
        Value::Array(arr) => arr.iter_mut().for_each(empty_str_to_null),
        Value::String(s) if s.is_empty() => *value = Value::Null,
        _ => {}
    }
}

/// Recursively replace empty-string values with `"0"`, except for keys in
/// `skip` (symbol/enum/string fields). The gateway sends untraded decimal fields
/// as `""`; the WS path parses those via `unwrap_or_default()` → `0`, and the
/// SDK's **non-`Option`** `Decimal` fields reject `""`, so this mirrors the WS
/// behaviour for quote endpoints whose price fields are not optional.
fn empty_str_to_zero_skip(value: &mut Value, skip: &[&str]) {
    match value {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                match v {
                    Value::String(s) if s.is_empty() && !skip.contains(&k.as_str()) => {
                        *v = Value::String("0".to_string());
                    }
                    _ => empty_str_to_zero_skip(v, skip),
                }
            }
        }
        Value::Array(arr) => arr.iter_mut().for_each(|v| empty_str_to_zero_skip(v, skip)),
        _ => {}
    }
}

/// Divide the decimal-string `vega`/`rho` greeks by 100 at each array element —
/// the REST payload reports them 100× the SDK/WS scale.
fn scale_greeks(value: &mut Value) {
    let Some(arr) = value.as_array_mut() else {
        return;
    };
    for row in arr.iter_mut() {
        let Some(obj) = row.as_object_mut() else {
            continue;
        };
        for k in ["vega", "rho"] {
            let scaled = obj
                .get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .and_then(|s| s.parse::<rust_decimal::Decimal>().ok())
                .map(|d| d / rust_decimal::Decimal::ONE_HUNDRED);
            if let Some(d) = scaled {
                obj.insert(k.to_string(), Value::String(d.to_string()));
            }
        }
    }
}

/// Lift every key of each array element's nested `key` object up to the element's
/// top level (the REST payload nests option/warrant-specific fields under
/// `option_extend`/`warrant_extend`; the SDK types carry them flat).
fn lift_nested(value: &mut Value, key: &str) {
    let Some(arr) = value.as_array_mut() else {
        return;
    };
    for item in arr.iter_mut() {
        if let Some(obj) = item.as_object_mut() {
            if let Some(Value::Object(ext)) = obj.remove(key) {
                for (k, v) in ext {
                    obj.insert(k, v);
                }
            }
        }
    }
}

/// Recursively rename object keys (every depth).
fn rename_keys(value: &mut Value, renames: &[(&str, &str)]) {
    match value {
        Value::Object(map) => {
            for (from, to) in renames {
                if let Some(v) = map.remove(*from) {
                    map.insert((*to).to_string(), v);
                }
            }
            for v in map.values_mut() {
                rename_keys(v, renames);
            }
        }
        Value::Array(arr) => arr.iter_mut().for_each(|v| rename_keys(v, renames)),
        _ => {}
    }
}

/// Reformat an 8-digit `YYYYMMDD` string to `YYYY-MM-DD` at each path (the SDK's
/// `Date` fields deserialize from ISO `YYYY-MM-DD`).
fn reformat_ymd(value: &mut Value, paths: &[&str]) {
    for p in paths {
        let segs: Vec<&str> = p.split('.').collect();
        walk(value, &segs, &|v| {
            if let Value::String(s) = v {
                if s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit()) {
                    *v = Value::String(format!("{}-{}-{}", &s[0..4], &s[4..6], &s[6..8]));
                }
            }
        });
    }
}

/// Format an `HHMM` integer (e.g. `930`) as the SDK `Time` string
/// (`"09:30:00.0"`); leaves non-integers untouched.
fn hhmm_to_time(v: &Value) -> Value {
    match v.as_i64() {
        Some(n) => Value::String(format!("{:02}:{:02}:00.0", n / 100, n % 100)),
        None => v.clone(),
    }
}

/// Insert `key: default` into every object in `value` (recursing arrays) that
/// lacks it. Used to supply fields the gateway omits but the SDK requires (e.g.
/// `Candlestick::open_updated`).
fn insert_missing(value: &mut Value, key: &str, default: &Value) {
    match value {
        Value::Array(arr) => arr.iter_mut().for_each(|v| insert_missing(v, key, default)),
        Value::Object(map) => {
            map.entry(key.to_string()).or_insert_with(|| default.clone());
        }
        _ => {}
    }
}

/// Recursively drop object keys (every depth).
fn drop_keys(value: &mut Value, keys: &[&str]) {
    match value {
        Value::Object(map) => {
            map.retain(|k, _| !keys.contains(&k.as_str()));
            for v in map.values_mut() {
                drop_keys(v, keys);
            }
        }
        Value::Array(arr) => arr.iter_mut().for_each(|v| drop_keys(v, keys)),
        _ => {}
    }
}

// ── HTTP transport ───────────────────────────────────────────────────────────

/// POST the given shim `path` with `body`, returning the raw gateway JSON.
async fn http_post(path: &str, body: Value) -> Result<Value> {
    let client = crate::openapi::http_client();
    let resp = client
        .request(Method::POST, path)
        .body(Json(body))
        .response::<Json<Value>>()
        .send()
        .await
        .map_err(anyhow::Error::from)?;
    Ok(resp.0)
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
        empty_str_to_zero_skip(&mut value, &["symbol"]);
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
        empty_str_to_zero_skip(&mut value, &["trade_type", "direction", "trade_session"]);
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
        // `IntradayLine` has no string fields; zero any "" decimal/volume before
        // numify.
        empty_str_to_zero_skip(&mut value, &[]);
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
        // `Candlestick` has no string fields; zero any "" decimal/volume
        // (matching WS) before numify.
        empty_str_to_zero_skip(&mut value, &[]);
        numify_paths(&mut value, &["*.volume"]);
        // The REST payload omits the SDK's `open_updated` flag; the WS path
        // leaves it `false` for historical bars, so default it here.
        insert_missing(&mut value, "open_updated", &Value::Bool(false));
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
                "trade_session": longbridge::quote::TradeSessions::Intraday as i32,
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
        // Runs before `numify` so "" → "0" → number. String/enum fields skipped.
        empty_str_to_zero_skip(
            &mut value,
            &[
                "symbol", "name_cn", "name_en", "name_hk", "exchange", "currency", "board",
            ],
        );
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
        // `inflow` is a non-`Option` `Decimal`; zero any "".
        empty_str_to_zero_skip(&mut value, &[]);
        from_value(value)
    }

    pub async fn capital_flow(&self, symbol: String) -> Result<Vec<CapitalFlowLine>> {
        let value = http_post("/quote/capital-flow", serde_json::json!({ "symbol": symbol })).await?;
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
                obj.entry(k.to_string()).or_insert_with(|| Value::Array(vec![]));
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
    /// `list`; map the string-code enums; `YYYYMMDD` → `YYYY-MM-DD`).
    fn reshape_option_chain(mut value: Value) -> Result<Vec<OptionChainContract>> {
        unwrap(&mut value, "list");
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
        // Before numify so "" → "0" → number. String/enum-code fields skipped.
        empty_str_to_zero_skip(
            &mut value,
            &["symbol", "underlying_symbol", "contract_type", "direction"],
        );
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
        // is not int-mapped (unlike `warrant_list`, whose `type` is an int).
        // Untraded price fields come back as "" — the SDK's non-`Option`
        // `Decimal`/`i64`s need a real 0 (matching the WS `unwrap_or_default`).
        // Before numify so "" → "0" → number.
        empty_str_to_zero_skip(&mut value, &["symbol", "underlying_symbol", "category"]);
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
        empty_str_to_null(&mut value);
        from_value(value)
    }

    pub async fn warrant_list(&self, symbol: String) -> Result<Vec<WarrantInfo>> {
        let value = http_post(
            "/quote/warrants",
            serde_json::json!({
                "symbol": symbol,
                // sort_by=LastDone(0), sort_order=Descending(1) — matches the
                // existing CLI/`LbQuoteApi` warrant_list defaults.
                "filter_config": {
                    "sort_by": 0, "sort_order": 1, "sort_offset": 0, "sort_count": 20,
                },
                "language": 1,
            }),
        )
        .await?;
        Self::reshape_warrant_list(value)
    }

    /// Reshape `/quote/calc-indexes` into `Vec<SecurityCalcIndex>` (unwrap
    /// `security_calc_index`; rename `change_val`→`change_value`; drop
    /// `volume_str`; scale greeks; numify counts; dates; `""`→null).
    fn reshape_calc_indexes(mut value: Value) -> Result<Vec<SecurityCalcIndex>> {
        unwrap(&mut value, "security_calc_index");
        rename_keys(&mut value, &[("change_val", "change_value")]);
        drop_keys(&mut value, &["volume_str"]);
        scale_greeks(&mut value);
        numify_paths(
            &mut value,
            &["*.volume", "*.outstanding_qty", "*.open_interest"],
        );
        reformat_ymd(&mut value, &["*.expiry_date"]);
        empty_str_to_null(&mut value);
        from_value(value)
    }

    pub async fn calc_indexes(
        &self,
        symbols: Vec<String>,
        indexes: Vec<longbridge::quote::CalcIndex>,
    ) -> Result<Vec<SecurityCalcIndex>> {
        // The gateway's `calc_index` wire values are the proto `CalcIndex`
        // enumeration, which is the SDK `CalcIndex` order shifted by one (proto
        // reserves 0 for `Unknown`): proto `LastDone` = 1 = SDK `LastDone`(0) + 1.
        // A fieldless enum casts to its discriminant, so this mirrors the SDK's
        // `From<CalcIndex> for proto::CalcIndex` without depending on the proto
        // crate.
        let index_ints: Vec<i32> = indexes.iter().map(|i| (*i as i32) + 1).collect();
        let value = http_post(
            "/quote/calc-indexes",
            serde_json::json!({ "symbols": symbols, "calc_index": index_ints }),
        )
        .await?;
        Self::reshape_calc_indexes(value)
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
/// accessor (never the raw `quote()`), so the `/v1/quote/cmd` beacon still fires.
fn ws() -> crate::cli::api::LbQuoteApi {
    crate::cli::api::LbQuoteApi::new(crate::openapi::quote_cmd())
}

/// Full `QuoteApi` so the trait-object path (`run_static`, the JSON-RPC serve
/// layer) shares the same HTTP backend as the direct command entry points.
/// Shim-migrated methods hit HTTP; the rest delegate to [`ws`].
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
        HttpQuoteApi::history_candlesticks_by_date(self, symbol, period, adjust, start, end).await
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
    async fn capital_distribution(
        &self,
        symbol: String,
    ) -> Result<CapitalDistributionResponse> {
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
        let raw: Value =
            serde_json::from_str(r#"{"trade_day":["20261012","20261013"]}"#).unwrap();
        let days = HttpQuoteApi::reshape_trading_days(raw).expect("reshape trading_days");
        assert_eq!(days.trading_days.len(), 2);
        assert!(days.half_trading_days.is_empty());
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
    }

    #[test]
    fn reshape_calc_indexes_round_trips() {
        let raw: Value = serde_json::from_str(CALC_INDEX_FIXTURE).unwrap();
        let r = HttpQuoteApi::reshape_calc_indexes(raw).expect("reshape calc_indexes");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].pe_ttm_ratio.unwrap().to_string(), "14.24");
        assert!(r[0].volume.is_none()); // "" → null → None
        assert!(r[0].amplitude.is_none());
        assert_eq!(r[0].change_value.unwrap().to_string(), "13.400"); // change_val rename
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
        let overnight = apple.overnight_quote.as_ref().expect("AAPL overnight present");
        assert_eq!(overnight.volume, 79_180);
        // trade_status wire int 0 → SDK `Normal`.
        assert_eq!(apple.trade_status, TradeStatus::Normal);
    }
}
