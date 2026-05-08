use anyhow::Result;
use serde_json::{Map, Value};

use super::api::http_get;
use super::output::{print_json_value, print_table};
use super::OutputFormat;
use crate::utils::counter::{counter_id_to_symbol, symbol_to_counter_id};

fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
}

fn val_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "-".to_string(),
        other => other.to_string(),
    }
}

fn fmt_ts(v: &Value) -> String {
    let ts = match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    };
    ts.map_or_else(|| val_str(v), crate::utils::datetime::format_timestamp)
}

fn state_stage_label(v: &Value) -> &'static str {
    match val_str(v).as_str() {
        "0" => "pending",
        "1" => "sub-start",
        "2" => "sub-end",
        "3" => "allotment",
        "4" => "grey-market",
        "5" => "listed",
        _ => "unknown",
    }
}

fn extract_tag(tags: &[Value], keyword: &str) -> String {
    tags.iter()
        .find_map(|t| t.as_str().filter(|s| s.contains(keyword)))
        .map_or_else(|| "-".to_string(), str::to_string)
}

// Transform subscription item: counter_id → symbol, sub_deadline → deadline (RFC 3339),
// state_stage → state (label).
fn transform_subscription(item: &Value) -> Value {
    let mut obj = Map::new();
    if let Some(map) = item.as_object() {
        for (k, v) in map {
            match k.as_str() {
                "counter_id" => {
                    obj.insert(
                        "symbol".to_string(),
                        Value::String(counter_id_to_symbol(&val_str(v))),
                    );
                }
                "sub_deadline" => {
                    obj.insert("deadline".to_string(), Value::String(fmt_ts(v)));
                }
                "state_stage" => {
                    obj.insert(
                        "state".to_string(),
                        Value::String(state_stage_label(v).to_string()),
                    );
                }
                _ => {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }
    }
    Value::Object(obj)
}

fn state_label(v: &Value) -> &'static str {
    match val_str(v).as_str() {
        "0" => "normal",
        "1" => "delayed",
        "2" => "cancelled",
        _ => "unknown",
    }
}

fn sub_state_label(v: &Value) -> &'static str {
    match val_str(v).as_str() {
        "0" => "not-subscribed",
        "1" => "not-won",
        "2" => "won",
        _ => "unknown",
    }
}

// Fields that are unix timestamps (numeric) and should be formatted as RFC 3339.
const TS_FIELDS: &[&str] = &[
    "ipo_date",
    "sub_date",
    "sub_end_date",
    "result_date",
    "mart_date",
    "mart_begin",
    "mart_end",
];

// Internal fields not useful to callers.
const SKIP_FIELDS: &[&str] = &[
    "code",
    "order_id",
    "sort",
    "watched",
    "remaining_second",
    "remaining_day",
];

