//! RPC methods for `longbridge serve`.
//!
//! # Where this layer sits
//!
//! Deliberately *below* the CLI commands, at the API client seam they all share:
//!
//! ```text
//!   CLI commands (151, AI-facing JSON, free to change)
//!         │
//!         ├── QuoteApi / TradeApi traits ──┐
//!         └── http_get / http_post ────────┤
//!                                          ▼
//!                            serve (raw upstream payloads)
//! ```
//!
//! Two consequences, both deliberate:
//!
//! 1. **Stable shapes.** A `result` here is `serde_json::to_value` of the SDK
//!    response type, or the untouched REST body — the Longbridge `OpenAPI`
//!    contract. The CLI's `--format json` reshapes data for AI consumption and
//!    is free to change; third-party clients must not be coupled to that.
//!
//! 2. **No parallel dev track.** Every CLI command reaches the network through
//!    one of these two seams, so `serve` covers all of them without per-command
//!    work. REST-backed commands need nothing at all — `api.get`/`api.post`
//!    already reach any endpoint. SDK-backed commands are covered by the trait
//!    mirror below, and [`tests::serve_exposes_every_api_trait_method`] fails
//!    the build if a trait method is ever added without one.

use anyhow::{bail, Result};
use longbridge::quote::{PushEvent, PushEventDetail, SubFlags};
use serde_json::{json, Value};

use super::params::{bail_param, param_err, Params};
use super::protocol::{Message, PROTOCOL_VERSION};
use crate::cli::api::{QuoteApi, TradeApi};

/// Namespace prefixes: `quote.<QuoteApi method>`, `trade.<TradeApi method>`.
///
/// Named after the API trait methods rather than the CLI command spellings on
/// purpose — CLI names may be tuned for AI ergonomics, the API seam is the
/// stable thing, and the mechanical mapping is what lets the coverage test
/// enforce itself.
const QUOTE_PREFIX: &str = "quote.";
const TRADE_PREFIX: &str = "trade.";

/// Methods that are not a trait mirror: session control, the REST passthrough,
/// and the live subscription that has no one-shot CLI equivalent.
const EXTRA_METHODS: &[&str] = &[
    "initialize",
    "shutdown",
    "api.get",
    "api.post",
    "quote.subscribe",
    "quote.unsubscribe",
];

/// The `QuoteApi` methods exposed, in trait declaration order.
const QUOTE_METHODS: &[&str] = &[
    "quote",
    "depth",
    "brokers",
    "trades",
    "intraday",
    "candlesticks",
    "history_candlesticks_by_date",
    "history_candlesticks_by_offset",
    "static_info",
    "us_crypto_overview",
    "calc_indexes",
    "capital_flow",
    "capital_distribution",
    "market_temperature",
    "history_market_temperature",
    "trading_session",
    "trading_days",
    "security_list",
    "participants",
    "subscriptions",
    "option_quote",
    "option_chain_expiry_date_list",
    "option_chain_info_by_date",
    "warrant_quote",
    "warrant_list",
    "warrant_issuers",
    "watchlist",
    "create_watchlist_group",
    "delete_watchlist_group",
    "update_watchlist_group",
];

/// The `TradeApi` methods exposed, in trait declaration order.
const TRADE_METHODS: &[&str] = &[
    "today_orders",
    "history_orders",
    "order_detail",
    "today_executions",
    "history_executions",
    "submit_order",
    "cancel_order",
    "replace_order",
    "account_balance",
    "cash_flow",
    "stock_positions",
    "fund_positions",
    "margin_ratio",
    "estimate_max_purchase_quantity",
];

/// The order-execution gate shared by `trade.submit_order`, `trade.cancel_order`
/// and `trade.replace_order`.
///
/// Those three are the only methods on this seam that move real money, so they
/// stay dry runs until the caller passes `"execute": true`. A client that omits
/// the flag gets the parsed order back and nothing reaches the exchange.
fn order_dry_run(preview: Value) -> Value {
    serde_json::json!({
        "dry_run": true,
        "preview": preview,
        "next_step": "DRY RUN — nothing was sent to the exchange. Show this preview to \
                      the user and re-send the identical request with \"execute\": true \
                      only after the user has explicitly confirmed this exact order.",
    })
}

fn quote_api() -> crate::cli::api::LbQuoteApi {
    crate::cli::api::LbQuoteApi::new(crate::openapi::quote_cmd())
}

