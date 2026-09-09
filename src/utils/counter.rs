//! Symbol / `counter_id` helpers.
//!
//! openapi#562 removed the SDK's client-side `symbol ↔ counter_id` conversion
//! together with its embedded ETF / IX / WT lookup tables. The CLI now sends
//! user-facing symbols directly to the backend on every request. The helpers
//! that remain here do **not** depend on the deleted tables:
//!
//! * [`counter_id_to_symbol`] and [`index_symbol_to_counter_id`] are pure
//!   string transforms over the `counter_id` the backend still returns (in
//!   responses) and still accepts (for the index-constituents endpoint).
//! * [`is_etf`] classifies a symbol by resolving it through the backend
//!   `POST /v1/quote/symbol-to-counter-ids` endpoint instead of a local table.

use serde_json::json;

/// Convert a `counter_id` back to a display symbol.
///
/// - `ST/US/TSLA`    → `TSLA.US`
/// - `ETF/US/SPY`    → `SPY.US`
/// - `IX/US/.DJI`    → `.DJI.US`
/// - `VA/HAS/BTCUSD` → `BTCUSD.HAS`  (crypto: `PAIR.EXCHANGE`)
pub fn counter_id_to_symbol(counter_id: &str) -> String {
    let parts: Vec<&str> = counter_id.splitn(3, '/').collect();
    if let [_prefix, market, code] = parts[..] {
        format!("{code}.{market}")
    } else {
        counter_id.to_string()
    }
}

/// Convert an index symbol (e.g. `HSI.HK`, `.DJI.US`) to a `counter_id`
/// (e.g. `IX/HK/HSI`, `IX/US/.DJI`), always using the `IX/` prefix.
///
/// Pure string transform — the index-constituents endpoint still keys on
/// `counter_id`.
pub fn index_symbol_to_counter_id(symbol: &str) -> String {
    if let Some((code, market)) = symbol.rsplit_once('.') {
        format!("IX/{}/{code}", market.to_uppercase())
    } else {
        symbol.to_string()
    }
}

/// Whether `symbol` resolves to an ETF, per the backend `counter_id` directory.
///
/// Resolves the symbol via `POST /v1/quote/symbol-to-counter-ids` and checks
/// whether the returned `counter_id` carries the `ETF/` prefix. Returns `false`
/// when the backend does not recognize the symbol or the request fails, so
/// callers fall back to the stock / index path.
pub async fn is_etf(symbol: &str, verbose: bool) -> bool {
    let body = json!({ "ticker_regions": [symbol] });
    crate::cli::api::http_post("/v1/quote/symbol-to-counter-ids", body, verbose)
        .await
        .ok()
        .and_then(|v| {
            v.get("list")
                .and_then(|list| list.get(symbol))
                .and_then(|cid| cid.as_str())
                .map(|cid| cid.starts_with("ETF/"))
        })
        .unwrap_or(false)
}
