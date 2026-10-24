use anyhow::{bail, Result};
use futures::stream::{self, StreamExt};
use longbridge::trade::{
    AllExecutionsResponse, EstimateMaxPurchaseQuantityOptions, Execution, GetAllExecutionsOptions,
    GetCashFlowOptions, GetHistoryExecutionsOptions, GetHistoryOrdersOptions,
    GetTodayExecutionsOptions, GetTodayOrdersOptions, GetUSHistoryOrders, GetUSRealizedPLOptions,
    OrderSide, OrderType, OutsideRTH, ReplaceOrderOptions, SubmitOrderOptions, TimeInForceType,
};
use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::str::FromStr;
use time::OffsetDateTime;

use super::{
    api::{LbTradeApi, TradeApi},
    output::{
        fmt_decimal, parse_datetime_end, parse_datetime_start, print_account_banner, print_table,
    },
    OutputFormat,
};
use crate::utils::datetime::fmt_rfc3339;

fn risk_level_name(level: i32) -> &'static str {
    match level {
        0 => "Safe",
        1 => "Medium Risk",
        2 => "Early Warning",
        3 => "Danger",
        _ => "Unknown",
    }
}

pub fn parse_order_type(s: &str) -> Result<OrderType> {
    match s.to_uppercase().as_str() {
        "LO" => Ok(OrderType::LO),
        "MO" => Ok(OrderType::MO),
        "ELO" => Ok(OrderType::ELO),
        "AO" => Ok(OrderType::AO),
        "ALO" => Ok(OrderType::ALO),
        "ODD" => Ok(OrderType::ODD),
        "SLO" => Ok(OrderType::SLO),
        "LIT" => Ok(OrderType::LIT),
        "MIT" => Ok(OrderType::MIT),
        "TSLPAMT" => Ok(OrderType::TSLPAMT),
        "TSLPPCT" => Ok(OrderType::TSLPPCT),
        _ => {
            bail!("Unknown order type '{s}'. Use: LO MO ELO AO ALO ODD SLO LIT MIT TSLPAMT TSLPPCT")
        }
    }
}

pub fn parse_tif(s: &str) -> Result<TimeInForceType> {
    match s.to_lowercase().as_str() {
        "day" => Ok(TimeInForceType::Day),
        "gtc" | "goodtilcanceled" => Ok(TimeInForceType::GoodTilCanceled),
        "gtd" | "goodtildate" => Ok(TimeInForceType::GoodTilDate),
        _ => bail!("Unknown time in force '{s}'. Use: day gtc gtd"),
    }
}

const INTERNAL_ORDER_FIELDS: &[&str] = &[
    "aaid",
    "org_id",
    "ploy_id",
    "ploy_type",
    "card_ids",
    "bid_size_list",
    "button_control",
    "current_millisecond",
    "deductions_status",
    "free_status",
    "platform_deductions_status",
    "force_only_rth",
    "limit_depth_level",
    "tag",
    "trend",
    "trigger_count",
    "trigger_status",
    "trigger_at",
    "account_channel",
];

fn normalize_us_order_map(m: &mut serde_json::Map<String, serde_json::Value>) {
    if let Some(action) = m.get("action").and_then(serde_json::Value::as_i64) {
        let label = match action {
            1 => "Buy",
            2 => "Sell",
            _ => "Unknown",
        };
        m.insert(
            "action".to_string(),
            serde_json::Value::String(label.to_string()),
        );
    }
    for ts_key in &[
        "submitted_at",
        "updated_at",
        "create_time",
        "done_at",
        "time",
    ] {
        if let Some(s) = m.get(*ts_key).and_then(|v| v.as_str()) {
            if let Ok(n) = s.parse::<i64>() {
                m.insert(
                    (*ts_key).to_string(),
                    serde_json::Value::Number(serde_json::Number::from(n)),
                );
            }
        }
    }
    if let Some(s) = m.get("status").and_then(|v| v.as_str()) {
        if let Some(clean) = s.strip_suffix("Status").filter(|s| !s.is_empty()) {
            m.insert(
                "status".to_string(),
                serde_json::Value::String(clean.to_string()),
            );
        }
    }
    if let Some(tif) = m.get("time_in_force").and_then(serde_json::Value::as_i64) {
        let label = match tif {
            1 => "Day",
            3 => "GTC",
            4 => "GTD",
            5 => "IOC",
            6 => "FOK",
            _ => "Unknown",
        };
        m.insert(
            "time_in_force".to_string(),
            serde_json::Value::String(label.to_string()),
        );
    }
    if let Some(st) = m.get("security_type").and_then(|v| v.as_str()) {
        let label = match st {
            "CS" => "Stock",
            "VA" => "Crypto",
            "OPT" => "Option",
            "WAR" => "Warrant",
            "IOPT" => "Inline-Warrant",
            "ETF" => "ETF",
            "ADR" => "ADR",
            _ => "",
        };
        if !label.is_empty() {
            m.insert(
                "security_type".to_string(),
                serde_json::Value::String(label.to_string()),
            );
        }
    }
    // The symbol comes from the gateway only: when the response has one the
    // raw counter_id is dropped, otherwise counter_id is kept as is (never
    // converted to a symbol here).
    if m.get("symbol")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|s| !s.is_empty())
    {
        m.remove("counter_id");
    } else if m.contains_key("counter_id") {
        m.remove("symbol");
    }
    for key in INTERNAL_ORDER_FIELDS {
        m.remove(*key);
    }
    // Normalize empty strings to explicit null. Null = field returned with no value;
    // AI agents use is_null() to detect this (e.g. price is null for market orders).
    for v in m.values_mut() {
        if matches!(v, serde_json::Value::String(s) if s.is_empty()) {
            *v = serde_json::Value::Null;
        }
    }
    // create_time is often absent; promote submitted_at so all output paths see a timestamp.
    if m.get("create_time").is_none_or(serde_json::Value::is_null) {
        if let Some(sa) = m.get("submitted_at").cloned() {
            m.insert("create_time".to_string(), sa);
        }
    }
}

pub async fn cmd_orders(
    history: bool,
    start: Option<String>,
    end: Option<String>,
    symbol: Option<String>,
    // US-only filters (interface 14); ignored for HK/CN accounts
    us_action: Option<String>,
    us_page: u32,
    us_limit: u32,
    format: &OutputFormat,
    _verbose: bool,
) -> Result<()> {
    // US accounts: SDK us_query_orders (interface 14)
    if crate::openapi::is_us_account().await {
        let side_str = us_action.as_deref().unwrap_or("").to_lowercase();
        let side = match side_str.as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => OrderSide::Unknown,
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let now_i64 = i64::try_from(now).unwrap_or(i64::MAX);
        // history=false: default start is today midnight UTC; history=true: 90 days
        let default_start = if history {
            now_i64 - 86400 * 90
        } else {
            now_i64 - (now_i64 % 86400)
        };
        let start_ts = match start.as_deref() {
            Some(s) => parse_datetime_start(s)?.unix_timestamp(),
            None => default_start,
        };
        let end_ts = match end.as_deref() {
            Some(s) => parse_datetime_end(s)?.unix_timestamp(),
            None => now_i64,
        };
        let opts = GetUSHistoryOrders {
            symbol: symbol.clone(),
            side,
            start_at: start_ts,
            end_at: end_ts,
            query_type: 0,
            page: i32::try_from(us_page).unwrap_or(1),
            limit: i32::try_from(us_limit).unwrap_or(20),
        };
        let resp = crate::openapi::trade().us_query_orders(opts).await?;
        let data = super::us_shape::orders_json(&resp.orders);
        if let Some(note) = us_orders_page_note(
            us_page,
            us_limit,
            resp.orders.len(),
            i64::from(resp.total_count),
        ) {
            eprintln!("{note}");
        }
        match format {
            OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&data)?),
            OutputFormat::Pretty => {
                let rows = super::us_shape::table_rows(&data, ORDER_HEADERS);
                print_table(ORDER_HEADERS, rows, format);
            }
        }
        return Ok(());
    }

    let ctx = crate::openapi::trade();

    let orders = if history {
        let mut opts = GetHistoryOrdersOptions::new();
        if let Some(s) = symbol {
            opts = opts.symbol(s);
        }
        if let Some(s) = start {
            opts = opts.start_at(parse_datetime_start(&s)?);
        }
        if let Some(e) = end {
            opts = opts.end_at(parse_datetime_end(&e)?);
        }
        ctx.history_orders(opts).await?
    } else {
        let opts = longbridge::trade::GetTodayOrdersOptions::new();
        let opts = if let Some(s) = symbol {
            opts.symbol(s)
        } else {
            opts
        };
        ctx.today_orders(opts).await?
    };

    let rows = ap_order_rows(&orders);
    print_table(ORDER_HEADERS, rows, format);
    Ok(())
}

/// The stderr note for a US order page: past the last page when page `page`
/// came back empty after page 1, more pages when `total` exceeds the rows
/// through this one, `None` otherwise. The server may not honour `limit`, so
/// the page size is the larger of `limit` and the `shown` rows it returned.
fn us_orders_page_note(page: u32, limit: u32, shown: usize, total: i64) -> Option<String> {
    if shown == 0 && page > 1 {
        return Some(format!(
            "Note: page {page} is past the last page (total {total})"
        ));
    }
    let shown_u = u64::try_from(shown).unwrap_or(u64::MAX);
    let page_size = u64::from(limit).max(shown_u);
    let through = u64::from(page.saturating_sub(1))
        .saturating_mul(page_size)
        .saturating_add(shown_u);
    let total_u = u64::try_from(total).unwrap_or_default();
    (total_u > through).then(|| {
        format!("Note: page {page} shows {shown} of {total} orders; use --page / --limit for more")
    })
}

/// Column headers of `order` / `order --history`; `--format json` keys are
/// these in snake case. US rows use the same keys (see `us_shape`).
const ORDER_HEADERS: &[&str] = &[
    "Order ID",
    "Symbol",
    "Side",
    "Order Type",
    "Status",
    "Quantity",
    "Price",
    "Executed Quantity",
    "Executed Price",
    "Created At",
];

/// AP `order` / `order --history` table rows, one per order.
fn ap_order_rows(orders: &[longbridge::trade::Order]) -> Vec<Vec<String>> {
    orders
        .iter()
        .map(|o| {
            vec![
                o.order_id.clone(),
                o.symbol.clone(),
                format!("{:?}", o.side),
                format!("{:?}", o.order_type),
                format!("{:?}", o.status),
                o.quantity.to_string(),
                fmt_decimal(&o.price),
                o.executed_quantity.to_string(),
                fmt_decimal(&o.executed_price),
                fmt_rfc3339(o.submitted_at),
            ]
        })
        .collect()
}

pub async fn cmd_order_detail(
    order_id: String,
    attached: bool,
    format: &OutputFormat,
) -> Result<()> {
    // US accounts always use the US order detail (interface 16).
    if crate::openapi::is_us_account().await {
        // Read the raw body: the typed SDK answer has no instrument identifier.
        let full = super::api::us_order_detail_raw(&order_id).await?;
        // --attached: show the attached child order instead of the main order.
        // current_attached_order may be nested inside full["order"] or at the
        // response root — check both.
        let attached_val = if full["order"]["current_attached_order"].is_null() {
            full["current_attached_order"].clone()
        } else {
            full["order"]["current_attached_order"].clone()
        };
        let using_attached = attached && !attached_val.is_null();
        let data = if using_attached {
            if let Some(o) = attached_val.as_object() {
                let mut m = o.clone();
                normalize_us_order_map(&mut m);
                serde_json::Value::Object(m)
            } else {
                attached_val
            }
        } else {
            super::us_shape::order_detail_json(&full)
        };
        match format {
            OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&data)?),
            OutputFormat::Pretty => {
                let val = |k: &str| -> String {
                    match &data[k] {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Number(n) => n.to_string(),
                        serde_json::Value::Null | serde_json::Value::Bool(_) => "-".to_string(),
                        other => other.to_string(),
                    }
                };
                let label = if using_attached {
                    "Attached order"
                } else {
                    "Order"
                };
                println!("{label} detail for {order_id}:");
                // US orders use id/symbol/action; HK/CN use order_id/symbol/side
                let id_val = if data["id"].is_null() {
                    val("order_id")
                } else {
                    val("id")
                };
                // No symbol from the gateway: show the raw counter_id instead.
                let (sym_key, sym_val) =
                    if data["symbol"].is_null() && !data["counter_id"].is_null() {
                        ("counter_id", val("counter_id"))
                    } else {
                        ("symbol", val("symbol"))
                    };
                let side_val = if data["action"].is_null() {
                    val("side")
                } else {
                    val("action")
                };
                println!("  {:<20} {}", "order_id", id_val);
                println!("  {sym_key:<20} {sym_val}");
                println!("  {:<20} {}", "side", side_val);
                for key in &["status", "order_type", "quantity", "price"] {
                    println!("  {key:<20} {}", val(key));
                }
            }
        }
        return Ok(());
    }
    let ctx = crate::openapi::trade();
    let detail = ctx.order_detail(order_id).await?;

    let executed_amount = detail.executed_price.map(|p| p * detail.executed_quantity);
    let outside_rth = detail
        .outside_rth
        .map_or_else(|| "-".to_string(), |v| format!("{v:?}"));

    match format {
        OutputFormat::Json => {
            let val = ap_order_detail_json(&detail);
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Pretty => {
            let headers = &["Field", "Value"];
            let rows = vec![
                vec!["Order ID".to_string(), detail.order_id.clone()],
                vec!["Stock Name".to_string(), detail.stock_name.clone()],
                vec!["Symbol".to_string(), detail.symbol.clone()],
                vec!["Side".to_string(), format!("{:?}", detail.side)],
                vec!["Order Type".to_string(), format!("{:?}", detail.order_type)],
                vec!["Status".to_string(), format!("{:?}", detail.status)],
                vec!["Quantity".to_string(), detail.quantity.to_string()],
                vec!["Price".to_string(), fmt_decimal(&detail.price)],
                vec![
                    "Executed Qty".to_string(),
                    detail.executed_quantity.to_string(),
                ],
                vec![
                    "Executed Price".to_string(),
                    fmt_decimal(&detail.executed_price),
                ],
                vec![
                    "Executed Amount".to_string(),
                    executed_amount.map(|a| a.to_string()).unwrap_or_default(),
                ],
                vec![
                    "Time in Force".to_string(),
                    format!("{:?}", detail.time_in_force),
                ],
                vec!["Outside RTH".to_string(), outside_rth],
                vec!["Currency".to_string(), detail.currency.clone()],
                vec!["Submitted At".to_string(), fmt_rfc3339(detail.submitted_at)],
                vec![
                    "Updated At".to_string(),
                    detail.updated_at.map(fmt_rfc3339).unwrap_or_default(),
                ],
                vec!["Remark".to_string(), detail.msg.clone()],
            ];
            print_table(headers, rows, &OutputFormat::Pretty);

            if !detail.history.is_empty() {
                println!();
                println!("History");
                let hist_headers = &["Status", "Time", "Price", "Quantity", "Message"];
                let hist_rows = detail
                    .history
                    .iter()
                    .map(|h| {
                        vec![
                            format!("{:?}", h.status),
                            fmt_rfc3339(h.time),
                            h.price.to_string(),
                            h.quantity.to_string(),
                            h.msg.clone(),
                        ]
                    })
                    .collect();
                print_table(hist_headers, hist_rows, &OutputFormat::Pretty);
            }
        }
    }
    Ok(())
}

/// AP `order <id> --format json` object.
fn ap_order_detail_json(detail: &longbridge::trade::OrderDetail) -> serde_json::Value {
    let executed_amount = detail.executed_price.map(|p| p * detail.executed_quantity);
    let outside_rth = detail
        .outside_rth
        .map_or_else(|| "-".to_string(), |v| format!("{v:?}"));
    let history: Vec<serde_json::Value> = detail
        .history
        .iter()
        .map(|h| {
            serde_json::json!({
                "status": format!("{:?}", h.status),
                "time": fmt_rfc3339(h.time),
                "price": h.price.to_string(),
                "quantity": h.quantity.to_string(),
                "msg": h.msg,
            })
        })
        .collect();
    serde_json::json!({
        "order_id": detail.order_id,
        "stock_name": detail.stock_name,
        "symbol": detail.symbol,
        "side": format!("{:?}", detail.side),
        "order_type": format!("{:?}", detail.order_type),
        "status": format!("{:?}", detail.status),
        "quantity": detail.quantity.to_string(),
        "price": fmt_decimal(&detail.price),
        "executed_quantity": detail.executed_quantity.to_string(),
        "executed_price": fmt_decimal(&detail.executed_price),
        "executed_amount": executed_amount.map(|a| a.to_string()).unwrap_or_default(),
        "time_in_force": format!("{:?}", detail.time_in_force),
        "outside_rth": outside_rth,
        "currency": detail.currency,
        "submitted_at": fmt_rfc3339(detail.submitted_at),
        "updated_at": detail.updated_at.map(fmt_rfc3339).unwrap_or_default(),
        "remark": detail.msg,
        "history": history,
    })
}