fn trade_api() -> crate::cli::api::LbTradeApi {
    crate::cli::api::LbTradeApi::new(crate::openapi::trade())
}

/// Wrap an SDK response as a JSON-RPC result.
fn ok<T: serde::Serialize>(value: T) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

/// Every method name with its namespace applied, sorted.
///
/// Derived from the same tables [`is_known`] routes on, so the list printed by
/// `serve -h` and returned by `initialize` cannot describe a surface the
/// dispatcher does not have.
pub fn all_methods() -> Vec<String> {
    let mut all: Vec<String> = EXTRA_METHODS.iter().map(|m| (*m).to_string()).collect();
    all.extend(QUOTE_METHODS.iter().map(|m| format!("{QUOTE_PREFIX}{m}")));
    all.extend(TRADE_METHODS.iter().map(|m| format!("{TRADE_PREFIX}{m}")));
    all.sort();
    all
}

/// The `initialize` capabilities object.
///
/// Split out from dispatch so the order-execution gate it advertises can be
/// asserted without spinning up a runtime.
fn session_capabilities() -> Value {
    json!({
        "subscribe": ["quote", "depth", "brokers", "trades"],
        // `initialize` is the client's only discovery surface, so the execution
        // gate has to be advertised here: a client that does not know about it
        // would silently get dry runs forever.
        "orderExecution": {
            "gatedMethods": [
                "trade.submit_order",
                "trade.cancel_order",
                "trade.replace_order",
            ],
            "note": "Dry run unless params include \"execute\": true. Send the \
                     request once without it, show the returned preview to the \
                     user, and resend with \"execute\": true only after the user \
                     has explicitly confirmed that exact order.",
        },
    })
}

pub fn is_known(method: &str) -> bool {
    EXTRA_METHODS.contains(&method)
        || method
            .strip_prefix(QUOTE_PREFIX)
            .is_some_and(|n| QUOTE_METHODS.contains(&n))
        || method
            .strip_prefix(TRADE_PREFIX)
            .is_some_and(|n| TRADE_METHODS.contains(&n))
}

/// Run one method. Errors surface as JSON-RPC errors and never end the
/// session: one bad symbol must not take down a client's live feed.
pub async fn call(method: &str, params: Option<Value>) -> Result<Value> {
    let p = Params(params.as_ref());

    match method {
        // ── Session ──────────────────────────────────────────────────────
        // Reports the surface rather than negotiating it: a client may call
        // any method without calling this first.
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "serverInfo": { "name": "longbridge", "version": env!("CARGO_PKG_VERSION") },
            "capabilities": session_capabilities(),
            "methods": all_methods(),
        })),
        // Handled by the run loop; listed so `is_known` accepts it.
        "shutdown" => Ok(Value::Null),

        // ── Raw REST passthrough ─────────────────────────────────────────
        // Covers every endpoint the CLI reaches over HTTP, including ones
        // added later, and returns the response body untouched.
        "api.get" => {
            let path = p.str("path")?;
            let query = p.query("query")?;
            let pairs: Vec<(&str, &str)> = query
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            crate::cli::api::http_get(&path, &pairs, false).await
        }
        "api.post" => {
            let path = p.str("path")?;
            let body = params
                .as_ref()
                .and_then(|v| v.get("body"))
                .cloned()
                .unwrap_or(Value::Null);
            crate::cli::api::http_post(&path, body, false).await
        }

        // ── Live subscription (no one-shot CLI equivalent) ───────────────
        "quote.subscribe" => {
            let symbols = p.strs("symbols")?;
            let flags = parse_fields(p)?;
            let ctx = crate::openapi::quote_cmd();
            ctx.subscribe(&symbols, flags).await?;

            // The feed itself does not replay the current value, so the result
            // carries a snapshot to paint the first screen with. Taken *after*
            // subscribing, which is the ordering that loses nothing: every
            // change from here on is guaranteed to arrive as a push. A push
            // that raced ahead of the snapshot can still be the older of the
            // two, so a client keeps whichever `timestamp` is newer — the same
            // rule it needs for out-of-order responses anyway.
            let mut result = active_subscriptions(ctx).await?;
            match quote_api().quote(symbols).await {
                Ok(quotes) => {
                    result["quotes"] = serde_json::to_value(quotes)?;
                }
                // The subscription is live either way, and reporting it as a
                // failure would leave the client believing otherwise while
                // pushes arrive. Omitting the field says the snapshot is the
                // part that did not happen; a client falls back to
                // `quote.quote`.
                Err(e) => tracing::warn!("quote.subscribe: snapshot failed: {e}"),
            }
            Ok(result)
        }
        "quote.unsubscribe" => {
            let symbols = p.strs("symbols")?;
            // Drop every field: a client unsubscribing a symbol wants it gone,
            // not downgraded to whatever it did not happen to name.
            let ctx = crate::openapi::quote_cmd();
            ctx.unsubscribe(&symbols, SubFlags::all()).await?;
            active_subscriptions(ctx).await
        }

        _ => {
            if let Some(name) = method.strip_prefix(QUOTE_PREFIX) {
                call_quote(name, p).await
            } else if let Some(name) = method.strip_prefix(TRADE_PREFIX) {
                call_trade(name, p).await
            } else {
                bail!("unknown method `{method}`")
            }
        }
    }
}

