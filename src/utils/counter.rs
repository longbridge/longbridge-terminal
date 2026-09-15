//! Symbol / `counter_id` helpers.
//!
//! openapi#562 removed the SDK's client-side `symbol ↔ counter_id` conversion
//! together with its embedded ETF / IX / WT lookup tables. The CLI now sends
//! user-facing symbols directly to the backend on every request. The helpers
//! that remain here do **not** depend on the deleted tables:
//!
//! * [`counter_id_to_symbol`] is a pure string transform over the `counter_id`
//!   the backend still includes in responses (its symbol-conversion rules add a
//!   `symbol` field but leave the original `counter_id` in place).
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

/// Whether `symbol` resolves to an ETF, per the backend `counter_id` directory.
///
/// Resolves the symbol via `POST /v1/quote/symbol-to-counter-ids` and checks
/// whether the returned `counter_id` carries the `ETF/` prefix. A successful
/// response is authoritative (unknown symbol → `false`). The classification
/// drives a branch (ETF vs stock / index path), so a *transient* request
/// failure must not silently misroute a genuine ETF: retry a few times with a
/// short backoff before falling back to `false`.
pub async fn is_etf(symbol: &str, verbose: bool) -> bool {
    let body = json!({ "ticker_regions": [symbol] });
    for attempt in 0..3 {
        match crate::cli::api::http_post("/v1/quote/symbol-to-counter-ids", body.clone(), verbose)
            .await
        {
            Ok(v) => {
                return v
                    .get("list")
                    .and_then(|list| list.get(symbol))
                    .and_then(|cid| cid.as_str())
                    .is_some_and(|cid| cid.starts_with("ETF/"));
            }
            Err(e) => {
                if verbose {
                    eprintln!("  is_etf probe attempt {} failed: {e}", attempt + 1);
                }
                if attempt < 2 {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
        }
    }
    false
}