/// One execution row for the today / history output.
struct ExecRow {
    order_id: String,
    trade_id: String,
    symbol: String,
    /// `Buy` / `Sell`, or `Unknown` when it cannot be resolved (as AP
    /// prints the SDK side).
    side: String,
    price: String,
    quantity: String,
    time_iso: String,
}

impl ExecRow {
    fn from_sdk(e: &Execution, side: String) -> Self {
        Self {
            order_id: e.order_id.clone(),
            trade_id: e.trade_id.clone(),
            symbol: e.symbol.clone(),
            side,
            price: e.price.to_string(),
            quantity: e.quantity.to_string(),
            time_iso: fmt_rfc3339(e.trade_done_at),
        }
    }

    /// The `--format json` object of this row; its keys are the
    /// `order executions` schema fields.
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "order_id": self.order_id,
            "trade_id": self.trade_id,
            "symbol": self.symbol,
            "side": self.side,
            "price": self.price,
            "quantity": self.quantity,
            "trade_done_at": self.time_iso,
            "time": self.time_iso,
        })
    }
}

/// Execution rows. AP accounts print the SDK side as is. US accounts: the
/// execution APIs may omit `side` (the SDK then reports `Unknown`), so it is
/// taken from `us_sides` (order ID -> side, from the account's orders or the
/// order detail), or kept `Unknown` when the order was not found.
fn exec_rows(executions: &[Execution], us_sides: Option<&HashMap<String, String>>) -> Vec<ExecRow> {
    executions
        .iter()
        .map(|e| {
            let side = match us_sides {
                Some(sides) if e.side == OrderSide::Unknown => sides
                    .get(&e.order_id)
                    .cloned()
                    .unwrap_or_else(|| format!("{:?}", e.side)),
                _ => format!("{:?}", e.side),
            };
            ExecRow::from_sdk(e, side)
        })
        .collect()
}

/// US order page size used to look up execution sides.
const US_SIDE_LOOKUP_LIMIT: i32 = 100;
/// Most US order pages read to look up execution sides.
const US_SIDE_LOOKUP_MAX_PAGES: i32 = 20;
/// How far before the earliest execution the side lookup reaches, so orders
/// submitted earlier (e.g. GTC orders filled later) are still found.
const US_SIDE_LOOKUP_LOOKBACK_SECS: i64 = 7 * 86_400;

/// Most unmatched US orders whose side is read from the order detail.
const US_SIDE_DETAIL_MAX: usize = 20;
/// Order detail requests in flight at once during the side lookup.
const US_SIDE_DETAIL_CONCURRENCY: usize = 4;

/// Side (`Buy` / `Sell`) of each US order behind an execution that has no
/// side, keyed by order ID. First reads the account's orders over the
/// execution range ([`us_sides_from_orders`]); orders still not found are
/// then read one by one from the order detail, at most
/// [`US_SIDE_DETAIL_MAX`] of them, [`US_SIDE_DETAIL_CONCURRENCY`] at a time.
/// Lookup failures only print a warning; orders not found are absent.
async fn us_execution_sides(
    api: &dyn TradeApi,
    executions: &[Execution],
    start: Option<OffsetDateTime>,
    end: Option<OffsetDateTime>,
    symbol: Option<&str>,
) -> HashMap<String, String> {
    let mut sides = us_sides_from_orders(api, executions, start, end, symbol)
        .await
        .unwrap_or_else(|e| {
            eprintln!("Warning: could not read US orders to fill the execution side: {e}");
            HashMap::new()
        });
    let mut missing: Vec<&str> = Vec::new();
    for e in executions {
        let id = e.order_id.as_str();
        if e.side == OrderSide::Unknown && !sides.contains_key(id) && !missing.contains(&id) {
            missing.push(id);
        }
    }
    if missing.is_empty() {
        return sides;
    }
    if missing.len() > US_SIDE_DETAIL_MAX {
        eprintln!(
            "Warning: {} US orders were not found in the order list; reading the side of the first {US_SIDE_DETAIL_MAX} from the order detail",
            missing.len()
        );
        missing.truncate(US_SIDE_DETAIL_MAX);
    }
    let results: Vec<(&str, Result<serde_json::Value>)> = stream::iter(missing)
        .map(|id| async move { (id, api.us_order_detail(id.to_string()).await) })
        .buffer_unordered(US_SIDE_DETAIL_CONCURRENCY)
        .collect()
        .await;
    for (id, res) in results {
        match res {
            Ok(body) => {
                let detail = super::us_shape::order_detail_json(&body);
                match detail.get("side").and_then(serde_json::Value::as_str) {
                    Some(side) if side != "Unknown" => {
                        sides.insert(id.to_string(), side.to_string());
                    }
                    _ => {}
                }
            }
            Err(e) => {
                eprintln!("Warning: could not read US order {id} to fill the execution side: {e}");
            }
        }
    }
    sides
}

/// Sides of the US orders behind side-less executions, read from the
/// account's orders (`us_query_orders`) over `start..end` widened to cover
/// every execution time (and [`US_SIDE_LOOKUP_LOOKBACK_SECS`] earlier), page
/// by page. Stops once all orders are found, after
/// [`US_SIDE_LOOKUP_MAX_PAGES`] pages, or when the pages run out: at an empty
/// page, past `total_count` when it is positive (the SDK reports a missing
/// `total_count` as 0), otherwise at a page shorter than both the requested
/// limit and the previous page. The server may cap the page below the
/// requested limit, so a short first page alone does not end the lookup.
///
/// Only a failure on the first page is returned as an error; a later page
/// failing prints a warning and keeps the sides found on earlier pages.
async fn us_sides_from_orders(
    api: &dyn TradeApi,
    executions: &[Execution],
    start: Option<OffsetDateTime>,
    end: Option<OffsetDateTime>,
    symbol: Option<&str>,
) -> Result<HashMap<String, String>> {
    let mut wanted: HashSet<&str> = executions
        .iter()
        .filter(|e| e.side == OrderSide::Unknown)
        .map(|e| e.order_id.as_str())
        .collect();
    let mut sides = HashMap::new();
    let times = executions
        .iter()
        .filter(|e| e.side == OrderSide::Unknown)
        .map(|e| e.trade_done_at.unix_timestamp());
    let (Some(first), Some(last)) = (times.clone().min(), times.max()) else {
        return Ok(sides);
    };
    let start_ts = start
        .map_or(first, |dt| dt.unix_timestamp().min(first))
        .saturating_sub(US_SIDE_LOOKUP_LOOKBACK_SECS);
    let end_ts = end.map_or(last, |dt| dt.unix_timestamp().max(last));

    let page_size = usize::try_from(US_SIDE_LOOKUP_LIMIT).unwrap_or(usize::MAX);
    let mut seen: i64 = 0;
    let mut prev_len: Option<usize> = None;
    for page in 1..=US_SIDE_LOOKUP_MAX_PAGES {
        let resp = match api
            .us_query_orders(GetUSHistoryOrders {
                symbol: symbol.map(str::to_string),
                side: OrderSide::Unknown,
                start_at: start_ts,
                end_at: end_ts,
                query_type: 0,
                page,
                limit: US_SIDE_LOOKUP_LIMIT,
            })
            .await
        {
            Ok(resp) => resp,
            Err(e) if page == 1 => return Err(e),
            Err(e) => {
                eprintln!(
                    "Warning: could not read US orders page {page} to fill the execution side: {e}"
                );
                break;
            }
        };
        if resp.orders.is_empty() {
            break;
        }
        seen = seen.saturating_add(i64::try_from(resp.orders.len()).unwrap_or(i64::MAX));
        for raw in &resp.orders {
            let item = super::us_shape::order_item(raw);
            let (Some(id), Some(side)) = (
                item.get("order_id").and_then(serde_json::Value::as_str),
                item.get("side").and_then(serde_json::Value::as_str),
            ) else {
                continue;
            };
            if side != "Unknown" && wanted.remove(id) {
                sides.insert(id.to_string(), side.to_string());
            }
        }
        let len = resp.orders.len();
        let last_page = if resp.total_count > 0 {
            seen >= i64::from(resp.total_count)
        } else {
            len < page_size && prev_len.is_some_and(|prev| len < prev)
        };
        prev_len = Some(len);
        if wanted.is_empty() || last_page {
            break;
        }
    }
    Ok(sides)
}

/// Executions between `start` and `end` (optionally one `symbol`), every page.
///
/// US accounts read `GET /v1/trade/execution/history`, which the SDK pages
/// through `has_more` itself (`/v3/trade/execution/all` answers HTTP 500 for
/// US accounts). AP accounts read `/v3/trade/execution/all`, filtered by
/// execution time, through `all_page` (one page per call), walking `page`
/// until `has_more` is false.
async fn fetch_history_executions<F, Fut>(
    api: &dyn TradeApi,
    is_us: bool,
    start: Option<OffsetDateTime>,
    end: Option<OffsetDateTime>,
    symbol: Option<&str>,
    mut all_page: F,
) -> Result<Vec<Execution>>
where
    F: FnMut(GetAllExecutionsOptions) -> Fut,
    Fut: std::future::Future<Output = Result<AllExecutionsResponse>>,
{
    if is_us {
        let mut opts = GetHistoryExecutionsOptions::new();
        if let Some(s) = symbol {
            opts = opts.symbol(s);
        }
        if let Some(dt) = start {
            opts = opts.start_at(dt);
        }
        if let Some(dt) = end {
            opts = opts.end_at(dt);
        }
        return api.history_executions(opts).await;
    }
    // Each v3 page holds at most 1000 records; bounded as a runaway guard.
    let mut all: Vec<Execution> = Vec::new();
    for page in 1..=1000u64 {
        let mut opts = GetAllExecutionsOptions::new().page(page);
        if let Some(s) = symbol {
            opts = opts.symbol(s);
        }
        if let Some(dt) = start {
            opts = opts.start_at(dt);
        }
        if let Some(dt) = end {
            opts = opts.end_at(dt);
        }
        let resp = all_page(opts).await?;
        if resp.trades.is_empty() {
            break;
        }
        all.extend(resp.trades);
        if !resp.has_more {
            break;
        }
    }
    Ok(all)
}

pub async fn cmd_executions(
    history: bool,
    start: Option<String>,
    end: Option<String>,
    symbol: Option<String>,
    format: &OutputFormat,
) -> Result<()> {
    let api = LbTradeApi::new(crate::openapi::trade());
    // The range applies to --history only.
    let (start_dt, end_dt) = if history {
        (
            start.as_deref().map(parse_datetime_start).transpose()?,
            end.as_deref().map(parse_datetime_end).transpose()?,
        )
    } else {
        (None, None)
    };
    let is_us = crate::openapi::is_us_account().await;
    let executions = if history {
        let ctx = crate::openapi::trade();
        fetch_history_executions(
            &api,
            is_us,
            start_dt,
            end_dt,
            symbol.as_deref(),
            |opts| async move { Ok(ctx.all_executions(opts).await?) },
        )
        .await?
    } else {
        let mut exec_opts = GetTodayExecutionsOptions::new();
        if let Some(s) = &symbol {
            exec_opts = exec_opts.symbol(s.clone());
        }
        api.today_executions(exec_opts).await?
    };

    let us_sides = if is_us {
        Some(us_execution_sides(&api, &executions, start_dt, end_dt, symbol.as_deref()).await)
    } else {
        None
    };
    let rows = exec_rows(&executions, us_sides.as_ref());

    match format {
        OutputFormat::Json => {
            let arr: Vec<serde_json::Value> = rows.iter().map(ExecRow::to_json).collect();
            println!("{}", serde_json::to_string_pretty(&arr)?);
        }
        OutputFormat::Pretty => {
            let headers = &[
                "Order ID", "Trade ID", "Symbol", "Side", "Price", "Quantity", "Time",
            ];
            let table_rows = rows
                .iter()
                .map(|r| {
                    vec![
                        r.order_id.clone(),
                        r.trade_id.clone(),
                        r.symbol.clone(),
                        r.side.clone(),
                        r.price.clone(),
                        r.quantity.clone(),
                        r.time_iso.clone(),
                    ]
                })
                .collect();
            print_table(headers, table_rows, &OutputFormat::Pretty);
        }
    }
    Ok(())
}

pub fn parse_outside_rth(s: &str) -> Result<OutsideRTH> {
    match s.to_uppercase().as_str() {
        "RTH_ONLY" => Ok(OutsideRTH::RTHOnly),
        "ANY_TIME" => Ok(OutsideRTH::AnyTime),
        "OVERNIGHT" => Ok(OutsideRTH::Overnight),
        _ => bail!("Unknown outside-rth '{s}'. Use: RTH_ONLY ANY_TIME OVERNIGHT"),
    }
}

/// Best-effort last traded price, used to sanity-check a previewed order.
/// A quote failure must never block a dry run, so errors collapse to `None`.
async fn preview_last_price(symbol: &str) -> Option<Decimal> {
    crate::openapi::track_quote_cmd();
    let quotes = crate::cli::quote_http::HttpQuoteApi
        .quote(vec![symbol.to_string()])
        .await
        .ok()?;
    quotes.first().map(|q| q.last_done)
}

/// Summary of an existing order, used by the cancel/replace previews so the
/// operator can confirm they are acting on the order they meant.
///
/// US accounts: the order must be fetched, so an invalid ID or a missing order
/// is an error and no confirmation code is issued. AP accounts: best effort,
/// `Ok(None)` when the detail cannot be fetched.
async fn preview_order_summary(order_id: &str) -> Result<Option<serde_json::Value>> {
    if crate::openapi::is_us_account().await {
        return us_preview_order_summary(order_id, |id| async move {
            super::api::us_order_detail_raw(&id).await
        })
        .await
        .map(Some);
    }
    Ok(ap_preview_order_summary(order_id).await)
}

/// The US cancel/replace preview summary of `order_id`, read with `fetch`
/// (`GET /v1/us/orders/{id}`). Fails, naming the reason, when the ID is not a
/// US order ID, the order cannot be fetched, or the body holds no order.
async fn us_preview_order_summary<F, Fut>(order_id: &str, fetch: F) -> Result<serde_json::Value>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<serde_json::Value>>,
{
    ensure_valid_order_id(true, order_id)
        .map_err(|e| anyhow::anyhow!("{e}; no confirmation code was issued"))?;
    let full = fetch(order_id.to_string()).await.map_err(|e| {
        if is_not_found(&e) {
            anyhow::anyhow!("order {order_id} not found ({e}); no confirmation code was issued")
        } else {
            anyhow::anyhow!(
                "could not fetch order {order_id}: {e}; no confirmation code was issued"
            )
        }
    })?;
    // `{order: ...}`: the order must be an object; a null or scalar `order`
    // means there is none. Otherwise the body is the order itself.
    let order = match full.get("order") {
        Some(o) => o.as_object(),
        None => full.as_object(),
    };
    if order.is_none_or(serde_json::Map::is_empty) {
        bail!("order {order_id} not found; no confirmation code was issued");
    }
    Ok(us_order_summary(&full))
}

/// Refuses an order ID a US account cannot use: US order IDs are decimal
/// digits only. AP order IDs are not checked.
fn ensure_valid_order_id(is_us: bool, order_id: &str) -> Result<()> {
    if is_us && !super::api::is_us_order_id(order_id) {
        bail!("invalid order ID `{order_id}`: US order IDs are digits only");
    }
    Ok(())
}

/// Whether a failed order fetch means the order does not exist (HTTP 404).
fn is_not_found(err: &anyhow::Error) -> bool {
    use longbridge::httpclient::HttpClientError;
    match err.downcast_ref::<HttpClientError>() {
        Some(
            HttpClientError::BadStatus(status)
            | HttpClientError::UnexpectedHttpResponse { status, .. },
        ) => status.as_u16() == 404,
        _ => false,
    }
}