async fn call_quote(name: &str, p: Params<'_>) -> Result<Value> {
    let api = quote_api();
    match name {
        "quote" => ok(api.quote(p.strs("symbols")?).await?),
        "depth" => ok(api.depth(p.str("symbol")?).await?),
        "brokers" => ok(api.brokers(p.str("symbol")?).await?),
        "trades" => ok(api
            .trades(p.str("symbol")?, p.usize_or("count", 20)?)
            .await?),
        "intraday" => ok(api.intraday(p.str("symbol")?).await?),
        "candlesticks" => ok(api
            .candlesticks(
                p.str("symbol")?,
                p.period("period")?,
                p.usize_or("count", 100)?,
                p.adjust("adjust")?,
            )
            .await?),
        "history_candlesticks_by_date" => ok(api
            .history_candlesticks_by_date(
                p.str("symbol")?,
                p.period("period")?,
                p.adjust("adjust")?,
                p.date_opt("start")?,
                p.date_opt("end")?,
            )
            .await?),
        "history_candlesticks_by_offset" => ok(api
            .history_candlesticks_by_offset(
                p.str("symbol")?,
                p.period("period")?,
                p.adjust("adjust")?,
                p.usize_or("count", 100)?,
            )
            .await?),
        "static_info" => ok(api.static_info(p.strs("symbols")?).await?),
        "us_crypto_overview" => api.us_crypto_overview(p.str("symbol")?).await,
        "calc_indexes" => {
            let indexes = crate::cli::quote::parse_calc_indexes(&p.strs("indexes")?);
            ok(api.calc_indexes(p.strs("symbols")?, indexes).await?)
        }
        "capital_flow" => ok(api.capital_flow(p.str("symbol")?).await?),
        "capital_distribution" => ok(api.capital_distribution(p.str("symbol")?).await?),
        "market_temperature" => ok(api.market_temperature(p.market("market")?).await?),
        "history_market_temperature" => ok(api
            .history_market_temperature(p.market("market")?, p.date("start")?, p.date("end")?)
            .await?),
        "trading_session" => ok(api.trading_session().await?),
        "trading_days" => ok(api
            .trading_days(p.market("market")?, p.date("start")?, p.date("end")?)
            .await?),
        "security_list" => ok(api.security_list(p.market("market")?).await?),
        "participants" => ok(api.participants().await?),
        // `Subscription` is one of the few SDK types without `Serialize`, so
        // its fields are mirrored by hand under their SDK names.
        "subscriptions" => ok(api
            .subscriptions()
            .await?
            .iter()
            .map(|s| {
                json!({
                    "symbol": s.symbol,
                    "sub_types": flags_to_names(s.sub_types),
                    "candlesticks": s.candlesticks.iter().map(|p| format!("{p:?}")).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>()),
        "option_quote" => ok(api.option_quote(p.strs("symbols")?).await?),
        "option_chain_expiry_date_list" => {
            ok(api.option_chain_expiry_date_list(p.str("symbol")?).await?)
        }
        "option_chain_info_by_date" => ok(api
            .option_chain_info_by_date(p.str("symbol")?, p.date("expiry_date")?)
            .await?),
        "warrant_quote" => ok(api.warrant_quote(p.strs("symbols")?).await?),
        "warrant_list" => ok(api.warrant_list(p.str("symbol")?).await?),
        "warrant_issuers" => ok(api.warrant_issuers().await?),
        "watchlist" => ok(api.watchlist().await?),
        "create_watchlist_group" => ok(json!({
            "id": api.create_watchlist_group(p.str("name")?).await?,
        })),
        "delete_watchlist_group" => {
            api.delete_watchlist_group(p.i64("id")?).await?;
            Ok(Value::Null)
        }
        "update_watchlist_group" => {
            api.update_watchlist_group(update_group_request(p)?).await?;
            Ok(Value::Null)
        }
        other => bail!("unknown method `{QUOTE_PREFIX}{other}`"),
    }
}

async fn call_trade(name: &str, p: Params<'_>) -> Result<Value> {
    use longbridge::trade::{
        EstimateMaxPurchaseQuantityOptions, GetCashFlowOptions, GetHistoryExecutionsOptions,
        GetHistoryOrdersOptions, GetTodayExecutionsOptions, GetTodayOrdersOptions,
        ReplaceOrderOptions, SubmitOrderOptions,
    };
    let api = trade_api();
    match name {
        "today_orders" => {
            let mut opts = GetTodayOrdersOptions::new();
            if let Some(s) = p.str_opt("symbol")? {
                opts = opts.symbol(s);
            }
            ok(api.today_orders(opts).await?)
        }
        "history_orders" => {
            let mut opts = GetHistoryOrdersOptions::new();
            if let Some(s) = p.str_opt("symbol")? {
                opts = opts.symbol(s);
            }
            if let Some(s) = p.str_opt("start")? {
                opts = opts.start_at(crate::cli::output::parse_datetime_start(&s)?);
            }
            if let Some(e) = p.str_opt("end")? {
                opts = opts.end_at(crate::cli::output::parse_datetime_end(&e)?);
            }
            ok(api.history_orders(opts).await?)
        }
        "order_detail" => ok(api.order_detail(p.str("order_id")?).await?),
        "today_executions" => {
            let mut opts = GetTodayExecutionsOptions::new();
            if let Some(s) = p.str_opt("symbol")? {
                opts = opts.symbol(s);
            }
            ok(api.today_executions(opts).await?)
        }
        "history_executions" => {
            let mut opts = GetHistoryExecutionsOptions::new();
            if let Some(s) = p.str_opt("symbol")? {
                opts = opts.symbol(s);
            }
            if let Some(s) = p.str_opt("start")? {
                opts = opts.start_at(crate::cli::output::parse_datetime_start(&s)?);
            }
            if let Some(e) = p.str_opt("end")? {
                opts = opts.end_at(crate::cli::output::parse_datetime_end(&e)?);
            }
            ok(api.history_executions(opts).await?)
        }
        "submit_order" => {
            let mut opts = SubmitOrderOptions::new(
                p.str("symbol")?,
                crate::cli::trade::parse_order_type(&p.str("order_type")?)?,
                parse_side(&p.str("side")?)?,
                decimal(&p.str("quantity")?, "quantity")?,
                crate::cli::trade::parse_tif(&p.str("time_in_force")?)?,
            );
            if let Some(v) = p.str_opt("price")? {
                opts = opts.submitted_price(decimal(&v, "price")?);
            }
            if let Some(v) = p.str_opt("trigger_price")? {
                opts = opts.trigger_price(decimal(&v, "trigger_price")?);
            }
            if let Some(v) = p.str_opt("outside_rth")? {
                opts = opts.outside_rth(crate::cli::trade::parse_outside_rth(&v)?);
            }
            if let Some(v) = p.str_opt("remark")? {
                opts = opts.remark(v);
            }
            if !p.bool_opt("execute")? {
                return Ok(order_dry_run(serde_json::json!({
                    "action": "submit_order",
                    "symbol": p.str("symbol")?,
                    "side": p.str("side")?,
                    "order_type": p.str("order_type")?,
                    "quantity": p.str("quantity")?,
                    "time_in_force": p.str("time_in_force")?,
                    "price": p.str_opt("price")?,
                    "trigger_price": p.str_opt("trigger_price")?,
                    "outside_rth": p.str_opt("outside_rth")?,
                    "remark": p.str_opt("remark")?,
                })));
            }
            ok(api.submit_order(opts).await?)
        }
        "cancel_order" => {
            let order_id = p.str("order_id")?;
            if !p.bool_opt("execute")? {
                return Ok(order_dry_run(serde_json::json!({
                    "action": "cancel_order",
                    "order_id": order_id,
                })));
            }
            api.cancel_order(order_id).await?;
            Ok(Value::Null)
        }
        "replace_order" => {
            let mut opts = ReplaceOrderOptions::new(
                p.str("order_id")?,
                decimal(&p.str("quantity")?, "quantity")?,
            );
            if let Some(v) = p.str_opt("price")? {
                opts = opts.price(decimal(&v, "price")?);
            }
            if !p.bool_opt("execute")? {
                return Ok(order_dry_run(serde_json::json!({
                    "action": "replace_order",
                    "order_id": p.str("order_id")?,
                    "new_quantity": p.str("quantity")?,
                    "new_price": p.str_opt("price")?,
                })));
            }
            api.replace_order(opts).await?;
            Ok(Value::Null)
        }
        "account_balance" => ok(api.account_balance(p.str_opt("currency")?).await?),
        "cash_flow" => {
            let opts = GetCashFlowOptions::new(
                crate::cli::output::parse_datetime_start(&p.str("start")?)?,
                crate::cli::output::parse_datetime_end(&p.str("end")?)?,
            );
            ok(api.cash_flow(opts).await?)
        }
        "stock_positions" => ok(api.stock_positions().await?),
        "fund_positions" => ok(api.fund_positions().await?),
        "margin_ratio" => ok(api.margin_ratio(p.str("symbol")?).await?),
        "estimate_max_purchase_quantity" => {
            let mut opts = EstimateMaxPurchaseQuantityOptions::new(
                p.str("symbol")?,
                crate::cli::trade::parse_order_type(&p.str("order_type")?)?,
                parse_side(&p.str("side")?)?,
            );
            if let Some(v) = p.str_opt("price")? {
                opts = opts.price(decimal(&v, "price")?);
            }
            ok(api.estimate_max_purchase_quantity(opts).await?)
        }
        other => bail!("unknown method `{TRADE_PREFIX}{other}`"),
    }
}

fn parse_side(s: &str) -> Result<longbridge::trade::OrderSide> {
    match s.to_lowercase().as_str() {
        "buy" => Ok(longbridge::trade::OrderSide::Buy),
        "sell" => Ok(longbridge::trade::OrderSide::Sell),
        other => bail_param!("`side` must be buy or sell, got `{other}`"),
    }
}

/// Decimals cross the wire as strings so a client cannot lose precision to a
/// JSON float on the way to an order.
fn decimal(raw: &str, field: &str) -> Result<rust_decimal::Decimal> {
    use std::str::FromStr;
    rust_decimal::Decimal::from_str(raw)
        .map_err(|_| param_err(format!("`{field}` must be a decimal string, got `{raw}`")))
}

fn update_group_request(p: Params<'_>) -> Result<longbridge::quote::RequestUpdateWatchlistGroup> {
    use longbridge::quote::SecuritiesUpdateMode;
    let mode = match p.str_opt("mode")?.as_deref() {
        None | Some("add") => SecuritiesUpdateMode::Add,
        Some("remove") => SecuritiesUpdateMode::Remove,
        Some("replace") => SecuritiesUpdateMode::Replace,
        Some(other) => bail_param!("`mode` must be add, remove or replace, got `{other}`"),
    };
    let securities = p.strs_opt("securities")?;
    Ok(longbridge::quote::RequestUpdateWatchlistGroup {
        id: p.i64("id")?,
        name: p.str_opt("name")?,
        securities: (!securities.is_empty()).then_some(securities),
        mode,
    })
}

fn parse_fields(p: Params<'_>) -> Result<SubFlags> {
    let names = p.strs_opt("fields")?;
    if names.is_empty() {
        return Ok(SubFlags::QUOTE);
    }
    let mut flags = SubFlags::empty();
    for name in &names {
        flags |= match name.as_str() {
            "quote" => SubFlags::QUOTE,
            "depth" => SubFlags::DEPTH,
            "brokers" => SubFlags::BROKER,
            "trades" => SubFlags::TRADE,
            other => bail_param!(
                "`fields`: unknown field `{other}`; expected quote, depth, brokers or trades"
            ),
        };
    }
    Ok(flags)
}

async fn active_subscriptions(ctx: &longbridge::quote::QuoteContext) -> Result<Value> {
    // The SDK keeps a symbol in the list with an empty flag set after its last
    // field is unsubscribed. Reporting those would make `subscribed` mean
    // "known to the SDK" rather than "receiving pushes", so drop them.
    let subs = ctx.subscriptions().await?;
    Ok(json!({
        "subscribed": subs
            .iter()
            .filter(|s| !s.sub_types.is_empty())
            .map(|s| json!({ "symbol": s.symbol, "fields": flags_to_names(s.sub_types) }))
            .collect::<Vec<_>>(),
    }))
}

fn flags_to_names(flags: SubFlags) -> Vec<&'static str> {
    let mut names = Vec::new();
    if flags.contains(SubFlags::QUOTE) {
        names.push("quote");
    }
    if flags.contains(SubFlags::DEPTH) {
        names.push("depth");
    }
    if flags.contains(SubFlags::BROKER) {
        names.push("brokers");
    }
    if flags.contains(SubFlags::TRADE) {
        names.push("trades");
    }
    names
}

/// Translate one WebSocket push into a client notification.
///
/// The `Push*` structs are among the few SDK types without `Serialize`, so
/// their fields are mirrored by hand — under the SDK's own names, so the
/// payload still tracks the upstream contract rather than a shape of our
/// invention. `None` drops event kinds the protocol does not expose yet.
pub fn push_to_notification(event: PushEvent) -> Option<Message> {
    let symbol = event.symbol;
    let (method, mut params) = match event.detail {
        PushEventDetail::Quote(q) => (
            "quote.updated",
            json!({
                "last_done": q.last_done,
                "open": q.open,
                "high": q.high,
                "low": q.low,
                "timestamp": crate::utils::datetime::fmt_rfc3339(q.timestamp),
                "volume": q.volume,
                "turnover": q.turnover,
                // `TradeStatus` has no `Serialize`; its Debug name is the same
                // spelling the CLI prints.
                "trade_status": format!("{:?}", q.trade_status),
                "trade_session": q.trade_session,
                "current_volume": q.current_volume,
                "current_turnover": q.current_turnover,
            }),
        ),
        PushEventDetail::Depth(d) => ("quote.depth", json!({ "asks": d.asks, "bids": d.bids })),
        PushEventDetail::Brokers(b) => (
            "quote.brokers",
            json!({ "ask_brokers": b.ask_brokers, "bid_brokers": b.bid_brokers }),
        ),
        PushEventDetail::Trade(t) => ("quote.trades", json!({ "trades": t.trades })),
        PushEventDetail::Candlestick(_) => return None,
    };
    // Splice the symbol in: the push payload carries only the data, and a
    // notification without a symbol is useless to a client tracking a list.
    params
        .as_object_mut()?
        .insert("symbol".into(), symbol.into());
    Some(Message::notification(method, params))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Extract the `async fn` names declared by a trait in `src/cli/api.rs`.
    fn trait_methods(trait_name: &str) -> Vec<String> {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cli/api.rs"),
        )
        .expect("read src/cli/api.rs");
        let start = src
            .find(&format!("pub trait {trait_name}"))
            .unwrap_or_else(|| panic!("trait {trait_name} not found"));
        let body = &src[start..];
        let end = body.find("\n}").expect("trait end");
        body[..end]
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let rest = line.strip_prefix("async fn ")?;
                Some(rest.split('(').next()?.trim().to_string())
            })
            .collect()
    }

    /// The guard that keeps `serve` from becoming a separate development
    /// track: every `QuoteApi`/`TradeApi` method — the seam every SDK-backed
    /// CLI command goes through — must be reachable over RPC. Adding a trait
    /// method without exposing it fails here.
    #[test]
    fn serve_exposes_every_api_trait_method() {
        for (trait_name, prefix, exposed) in [
            ("QuoteApi", QUOTE_PREFIX, QUOTE_METHODS),
            ("TradeApi", TRADE_PREFIX, TRADE_METHODS),
        ] {
            let declared = trait_methods(trait_name);
            assert!(
                !declared.is_empty(),
                "failed to parse {trait_name} from src/cli/api.rs"
            );
            let missing: Vec<_> = declared
                .iter()
                .filter(|m| !exposed.contains(&m.as_str()))
                .collect();
            assert!(
                missing.is_empty(),
                "{trait_name} methods not exposed by `serve`: {missing:?}\n\
                 Add them to `{prefix}` dispatch so third-party clients keep parity with the CLI."
            );

            // And the reverse: no method advertised that the trait dropped.
            let stale: Vec<_> = exposed
                .iter()
                .filter(|m| !declared.iter().any(|d| d == *m))
                .collect();
            assert!(
                stale.is_empty(),
                "{trait_name}: stale serve methods {stale:?}"
            );
        }
    }

    #[test]
    fn every_advertised_method_is_routable() {
        let catalog = all_methods();
        for method in &catalog {
            assert!(is_known(method), "{method} advertised but not routable");
        }
        // The catalog is the client's only discovery surface, so it must list
        // the whole seam, mutating methods included.
        for expected in [
            "api.get",
            "quote.watchlist",
            "quote.subscribe",
            "trade.submit_order",
            "trade.stock_positions",
        ] {
            assert!(
                catalog.contains(&expected.to_string()),
                "{expected} missing from catalog"
            );
        }
    }

    #[test]
    fn initialize_advertises_the_order_execution_gate() {
        // The gate is only useful if clients can discover it; losing this entry
        // turns every order call into a silent no-op from the client's view.
        let caps = session_capabilities();
        let gated = caps["orderExecution"]["gatedMethods"]
            .as_array()
            .expect("gatedMethods must be advertised");
        for expected in [
            "trade.submit_order",
            "trade.cancel_order",
            "trade.replace_order",
        ] {
            assert!(
                gated.iter().any(|m| m == expected),
                "{expected} must be advertised as execution-gated"
            );
        }
        let note = caps["orderExecution"]["note"]
            .as_str()
            .expect("gate note must be advertised");
        assert!(
            note.contains("\"execute\": true"),
            "note must name the flag"
        );
    }

    #[test]
    fn unknown_and_near_miss_methods_are_rejected() {
        assert!(!is_known("quote.nope"));
        assert!(!is_known("nope.quote"));
        assert!(!is_known("quote"));
        assert!(!is_known(""));
        assert!(is_known("quote.watchlist"));
        assert!(is_known("trade.stock_positions"));
        assert!(is_known("api.get"));
    }

    #[test]
    fn fields_default_to_quote_and_combine() {
        let v = json!({});
        assert_eq!(parse_fields(Params(Some(&v))).unwrap(), SubFlags::QUOTE);
        let v = json!({"fields": ["quote", "depth"]});
        assert_eq!(
            parse_fields(Params(Some(&v))).unwrap(),
            SubFlags::QUOTE | SubFlags::DEPTH
        );
        let v = json!({"fields": ["candles"]});
        let err = parse_fields(Params(Some(&v))).unwrap_err().to_string();
        assert!(err.contains("candles") && err.contains("depth"), "{err}");
    }

    #[test]
    fn decimals_are_parsed_from_strings_not_floats() {
        assert_eq!(
            decimal("123.456", "price").unwrap(),
            rust_decimal::Decimal::new(123_456, 3)
        );
        let err = decimal("abc", "price").unwrap_err().to_string();
        assert!(err.starts_with("`price`"), "{err}");
    }

    #[test]
    fn order_side_accepts_either_case() {
        use longbridge::trade::OrderSide;
        assert_eq!(parse_side("buy").unwrap(), OrderSide::Buy);
        assert_eq!(parse_side("SELL").unwrap(), OrderSide::Sell);
        assert!(parse_side("hold").unwrap_err().to_string().contains("side"));
    }

    #[test]
    fn watchlist_update_defaults_to_add_and_omits_empty_securities() {
        use longbridge::quote::SecuritiesUpdateMode;
        let v = json!({"id": 42});
        let req = update_group_request(Params(Some(&v))).unwrap();
        assert_eq!(req.id, 42);
        assert!(req.securities.is_none());
        assert!(matches!(req.mode, SecuritiesUpdateMode::Add));

        let v = json!({"id": 1, "mode": "replace", "securities": ["700.HK"]});
        let req = update_group_request(Params(Some(&v))).unwrap();
        assert_eq!(req.securities.unwrap(), vec!["700.HK".to_string()]);
        assert!(matches!(req.mode, SecuritiesUpdateMode::Replace));

        let v = json!({"id": 1, "mode": "obliterate"});
        assert!(update_group_request(Params(Some(&v)))
            .unwrap_err()
            .to_string()
            .contains("obliterate"));
    }

    #[test]
    fn flag_names_round_trip() {
        assert_eq!(flags_to_names(SubFlags::QUOTE), vec!["quote"]);
        assert_eq!(
            flags_to_names(SubFlags::all()),
            vec!["quote", "depth", "brokers", "trades"]
        );
    }
}