fn transform_ipo_list_item(item: &Value) -> Value {
    let mut obj = Map::new();
    if let Some(map) = item.as_object() {
        for (k, v) in map {
            if SKIP_FIELDS.contains(&k.as_str()) {
                continue;
            }
            if k == "counter_id" {
                obj.insert(
                    "symbol".to_string(),
                    Value::String(counter_id_to_symbol(&val_str(v))),
                );
            } else if k == "state_stage" {
                obj.insert(
                    "state".to_string(),
                    Value::String(state_stage_label(v).to_string()),
                );
            } else if k == "state" {
                obj.insert(k.clone(), Value::String(state_label(v).to_string()));
            } else if k == "sub_state" {
                obj.insert(k.clone(), Value::String(sub_state_label(v).to_string()));
            } else if k == "mart_status" {
                let label = if val_str(v) == "1" { "open" } else { "closed" };
                obj.insert(k.clone(), Value::String(label.to_string()));
            } else if TS_FIELDS.contains(&k.as_str()) && v.is_number() {
                obj.insert(k.clone(), Value::String(fmt_ts(v)));
            } else {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    Value::Object(obj)
}

// Transform history item: format created_at timestamp.
fn transform_history_item(item: &Value) -> Value {
    let mut obj = Map::new();
    if let Some(map) = item.as_object() {
        for (k, v) in map {
            if k == "created_at" {
                obj.insert(k.clone(), Value::String(fmt_ts(v)));
            } else {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    Value::Object(obj)
}

async fn member_id() -> Result<i64> {
    crate::openapi::quote()
        .member_id()
        .await
        .map_err(anyhow::Error::from)
}

// ── read-only IPO list commands ────────────────────────────────────────────────

/// List IPO stocks currently in subscription or pre-filing stage.
pub async fn cmd_ipo_subscriptions(format: &OutputFormat, verbose: bool) -> Result<()> {
    let mid = member_id().await?;
    let mid_str = mid.to_string();
    let data = http_get(
        "/v1/ipo/subscriptions",
        &[("memebr_id", mid_str.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => {
            if let Some(list) = data["list"].as_array() {
                let transformed: Vec<Value> = list.iter().map(transform_subscription).collect();
                print_json(&Value::Array(transformed));
            } else {
                print_json(&data);
            }
        }
        OutputFormat::Pretty => {
            if let Some(list) = data["list"].as_array() {
                if list.is_empty() {
                    println!("No active IPO subscriptions.");
                    return Ok(());
                }
                let headers = [
                    "name",
                    "symbol",
                    "currency",
                    "entrance_fee",
                    "est_sub",
                    "fin_rate",
                    "max_lev",
                    "issue_price",
                    "deadline",
                    "state",
                ];
                let rows: Vec<Vec<String>> = list
                    .iter()
                    .map(|item| {
                        let stage = state_stage_label(&item["state_stage"]).to_string();
                        let tags: &[Value] = item["tags"].as_array().map_or(&[], |a| a.as_slice());
                        let fin_rate = extract_tag(tags, "融资利率");
                        let max_lev = extract_tag(tags, "杠杆");
                        vec![
                            val_str(&item["name"]),
                            counter_id_to_symbol(&val_str(&item["counter_id"])),
                            val_str(&item["currency"]),
                            val_str(&item["entrance_fee"]),
                            val_str(&item["rate_forcast"]),
                            fin_rate,
                            max_lev,
                            val_str(&item["issue_price"]),
                            fmt_ts(&item["sub_deadline"]),
                            stage,
                        ]
                    })
                    .collect();
                print_table(&headers, rows, &OutputFormat::Pretty);
            } else {
                print_json(&data);
            }
        }
    }
    Ok(())
}

/// List IPO stocks in the wait-listing (grey market) stage.
pub async fn cmd_ipo_wait_listing(format: &OutputFormat, verbose: bool) -> Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mid = member_id().await?;
    let mid_str = mid.to_string();
    let day_str = now.to_string();
    let data = http_get(
        "/v1/ipo/wait-listing",
        &[
            ("day_time", day_str.as_str()),
            ("memebr_id", mid_str.as_str()),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => {
            if let Some(list) = data["ipos"].as_array() {
                let transformed: Vec<Value> = list.iter().map(transform_ipo_list_item).collect();
                print_json(&Value::Array(transformed));
            } else {
                print_json(&data);
            }
        }
        OutputFormat::Pretty => {
            if let Some(list) = data["ipos"].as_array() {
                if list.is_empty() {
                    println!("No IPO stocks in wait-listing.");
                    return Ok(());
                }
                let headers = ["name", "symbol", "issue_price", "ipo_date", "state"];
                let rows: Vec<Vec<String>> = list
                    .iter()
                    .map(|item| {
                        vec![
                            val_str(&item["name"]),
                            counter_id_to_symbol(&val_str(&item["counter_id"])),
                            val_str(&item["issue_price"]),
                            fmt_ts(&item["ipo_date"]),
                            state_stage_label(&item["state_stage"]).to_string(),
                        ]
                    })
                    .collect();
                print_table(&headers, rows, &OutputFormat::Pretty);
            } else {
                print_json(&data);
            }
        }
    }
    Ok(())
}

/// List recently listed IPO stocks.
pub async fn cmd_ipo_listed(
    page: u32,
    limit: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let mid = member_id().await?;
    let mid_str = mid.to_string();
    let page_str = page.to_string();
    let size_str = limit.to_string();
    let data = http_get(
        "/v1/ipo/listed",
        &[
            ("page", page_str.as_str()),
            ("size", size_str.as_str()),
            ("memebr_id", mid_str.as_str()),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => {
            if let Some(list) = data["list"].as_array() {
                let transformed: Vec<Value> = list.iter().map(transform_ipo_list_item).collect();
                print_json(&Value::Array(transformed));
            } else {
                print_json(&data);
            }
        }
        OutputFormat::Pretty => {
            if let Some(list) = data["list"].as_array() {
                if list.is_empty() {
                    println!("No listed IPO stocks found.");
                    return Ok(());
                }
                let headers = ["name", "symbol", "issue_price", "listing_date"];
                let rows: Vec<Vec<String>> = list
                    .iter()
                    .map(|item| {
                        vec![
                            val_str(&item["name"]),
                            counter_id_to_symbol(&val_str(&item["counter_id"])),
                            val_str(&item["issue_price"]),
                            val_str(&item["listing_date"]),
                        ]
                    })
                    .collect();
                print_table(&headers, rows, &OutputFormat::Pretty);
            } else {
                print_json(&data);
            }
        }
    }
    Ok(())
}

/// Show the IPO calendar (all upcoming and recent IPOs).
pub async fn cmd_ipo_calendar(format: &OutputFormat, verbose: bool) -> Result<()> {
    let data = http_get("/v1/ipo/calendar", &[], verbose).await?;
    match format {
        OutputFormat::Json => {
            if let Some(list) = data["list"].as_array() {
                let transformed: Vec<Value> = list.iter().map(transform_ipo_list_item).collect();
                print_json(&Value::Array(transformed));
            } else {
                print_json(&data);
            }
        }
        OutputFormat::Pretty => {
            if let Some(list) = data["list"].as_array() {
                if list.is_empty() {
                    println!("No IPO calendar entries found.");
                    return Ok(());
                }
                let headers = ["date", "name", "symbol", "type"];
                let rows: Vec<Vec<String>> = list
                    .iter()
                    .map(|item| {
                        vec![
                            val_str(&item["date"]),
                            val_str(&item["name"]),
                            counter_id_to_symbol(&val_str(&item["counter_id"])),
                            val_str(&item["type"]),
                        ]
                    })
                    .collect();
                print_table(&headers, rows, &OutputFormat::Pretty);
            } else {
                print_json(&data);
            }
        }
    }
    Ok(())
}

/// Show IPO subscription page information for a symbol.
pub async fn cmd_ipo_info(symbol: String, format: &OutputFormat, verbose: bool) -> Result<()> {
    let account_channel = crate::auth::account_channel_or_default();
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/ipo/info",
        &[
            ("counter_id", cid.as_str()),
            ("account_channel", account_channel.as_str()),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_json_value(&data, format),
    }
    Ok(())
}

/// Show IPO profile (prospectus summary) for a symbol.
pub async fn cmd_ipo_profile(symbol: String, format: &OutputFormat, verbose: bool) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get("/v1/ipo/profile", &[("counter_id", cid.as_str())], verbose).await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_json_value(&data, format),
    }
    Ok(())
}

/// Show the IPO timeline for a symbol.
pub async fn cmd_ipo_timeline(
    symbol: String,
    market: &str,
    flag: u8,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let flag_str = flag.to_string();
    let data = http_get(
        "/v1/ipo/timeline",
        &[
            ("counter_id", cid.as_str()),
            ("market", market),
            ("flag", flag_str.as_str()),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            if let Some(timeline) = data["timeline"].as_array() {
                let headers = ["date", "event", "status"];
                let rows: Vec<Vec<String>> = timeline
                    .iter()
                    .map(|item| {
                        vec![
                            val_str(&item["date"]),
                            val_str(&item["event"]),
                            val_str(&item["status"]),
                        ]
                    })
                    .collect();
                print_table(&headers, rows, &OutputFormat::Pretty);
            } else {
                print_json(&data);
            }
        }
    }
    Ok(())
}

/// Show the current active IPO order status for a symbol.
pub async fn cmd_ipo_order(symbol: String, format: &OutputFormat, verbose: bool) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/ipo/active-order",
        &[("counter_id", cid.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_json_value(&data, format),
    }
    Ok(())
}

/// List active IPO holding orders for the current account.
pub async fn cmd_ipo_orders(
    symbol: Option<String>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let account_channel = crate::auth::account_channel_or_default();
    let mut params: Vec<(&str, &str)> = vec![("account_channel", account_channel.as_str())];
    let cid;
    if let Some(ref sym) = symbol {
        cid = symbol_to_counter_id(sym);
        params.push(("counter_id", cid.as_str()));
    }
    let data = http_get("/v1/ipo/orders", &params, verbose).await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            if let Some(orders) = data["orders"].as_array() {
                if orders.is_empty() {
                    println!("No active IPO orders.");
                    return Ok(());
                }
                let headers = ["id", "name", "code", "qty", "status"];
                let rows: Vec<Vec<String>> = orders
                    .iter()
                    .map(|o| {
                        vec![
                            val_str(&o["id"]),
                            val_str(&o["name"]),
                            val_str(&o["code"]),
                            val_str(&o["sub_qty"]),
                            val_str(&o["status"]),
                        ]
                    })
                    .collect();
                print_table(&headers, rows, &OutputFormat::Pretty);
            } else {
                print_json(&data);
            }
        }
    }
    Ok(())
}

/// Show IPO order detail by order ID.
pub async fn cmd_ipo_order_detail(
    order_id: String,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let account_channel = crate::auth::account_channel_or_default();
    let path = format!("/v1/ipo/orders/{order_id}");
    let data = http_get(
        &path,
        &[("account_channel", account_channel.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_json_value(&data, format),
    }
    Ok(())
}

/// List IPO subscription history.
pub async fn cmd_ipo_history(
    market: Option<String>,
    status: Option<String>,
    page: u32,
    limit: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let page_str = page.to_string();
    let limit_str = limit.to_string();
    let mut params: Vec<(&str, &str)> =
        vec![("page", page_str.as_str()), ("limit", limit_str.as_str())];
    if let Some(ref m) = market {
        params.push(("market", m.as_str()));
    }
    if let Some(ref s) = status {
        params.push(("status", s.as_str()));
    }
    let data = http_get("/v1/ipo/orders/history", &params, verbose).await?;
    match format {
        OutputFormat::Json => {
            if let Some(arr) = data.as_array() {
                let transformed: Vec<Value> = arr.iter().map(transform_history_item).collect();
                print_json(&Value::Array(transformed));
            } else {
                print_json(&data);
            }
        }
        OutputFormat::Pretty => {
            if let Some(arr) = data.as_array() {
                if arr.is_empty() {
                    println!("No IPO history found.");
                    return Ok(());
                }
                let headers = ["id", "name", "code", "qty", "won", "status", "date"];
                let rows: Vec<Vec<String>> = arr
                    .iter()
                    .map(|o| {
                        vec![
                            val_str(&o["id"]),
                            val_str(&o["name"]),
                            val_str(&o["code"]),
                            val_str(&o["sub_qty"]),
                            val_str(&o["lot_win_qty"]),
                            val_str(&o["status"]),
                            fmt_ts(&o["created_at"]),
                        ]
                    })
                    .collect();
                print_table(&headers, rows, &OutputFormat::Pretty);
            } else {
                print_json(&data);
            }
        }
    }
    Ok(())
}

/// Check if the current user is eligible to subscribe to an IPO.
pub async fn cmd_ipo_eligibility(
    symbol: String,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/ipo/eligibility",
        &[("counter_id", cid.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_json_value(&data, format),
    }
    Ok(())
}

/// Show IPO profit/loss summary for the given period.
pub async fn cmd_ipo_profit_loss(period: &str, format: &OutputFormat, verbose: bool) -> Result<()> {
    let account_channel = crate::auth::account_channel_or_default();
    let data = http_get(
        "/v1/ipo/profit-loss",
        &[
            ("period", period),
            ("account_channel", account_channel.as_str()),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_json_value(&data, format),
    }
    Ok(())
}

/// List IPO profit/loss items for the given period.
pub async fn cmd_ipo_profit_loss_items(
    period: &str,
    page: u32,
    limit: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let account_channel = crate::auth::account_channel_or_default();
    let page_str = page.to_string();
    let size_str = limit.to_string();
    let data = http_get(
        "/v1/ipo/profit-loss/items",
        &[
            ("period", period),
            ("page", page_str.as_str()),
            ("size", size_str.as_str()),
            ("account_channel", account_channel.as_str()),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_json_value(&data, format),
    }
    Ok(())
}

/// Show IPO holding portfolio detail for a symbol.
pub async fn cmd_ipo_holdings(symbol: String, format: &OutputFormat, verbose: bool) -> Result<()> {
    let account_channel = crate::auth::account_channel_or_default();
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/ipo/holdings",
        &[
            ("counter_id", cid.as_str()),
            ("need_realtime", "true"),
            ("account_channel", account_channel.as_str()),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_json_value(&data, format),
    }
    Ok(())
}

// ── US IPO commands ───────────────────────────────────────────────────────────

/// List US IPO stocks currently in subscription stage.
pub async fn cmd_ipo_us_subscriptions(format: &OutputFormat, verbose: bool) -> Result<()> {
    let data = http_get("/v1/ipo/us/subscriptions", &[], verbose).await?;
    match format {
        OutputFormat::Json => {
            if let Some(list) = data["list"].as_array() {
                let transformed: Vec<Value> = list.iter().map(transform_subscription).collect();
                print_json(&Value::Array(transformed));
            } else {
                print_json(&data);
            }
        }
        OutputFormat::Pretty => {
            if let Some(list) = data["list"].as_array() {
                if list.is_empty() {
                    println!("No active US IPO subscriptions.");
                    return Ok(());
                }
                let headers = [
                    "name",
                    "symbol",
                    "currency",
                    "issue_price",
                    "deadline",
                    "state",
                ];
                let rows: Vec<Vec<String>> = list
                    .iter()
                    .map(|item| {
                        let stage = state_stage_label(&item["state_stage"]).to_string();
                        vec![
                            val_str(&item["name"]),
                            counter_id_to_symbol(&val_str(&item["counter_id"])),
                            val_str(&item["currency"]),
                            val_str(&item["issue_price"]),
                            fmt_ts(&item["sub_deadline"]),
                            stage,
                        ]
                    })
                    .collect();
                print_table(&headers, rows, &OutputFormat::Pretty);
            } else {
                print_json(&data);
            }
        }
    }
    Ok(())
}

/// List US IPO stocks in wait-listing stage.
pub async fn cmd_ipo_us_wait_listing(format: &OutputFormat, verbose: bool) -> Result<()> {
    let data = http_get("/v1/ipo/us/wait-listing", &[], verbose).await?;
    match format {
        OutputFormat::Json => {
            if let Some(list) = data["ipos"].as_array() {
                let transformed: Vec<Value> = list.iter().map(transform_ipo_list_item).collect();
                print_json(&Value::Array(transformed));
            } else {
                print_json(&data);
            }
        }
        OutputFormat::Pretty => {
            if let Some(list) = data["ipos"].as_array() {
                if list.is_empty() {
                    println!("No US IPO stocks in wait-listing.");
                    return Ok(());
                }
                let headers = ["name", "symbol", "issue_price", "ipo_date", "state"];
                let rows: Vec<Vec<String>> = list
                    .iter()
                    .map(|item| {
                        vec![
                            val_str(&item["name"]),
                            counter_id_to_symbol(&val_str(&item["counter_id"])),
                            val_str(&item["issue_price"]),
                            fmt_ts(&item["ipo_date"]),
                            state_stage_label(&item["state_stage"]).to_string(),
                        ]
                    })
                    .collect();
                print_table(&headers, rows, &OutputFormat::Pretty);
            } else {
                print_json(&data);
            }
        }
    }
    Ok(())
}

/// List recently listed US IPO stocks.
pub async fn cmd_ipo_us_listed(
    page: u32,
    limit: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let page_str = page.to_string();
    let size_str = limit.to_string();
    let data = http_get(
        "/v1/ipo/us/listed",
        &[("page", page_str.as_str()), ("size", size_str.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => {
            if let Some(list) = data["list"].as_array() {
                let transformed: Vec<Value> = list.iter().map(transform_ipo_list_item).collect();
                print_json(&Value::Array(transformed));
            } else {
                print_json(&data);
            }
        }
        OutputFormat::Pretty => {
            if let Some(list) = data["list"].as_array() {
                if list.is_empty() {
                    println!("No listed US IPO stocks found.");
                    return Ok(());
                }
                let headers = ["name", "symbol", "issue_price", "listing_date"];
                let rows: Vec<Vec<String>> = list
                    .iter()
                    .map(|item| {
                        vec![
                            val_str(&item["name"]),
                            counter_id_to_symbol(&val_str(&item["counter_id"])),
                            val_str(&item["issue_price"]),
                            val_str(&item["listing_date"]),
                        ]
                    })
                    .collect();
                print_table(&headers, rows, &OutputFormat::Pretty);
            } else {
                print_json(&data);
            }
        }
    }
    Ok(())
}