/// Best-effort AP summary of an existing order: `None` when the detail cannot
/// be fetched.
async fn ap_preview_order_summary(order_id: &str) -> Option<serde_json::Value> {
    let ctx = crate::openapi::trade();
    let raw = serde_json::to_value(ctx.order_detail(order_id.to_string()).await.ok()?).ok()?;

    let pick = |key: &str| -> Option<String> {
        match raw.get(key)? {
            serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
            serde_json::Value::Null => None,
            other => Some(other.to_string()),
        }
    };
    Some(serde_json::json!({
        "symbol": pick("symbol"),
        "side": pick("side").or_else(|| pick("action")),
        // Non-US `OrderDetail` serialises the enum verbatim ("FilledStatus");
        // trim the suffix so previews read like the rest of the order output.
        "status": pick("status").map(|s| s.strip_suffix("Status").unwrap_or(&s).to_string()),
        "quantity": pick("quantity"),
        "executed_quantity": pick("executed_quantity"),
        "price": pick("price"),
    }))
}

/// The cancel/replace preview summary of a raw `/v1/us/orders/{id}` body
/// (`{order: {...}}` or the order itself). The security is the response's
/// `symbol`, or, when there is no symbol, its raw value under `counter_id`
/// (never converted), so the user sees which security they are acting on.
fn us_order_summary(full: &serde_json::Value) -> serde_json::Value {
    let order = full.get("order").filter(|o| o.is_object()).unwrap_or(full);
    let pick = |key: &str| -> Option<String> {
        match order.get(key)? {
            serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    };
    let side = match order.get("action").and_then(serde_json::Value::as_i64) {
        Some(1) => Some("Buy".to_string()),
        Some(2) => Some("Sell".to_string()),
        _ => None,
    };
    let mut summary = serde_json::json!({
        "side": side,
        "status": pick("status").map(|s| super::us_shape::order_status(&s)),
        "quantity": pick("quantity"),
        "executed_quantity": pick("executed_qty"),
        "price": pick("price"),
    });
    let (key, security) = match pick("symbol") {
        Some(symbol) => ("symbol", Some(symbol)),
        None => ("counter_id", pick("counter_id")),
    };
    summary[key] = security.into();
    summary
}

fn print_order_summary(summary: Option<&serde_json::Value>, order_id: &str) {
    println!("  {:<18}{order_id}", "Order ID");
    let Some(summary) = summary else {
        println!(
            "  {:<18}(unavailable — could not fetch order detail)",
            "Order"
        );
        return;
    };
    for (label, key) in [
        ("Symbol", "symbol"),
        ("Counter ID", "counter_id"),
        ("Side", "side"),
        ("Status", "status"),
        ("Quantity", "quantity"),
        ("Filled", "executed_quantity"),
        ("Price", "price"),
    ] {
        if let Some(v) = summary.get(key).and_then(serde_json::Value::as_str) {
            println!("  {label:<18}{v}");
        }
    }
}

pub async fn cmd_submit_order(
    symbol: String,
    quantity: u64,
    price: Option<String>,
    trigger_price: Option<String>,
    trailing_amount: Option<String>,
    trailing_percent: Option<String>,
    limit_offset: Option<String>,
    expire_date: Option<String>,
    outside_rth: Option<String>,
    remark: Option<String>,
    order_type: String,
    tif: String,
    side: OrderSide,
    execute: Option<String>,
    format: &OutputFormat,
) -> Result<()> {
    let ot = parse_order_type(&order_type)?;
    let tif_val = parse_tif(&tif)?;
    let qty = Decimal::from(quantity);

    let mut opts = SubmitOrderOptions::new(symbol.clone(), ot, side, qty, tif_val);
    if let Some(ref p) = price {
        let price_dec = Decimal::from_str(p).map_err(|_| anyhow::anyhow!("Invalid price: {p}"))?;
        opts = opts.submitted_price(price_dec);
    }
    if let Some(ref tp) = trigger_price {
        let tp_dec =
            Decimal::from_str(tp).map_err(|_| anyhow::anyhow!("Invalid trigger price: {tp}"))?;
        opts = opts.trigger_price(tp_dec);
    }
    if let Some(ref ta) = trailing_amount {
        let ta_dec =
            Decimal::from_str(ta).map_err(|_| anyhow::anyhow!("Invalid trailing amount: {ta}"))?;
        opts = opts.trailing_amount(ta_dec);
    }
    if let Some(ref tp) = trailing_percent {
        let tp_dec =
            Decimal::from_str(tp).map_err(|_| anyhow::anyhow!("Invalid trailing percent: {tp}"))?;
        opts = opts.trailing_percent(tp_dec);
    }
    if let Some(ref lo) = limit_offset {
        let lo_dec =
            Decimal::from_str(lo).map_err(|_| anyhow::anyhow!("Invalid limit offset: {lo}"))?;
        opts = opts.limit_offset(lo_dec);
    }
    if let Some(ref ed) = expire_date {
        let date = time::Date::parse(
            ed,
            &time::format_description::parse("[year]-[month]-[day]")
                .map_err(|e| anyhow::anyhow!("Date format error: {e}"))?,
        )
        .map_err(|_| anyhow::anyhow!("Invalid expire date '{ed}'. Use YYYY-MM-DD"))?;
        opts = opts.expire_date(date);
    }
    if let Some(ref rth) = outside_rth {
        opts = opts.outside_rth(parse_outside_rth(rth)?);
    }
    if let Some(ref r) = remark {
        opts = opts.remark(r.clone());
    }

    // Confirm before submitting
    let mut price_display = match (price.as_deref(), trigger_price.as_deref()) {
        (Some(p), Some(tp)) => format!("{p} (trigger: {tp})"),
        (Some(p), None) => p.to_string(),
        (None, Some(tp)) => format!("market (trigger: {tp})"),
        (None, None) => "market".to_string(),
    };
    if let Some(ref ta) = trailing_amount {
        let _ = write!(price_display, " trailing-amount: {ta}");
    }
    if let Some(ref tp) = trailing_percent {
        let _ = write!(price_display, " trailing-percent: {tp}%");
    }
    if let Some(ref lo) = limit_offset {
        let _ = write!(price_display, " limit-offset: {lo}");
    }
    if let Some(ref ed) = expire_date {
        let _ = write!(price_display, " expire: {ed}");
    }
    if let Some(ref rth) = outside_rth {
        let _ = write!(price_display, " outside-rth: {rth}");
    }
    let scope = crate::utils::dry_run::Scope::order(
        &format!("{side:?}"),
        &symbol,
        &quantity.to_string(),
        price.as_deref().unwrap_or(""),
    );

    // Two-step by design: without --execute this previews and sends nothing.
    let Some(code) = execute else {
        let confirmation = scope.code();
        let last = preview_last_price(&symbol).await;
        let reference = price
            .as_deref()
            .and_then(|p| Decimal::from_str(p).ok())
            .or(last);
        let estimated = reference.map(|p| (p * qty).round_dp(2));
        match format {
            OutputFormat::Json => {
                let val = serde_json::json!({
                    "dry_run": true,
                    "submitted": false,
                    "side": format!("{side:?}"),
                    "symbol": symbol,
                    "quantity": quantity,
                    "order_type": order_type.to_uppercase(),
                    "time_in_force": tif.to_lowercase(),
                    "price": price,
                    "trigger_price": trigger_price,
                    "trailing_amount": trailing_amount,
                    "trailing_percent": trailing_percent,
                    "limit_offset": limit_offset,
                    "expire_date": expire_date,
                    "outside_rth": outside_rth,
                    "remark": remark,
                    "last_price": last.map(|d| d.to_string()),
                    "estimated_amount": estimated.map(|d| d.to_string()),
                    "confirmation_code": confirmation,
                    "message": crate::utils::dry_run::message(&confirmation, "place this order"),
                });
                println!("{}", serde_json::to_string_pretty(&val)?);
            }
            OutputFormat::Pretty => {
                println!("DRY RUN — no order was placed.");
                println!();
                println!("  {:<18}{side:?}", "Action");
                println!("  {:<18}{symbol}", "Symbol");
                println!("  {:<18}{quantity}", "Quantity");
                println!("  {:<18}{}", "Order type", order_type.to_uppercase());
                println!("  {:<18}{price_display}", "Price");
                println!("  {:<18}{}", "Time in force", tif.to_lowercase());
                if let Some(p) = last {
                    println!("  {:<18}{p}", "Last price");
                }
                if let Some(a) = estimated {
                    println!("  {:<18}~{a}", "Est. amount");
                }
                crate::utils::dry_run::print_notice(&confirmation, "place this order");
            }
        }
        return Ok(());
    };
    scope.verify(&code)?;

    println!("Submitting {side:?} order: {quantity} {symbol} @ {price_display}");
    let ctx = crate::openapi::trade();
    let resp = ctx.submit_order(opts).await?;

    match format {
        OutputFormat::Json => {
            let val = serde_json::json!({ "order_id": resp.order_id });
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Pretty => {
            println!("Order submitted successfully.");
            println!("Order ID: {}", resp.order_id);
        }
    }
    Ok(())
}

pub async fn cmd_cancel_order(
    order_id: String,
    execute: Option<String>,
    format: &OutputFormat,
) -> Result<()> {
    let scope = crate::utils::dry_run::Scope::on_order("cancel", &order_id);
    // Two-step by design: without --execute this previews and cancels nothing.
    let Some(code) = execute else {
        let summary = preview_order_summary(&order_id).await?;
        let confirmation = scope.code();
        match format {
            OutputFormat::Json => {
                let val = serde_json::json!({
                    "dry_run": true,
                    "cancelled": false,
                    "action": "cancel",
                    "order_id": order_id,
                    "order": summary,
                    "confirmation_code": confirmation,
                    "message": crate::utils::dry_run::message(&confirmation, "cancel this order"),
                });
                println!("{}", serde_json::to_string_pretty(&val)?);
            }
            OutputFormat::Pretty => {
                println!("DRY RUN — no order was cancelled.");
                println!();
                println!("  {:<18}Cancel", "Action");
                print_order_summary(summary.as_ref(), &order_id);
                crate::utils::dry_run::print_notice(&confirmation, "cancel this order");
            }
        }
        return Ok(());
    };
    // Reject a malformed US order ID before checking the confirmation code.
    ensure_valid_order_id(crate::openapi::is_us_account().await, &order_id)?;
    scope.verify(&code)?;

    let ctx = crate::openapi::trade();
    ctx.cancel_order(order_id.clone()).await?;
    match format {
        OutputFormat::Json => {
            let val = serde_json::json!({ "order_id": order_id, "cancelled": true });
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Pretty => println!("Order {order_id} cancelled."),
    }
    Ok(())
}

pub async fn cmd_replace_order(
    order_id: String,
    qty: Option<u64>,
    price: Option<String>,
    execute: Option<String>,
    format: &OutputFormat,
) -> Result<()> {
    let quantity = qty.ok_or_else(|| anyhow::anyhow!("--qty is required"))?;
    let qty_dec = Decimal::from(quantity);

    let mut opts = ReplaceOrderOptions::new(order_id.clone(), qty_dec);
    if let Some(ref p) = price {
        let price_dec = Decimal::from_str(p).map_err(|_| anyhow::anyhow!("Invalid price: {p}"))?;
        opts = opts.price(price_dec);
    }

    let scope = crate::utils::dry_run::Scope::replace(
        &order_id,
        &quantity.to_string(),
        price.as_deref().unwrap_or(""),
    );
    // Two-step by design: without --execute this previews and changes nothing.
    let Some(code) = execute else {
        let summary = preview_order_summary(&order_id).await?;
        let confirmation = scope.code();
        match format {
            OutputFormat::Json => {
                let val = serde_json::json!({
                    "dry_run": true,
                    "modified": false,
                    "action": "replace",
                    "order_id": order_id,
                    "current": summary,
                    "new_quantity": quantity,
                    "new_price": price,
                    "confirmation_code": confirmation,
                    "message": crate::utils::dry_run::message(&confirmation, "apply this change"),
                });
                println!("{}", serde_json::to_string_pretty(&val)?);
            }
            OutputFormat::Pretty => {
                println!("DRY RUN — no order was modified.");
                println!();
                println!("  {:<18}Replace", "Action");
                print_order_summary(summary.as_ref(), &order_id);
                println!("  {:<18}{quantity}", "New quantity");
                println!(
                    "  {:<18}{}",
                    "New price",
                    price.as_deref().unwrap_or("(unchanged)")
                );
                crate::utils::dry_run::print_notice(&confirmation, "apply this change");
            }
        }
        return Ok(());
    };
    // Reject a malformed US order ID before checking the confirmation code.
    ensure_valid_order_id(crate::openapi::is_us_account().await, &order_id)?;
    scope.verify(&code)?;

    let ctx = crate::openapi::trade();
    ctx.replace_order(opts).await?;
    match format {
        OutputFormat::Json => {
            let val = serde_json::json!({ "order_id": order_id, "modified": true });
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Pretty => println!("Order {order_id} modified."),
    }
    Ok(())
}

/// AP `assets --format json` records, one per currency.
fn ap_assets_json(balances: &[longbridge::trade::AccountBalance]) -> Vec<serde_json::Value> {
    balances
        .iter()
        .map(|b| {
            let cash_infos: Vec<serde_json::Value> = b
                .cash_infos
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "currency": c.currency,
                        "available_cash": c.available_cash.to_string(),
                        "frozen_cash": c.frozen_cash.to_string(),
                        "settling_cash": c.settling_cash.to_string(),
                        "withdraw_cash": c.withdraw_cash.to_string(),
                    })
                })
                .collect();
            serde_json::json!({
                "currency": b.currency,
                "net_assets": b.net_assets.to_string(),
                "total_cash": b.total_cash.to_string(),
                "buy_power": b.buy_power.to_string(),
                "max_finance_amount": b.max_finance_amount.to_string(),
                "remaining_finance_amount": b.remaining_finance_amount.to_string(),
                "init_margin": b.init_margin.to_string(),
                "maintenance_margin": b.maintenance_margin.to_string(),
                "margin_call": b.margin_call.to_string(),
                "risk_level": risk_level_name(b.risk_level),
                "cash_infos": cash_infos,
            })
        })
        .collect()
}

fn print_assets(balances: &[longbridge::trade::AccountBalance], format: &OutputFormat) {
    match format {
        OutputFormat::Json => {
            let records = ap_assets_json(balances);
            println!(
                "{}",
                serde_json::to_string_pretty(&records).unwrap_or_default()
            );
        }
        OutputFormat::Pretty => {
            let headers = &[
                "Currency",
                "Net Assets",
                "Total Cash",
                "Buy Power",
                "Max Finance",
                "Remaining Finance",
                "Init Margin",
                "Maintenance Margin",
                "Margin Call",
                "Risk Level",
            ];
            let rows = balances
                .iter()
                .map(|b| {
                    vec![
                        b.currency.clone(),
                        b.net_assets.to_string(),
                        b.total_cash.to_string(),
                        b.buy_power.to_string(),
                        b.max_finance_amount.to_string(),
                        b.remaining_finance_amount.to_string(),
                        b.init_margin.to_string(),
                        b.maintenance_margin.to_string(),
                        b.margin_call.to_string(),
                        risk_level_name(b.risk_level).to_string(),
                    ]
                })
                .collect::<Vec<_>>();
            print_table(headers, rows, format);

            let cash_headers = &[
                "Currency",
                "Available Cash",
                "Frozen Cash",
                "Settling Cash",
                "Withdrawable",
            ];
            let mut cash_rows = vec![];
            for b in balances {
                for c in &b.cash_infos {
                    cash_rows.push(vec![
                        c.currency.clone(),
                        c.available_cash.to_string(),
                        c.frozen_cash.to_string(),
                        c.settling_cash.to_string(),
                        c.withdraw_cash.to_string(),
                    ]);
                }
            }
            if !cash_rows.is_empty() {
                println!();
                print_table(cash_headers, cash_rows, format);
            }
        }
    }
}

pub async fn cmd_assets(currency: Option<String>, format: &OutputFormat) -> Result<()> {
    if crate::openapi::is_us_account().await {
        let api = LbTradeApi::new(crate::openapi::trade());
        return super::us_asset::run_us_assets(&api, currency, format).await;
    }
    let ctx = crate::openapi::trade();
    let balances = ctx.account_balance(currency.as_deref()).await?;
    print_account_banner(format);
    print_assets(&balances, format);
    Ok(())
}

