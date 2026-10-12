//! Reshape primitives for the quote WS→HTTP shim: pure `serde_json::Value`
//! transforms that turn the gateway's proto-JSON into the exact shape each SDK
//! struct's `Deserialize` expects. Kept separate from the endpoint code in the
//! parent module so the transforms can be read and reviewed on their own.

use longbridge::quote::{TradeDirection, TradeSession, TradeStatus};
use serde_json::Value;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
// ── reshape primitives ───────────────────────────────────────────────────────

/// Lift the single container field (e.g. `secu_quote`) out of the gateway's
/// response object, so the root matches the SDK's array- or object-rooted shape.
pub(super) fn unwrap(value: &mut Value, key: &str) {
    if let Some(inner) = value.get_mut(key).map(Value::take) {
        *value = inner;
    }
}

/// Apply `f` to every node reached by a `*`-aware dotted `path`.
pub(super) fn walk(value: &mut Value, segs: &[&str], f: &dyn Fn(&mut Value)) {
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
pub(super) fn convert_unix_paths(value: &mut Value, paths: &[&str]) {
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
pub(super) fn convert_unix_default_paths(value: &mut Value, paths: &[&str]) {
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
pub(super) fn set_empty_at(value: &mut Value, paths: &[&str], repl: &Value) {
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
pub(super) fn numify_paths(value: &mut Value, paths: &[&str]) {
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
pub(super) fn map_int_enum(value: &mut Value, paths: &[&str], f: &dyn Fn(i64) -> Option<Value>) {
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
pub(super) fn trade_status(n: i64) -> Option<Value> {
    let e = TradeStatus::try_from(n as i32).unwrap_or(TradeStatus::Normal);
    serde_json::to_value(e).ok()
}

/// `direction` wire int → SDK `TradeDirection` serialized form (`From<i32>` is
/// infallible, defaulting to `Neutral`).
pub(super) fn trade_direction(n: i64) -> Option<Value> {
    serde_json::to_value(TradeDirection::from(n as i32)).ok()
}

/// `trade_session` wire int → SDK `TradeSession` serialized form. Mirrors the
/// SDK's `From<proto::TradeSession>` (0=Normal→Intraday, 1=Pre, 2=Post,
/// 3=Overnight) without taking a dependency on the proto crate; unknown →
/// `Intraday` (the SDK default).
pub(super) fn trade_session(n: i64) -> Option<Value> {
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
pub(super) fn warrant_type(n: i64) -> Option<Value> {
    let e = longbridge::quote::WarrantType::try_from(n as i32)
        .unwrap_or(longbridge::quote::WarrantType::Unknown);
    serde_json::to_value(e).ok()
}

/// `status` wire int → SDK `WarrantStatus` serialized form (unknown →
/// `Unknown`).
pub(super) fn warrant_status(n: i64) -> Option<Value> {
    let e = longbridge::quote::WarrantStatus::try_from(n as i32)
        .unwrap_or(longbridge::quote::WarrantStatus::Unknown);
    serde_json::to_value(e).ok()
}

/// The gateway's `calc_index` wire value for an SDK `CalcIndex`: the proto
/// `CalcIndex` enumeration, which is the SDK enum's declaration order shifted by
/// one (proto reserves 0 for `Unknown`), e.g. proto `LastDone` = 1 = SDK
/// `LastDone`(0) + 1. A fieldless enum casts to its discriminant, so this mirrors
/// the SDK's `From<CalcIndex> for proto::CalcIndex` without depending on the
/// proto crate. This is an implicit order coupling: `calc_index_wire_matches_proto`
/// pins known values so any SDK reordering fails loudly in tests, not silently
/// at the gateway.
pub(super) fn calc_index_wire(index: longbridge::quote::CalcIndex) -> i32 {
    (index as i32) + 1
}

/// Drop rows whose `symbol` was already seen, keeping first occurrence and
/// order. Guards paged results against a gateway that repeats a page (e.g.
/// ignores `sort_offset`), which would otherwise surface duplicate warrants.
/// Rows without a `symbol` are kept as-is.
pub(super) fn dedupe_by_symbol(rows: Vec<Value>) -> Vec<Value> {
    let mut seen = std::collections::HashSet::new();
    rows.into_iter()
        .filter(|row| match row.get("symbol").and_then(Value::as_str) {
            Some(sym) => seen.insert(sym.to_owned()),
            None => true,
        })
        .collect()
}

/// Replace a string code at each path with the SDK enum `E`'s serialized name
/// via its `FromStr` (strum) parse — e.g. option `direction` `"C"` → `"Call"`,
/// `standard_attr` `""` → `"Normal"`. An unparseable code becomes `Unknown`
/// (every option enum has that variant), mirroring the WS path's
/// `parse().unwrap_or_default()` — left raw it would fail the whole batch.
pub(super) fn map_str_enum<E>(value: &mut Value, paths: &[&str])
where
    E: std::str::FromStr + serde::Serialize,
{
    for p in paths {
        let segs: Vec<&str> = p.split('.').collect();
        walk(value, &segs, &|v| {
            if let Value::String(s) = v {
                *v = s
                    .parse::<E>()
                    .ok()
                    .and_then(|e| serde_json::to_value(e).ok())
                    .unwrap_or_else(|| Value::String("Unknown".to_string()));
            }
        });
    }
}

/// Replace an empty-string value with `"0"` at the given (array-element) keys —
/// for structs that mix non-`Option` numeric fields (which need `0`) with
/// `Option` fields (which an [`empty_str_to_null`] pass then turns to `None`).
pub(super) fn empty_keys_to_zero(value: &mut Value, keys: &[&str]) {
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
pub(super) fn default_unparseable_enum<E>(value: &mut Value, paths: &[&str], fallback: &str)
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
/// fields deserialize as `None` rather than failing to parse `""` as a decimal),
/// except under the object keys in `keep` — the struct's `String` fields, where
/// `""` is a valid value and `null` would fail the whole batch decode.
pub(super) fn empty_str_to_null(value: &mut Value, keep: &[&str]) {
    match value {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if !keep.contains(&k.as_str()) {
                    empty_str_to_null(v, keep);
                }
            }
        }
        Value::Array(arr) => arr.iter_mut().for_each(|v| empty_str_to_null(v, keep)),
        Value::String(s) if s.is_empty() => *value = Value::Null,
        _ => {}
    }
}

/// Replace `""` with `"0"` at exactly the listed numeric paths. The gateway sends
/// untraded decimal/count fields as `""`; the WS path parses those via
/// `unwrap_or_default()` → `0`, and the SDK's **non-`Option`** `Decimal`/`i64`
/// fields reject `""`. This is an explicit allow-list of the numeric fields per
/// struct (rather than "zero every empty string except a skip-list"), so a
/// string field that legitimately carries `""` can never be corrupted to `"0"`,
/// and adding a field to a struct is a deliberate edit here. Must run before
/// `numify_paths` so `""` → `"0"` → number for integer fields.
pub(super) fn zero_empty_numeric(value: &mut Value, paths: &[&str]) {
    set_empty_at(value, paths, &Value::String("0".into()));
}

// Non-`Option` numeric fields per SDK struct (the ones `zero_empty_numeric` may
// legitimately turn `""` into 0). Keep in sync with the struct definitions.
pub(super) const QUOTE_NUMERIC: &[&str] = &[
    "*.last_done",
    "*.prev_close",
    "*.open",
    "*.high",
    "*.low",
    "*.volume",
    "*.turnover",
    // extended-session `PrePostQuote` blocks (non-`Option` fields inside)
    "*.pre_market_quote.last_done",
    "*.pre_market_quote.prev_close",
    "*.pre_market_quote.high",
    "*.pre_market_quote.low",
    "*.pre_market_quote.volume",
    "*.pre_market_quote.turnover",
    "*.post_market_quote.last_done",
    "*.post_market_quote.prev_close",
    "*.post_market_quote.high",
    "*.post_market_quote.low",
    "*.post_market_quote.volume",
    "*.post_market_quote.turnover",
    "*.overnight_quote.last_done",
    "*.overnight_quote.prev_close",
    "*.overnight_quote.high",
    "*.overnight_quote.low",
    "*.overnight_quote.volume",
    "*.overnight_quote.turnover",
];
pub(super) const TRADE_NUMERIC: &[&str] = &["*.price", "*.volume"];
pub(super) const INTRADAY_NUMERIC: &[&str] = &["*.price", "*.volume", "*.turnover", "*.avg_price"];
pub(super) const CANDLE_NUMERIC: &[&str] = &[
    "*.close",
    "*.open",
    "*.low",
    "*.high",
    "*.volume",
    "*.turnover",
];
pub(super) const STATIC_NUMERIC: &[&str] = &[
    "*.lot_size",
    "*.total_shares",
    "*.circulating_shares",
    "*.hk_shares",
    "*.eps",
    "*.eps_ttm",
    "*.bps",
    "*.dividend_yield",
];
pub(super) const CAPITAL_FLOW_NUMERIC: &[&str] = &["*.inflow"];
pub(super) const OPTION_QUOTE_NUMERIC: &[&str] = &[
    "*.last_done",
    "*.prev_close",
    "*.open",
    "*.high",
    "*.low",
    "*.volume",
    "*.turnover",
    "*.implied_volatility",
    "*.open_interest",
    "*.strike_price",
    "*.contract_multiplier",
    "*.contract_size",
    "*.historical_volatility",
];
pub(super) const WARRANT_QUOTE_NUMERIC: &[&str] = &[
    "*.last_done",
    "*.prev_close",
    "*.open",
    "*.high",
    "*.low",
    "*.volume",
    "*.turnover",
    "*.implied_volatility",
    "*.outstanding_ratio",
    "*.outstanding_quantity",
    "*.conversion_ratio",
    "*.strike_price",
    "*.upper_strike_price",
    "*.lower_strike_price",
    "*.call_price",
];

/// The `SecurityCalcIndex` field a `CalcIndex` fills: the variant name in
/// `snake_case` (`PeTtmRatio` → `pe_ttm_ratio`), which is how the SDK names every
/// field it sets in `SecurityCalcIndex::from_proto`.
pub(super) fn calc_index_field(index: longbridge::quote::CalcIndex) -> String {
    let mut field = String::new();
    for (i, ch) in format!("{index:?}").chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                field.push('_');
            }
            field.push(ch.to_ascii_lowercase());
        } else {
            field.push(ch);
        }
    }
    field
}

/// Null every index field of each row that is not in `requested`, keeping
/// `symbol`. The gateway returns whatever it computed; the WS path fills only
/// the requested indexes and leaves the rest `None`, and `serve` returns the
/// whole struct, so extra fields would leak into its JSON.
pub(super) fn keep_requested_indexes(
    value: &mut Value,
    requested: &[longbridge::quote::CalcIndex],
) {
    let keep: Vec<String> = requested.iter().map(|i| calc_index_field(*i)).collect();
    let Some(rows) = value.as_array_mut() else {
        return;
    };
    for row in rows {
        let Some(obj) = row.as_object_mut() else {
            continue;
        };
        for (k, v) in obj.iter_mut() {
            if k != "symbol" && !keep.iter().any(|f| f == k) {
                *v = Value::Null;
            }
        }
    }
}

/// Lift every key of each array element's nested `key` object up to the element's
/// top level (the REST payload nests option/warrant-specific fields under
/// `option_extend`/`warrant_extend`; the SDK types carry them flat).
pub(super) fn lift_nested(value: &mut Value, key: &str) {
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
pub(super) fn rename_keys(value: &mut Value, renames: &[(&str, &str)]) {
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
pub(super) fn reformat_ymd(value: &mut Value, paths: &[&str]) {
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
pub(super) fn hhmm_to_time(v: &Value) -> Value {
    match v.as_i64() {
        Some(n) => Value::String(format!("{:02}:{:02}:00.0", n / 100, n % 100)),
        None => v.clone(),
    }
}

/// Insert `key: default` into every object in `value` (recursing arrays) that
/// lacks it. Used to supply fields the gateway omits but the SDK requires (e.g.
/// `Candlestick::open_updated`).
pub(super) fn insert_missing(value: &mut Value, key: &str, default: &Value) {
    match value {
        Value::Array(arr) => arr.iter_mut().for_each(|v| insert_missing(v, key, default)),
        Value::Object(map) => {
            map.entry(key.to_string())
                .or_insert_with(|| default.clone());
        }
        _ => {}
    }
}

/// Recursively drop object keys (every depth).
pub(super) fn drop_keys(value: &mut Value, keys: &[&str]) {
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