pub async fn cmd_cash_flow(
    start: Option<String>,
    end: Option<String>,
    format: &OutputFormat,
) -> Result<()> {
    let now = time::OffsetDateTime::now_utc();
    let start_at = start
        .as_deref()
        .map(parse_datetime_start)
        .transpose()?
        .unwrap_or_else(|| now - time::Duration::days(30));
    let end_at = end
        .as_deref()
        .map(parse_datetime_end)
        .transpose()?
        .unwrap_or(now);

    if crate::openapi::is_us_account().await {
        let api = LbTradeApi::new(crate::openapi::trade());
        return super::us_asset::run_us_cash_flow(&api, start_at, end_at, format).await;
    }

    let ctx = crate::openapi::trade();
    let opts = longbridge::trade::GetCashFlowOptions::new(start_at, end_at);
    let flows = ctx.cash_flow(opts).await?;

    print_table(CASH_FLOW_HEADERS, ap_cash_flow_rows(&flows), format);
    Ok(())
}

/// Column headers of `cash-flow`; `--format json` keys are these in snake case.
const CASH_FLOW_HEADERS: &[&str] = &[
    "Flow Name",
    "Symbol",
    "Business Type",
    "Balance",
    "Currency",
    "Time",
    "Description",
];

/// AP `cash-flow` table rows, one per flow.
fn ap_cash_flow_rows(flows: &[longbridge::trade::CashFlow]) -> Vec<Vec<String>> {
    flows
        .iter()
        .map(|f| {
            vec![
                f.transaction_flow_name.clone(),
                f.symbol.clone().unwrap_or_default(),
                format!("{:?}", f.business_type),
                f.balance.to_string(),
                f.currency.clone(),
                fmt_rfc3339(f.business_time),
                f.description.clone(),
            ]
        })
        .collect()
}

pub async fn cmd_fund_positions(format: &OutputFormat) -> Result<()> {
    super::us_asset::ensure_not_us(crate::openapi::is_us_account().await, "Fund positions")?;
    let ctx = crate::openapi::trade();
    let resp = ctx.fund_positions(None).await?;

    print_account_banner(format);
    let headers = &[
        "Counter ID",
        "Name",
        "Net Asset Value",
        "Cost Net Asset Value",
        "Currency",
        "Holding Units",
    ];
    let mut rows = vec![];
    for channel in &resp.channels {
        for pos in &channel.positions {
            rows.push(vec![
                pos.counter_id.clone(),
                pos.symbol_name.clone(),
                pos.current_net_asset_value.to_string(),
                pos.cost_net_asset_value.to_string(),
                pos.currency.clone(),
                pos.holding_units.to_string(),
            ]);
        }
    }

    print_table(headers, rows, format);
    Ok(())
}

pub async fn cmd_margin_ratio(symbol: String, format: &OutputFormat) -> Result<()> {
    super::us_asset::ensure_not_us(crate::openapi::is_us_account().await, "Margin ratio")?;
    let ctx = crate::openapi::trade();
    let ratio = ctx.margin_ratio(symbol.clone()).await?;

    let headers = &["Field", "Value"];
    let rows = vec![
        vec!["Symbol".to_string(), symbol],
        vec![
            "Initial Margin Ratio".to_string(),
            ratio.im_factor.to_string(),
        ],
        vec![
            "Maintenance Margin Ratio".to_string(),
            ratio.mm_factor.to_string(),
        ],
        vec![
            "Forced Liquidation Ratio".to_string(),
            ratio.fm_factor.to_string(),
        ],
    ];

    print_table(headers, rows, format);
    Ok(())
}

pub async fn cmd_max_qty(
    symbol: String,
    side: &str,
    price: Option<String>,
    order_type: &str,
    format: &OutputFormat,
) -> Result<()> {
    let ctx = crate::openapi::trade();
    let side_val = match side.to_lowercase().as_str() {
        "buy" => OrderSide::Buy,
        "sell" => OrderSide::Sell,
        _ => bail!("Unknown side '{side}'. Use: Buy Sell"),
    };
    let ot = parse_order_type(order_type)?;

    let price_dec = price
        .as_deref()
        .map(|p| Decimal::from_str(p).map_err(|_| anyhow::anyhow!("Invalid price: {p}")))
        .transpose()?;

    let opts =
        longbridge::trade::EstimateMaxPurchaseQuantityOptions::new(symbol.clone(), ot, side_val);
    let opts = if let Some(p) = price_dec {
        opts.price(p)
    } else {
        opts
    };

    let resp = ctx.estimate_max_purchase_quantity(opts).await?;

    let headers = &["Field", "Value"];
    let rows = vec![
        vec!["Symbol".to_string(), symbol],
        vec!["Cash Max Qty".to_string(), resp.cash_max_qty.to_string()],
        vec![
            "Margin Max Qty".to_string(),
            resp.margin_max_qty.to_string(),
        ],
    ];

    print_table(headers, rows, format);
    Ok(())
}

pub async fn cmd_portfolio(format: &OutputFormat) -> Result<()> {
    if crate::openapi::is_us_account().await {
        let api = LbTradeApi::new(crate::openapi::trade());
        return super::us_asset::run_us_portfolio(&api, format).await;
    }
    // Portfolio fetches quotes for its holdings, so record the quote operation
    // here at the CLI entry point.
    crate::openapi::track_quote_cmd();
    // Enrich holdings with quotes over HTTP REST (the TUI's own call to
    // `fetch_portfolio()` keeps the WebSocket context).
    let portfolio = crate::openapi::account::fetch_portfolio_with(|symbols| async move {
        let api = crate::cli::quote_http::HttpQuoteApi;
        api.quote(symbols).await
    })
    .await?;

    print_account_banner(format);
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&portfolio)?);
        }
        OutputFormat::Pretty => {
            let o = &portfolio.overview;

            // Overview section
            let risk_label = match o.risk_level {
                0 => "Safe",
                1 => "Middle",
                2 => "Warning",
                3 => "Danger",
                _ => "Unknown",
            };
            let overview_headers = &["Field", "Value"];
            let overview_rows = vec![
                vec!["Currency".to_string(), o.currency.clone()],
                vec!["Total Asset".to_string(), fmt_decimal(&Some(o.total_asset))],
                vec!["Market Cap".to_string(), fmt_decimal(&Some(o.market_cap))],
                vec!["Total Cash".to_string(), fmt_decimal(&Some(o.total_cash))],
                vec!["P/L".to_string(), format!("{:.2}", o.total_pl)],
                vec![
                    "Intraday P/L".to_string(),
                    format!("{:.2}", o.total_today_pl),
                ],
                vec!["Margin Call".to_string(), fmt_decimal(&Some(o.margin_call))],
                vec!["Risk Level".to_string(), risk_label.to_string()],
                vec![
                    "Credit Limit".to_string(),
                    fmt_decimal(&Some(o.credit_limit)),
                ],
                vec![
                    "Fund Market Value".to_string(),
                    fmt_decimal(&Some(o.fund_market_value)),
                ],
            ];
            print_table(overview_headers, overview_rows, format);

            // Asset distribution section
            {
                println!();
                let total = o.total_asset;
                // Aggregate USD market value per market (from symbol suffix)
                let mut market_values: std::collections::BTreeMap<String, rust_decimal::Decimal> =
                    std::collections::BTreeMap::new();
                for h in &portfolio.holdings {
                    let market_label = if let Some(dot_pos) = h.symbol.rfind('.') {
                        match &h.symbol[dot_pos + 1..] {
                            "US" => "US",
                            "SH" | "SZ" => "CN",
                            "SG" => "SG",
                            _ => "HK",
                        }
                    } else {
                        "HK"
                    };
                    *market_values.entry(market_label.to_string()).or_default() +=
                        h.market_value_usd;
                }
                // Add cash and fund
                market_values.insert("Cash".to_string(), o.total_cash);
                if o.fund_market_value > rust_decimal::Decimal::ZERO {
                    market_values.insert("Fund".to_string(), o.fund_market_value);
                }

                // Sort by value descending
                let mut dist: Vec<(String, rust_decimal::Decimal)> =
                    market_values.into_iter().collect();
                dist.sort_by_key(|b| std::cmp::Reverse(b.1));

                let dist_headers = &["Market", "Value (USD)", "%"];
                let dist_rows = dist
                    .iter()
                    .map(|(label, value)| {
                        let pct = if total > rust_decimal::Decimal::ZERO {
                            value / total * rust_decimal::Decimal::ONE_HUNDRED
                        } else {
                            rust_decimal::Decimal::ZERO
                        };
                        vec![
                            label.clone(),
                            format!("{:.2}", value),
                            format!("{:.2}%", pct),
                        ]
                    })
                    .collect::<Vec<_>>();
                print_table(dist_headers, dist_rows, format);
            }

            // Holdings section
            if portfolio.holdings.is_empty() {
                println!("\nNo holdings data");
            } else {
                println!();
                let holding_headers = &[
                    "Symbol",
                    "Name",
                    "Qty",
                    "Price",
                    "Cost",
                    "Mkt Value",
                    "P/L",
                    "P/L%",
                    "Intraday",
                    "Intraday%",
                    "Currency",
                ];
                let holding_rows = portfolio
                    .holdings
                    .iter()
                    .map(|h| {
                        let pl = h.cost_price.map_or(rust_decimal::Decimal::ZERO, |cost| {
                            (h.market_price - cost) * h.quantity
                        });
                        let pl_pct = h
                            .cost_price
                            .filter(|&c| c > rust_decimal::Decimal::ZERO)
                            .map_or(rust_decimal::Decimal::ZERO, |cost| {
                                (h.market_price - cost) / cost * rust_decimal::Decimal::ONE_HUNDRED
                            });
                        let today_pl = h
                            .prev_close
                            .filter(|&pc| pc > rust_decimal::Decimal::ZERO)
                            .map_or(rust_decimal::Decimal::ZERO, |pc| {
                                (h.market_price - pc) * h.quantity
                            });
                        let today_pl_pct = h
                            .prev_close
                            .filter(|&pc| pc > rust_decimal::Decimal::ZERO)
                            .map_or(rust_decimal::Decimal::ZERO, |pc| {
                                (h.market_price - pc) / pc * rust_decimal::Decimal::ONE_HUNDRED
                            });
                        vec![
                            h.symbol.clone(),
                            h.name.clone(),
                            h.quantity.to_string(),
                            fmt_decimal(&Some(h.market_price)),
                            h.cost_price
                                .map(|c| fmt_decimal(&Some(c)))
                                .unwrap_or_default(),
                            fmt_decimal(&Some(h.market_value)),
                            fmt_decimal(&Some(pl)),
                            format!("{:.2}%", pl_pct),
                            fmt_decimal(&Some(today_pl)),
                            format!("{:.2}%", today_pl_pct),
                            format!("{:?}", h.currency),
                        ]
                    })
                    .collect::<Vec<_>>();
                print_table(holding_headers, holding_rows, format);
            }

            // Cash balances section
            if !portfolio.cash_balances.is_empty() {
                println!();
                let cash_headers = &["Currency", "Total", "Available", "Frozen", "Withdrawable"];
                let cash_rows = portfolio
                    .cash_balances
                    .iter()
                    .map(|c| {
                        vec![
                            format!("{:?}", c.currency),
                            fmt_decimal(&Some(c.total_amount)),
                            fmt_decimal(&Some(c.balance)),
                            fmt_decimal(&Some(c.frozen_cash)),
                            fmt_decimal(&Some(c.withdraw_cash)),
                        ]
                    })
                    .collect::<Vec<_>>();
                print_table(cash_headers, cash_rows, format);
            }

            println!("\n{}", t!("Portfolio.QuoteDisclaimer"));
        }
    }

    Ok(())
}

// ─── Testable run_* functions ─────────────────────────────────────────────────

pub async fn run_today_orders(
    api: &dyn TradeApi,
    opts: GetTodayOrdersOptions,
    format: &OutputFormat,
) -> Result<()> {
    let orders = api.today_orders(opts).await?;
    let headers = &[
        "Order ID",
        "Symbol",
        "Side",
        "Order Type",
        "Status",
        "Quantity",
        "Price",
        "Created At",
    ];
    let rows = orders
        .iter()
        .map(|o| {
            vec![
                o.order_id.clone(),
                o.symbol.clone(),
                format!("{:?}", o.side),
                format!("{:?}", o.order_type),
                format!("{:?}", o.status),
                o.quantity.to_string(),
                fmt_decimal(&o.price),
                fmt_rfc3339(o.submitted_at),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_history_orders(
    api: &dyn TradeApi,
    opts: GetHistoryOrdersOptions,
    format: &OutputFormat,
) -> Result<()> {
    let orders = api.history_orders(opts).await?;
    let headers = &[
        "Order ID",
        "Symbol",
        "Side",
        "Order Type",
        "Status",
        "Quantity",
        "Price",
        "Created At",
    ];
    let rows = orders
        .iter()
        .map(|o| {
            vec![
                o.order_id.clone(),
                o.symbol.clone(),
                format!("{:?}", o.side),
                format!("{:?}", o.order_type),
                format!("{:?}", o.status),
                o.quantity.to_string(),
                fmt_decimal(&o.price),
                fmt_rfc3339(o.submitted_at),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_order_detail(
    api: &dyn TradeApi,
    order_id: String,
    format: &OutputFormat,
) -> Result<()> {
    let detail = api.order_detail(order_id).await?;
    match format {
        OutputFormat::Json => {
            let val = serde_json::json!({"order_id": detail.order_id, "symbol": detail.symbol, "side": format!("{:?}", detail.side), "status": format!("{:?}", detail.status)});
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Pretty => {
            let headers = &["Field", "Value"];
            let rows = vec![
                vec!["Order ID".to_string(), detail.order_id.clone()],
                vec!["Symbol".to_string(), detail.symbol.clone()],
                vec!["Side".to_string(), format!("{:?}", detail.side)],
                vec!["Status".to_string(), format!("{:?}", detail.status)],
            ];
            print_table(headers, rows, &OutputFormat::Pretty);
        }
    }
    Ok(())
}

pub async fn run_today_executions(
    api: &dyn TradeApi,
    opts: GetTodayExecutionsOptions,
    format: &OutputFormat,
) -> Result<()> {
    let executions = api.today_executions(opts).await?;
    let headers = &[
        "Order ID", "Trade ID", "Symbol", "Price", "Quantity", "Time",
    ];
    let rows = executions
        .iter()
        .map(|e| {
            vec![
                e.order_id.clone(),
                e.trade_id.clone(),
                e.symbol.clone(),
                e.price.to_string(),
                e.quantity.to_string(),
                fmt_rfc3339(e.trade_done_at),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_history_executions(
    api: &dyn TradeApi,
    opts: GetHistoryExecutionsOptions,
    format: &OutputFormat,
) -> Result<()> {
    let executions = api.history_executions(opts).await?;
    let headers = &[
        "Order ID", "Trade ID", "Symbol", "Price", "Quantity", "Time",
    ];
    let rows = executions
        .iter()
        .map(|e| {
            vec![
                e.order_id.clone(),
                e.trade_id.clone(),
                e.symbol.clone(),
                e.price.to_string(),
                e.quantity.to_string(),
                fmt_rfc3339(e.trade_done_at),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_submit_order(
    api: &dyn TradeApi,
    opts: SubmitOrderOptions,
    format: &OutputFormat,
) -> Result<()> {
    let resp = api.submit_order(opts).await?;
    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({"order_id": resp.order_id}))?
            );
        }
        OutputFormat::Pretty => println!("Order ID: {}", resp.order_id),
    }
    Ok(())
}

pub async fn run_cancel_order(api: &dyn TradeApi, order_id: String) -> Result<()> {
    api.cancel_order(order_id).await?;
    Ok(())
}

pub async fn run_replace_order(api: &dyn TradeApi, opts: ReplaceOrderOptions) -> Result<()> {
    api.replace_order(opts).await?;
    Ok(())
}

pub async fn run_assets(
    api: &dyn TradeApi,
    currency: Option<String>,
    format: &OutputFormat,
) -> Result<()> {
    let balances = api.account_balance(currency).await?;
    print_assets(&balances, format);
    Ok(())
}

pub async fn run_cash_flow(
    api: &dyn TradeApi,
    opts: GetCashFlowOptions,
    format: &OutputFormat,
) -> Result<()> {
    let flows = api.cash_flow(opts).await?;
    let headers = &[
        "Flow Name",
        "Symbol",
        "Business Type",
        "Balance",
        "Currency",
        "Time",
    ];
    let rows = flows
        .iter()
        .map(|f| {
            vec![
                f.transaction_flow_name.clone(),
                f.symbol.clone().unwrap_or_default(),
                format!("{:?}", f.business_type),
                f.balance.to_string(),
                f.currency.clone(),
                fmt_rfc3339(f.business_time),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_positions(api: &dyn TradeApi, format: &OutputFormat) -> Result<()> {
    let resp = api.stock_positions().await?;
    let headers = &[
        "Symbol",
        "Name",
        "Quantity",
        "Available",
        "Cost Price",
        "Currency",
        "Market",
    ];
    let mut rows = vec![];
    for channel in &resp.channels {
        for pos in &channel.positions {
            rows.push(vec![
                pos.symbol.clone(),
                pos.symbol_name.clone(),
                pos.quantity.to_string(),
                pos.available_quantity.to_string(),
                pos.cost_price.to_string(),
                pos.currency.clone(),
                format!("{:?}", pos.market),
            ]);
        }
    }
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_fund_positions(api: &dyn TradeApi, format: &OutputFormat) -> Result<()> {
    let resp = api.fund_positions().await?;
    let headers = &[
        "Counter ID",
        "Name",
        "Net Asset Value",
        "Cost NAV",
        "Currency",
        "Holding Units",
    ];
    let mut rows = vec![];
    for channel in &resp.channels {
        for pos in &channel.positions {
            rows.push(vec![
                pos.counter_id.clone(),
                pos.symbol_name.clone(),
                pos.current_net_asset_value.to_string(),
                pos.cost_net_asset_value.to_string(),
                pos.currency.clone(),
                pos.holding_units.to_string(),
            ]);
        }
    }
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_margin_ratio(
    api: &dyn TradeApi,
    symbol: String,
    format: &OutputFormat,
) -> Result<()> {
    let ratio = api.margin_ratio(symbol.clone()).await?;
    let headers = &["Field", "Value"];
    let rows = vec![
        vec!["Symbol".to_string(), symbol],
        vec![
            "Initial Margin Ratio".to_string(),
            ratio.im_factor.to_string(),
        ],
        vec![
            "Maintenance Margin Ratio".to_string(),
            ratio.mm_factor.to_string(),
        ],
        vec![
            "Forced Liquidation Ratio".to_string(),
            ratio.fm_factor.to_string(),
        ],
    ];
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_max_qty(
    api: &dyn TradeApi,
    opts: EstimateMaxPurchaseQuantityOptions,
    symbol: String,
    format: &OutputFormat,
) -> Result<()> {
    let resp = api.estimate_max_purchase_quantity(opts).await?;
    let headers = &["Field", "Value"];
    let rows = vec![
        vec!["Symbol".to_string(), symbol],
        vec!["Cash Max Qty".to_string(), resp.cash_max_qty.to_string()],
        vec![
            "Margin Max Qty".to_string(),
            resp.margin_max_qty.to_string(),
        ],
    ];
    print_table(headers, rows, format);
    Ok(())
}

// ── Pending commands ─────────────────────────────────────────────────────────

fn val_str(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "-".to_owned(),
        other => other.to_string(),
    }
}

fn print_json_value(data: &serde_json::Value) {
    let mut v = data.clone();
    super::output::strip_counter_ids(&mut v);
    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
}

pub async fn cmd_alert_list(
    symbol: Option<String>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let mut params: Vec<(&str, &str)> = vec![];
    if let Some(ref sym) = symbol {
        params.push(("symbol", sym.as_str()));
    }
    let data = super::api::http_get("/v1/notify/reminders", &params, verbose).await?;
    match format {
        OutputFormat::Json => print_json_value(&data),
        OutputFormat::Pretty => {
            let stocks = match data
                .get("lists")
                .or_else(|| data.get("list"))
                .and_then(|v| v.as_array())
            {
                Some(a) if !a.is_empty() => a,
                _ => {
                    println!("No alerts found.");
                    return Ok(());
                }
            };
            let headers = ["id", "symbol", "price", "alert", "enabled", "frequency"];
            let mut rows: Vec<Vec<String>> = Vec::new();
            for stock in stocks {
                let sym = super::output::item_symbol(stock);
                let price = val_str(&stock["price"]);
                let Some(indicators) = stock.get("indicators").and_then(|v| v.as_array()) else {
                    continue;
                };
                for ind in indicators {
                    let enabled = if ind["enabled"].as_bool().unwrap_or(false) {
                        "\u{2713}" // ✓
                    } else {
                        ""
                    };
                    let freq = match ind["frequency"].as_i64() {
                        Some(1) => "daily",
                        Some(2) => "every",
                        Some(3) => "once",
                        _ => "-",
                    };
                    rows.push(vec![
                        val_str(&ind["id"]),
                        sym.clone(),
                        price.clone(),
                        val_str(&ind["text"]),
                        enabled.to_string(),
                        freq.to_string(),
                    ]);
                }
            }
            if rows.is_empty() {
                println!("No alerts found.");
            } else {
                print_table(&headers, rows, format);
            }
        }
    }
    Ok(())
}

pub async fn cmd_alert_add(
    symbol: String,
    price: &str,
    direction: &str,
    alert_type: &str,
    frequency: &str,
    _note: Option<String>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    // indicator_id: 1=price_rise, 2=price_fall, 3=change%_rise, 4=change%_fall
    let indicator_id: i32 = match (alert_type, direction) {
        ("percent", "fall" | "down") => 4,
        ("percent", _) => 3,
        (_, "fall" | "down") => 2,
        _ => 1,
    };
    let freq: i32 = match frequency {
        "daily" => 1,
        "every" => 2,
        _ => 3, // once
    };
    let setting_key = match indicator_id {
        3 | 4 => "chg",
        _ => "price",
    };
    let body = serde_json::json!({
        "symbol": symbol,
        "indicator_id": indicator_id.to_string(),
        "value_map": { setting_key: price },
        "frequency": freq,
        "enabled": true,
        "scope": 0,
        "state": [1],
    });
    let data = super::api::http_post("/v1/notify/reminders", body, verbose).await?;
    match format {
        OutputFormat::Json => print_json_value(&data),
        OutputFormat::Pretty => println!("Alert added for {symbol} at {price} ({direction})"),
    }
    Ok(())
}

pub async fn cmd_alert_delete(id: String, format: &OutputFormat, verbose: bool) -> Result<()> {
    let id_num: i64 = id
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid alert id '{id}': must be a numeric id"))?;
    let body = serde_json::json!({ "ids": [id_num] });
    let data = super::api::http_delete("/v1/notify/reminders", body, verbose).await?;
    match format {
        OutputFormat::Json => print_json_value(&data),
        OutputFormat::Pretty => println!("Alert {id} deleted"),
    }
    Ok(())
}

pub async fn cmd_alert_set_enabled(
    id: String,
    enabled: bool,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    // Fetch the existing alert to get all required fields
    let list_data = super::api::http_get("/v1/notify/reminders", &[], verbose).await?;
    let stocks = list_data
        .get("lists")
        .or_else(|| list_data.get("list"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let id_num: i64 = id
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid alert id '{id}': must be a numeric id"))?;
    let mut found = false;

    for stock in &stocks {
        let counter_id = stock["counter_id"].as_str().unwrap_or("");
        if let Some(indicators) = stock["indicators"].as_array() {
            for ind in indicators {
                let Some(ind_id) = ind["id"].as_str().and_then(|s| s.parse::<i64>().ok()) else {
                    continue;
                };
                if ind_id == id_num {
                    let body = serde_json::json!({
                        "id": ind_id,
                        "counter_id": counter_id,
                        "indicator_id": ind["indicator_id"].as_str().unwrap_or("1"),
                        "value_map": ind["value_map"],
                        "frequency": ind["frequency"],
                        "enabled": enabled,
                        "scope": ind["scope"],
                        "state": ind["state"],
                    });
                    super::api::http_post("/v1/notify/reminders", body, verbose).await?;
                    found = true;
                    break;
                }
            }
        }
        if found {
            break;
        }
    }

    if !found {
        bail!("Alert id {id} not found");
    }

    let action = if enabled { "enabled" } else { "disabled" };
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::json!({"id": id, "status": action}));
        }
        OutputFormat::Pretty => println!("Alert {id} {action}"),
    }
    Ok(())
}

/// `--schema` description of an option order's underlying symbol.
const UNDERLYING_SYMBOL_DESC: &str =
    "(US only) Option orders: underlying symbol, when the gateway returns one";
/// `--schema` description of the raw underlying counter ID fallback.
const UNDERLYING_COUNTER_ID_DESC: &str = "(US only) Option orders: raw underlying counter ID, only when the gateway returns no underlying_symbol";
/// `--schema` description of the raw counter ID fallback for the security.
const COUNTER_ID_DESC: &str = "(US only) Raw counter ID, present only when no symbol is returned";

/// `--schema` field notes for the US-only keys of `order` rows.
const US_ORDER_LEGEND: &[(&str, &str)] = &[
    ("updated_at", "(US only) Last update time"),
    ("stock_name", "(US only) Security name"),
    ("currency", "(US only) Order currency"),
    ("msg", "(US only) Upstream order message"),
    ("trigger_price", "(US only) Trigger price"),
    ("last_done", "(US only) Last done price"),
    ("executed_amount", "(US only) Executed amount"),
    ("gtd", "(US only) Good-till date"),
    ("operate_direction", "(US only) OPEN / CLOSE"),
    ("security_type", "(US only) Raw US security type code"),
    ("counter_id", COUNTER_ID_DESC),
    ("underlying_symbol", UNDERLYING_SYMBOL_DESC),
    ("underlying_counter_id", UNDERLYING_COUNTER_ID_DESC),
    (
        "us_time_in_force",
        "(US only) Raw US time-in-force code, not the AP enum",
    ),
    ("us_tag", "(US only) Raw US order tag code, not the AP enum"),
];

pub(crate) fn schema_for_path(path: &[String]) -> Option<super::schema::ResponseSchema> {
    use super::schema::{array, field, fields, object, schema, text, with_legend, RootKind};

    let command = path.join(" ");
    let schema = match command.as_str() {
        "order" => with_legend(
            schema(
                "Orders (today, or --history). Same array shape for HK/CN and US accounts; fields marked (US only) appear only on US rows. US accounts: one page per call (--page / --limit); when more orders exist past this page, a note with the total goes to stderr, not into the JSON",
                RootKind::Array,
                fields(&[
                    "order_id",
                    "symbol",
                    "counter_id",
                    "side",
                    "order_type",
                    "status",
                    "quantity",
                    "price",
                    "executed_quantity",
                    "executed_price",
                    "created_at",
                    "updated_at",
                    "stock_name",
                    "currency",
                    "msg",
                    "trigger_price",
                    "last_done",
                    "executed_amount",
                    "gtd",
                    "operate_direction",
                    "security_type",
                    "underlying_symbol",
                    "underlying_counter_id",
                    "us_time_in_force",
                    "us_tag",
                ]),
            ),
            US_ORDER_LEGEND,
        ),
        "order detail" => with_legend(
            schema(
                "Order detail. Same object shape for HK/CN and US accounts; fields marked (AP only) / (US only) appear on that region only. With --attached, US prints the raw attached order",
                RootKind::Object,
                fields(&[
                    "order_id",
                    "stock_name",
                    "symbol",
                    "counter_id",
                    "side",
                    "order_type",
                    "status",
                    "quantity",
                    "price",
                    "executed_quantity",
                    "executed_price",
                    "executed_amount",
                    "time_in_force",
                    "outside_rth",
                    "currency",
                    "submitted_at",
                    "updated_at",
                    "remark",
                    "history",
                    "us_time_in_force",
                    "us_tag",
                    "gtd",
                    "trigger_price",
                    "last_done",
                    "operate_direction",
                    "security_type",
                    "underlying_symbol",
                    "underlying_counter_id",
                    "attached_orders",
                ]),
            ),
            &[
                (
                    "time_in_force",
                    "(AP only) Time in force; US accounts report the raw code as us_time_in_force",
                ),
                ("outside_rth", "(AP only) Outside regular trading hours setting"),
                ("attached_orders", "(US only) Attached child orders, when any"),
                ("us_time_in_force", "(US only) Raw US time-in-force code, not the AP enum"),
                ("us_tag", "(US only) Raw US order tag code, not the AP enum"),
                ("gtd", "(US only) Good-till date"),
                ("trigger_price", "(US only) Trigger price"),
                ("last_done", "(US only) Last done price"),
                ("operate_direction", "(US only) OPEN / CLOSE"),
                ("security_type", "(US only) Raw US security type code"),
                ("counter_id", COUNTER_ID_DESC),
                ("underlying_symbol", UNDERLYING_SYMBOL_DESC),
                ("underlying_counter_id", UNDERLYING_COUNTER_ID_DESC),
            ],
        ),
        "order executions" => with_legend(
            array(
                "Order executions (today, or --history). Same array shape for HK/CN and US accounts. US --history comes from the v1 history: not filtered by execution time and can lag across days, so fills near the range edges may be missing or extra",
                &[
                    "order_id",
                    "trade_id",
                    "symbol",
                    "side",
                    "price",
                    "quantity",
                    "trade_done_at",
                    "time",
                ],
            ),
            &[
                ("side", "Buy / Sell, or Unknown when it cannot be resolved. US accounts: filled from the account's orders, then the order detail (by order_id), when the execution lacks it; Unknown when the order is not found"),
                ("trade_done_at", "Execution time (RFC 3339)"),
                ("time", "Same value as trade_done_at"),
            ],
        ),
        "order buy" | "order sell" => object("Submitted order", &["order_id"]),
        "order cancel" | "order replace" => text("Order mutation status message"),
        "assets" => with_legend(
            schema(
                "Account asset overview: array of per-currency balances. US accounts: one element (the account currency) with currency, net_assets, total_cash, buy_power (cash buying power), maintenance_margin (mmr) and cash_infos; fields marked (AP only) are absent on US; US cash_infos have no settling_cash but add total_amount, settled_cash, outstanding",
                RootKind::Array,
                [
                    fields(&[
                        "currency",
                        "net_assets",
                        "total_cash",
                        "buy_power",
                        "max_finance_amount",
                        "remaining_finance_amount",
                        "init_margin",
                        "maintenance_margin",
                        "margin_call",
                        "risk_level",
                        "cash_infos",
                    ]),
                    vec![field(
                        "us",
                        "object",
                        "(US only) {buy_limit_info (cash, overnight, day-trade, option, crypto buying power), margin_status (null for cash accounts)}",
                    )],
                ]
                .concat(),
            ),
            &[
                (
                    "net_assets",
                    "Net assets. US: the margin net liquidation value (nlv); absent when the account reports no net liquidation value",
                ),
                ("max_finance_amount", "(AP only) Maximum financing amount"),
                ("remaining_finance_amount", "(AP only) Remaining financing amount"),
                ("init_margin", "(AP only) Initial margin"),
                ("margin_call", "(AP only) Margin call amount"),
                ("risk_level", "(AP only) Risk level"),
            ],
        ),
        "cash-flow" => with_legend(
            schema(
                "Cash flow records, same array shape for HK/CN and US accounts; fields marked (AP only) / (US only) appear on that region only. US rows come from the account transaction history: balance is the signed amount (0 for crypto transfers and fees)",
                RootKind::Array,
                fields(&[
                    "flow_name",
                    "symbol",
                    "business_type",
                    "balance",
                    "currency",
                    "time",
                    "description",
                    "direction",
                    "biz_code",
                    "biz_code_name",
                    "quantity",
                    "quantity_description",
                    "ref_no",
                    "equity_type",
                ]),
            ),
            &[
                ("business_type", "(AP only) Business type"),
                (
                    "direction",
                    "(US only) In / Out, from the amount sign or, when the amount is 0, the quantity sign",
                ),
                ("biz_code", "(US only) Transaction type code"),
                ("biz_code_name", "(US only) Transaction type name"),
                ("quantity", "(US only) Signed security quantity, when the row moves securities"),
                ("quantity_description", "(US only) Human-readable quantity change"),
                ("ref_no", "(US only) Transaction reference number"),
                ("equity_type", "(US only) Equity type of the security, e.g. CRYPTO"),
            ],
        ),
        "portfolio" => with_legend(
            schema(
                "Portfolio overview, same object shape for HK/CN and US accounts. US: overview has only currency, total_asset and total_cash; holdings are the `positions` rows; cash_balances add settled_cash, outstanding; account type, buying power and margin status are under `us`",
                RootKind::Object,
                [
                    fields(&["overview", "holdings", "cash_balances", "market_accounts"]),
                    vec![field(
                        "us",
                        "object",
                        "(US only) {account_type, asset_timestamp, cash_buy_power, overnight_buy_power, multi_leg, buy_limit_info, margin_status}",
                    )],
                ]
                .concat(),
            ),
            &[
                (
                    "overview",
                    "Account totals. US: {currency, total_asset (margin net liquidation value; absent when the account reports none), total_cash}",
                ),
                ("market_accounts", "(AP only) Per-market account breakdown"),
            ],
        ),
        "positions" => with_legend(
            schema(
                "Stock positions, same array shape for HK/CN and US accounts; fields marked (AP only) / (US only) appear on that region only. US rows list stock, option and crypto holdings together and add the (US only) fields plus other raw US fields",
                RootKind::Array,
                fields(&[
                    "symbol",
                    "name",
                    "quantity",
                    "available",
                    "cost_price",
                    "currency",
                    "market",
                    "asset_type",
                    "last_done",
                    "market_price",
                    "prev_close",
                    "today_pl",
                    "position_side",
                ]),
            ),
            &[
                ("available", "(AP only) Quantity available to sell"),
                ("asset_type", "(US only) STOCK / OPTION / CRYPTO"),
                ("last_done", "(US only) Last done price"),
                ("market_price", "(US only) Market price used to value the holding"),
                ("prev_close", "(US only) Previous close price"),
                ("today_pl", "(US only) Today's profit and loss"),
                ("position_side", "(US only) LONG / SHORT"),
            ],
        ),
        "fund-positions" => array(
            "Current fund positions (HK/CN accounts only; US accounts get an error)",
            &[
                "symbol",
                "name",
                "current_net_asset_value",
                "cost_net_asset_value",
                "currency",
                "holding_units",
            ],
        ),
        "margin-ratio" => array(
            "Key/value margin ratio result (HK/CN accounts only; US accounts get an error)",
            &["field", "value"],
        ),
        "max-qty" => array("Key/value account calculation result", &["field", "value"]),
        "alert" => object("Price alert list", &["lists"]),
        "alert add" | "alert delete" => {
            object("Price alert mutation result", &["id", "status", "data"])
        }
        "alert enable" | "alert disable" => object("Price alert status update", &["id", "status"]),
        _ => return None,
    };
    Some(schema)
}

// ── US-specific commands ─────────────────────────────────────────────────────

/// `longbridge positions` — US account full overview.
///
/// When the session is a US account (`token.ac` starts with `us_lb`), calls
/// `GET /v1/us/asset/overview` and renders stock / option / crypto positions
/// plus buy-power. Falls back to the standard HK/CN SDK path otherwise.
pub async fn cmd_positions(format: &OutputFormat) -> Result<()> {
    if crate::openapi::is_us_account().await {
        cmd_us_positions(format).await
    } else {
        let ctx = crate::openapi::trade();
        let resp = ctx.stock_positions(None).await?;
        print_account_banner(format);
        print_table(POSITION_HEADERS, ap_position_rows(&resp), format);
        Ok(())
    }
}

/// Column headers of `positions`; `--format json` keys are these in snake case.
const POSITION_HEADERS: &[&str] = &[
    "Symbol",
    "Name",
    "Quantity",
    "Available",
    "Cost Price",
    "Currency",
    "Market",
];

/// AP `positions` table rows, one per position across channels.
fn ap_position_rows(resp: &longbridge::trade::StockPositionsResponse) -> Vec<Vec<String>> {
    resp.channels
        .iter()
        .flat_map(|channel| &channel.positions)
        .map(|pos| {
            vec![
                pos.symbol.clone(),
                pos.symbol_name.clone(),
                pos.quantity.to_string(),
                pos.available_quantity.to_string(),
                pos.cost_price.to_string(),
                pos.currency.clone(),
                format!("{:?}", pos.market),
            ]
        })
        .collect()
}

fn print_json_us(data: &serde_json::Value) {
    let mut v = data.clone();
    super::output::strip_counter_ids(&mut v);
    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
}

async fn cmd_us_positions(format: &OutputFormat) -> Result<()> {
    let api = LbTradeApi::new(crate::openapi::trade());
    run_us_positions(&api, format).await
}

/// `longbridge positions` for US accounts, through the [`TradeApi`] seam.
pub async fn run_us_positions(api: &dyn TradeApi, format: &OutputFormat) -> Result<()> {
    let overview = api.us_asset_overview().await?;
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&super::us_shape::positions_json(&overview)?)?
        ),
        OutputFormat::Pretty => print_us_positions_pretty(&normalize_us_overview(&overview)?),
    }
    Ok(())
}

/// `--format json` shape of the US asset overview, shared by `positions` and
/// `portfolio`.
pub(crate) fn normalize_us_overview(
    resp: &longbridge::trade::USAssetOverview,
) -> Result<serde_json::Value> {
    let mut data = serde_json::to_value(resp)?;
    // Normalize for AI agent: account_type code → readable string, timestamp string → int
    if let Some(map) = data.as_object_mut() {
        if let Some(s) = map.get("account_type").and_then(|v| v.as_str()) {
            let label = match s {
                "0" => "Standard (Margin)",
                "1" => "Cash",
                other => other,
            };
            map.insert(
                "account_type".to_string(),
                serde_json::Value::String(label.to_string()),
            );
        }
        if let Some(s) = map.get("asset_timestamp").and_then(|v| v.as_str()) {
            if let Ok(n) = s.parse::<i64>() {
                map.insert(
                    "asset_timestamp".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(n)),
                );
            }
        }
        // Ensure list fields are always [] rather than null/absent
        for list_key in &["stock_list", "option_list", "crypto_list", "cash_list"] {
            if map.get(*list_key).is_none_or(serde_json::Value::is_null) {
                map.insert((*list_key).to_string(), serde_json::Value::Array(vec![]));
            }
        }
        // Crypto entries carry the bare pair code (`BTCUSD`) and the market
        // (`BKKT`); add the user-facing `full_symbol` (`BTCUSD.BKKT`), as
        // stock entries have it.
        if let Some(cryptos) = map
            .get_mut("crypto_list")
            .and_then(serde_json::Value::as_array_mut)
        {
            for entry in cryptos
                .iter_mut()
                .filter_map(serde_json::Value::as_object_mut)
            {
                let symbol = entry.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
                let market = entry.get("market").and_then(|v| v.as_str()).unwrap_or("");
                if !symbol.is_empty() && !market.is_empty() && !symbol.contains('.') {
                    let full = format!("{symbol}.{market}");
                    entry.insert("full_symbol".to_string(), serde_json::Value::String(full));
                }
            }
        }
    }
    Ok(data)
}

fn print_us_positions_pretty(data: &serde_json::Value) {
    let val = us_val;
    let acct_type = data["account_type"].as_str().unwrap_or("-");
    println!("Account Type:   {acct_type}");
    println!("Cash Buy Power: {}", val(&data["cash_buy_power"]));
    println!();

    let empty = vec![];

    // Cash balances
    let cash_list = data["cash_list"].as_array().unwrap_or(&empty);
    if !cash_list.is_empty() {
        println!("── Cash ────────────────────────────────────────────────────────");
        print_table(
            &["Currency", "Total Cash", "Settled Cash", "Frozen"],
            cash_list
                .iter()
                .map(|c| {
                    vec![
                        val(&c["currency"]),
                        val(&c["total_cash"]),
                        val(&c["settled_cash"]),
                        val(&c["frozen_buy_cash"]),
                    ]
                })
                .collect(),
            &OutputFormat::Pretty,
        );
    }

    print_us_holdings_pretty(data);
}

/// The holding value of a raw JSON field, `-` when absent.
fn us_val(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => "-".to_string(),
    }
}

/// The user-facing symbol of a US holding: stock entries carry it as
/// `full_symbol` (`symbol` is the bare ticker), crypto entries get a
/// `full_symbol` from [`normalize_us_overview`] (`symbol` is the bare pair
/// code), and raw option entries carry it as `counter_id`.
fn us_holding_symbol(p: &serde_json::Value) -> String {
    ["full_symbol", "symbol", "counter_id"]
        .iter()
        .find_map(|k| p[*k].as_str().filter(|s| !s.is_empty()))
        .map_or_else(|| "-".to_string(), str::to_string)
}

/// Stock / option / crypto tables of a normalized US asset overview, shared by
/// `positions` and `portfolio`.
pub(crate) fn print_us_holdings_pretty(data: &serde_json::Value) {
    let val = us_val;
    let empty = vec![];

    let stock_list = data["stock_list"].as_array().unwrap_or(&empty);
    if !stock_list.is_empty() {
        println!("── Stocks ──────────────────────────────────────────────────────");
        print_table(
            &["Symbol", "Qty", "Cost", "Last", "Today P&L"],
            stock_list
                .iter()
                .map(|p| {
                    vec![
                        us_holding_symbol(p),
                        val(&p["quantity"]),
                        val(&p["average_cost"]),
                        val(&p["last_done"]),
                        val(&p["today_pl"]),
                    ]
                })
                .collect(),
            &OutputFormat::Pretty,
        );
    }

    // Option positions (raw serde_json::Value entries — field names follow API response)
    let option_list = data["option_list"].as_array().unwrap_or(&empty);
    if !option_list.is_empty() {
        println!("── Options ─────────────────────────────────────────────────────");
        print_table(
            &["Symbol", "Underlying", "Qty", "Cost"],
            option_list
                .iter()
                .map(|p| {
                    vec![
                        us_holding_symbol(p),
                        val(&p["underlying_counter_id"]),
                        val(&p["quantity"]),
                        val(&p["average_cost"]),
                    ]
                })
                .collect(),
            &OutputFormat::Pretty,
        );
    }

    let cryptos = data["crypto_list"].as_array().unwrap_or(&empty);
    if !cryptos.is_empty() {
        println!("── Crypto ──────────────────────────────────────────────────────");
        print_table(
            &["Symbol", "Qty", "Cost", "Last", "Today P&L"],
            cryptos
                .iter()
                .map(|p| {
                    vec![
                        us_holding_symbol(p),
                        val(&p["quantity"]),
                        val(&p["average_cost"]),
                        val(&p["last_done"]),
                        val(&p["today_pl"]),
                    ]
                })
                .collect(),
            &OutputFormat::Pretty,
        );
    }
}

/// `longbridge profit-analysis realized` — US accounts only.
///
/// Calls `GET /v1/us/asset/pl/realized`.
/// HK/CN token → `DcRegionRestricted` error from the SDK.
pub async fn cmd_us_realized_pl(
    category: &str,
    currency: &str,
    format: &OutputFormat,
    _verbose: bool,
) -> Result<()> {
    // GetUSRealizedPLOptions.category: "" = all, "STOCK", "OPTION", "CRYPTO"
    let cat = match category.to_lowercase().as_str() {
        "all" | "0" => "",
        "stock" | "1" => "STOCK",
        "option" | "2" => "OPTION",
        "crypto" | "3" => "CRYPTO",
        other => {
            anyhow::bail!("Invalid category '{other}'. Valid values: all | stock | option | crypto")
        }
    };
    let resp = crate::openapi::trade()
        .us_realized_pl(GetUSRealizedPLOptions {
            currency: currency.to_string(),
            category: cat.to_string(),
        })
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("data center") {
                anyhow::anyhow!("This command is only available for US accounts")
            } else {
                anyhow::Error::from(e)
            }
        })?;
    let mut data = serde_json::to_value(&resp)?;
    // Annotate category/period integers with labels for AI agents
    if let Some(list) = data["realized_pl_list"].as_array_mut() {
        for entry in list.iter_mut() {
            let cat = entry["category"].as_i64().unwrap_or(0);
            entry["category_name"] = serde_json::json!(match cat {
                0 => "All",
                1 => "Stock",
                2 => "Option",
                3 => "Crypto",
                _ => "Unknown",
            });
            if let Some(metrics) = entry["metrics"].as_array_mut() {
                for m in metrics.iter_mut() {
                    let period = m["period"].as_i64().unwrap_or(0);
                    m["period_name"] = serde_json::json!(match period {
                        1 => "YTD",
                        2 => "Since Inception",
                        _ => "Unknown",
                    });
                    // rate is a decimal fraction: -0.9303 means -93.03%
                    m["rate_unit"] = serde_json::json!("decimal_fraction");
                }
            }
        }
    }
    match format {
        OutputFormat::Json => print_json_us(&data),
        OutputFormat::Pretty => {
            let empty = vec![];
            let list = data["realized_pl_list"].as_array().unwrap_or(&empty);
            if list.is_empty() {
                println!("No realized P&L data.");
            } else {
                let cat_name = |c: i64| match c {
                    0 => "All",
                    1 => "Stock",
                    2 => "Option",
                    3 => "Crypto",
                    _ => "Unknown",
                };
                let num = |v: &serde_json::Value| match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    _ => "-".to_owned(),
                };
                print_table(
                    &["Category", "Period", "Currency", "Amount", "Return Rate"],
                    list.iter()
                        .flat_map(|entry| {
                            let cat = entry["category"].as_i64().unwrap_or(0);
                            let currency = entry["currency"].as_str().unwrap_or("-");
                            entry["metrics"]
                                .as_array()
                                .unwrap_or(&empty)
                                .iter()
                                .map(move |m| {
                                    vec![
                                        cat_name(cat).to_string(),
                                        m["period_name"].as_str().unwrap_or("-").to_string(),
                                        currency.to_string(),
                                        num(&m["amount"]),
                                        num(&m["rate"]),
                                    ]
                                })
                                .collect::<Vec<_>>()
                        })
                        .collect(),
                    format,
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::api::MockTradeApi;
    use longbridge::trade::QueryUSOrdersResponse;

    fn make_submit_opts() -> SubmitOrderOptions {
        SubmitOrderOptions::new(
            "TSLA.US",
            OrderType::LO,
            OrderSide::Buy,
            Decimal::from(100u64),
            TimeInForceType::Day,
        )
    }

    #[tokio::test]
    async fn test_run_today_orders_dispatches() {
        let mut mock = MockTradeApi::new();
        mock.expect_today_orders()
            .times(1)
            .returning(|_| Ok(vec![]));
        run_today_orders(&mock, GetTodayOrdersOptions::new(), &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_history_orders_dispatches() {
        let mut mock = MockTradeApi::new();
        mock.expect_history_orders()
            .times(1)
            .returning(|_| Ok(vec![]));
        run_history_orders(&mock, GetHistoryOrdersOptions::new(), &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_today_executions_dispatches() {
        let mut mock = MockTradeApi::new();
        mock.expect_today_executions()
            .times(1)
            .returning(|_| Ok(vec![]));
        run_today_executions(
            &mock,
            GetTodayExecutionsOptions::new(),
            &OutputFormat::Pretty,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_history_executions_dispatches() {
        let mut mock = MockTradeApi::new();
        mock.expect_history_executions()
            .times(1)
            .returning(|_| Ok(vec![]));
        run_history_executions(
            &mock,
            GetHistoryExecutionsOptions::new(),
            &OutputFormat::Pretty,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_submit_order_dispatches() {
        let mut mock = MockTradeApi::new();
        mock.expect_submit_order().times(1).returning(|_| {
            Ok(longbridge::trade::SubmitOrderResponse {
                order_id: "order-1".to_string(),
            })
        });
        run_submit_order(&mock, make_submit_opts(), &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_cancel_order_dispatches() {
        let mut mock = MockTradeApi::new();
        mock.expect_cancel_order()
            .with(mockall::predicate::eq("order-1".to_string()))
            .times(1)
            .returning(|_| Ok(()));
        run_cancel_order(&mock, "order-1".to_string())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_replace_order_dispatches() {
        let mut mock = MockTradeApi::new();
        mock.expect_replace_order().times(1).returning(|_| Ok(()));
        let opts = ReplaceOrderOptions::new("order-1", Decimal::from(200u64));
        run_replace_order(&mock, opts).await.unwrap();
    }

    #[tokio::test]
    async fn test_run_assets_dispatches() {
        let mut mock = MockTradeApi::new();
        mock.expect_account_balance()
            .with(mockall::predicate::eq(None::<String>))
            .times(1)
            .returning(|_| Ok(vec![]));
        run_assets(&mock, None, &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_positions_dispatches() {
        use longbridge::trade::StockPositionsResponse;
        let mut mock = MockTradeApi::new();
        mock.expect_stock_positions()
            .times(1)
            .returning(|| Ok(StockPositionsResponse { channels: vec![] }));
        run_positions(&mock, &OutputFormat::Pretty).await.unwrap();
    }

    #[tokio::test]
    async fn test_run_fund_positions_dispatches() {
        use longbridge::trade::FundPositionsResponse;
        let mut mock = MockTradeApi::new();
        mock.expect_fund_positions()
            .times(1)
            .returning(|| Ok(FundPositionsResponse { channels: vec![] }));
        run_fund_positions(&mock, &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_margin_ratio_dispatches() {
        use longbridge::trade::MarginRatio;
        let mut mock = MockTradeApi::new();
        mock.expect_margin_ratio()
            .with(mockall::predicate::eq("TSLA.US".to_string()))
            .times(1)
            .returning(|_| {
                Ok(MarginRatio {
                    im_factor: Decimal::ZERO,
                    mm_factor: Decimal::ZERO,
                    fm_factor: Decimal::ZERO,
                })
            });
        run_margin_ratio(&mock, "TSLA.US".to_string(), &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_us_positions_uses_the_us_overview() {
        // No `stock_positions` expectation: calling the AP endpoint panics.
        let mut mock = MockTradeApi::new();
        mock.expect_us_asset_overview()
            .times(1)
            .returning(|| Ok(longbridge::trade::USAssetOverview::default()));
        run_us_positions(&mock, &OutputFormat::Json).await.unwrap();
    }

    #[test]
    fn test_us_holding_symbol_prefers_the_qualified_symbol() {
        use serde_json::json;
        assert_eq!(
            us_holding_symbol(&json!({"symbol": "AAPL", "full_symbol": "AAPL.US"})),
            "AAPL.US"
        );
        assert_eq!(
            us_holding_symbol(&json!({"symbol": "BTCUSD.BKKT", "full_symbol": ""})),
            "BTCUSD.BKKT"
        );
        assert_eq!(
            us_holding_symbol(&json!({"counter_id": "AAPL261120C345000.US"})),
            "AAPL261120C345000.US"
        );
        assert_eq!(us_holding_symbol(&json!({})), "-");
    }

    #[test]
    fn test_normalize_us_overview_qualifies_crypto_symbols() {
        let overview: longbridge::trade::USAssetOverview =
            serde_json::from_value(serde_json::json!({
                "account_type": "1",
                "stock_list": [{"symbol": "MULL", "full_symbol": "MULL.US", "market": "US"}],
                "crypto_list": [
                    {"symbol": "BTCUSD", "market": "BKKT", "quantity": "0.5"},
                    {"symbol": "ETHUSD", "market": ""}
                ]
            }))
            .unwrap();
        let v = normalize_us_overview(&overview).unwrap();
        assert_eq!(v["crypto_list"][0]["full_symbol"], "BTCUSD.BKKT");
        assert_eq!(v["crypto_list"][0]["symbol"], "BTCUSD");
        assert_eq!(v["crypto_list"][0]["quantity"], "0.5");
        assert!(v["crypto_list"][1].get("full_symbol").is_none());
        assert_eq!(us_holding_symbol(&v["crypto_list"][0]), "BTCUSD.BKKT");
        assert_eq!(us_holding_symbol(&v["stock_list"][0]), "MULL.US");
    }

    #[test]
    fn test_parse_order_type_valid() {
        assert!(parse_order_type("LO").is_ok());
        assert!(parse_order_type("lo").is_ok());
        assert!(parse_order_type("MO").is_ok());
        assert!(parse_order_type("ELO").is_ok());
    }

    #[test]
    fn test_parse_order_type_invalid() {
        assert!(parse_order_type("LIMIT").is_err());
        assert!(parse_order_type("").is_err());
    }

    #[test]
    fn test_parse_tif_valid() {
        assert!(parse_tif("day").is_ok());
        assert!(parse_tif("gtc").is_ok());
        assert!(parse_tif("goodtilcanceled").is_ok());
        assert!(parse_tif("gtd").is_ok());
    }

    #[test]
    fn test_parse_tif_invalid() {
        assert!(parse_tif("ioc").is_err());
    }

    /// Golden `--format json` output of the AP (HK/CN) account commands. US
    /// accounts are mapped onto these keys, so these must not drift.
    mod ap_golden {
        use super::*;
        use crate::cli::output::table_rows_to_json;
        use longbridge::trade::{
            AccountBalance, BalanceType, CashFlow, CashFlowDirection, CashInfo,
            CommissionFreeStatus, DeductionStatus, Order, OrderDetail, OrderHistoryDetail,
            OrderStatus, OrderTag, StockPosition, StockPositionChannel, StockPositionsResponse,
        };
        use rust_decimal_macros::dec;
        use serde_json::json;

        fn ts(n: i64) -> time::OffsetDateTime {
            time::OffsetDateTime::from_unix_timestamp(n).unwrap()
        }

        fn order() -> Order {
            Order {
                order_id: "700000000000000001".to_string(),
                status: OrderStatus::Filled,
                stock_name: "Example Corp".to_string(),
                quantity: dec!(2),
                executed_quantity: dec!(2),
                price: Some(dec!(20.5)),
                executed_price: Some(dec!(20.48)),
                submitted_at: ts(1_790_000_000),
                side: OrderSide::Sell,
                symbol: "EXMP.US".to_string(),
                order_type: OrderType::LO,
                last_done: None,
                trigger_price: None,
                msg: String::new(),
                tag: OrderTag::Normal,
                time_in_force: TimeInForceType::Day,
                expire_date: None,
                updated_at: Some(ts(1_790_000_060)),
                trigger_at: None,
                trailing_amount: None,
                trailing_percent: None,
                limit_offset: None,
                trigger_status: None,
                currency: "USD".to_string(),
                outside_rth: None,
                limit_depth_level: None,
                trigger_count: None,
                monitor_price: None,
                remark: String::new(),
                delayed_until: None,
                attached_orders: vec![],
                multi_leg: None,
            }
        }

        #[test]
        fn orders() {
            let mut market = order();
            market.price = None;
            market.executed_price = None;
            market.side = OrderSide::Buy;
            market.order_type = OrderType::MO;
            market.status = OrderStatus::Rejected;
            let got = table_rows_to_json(ORDER_HEADERS, &ap_order_rows(&[order(), market]));
            assert_eq!(
                serde_json::Value::Array(got),
                json!([
                    {"order_id": "700000000000000001", "symbol": "EXMP.US", "side": "Sell",
                     "order_type": "LO", "status": "Filled", "quantity": "2", "price": "20.500",
                     "executed_quantity": "2", "executed_price": "20.480",
                     "created_at": "2026-09-21T14:13:20Z"},
                    {"order_id": "700000000000000001", "symbol": "EXMP.US", "side": "Buy",
                     "order_type": "MO", "status": "Rejected", "quantity": "2", "price": "-",
                     "executed_quantity": "2", "executed_price": "-",
                     "created_at": "2026-09-21T14:13:20Z"}
                ])
            );
        }

        #[test]
        fn order_detail() {
            let o = order();
            let detail = OrderDetail {
                order_id: o.order_id,
                status: o.status,
                stock_name: o.stock_name,
                quantity: o.quantity,
                executed_quantity: o.executed_quantity,
                price: o.price,
                executed_price: o.executed_price,
                submitted_at: o.submitted_at,
                side: o.side,
                symbol: o.symbol,
                order_type: o.order_type,
                last_done: None,
                trigger_price: None,
                msg: "done".to_string(),
                tag: o.tag,
                time_in_force: o.time_in_force,
                expire_date: None,
                updated_at: o.updated_at,
                trigger_at: None,
                trailing_amount: None,
                trailing_percent: None,
                limit_offset: None,
                trigger_status: None,
                currency: o.currency,
                outside_rth: Some(OutsideRTH::RTHOnly),
                limit_depth_level: None,
                trigger_count: None,
                monitor_price: None,
                remark: String::new(),
                delayed_until: None,
                free_status: CommissionFreeStatus::None,
                free_amount: None,
                free_currency: None,
                deductions_status: DeductionStatus::None,
                deductions_amount: None,
                deductions_currency: None,
                platform_deducted_status: DeductionStatus::None,
                platform_deducted_amount: None,
                platform_deducted_currency: None,
                history: vec![OrderHistoryDetail {
                    price: dec!(20.48),
                    quantity: dec!(2),
                    status: OrderStatus::Filled,
                    msg: String::new(),
                    time: ts(1_790_000_060),
                }],
                charge_detail: None,
                attached_orders: vec![],
                multi_leg: None,
            };
            assert_eq!(
                ap_order_detail_json(&detail),
                json!({
                    "order_id": "700000000000000001", "stock_name": "Example Corp",
                    "symbol": "EXMP.US", "side": "Sell", "order_type": "LO", "status": "Filled",
                    "quantity": "2", "price": "20.500", "executed_quantity": "2",
                    "executed_price": "20.480", "executed_amount": "40.96",
                    "time_in_force": "Day", "outside_rth": "RTHOnly", "currency": "USD",
                    "submitted_at": "2026-09-21T14:13:20Z", "updated_at": "2026-09-21T14:14:20Z",
                    "remark": "done",
                    "history": [{"status": "Filled", "time": "2026-09-21T14:14:20Z",
                                 "price": "20.48", "quantity": "2", "msg": ""}]
                })
            );
        }

        #[test]
        fn assets() {
            let balance = AccountBalance {
                total_cash: dec!(1000.5),
                max_finance_amount: dec!(0),
                remaining_finance_amount: dec!(0),
                risk_level: 0,
                margin_call: dec!(0),
                currency: "HKD".to_string(),
                cash_infos: vec![CashInfo {
                    withdraw_cash: dec!(900),
                    available_cash: dec!(1000.5),
                    frozen_cash: dec!(0),
                    settling_cash: dec!(10),
                    currency: "HKD".to_string(),
                }],
                net_assets: dec!(2000),
                init_margin: dec!(0),
                maintenance_margin: dec!(0),
                buy_power: dec!(1000.5),
                frozen_transaction_fees: vec![],
            };
            assert_eq!(
                serde_json::Value::Array(ap_assets_json(&[balance])),
                json!([{
                    "currency": "HKD", "net_assets": "2000", "total_cash": "1000.5",
                    "buy_power": "1000.5", "max_finance_amount": "0",
                    "remaining_finance_amount": "0", "init_margin": "0",
                    "maintenance_margin": "0", "margin_call": "0", "risk_level": "Safe",
                    "cash_infos": [{"currency": "HKD", "available_cash": "1000.5",
                                    "frozen_cash": "0", "settling_cash": "10",
                                    "withdraw_cash": "900"}]
                }])
            );
        }

        #[test]
        fn cash_flow() {
            let flow = CashFlow {
                transaction_flow_name: "Deposit".to_string(),
                direction: CashFlowDirection::In,
                business_type: BalanceType::Cash,
                balance: dec!(100),
                currency: "HKD".to_string(),
                business_time: ts(1_790_000_000),
                symbol: None,
                description: "bank".to_string(),
            };
            let got = table_rows_to_json(CASH_FLOW_HEADERS, &ap_cash_flow_rows(&[flow]));
            assert_eq!(
                serde_json::Value::Array(got),
                json!([{"flow_name": "Deposit", "symbol": "", "business_type": "Cash",
                        "balance": "100", "currency": "HKD", "time": "2026-09-21T14:13:20Z",
                        "description": "bank"}])
            );
        }

        #[test]
        fn positions() {
            let resp = StockPositionsResponse {
                channels: vec![StockPositionChannel {
                    account_channel: "lb".to_string(),
                    positions: vec![StockPosition {
                        symbol: "700.HK".to_string(),
                        symbol_name: "Example HK".to_string(),
                        quantity: dec!(100),
                        available_quantity: dec!(80),
                        currency: "HKD".to_string(),
                        cost_price: dec!(300.5),
                        market: longbridge::Market::HK,
                        init_quantity: None,
                    }],
                }],
            };
            let got = table_rows_to_json(POSITION_HEADERS, &ap_position_rows(&resp));
            assert_eq!(
                serde_json::Value::Array(got),
                json!([{"symbol": "700.HK", "name": "Example HK", "quantity": "100",
                        "available": "80", "cost_price": "300.5", "currency": "HKD",
                        "market": "HK"}])
            );
        }
    }

    #[test]
    fn us_cancel_preview_shows_the_security() {
        let full = serde_json::json!({"order": {
            "id": "900000000000000001", "symbol": "EXMP.US", "action": 2,
            "status": "NewStatus", "quantity": "2", "executed_qty": "0", "price": "20.5"
        }});
        assert_eq!(
            us_order_summary(&full),
            serde_json::json!({"symbol": "EXMP.US", "side": "Sell", "status": "New",
                               "quantity": "2", "executed_quantity": "0", "price": "20.5"})
        );
        // No symbol in the response: the raw counter_id is shown unconverted.
        let full = serde_json::json!({"order": {
            "id": "1", "symbol": "", "counter_id": "ST/US/EXMP", "action": 1
        }});
        let got = us_order_summary(&full);
        assert_eq!(got["counter_id"], "ST/US/EXMP");
        assert!(got.get("symbol").is_none(), "{got}");
        assert_eq!(got["side"], "Buy");
    }

    #[tokio::test]
    async fn us_cancel_preview_refuses_without_the_order() {
        // Invalid ID: refused before any fetch.
        for bad in ["", "20240101-123456789", "../1", "１２３"] {
            let err = us_preview_order_summary(bad, |_| async {
                panic!("an invalid order ID must not be fetched")
            })
            .await
            .unwrap_err()
            .to_string();
            assert!(err.contains("invalid order ID"), "{bad}: {err}");
            assert!(err.contains("no confirmation code"), "{err}");
        }
        // HTTP 404: the order does not exist.
        let err = us_preview_order_summary("701276261045858304", |_| async {
            Err(
                longbridge::httpclient::HttpClientError::BadStatus(reqwest::StatusCode::NOT_FOUND)
                    .into(),
            )
        })
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("not found") && err.contains("404"), "{err}");
        assert!(err.contains("no confirmation code"), "{err}");
        // Any other failure: not claimed as missing, the reason is named.
        let err = us_preview_order_summary("701276261045858304", |_| async {
            Err(anyhow::anyhow!("request timeout"))
        })
        .await
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("could not fetch order 701276261045858304: request timeout"),
            "{err}"
        );
        assert!(!err.contains("not found"), "{err}");
        assert!(err.contains("no confirmation code"), "{err}");
        // `order` present but null or not an object: no order.
        for body in [
            serde_json::json!({"order": null}),
            serde_json::json!({"order": "x"}),
            serde_json::json!({"order": {}}),
        ] {
            let err = us_preview_order_summary("701276261045858304", |_| async move { Ok(body) })
                .await
                .unwrap_err()
                .to_string();
            assert!(err.contains("not found"), "{err}");
            assert!(err.contains("no confirmation code"), "{err}");
        }
        // Empty body: no order.
        let err = us_preview_order_summary("701276261045858304", |_| async {
            Ok(serde_json::json!({}))
        })
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("not found"), "{err}");
        // Found: the summary of the fetched order.
        let got = us_preview_order_summary("701276261045858304", |id| async move {
            assert_eq!(id, "701276261045858304");
            Ok(serde_json::json!({"order": {"symbol": "EXMP.US", "action": 1}}))
        })
        .await
        .unwrap();
        assert_eq!(got["symbol"], "EXMP.US");
        assert_eq!(got["side"], "Buy");
    }

    #[test]
    fn us_attached_order_keeps_gateway_symbol_and_never_derives_one() {
        let norm = |v: serde_json::Value| {
            let mut m = v
                .as_object()
                .cloned()
                .expect("test order must be an object");
            normalize_us_order_map(&mut m);
            serde_json::Value::Object(m)
        };
        let got = norm(serde_json::json!({"symbol": "EXMP.US", "counter_id": "ST/US/EXMP"}));
        assert_eq!(got["symbol"], "EXMP.US");
        assert!(got.get("counter_id").is_none(), "{got}");
        let got = norm(serde_json::json!({"symbol": "", "counter_id": "ST/US/EXMP"}));
        assert_eq!(got["counter_id"], "ST/US/EXMP");
        assert!(got.get("symbol").is_none(), "{got}");
        let got = norm(serde_json::json!({"counter_id": "ST/US/EXMP"}));
        assert_eq!(got["counter_id"], "ST/US/EXMP");
        assert!(got.get("symbol").is_none(), "{got}");
    }

    #[test]
    fn us_order_page_note_only_when_more_pages_exist() {
        assert!(us_orders_page_note(1, 20, 20, 45).is_some());
        assert!(us_orders_page_note(2, 20, 20, 45).is_some());
        // Last page: 40 + 5 = 45 shown through.
        assert!(us_orders_page_note(3, 20, 5, 45).is_none());
        assert!(us_orders_page_note(1, 20, 3, 3).is_none());
        assert!(us_orders_page_note(1, 20, 0, 0).is_none());
        // Past the last page: noted whatever limit the server applied.
        let note = us_orders_page_note(5, 20, 0, 45).expect("past-last note");
        assert!(
            note.contains("page 5 is past the last page (total 45)"),
            "{note}"
        );
        assert!(us_orders_page_note(2, 20, 0, 0).is_some());
        // Server ignored limit and returned all 45 rows on page 1.
        assert!(us_orders_page_note(1, 20, 45, 45).is_none());
    }

    #[test]
    fn schema_marks_region_only_fields() {
        let desc = |cmd: &str, key: &str| -> String {
            let path: Vec<String> = cmd.split(' ').map(str::to_string).collect();
            let schema = schema_for_path(&path).unwrap_or_else(|| panic!("schema for {cmd}"));
            schema
                .fields
                .iter()
                .find(|f| f.name == key)
                .unwrap_or_else(|| panic!("{cmd}: {key} missing"))
                .description
                .clone()
        };
        for (cmd, key, marker) in [
            ("cash-flow", "direction", "(US only)"),
            ("cash-flow", "biz_code", "(US only)"),
            ("cash-flow", "biz_code_name", "(US only)"),
            ("cash-flow", "quantity", "(US only)"),
            ("cash-flow", "business_type", "(AP only)"),
            ("positions", "available", "(AP only)"),
            ("positions", "asset_type", "(US only)"),
            ("order detail", "time_in_force", "(AP only)"),
            ("order detail", "outside_rth", "(AP only)"),
            ("order detail", "us_time_in_force", "(US only)"),
            ("order detail", "us_tag", "(US only)"),
            ("order", "us_tag", "(US only)"),
            ("order", "counter_id", "(US only)"),
            ("order detail", "counter_id", "(US only)"),
            ("order", "underlying_symbol", "(US only)"),
            ("order", "underlying_counter_id", "(US only)"),
            ("order detail", "underlying_symbol", "(US only)"),
            ("order detail", "underlying_counter_id", "(US only)"),
            ("positions", "last_done", "(US only)"),
            ("positions", "market_price", "(US only)"),
            ("positions", "prev_close", "(US only)"),
            ("positions", "today_pl", "(US only)"),
            ("positions", "position_side", "(US only)"),
            ("assets", "risk_level", "(AP only)"),
        ] {
            assert!(desc(cmd, key).starts_with(marker), "{cmd} {key}");
        }
        assert!(desc("assets", "net_assets").contains("absent"));
        assert!(desc("portfolio", "overview").contains("absent"));
    }

    #[test]
    fn execute_validates_us_order_ids() {
        assert!(ensure_valid_order_id(true, "701276261045858304").is_ok());
        for bad in ["", "20240101-123456789", "../1", "１２３"] {
            let err = ensure_valid_order_id(true, bad).unwrap_err().to_string();
            assert!(err.contains("invalid order ID"), "{bad}: {err}");
        }
        // AP order IDs are not checked.
        assert!(ensure_valid_order_id(false, "20240101-123456789").is_ok());
    }

    fn exec(trade_id: &str) -> Execution {
        Execution {
            order_id: "1".to_string(),
            trade_id: trade_id.to_string(),
            symbol: "EXMP.US".to_string(),
            trade_done_at: time::OffsetDateTime::UNIX_EPOCH,
            quantity: Decimal::ONE,
            price: Decimal::ONE,
            side: OrderSide::Buy,
        }
    }

    /// A US v1 history trade as the gateway sends it: no `side`.
    fn exec_without_side(order_id: &str, trade_id: &str) -> Execution {
        serde_json::from_value(serde_json::json!({
            "order_id": order_id,
            "trade_id": trade_id,
            "symbol": "EXMP.US",
            "trade_done_at": "1790203596",
            "quantity": "2",
            "price": "10.5"
        }))
        .expect("an execution without side deserializes")
    }

    fn us_orders(orders: serde_json::Value, total_count: i32) -> QueryUSOrdersResponse {
        QueryUSOrdersResponse {
            orders: orders.as_array().expect("orders array").clone(),
            total_count,
        }
    }

    fn detail_err() -> anyhow::Error {
        anyhow::anyhow!("detail unavailable")
    }

    #[tokio::test]
    async fn us_execution_side_comes_from_orders() {
        let execs = vec![
            exec_without_side("11", "11-1"),
            exec_without_side("22", "22-1"),
            exec_without_side("44", "44-1"),
        ];
        assert_eq!(execs[0].side, OrderSide::Unknown);
        let mut mock = MockTradeApi::new();
        let mut seq = mockall::Sequence::new();
        mock.expect_us_query_orders()
            .times(1)
            .in_sequence(&mut seq)
            .withf(|o| {
                o.page == 1
                    && o.side == OrderSide::Unknown
                    && o.symbol.as_deref() == Some("EXMP.US")
                    && o.start_at <= 1_790_203_596 - US_SIDE_LOOKUP_LOOKBACK_SECS
                    && o.end_at >= 1_790_203_596
            })
            .returning(|_| {
                Ok(us_orders(
                    serde_json::json!([{ "id": "99", "action": 1 }, { "id": "11", "action": 2 }]),
                    3,
                ))
            });
        mock.expect_us_query_orders()
            .times(1)
            .in_sequence(&mut seq)
            .withf(|o| o.page == 2)
            .returning(|_| {
                Ok(us_orders(
                    serde_json::json!([{ "id": "33", "action": 1 }]),
                    3,
                ))
            });
        // Orders 22 and 44 are not in the list: read from the order detail.
        mock.expect_us_order_detail()
            .times(2)
            .returning(|id| match id.as_str() {
                "44" => Ok(serde_json::json!({ "order": { "id": "44", "action": 1 } })),
                _ => Err(detail_err()),
            });
        let sides = us_execution_sides(&mock, &execs, None, None, Some("EXMP.US")).await;
        let rows = exec_rows(&execs, Some(&sides));
        assert_eq!(rows[0].side, "Sell");
        // Order 22 was not found anywhere: "Unknown", as AP prints it.
        assert_eq!(rows[1].side, "Unknown");
        let json = rows[1].to_json();
        assert_eq!(json["side"], "Unknown", "{json}");
        assert_eq!(rows[2].side, "Buy");
    }

    #[tokio::test]
    async fn us_execution_side_pages_without_total_count() {
        let mut mock = MockTradeApi::new();
        let mut seq = mockall::Sequence::new();
        // total_count 0 (missing) with a full page: read the next page.
        mock.expect_us_query_orders()
            .times(1)
            .in_sequence(&mut seq)
            .withf(|o| o.page == 1)
            .returning(|_| {
                let page: Vec<serde_json::Value> = (0..US_SIDE_LOOKUP_LIMIT)
                    .map(|i| serde_json::json!({ "id": format!("{}", 1000 + i), "action": 1 }))
                    .collect();
                Ok(us_orders(serde_json::Value::Array(page), 0))
            });
        // A short page is the last one: no page 3.
        mock.expect_us_query_orders()
            .times(1)
            .in_sequence(&mut seq)
            .withf(|o| o.page == 2)
            .returning(|_| {
                Ok(us_orders(
                    serde_json::json!([{ "id": "99", "action": 1 }]),
                    0,
                ))
            });
        mock.expect_us_order_detail()
            .times(1)
            .withf(|id| id == "11")
            .returning(|_| Ok(serde_json::json!({ "order": { "id": "11", "action": 2 } })));
        let execs = vec![exec_without_side("11", "11-1")];
        let sides = us_execution_sides(&mock, &execs, None, None, None).await;
        assert_eq!(sides.get("11").map(String::as_str), Some("Sell"));
    }

    #[tokio::test]
    async fn us_execution_side_keeps_earlier_pages_when_a_later_page_fails() {
        let mut mock = MockTradeApi::new();
        let mut seq = mockall::Sequence::new();
        mock.expect_us_query_orders()
            .times(1)
            .in_sequence(&mut seq)
            .withf(|o| o.page == 1)
            .returning(|_| {
                Ok(us_orders(
                    serde_json::json!([{ "id": "11", "action": 2 }]),
                    5,
                ))
            });
        // Page 2 fails: stop paging, keep the side of order 11 from page 1.
        mock.expect_us_query_orders()
            .times(1)
            .in_sequence(&mut seq)
            .withf(|o| o.page == 2)
            .returning(|_| Err(anyhow::anyhow!("orders unavailable")));
        // Only order 22 is left for the order detail.
        mock.expect_us_order_detail()
            .times(1)
            .withf(|id| id == "22")
            .returning(|_| Ok(serde_json::json!({ "order": { "id": "22", "action": 1 } })));
        let execs = vec![
            exec_without_side("11", "11-1"),
            exec_without_side("22", "22-1"),
        ];
        let sides = us_execution_sides(&mock, &execs, None, None, None).await;
        assert_eq!(sides.get("11").map(String::as_str), Some("Sell"));
        assert_eq!(sides.get("22").map(String::as_str), Some("Buy"));
    }

    #[tokio::test]
    async fn us_execution_side_falls_back_to_detail_when_the_list_fails() {
        let mut mock = MockTradeApi::new();
        mock.expect_us_query_orders()
            .times(1)
            .withf(|o| o.page == 1)
            .returning(|_| Err(anyhow::anyhow!("orders unavailable")));
        mock.expect_us_order_detail()
            .times(1)
            .withf(|id| id == "11")
            .returning(|_| Ok(serde_json::json!({ "order": { "id": "11", "action": 2 } })));
        let execs = vec![exec_without_side("11", "11-1")];
        let sides = us_execution_sides(&mock, &execs, None, None, None).await;
        let rows = exec_rows(&execs, Some(&sides));
        assert_eq!(rows[0].side, "Sell");
    }

    #[tokio::test]
    async fn us_execution_side_pages_past_a_server_capped_page_size() {
        fn page_of(n: i32) -> QueryUSOrdersResponse {
            let page: Vec<serde_json::Value> = (0..n)
                .map(|i| serde_json::json!({ "id": format!("{}", 1000 + i), "action": 1 }))
                .collect();
            us_orders(serde_json::Value::Array(page), 0)
        }
        // The server caps pages at 50 (below the requested limit): a page as
        // long as the previous one is not the last; a shorter one is.
        let mut mock = MockTradeApi::new();
        let mut seq = mockall::Sequence::new();
        for (page, len) in [(1, 50), (2, 50), (3, 20)] {
            mock.expect_us_query_orders()
                .times(1)
                .in_sequence(&mut seq)
                .withf(move |o| o.page == page)
                .returning(move |_| Ok(page_of(len)));
        }
        mock.expect_us_order_detail()
            .times(1)
            .returning(|_| Err(detail_err()));
        let execs = vec![exec_without_side("11", "11-1")];
        let sides = us_execution_sides(&mock, &execs, None, None, None).await;
        assert!(sides.is_empty());

        // A short first page alone does not end the lookup; an empty page does.
        let mut mock = MockTradeApi::new();
        let mut seq = mockall::Sequence::new();
        for (page, len) in [(1, 30), (2, 0)] {
            mock.expect_us_query_orders()
                .times(1)
                .in_sequence(&mut seq)
                .withf(move |o| o.page == page)
                .returning(move |_| Ok(page_of(len)));
        }
        mock.expect_us_order_detail()
            .times(1)
            .returning(|_| Err(detail_err()));
        let sides = us_execution_sides(&mock, &execs, None, None, None).await;
        assert!(sides.is_empty());
    }

    #[tokio::test]
    async fn us_execution_side_lookup_is_capped_and_skipped_when_known() {
        let mut mock = MockTradeApi::new();
        mock.expect_us_query_orders()
            .times(usize::try_from(US_SIDE_LOOKUP_MAX_PAGES).unwrap())
            .returning(|_| {
                Ok(us_orders(
                    serde_json::json!([{ "id": "99", "action": 1 }]),
                    1_000_000,
                ))
            });
        mock.expect_us_order_detail()
            .times(1)
            .returning(|_| Err(detail_err()));
        let execs = vec![exec_without_side("11", "11-1")];
        let sides = us_execution_sides(&mock, &execs, None, None, None).await;
        assert!(sides.is_empty());

        // The order detail fallback reads at most US_SIDE_DETAIL_MAX orders.
        let mut mock = MockTradeApi::new();
        mock.expect_us_query_orders()
            .times(1)
            .returning(|_| Ok(us_orders(serde_json::json!([]), 0)));
        mock.expect_us_order_detail()
            .times(US_SIDE_DETAIL_MAX)
            .returning(|id| Ok(serde_json::json!({ "order": { "id": id, "action": 1 } })));
        let execs: Vec<Execution> = (0..US_SIDE_DETAIL_MAX + 5)
            .map(|i| exec_without_side(&format!("{}", 500 + i), "t"))
            .collect();
        let sides = us_execution_sides(&mock, &execs, None, None, None).await;
        assert_eq!(sides.len(), US_SIDE_DETAIL_MAX);

        // Every execution already has a side: no order lookup at all.
        let mock = MockTradeApi::new();
        let sides = us_execution_sides(&mock, &[exec("t1")], None, None, None).await;
        assert!(sides.is_empty());
        let rows = exec_rows(&[exec("t1")], Some(&sides));
        assert_eq!(rows[0].side, "Buy");
    }

    #[test]
    fn ap_execution_side_is_unchanged() {
        let rows = exec_rows(&[exec_without_side("11", "11-1"), exec("t1")], None);
        assert_eq!(rows[0].side, "Unknown");
        assert_eq!(rows[1].side, "Buy");
    }

    #[tokio::test]
    async fn us_preview_classifies_fetch_errors() {
        use longbridge::httpclient::HttpClientError;
        // An unparseable HTTP 404 response: the order does not exist.
        let err = us_preview_order_summary("701276261045858304", |_| async {
            Err(HttpClientError::UnexpectedHttpResponse {
                status: reqwest::StatusCode::NOT_FOUND,
                trace_id: String::new(),
                headers: Box::default(),
                body: String::new(),
            }
            .into())
        })
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("order 701276261045858304 not found"), "{err}");
        // An OpenAPI error code is not an HTTP 404, even when it reads 404.
        let err = us_preview_order_summary("701276261045858304", |_| async {
            Err(HttpClientError::OpenApi {
                code: 404,
                message: "order not exist".to_string(),
                trace_id: String::new(),
            }
            .into())
        })
        .await
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("could not fetch order 701276261045858304"),
            "{err}"
        );
        assert!(err.contains("no confirmation code"), "{err}");
    }

    #[tokio::test]
    async fn us_history_executions_use_v1_history() {
        let mut mock = MockTradeApi::new();
        // No all_executions expectation: a v3 call would fail the test.
        mock.expect_history_executions()
            .times(1)
            .withf(|opts| {
                let v = serde_json::to_value(opts).expect("options serialise");
                v["symbol"] == "EXMP.US" && v.get("start_at").is_some() && v.get("end_at").is_some()
            })
            .returning(|_| Ok(vec![exec("t1"), exec("t2")]));
        let start = parse_datetime_start("2026-01-01").unwrap();
        let end = parse_datetime_end("2026-01-31").unwrap();
        let got = fetch_history_executions(
            &mock,
            true,
            Some(start),
            Some(end),
            Some("EXMP.US"),
            |_| async { panic!("US accounts must not call /v3/trade/execution/all") },
        )
        .await
        .unwrap();
        assert_eq!(got.len(), 2);
    }

    #[tokio::test]
    async fn ap_history_executions_page_v3_until_no_more() {
        // No history_executions expectation: a v1 call would fail the test.
        let mock = MockTradeApi::new();
        let mut pages = Vec::new();
        let got = fetch_history_executions(&mock, false, None, None, Some("EXMP.US"), |opts| {
            let v = serde_json::to_value(&opts).expect("options serialise");
            pages.push(v["page"].as_u64().expect("page set"));
            assert_eq!(v["symbol"], "EXMP.US");
            let page = pages.len();
            async move {
                Ok(AllExecutionsResponse {
                    has_more: page < 2,
                    trades: vec![exec(&format!("t{page}"))],
                })
            }
        })
        .await
        .unwrap();
        let ids: Vec<&str> = got.iter().map(|e| e.trade_id.as_str()).collect();
        assert_eq!(ids, ["t1", "t2"]);
        assert_eq!(pages, [1, 2]);
    }

    #[test]
    fn schema_lists_emitted_fields() {
        let names = |cmd: &str| -> Vec<String> {
            let path: Vec<String> = cmd.split(' ').map(str::to_string).collect();
            schema_for_path(&path)
                .unwrap_or_else(|| panic!("schema for {cmd}"))
                .fields
                .into_iter()
                .map(|f| f.name)
                .collect()
        };
        let mut execs = names("order executions");
        execs.sort();
        let row = ExecRow::from_sdk(&exec("t1"), "Buy".to_string()).to_json();
        let mut keys: Vec<String> = row
            .as_object()
            .expect("execution row is an object")
            .keys()
            .cloned()
            .collect();
        keys.sort();
        assert_eq!(
            keys, execs,
            "order executions JSON keys equal the schema fields"
        );
    }
}
