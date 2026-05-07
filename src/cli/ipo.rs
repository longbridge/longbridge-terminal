use anyhow::Result;
use serde_json::Value;
use std::io::Write;

use super::api::{http_get, http_post};
use super::output::print_table;
use super::OutputFormat;
use crate::utils::counter::symbol_to_counter_id;

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
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn confirm_action(action: &str) -> Result<bool> {
    print!("Are you sure you want to {action}? [y/N] ");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(input.trim().eq_ignore_ascii_case("y"))
}

async fn member_id() -> Result<i64> {
    crate::openapi::quote().member_id().await
}

// ── read-only IPO list commands ────────────────────────────────────────────────

/// List IPO stocks currently in subscription or pre-filing stage.
pub async fn cmd_ipo_subscriptions(format: &OutputFormat, verbose: bool) -> Result<()> {
    let mid = member_id().await?;
    let mid_str = mid.to_string();
    let data = http_get(
        "/newmarket/hk/ipo/subscribing",
        &[("memebr_id", mid_str.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            if let Some(list) = data["list"].as_array() {
                if list.is_empty() {
                    println!("No active IPO subscriptions.");
                    return Ok(());
                }
                let headers = ["name", "counter_id", "issue_price", "deadline", "state"];
                let rows: Vec<Vec<String>> = list
                    .iter()
                    .map(|item| {
                        let stage = match val_str(&item["state_stage"]).as_str() {
                            "1" => "sub-start",
                            "2" => "sub-end",
                            "3" => "allotment",
                            "4" => "grey-market",
                            "5" => "listed",
                            s => s,
                        }
                        .to_string();
                        vec![
                            val_str(&item["name"]),
                            val_str(&item["counter_id"]),
                            val_str(&item["issue_price"]),
                            val_str(&item["sub_deadline"]),
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
        "/newmarket/hk/ipo/wait_listing",
        &[
            ("day_time", day_str.as_str()),
            ("memebr_id", mid_str.as_str()),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            if let Some(list) = data["list"].as_array() {
                if list.is_empty() {
                    println!("No IPO stocks in wait-listing.");
                    return Ok(());
                }
                let headers = ["name", "counter_id", "issue_price", "listing_date"];
                let rows: Vec<Vec<String>> = list
                    .iter()
                    .map(|item| {
                        vec![
                            val_str(&item["name"]),
                            val_str(&item["counter_id"]),
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
        "/newmarket/hk/ipo/ipo_listing",
        &[
            ("page", page_str.as_str()),
            ("size", size_str.as_str()),
            ("memebr_id", mid_str.as_str()),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            if let Some(list) = data["list"].as_array() {
                if list.is_empty() {
                    println!("No listed IPO stocks found.");
                    return Ok(());
                }
                let headers = ["name", "counter_id", "issue_price", "listing_date"];
                let rows: Vec<Vec<String>> = list
                    .iter()
                    .map(|item| {
                        vec![
                            val_str(&item["name"]),
                            val_str(&item["counter_id"]),
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
    let data = http_get("/newmarket/ipo/calendar", &[], verbose).await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            if let Some(list) = data["list"].as_array() {
                if list.is_empty() {
                    println!("No IPO calendar entries found.");
                    return Ok(());
                }
                let headers = ["date", "name", "counter_id", "type"];
                let rows: Vec<Vec<String>> = list
                    .iter()
                    .map(|item| {
                        vec![
                            val_str(&item["date"]),
                            val_str(&item["name"]),
                            val_str(&item["counter_id"]),
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
        "/v3/ipo/info",
        &[
            ("counter_id", cid.as_str()),
            ("account_channel", account_channel.as_str()),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_json(&data),
    }
    Ok(())
}

/// Show IPO profile (prospectus summary) for a symbol.
pub async fn cmd_ipo_profile(symbol: String, format: &OutputFormat, verbose: bool) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/stock-info/ipo-profile",
        &[("counter_id", cid.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_json(&data),
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
        "/stock-info/ipo-timeline",
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
        "/ipo/active_order",
        &[("counter_id", cid.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_json(&data),
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
    let data = http_get("/ipo/holding", &params, verbose).await?;
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
    let data = http_get(
        "/v1/ipo/detail",
        &[
            ("order_id", order_id.as_str()),
            ("account_channel", account_channel.as_str()),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_json(&data),
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
    let data = http_get("/ipo/history", &params, verbose).await?;
    match format {
        OutputFormat::Json => print_json(&data),
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
                            val_str(&o["created_at"]),
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
        "/v1/ipo/check_user_can_subscribe",
        &[("counter_id", cid.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_json(&data),
    }
    Ok(())
}

/// Show IPO profit/loss summary for the given period.
pub async fn cmd_ipo_profit_loss(period: &str, format: &OutputFormat, verbose: bool) -> Result<()> {
    let account_channel = crate::auth::account_channel_or_default();
    let data = http_get(
        "/portfolio/asset/ipo_analysis",
        &[
            ("period", period),
            ("account_channel", account_channel.as_str()),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_json(&data),
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
        "/portfolio/asset/ipo_analysis_sublist",
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
        OutputFormat::Pretty => print_json(&data),
    }
    Ok(())
}

/// Show IPO holding portfolio detail for a symbol.
pub async fn cmd_ipo_holdings(symbol: String, format: &OutputFormat, verbose: bool) -> Result<()> {
    let account_channel = crate::auth::account_channel_or_default();
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/portfolio/ipo/detail",
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
        OutputFormat::Pretty => print_json(&data),
    }
    Ok(())
}

// ── write IPO commands (require confirmation) ──────────────────────────────────

/// Submit an IPO subscription order.
pub async fn cmd_ipo_submit(
    symbol: String,
    qty: String,
    amount: String,
    financing_amount: String,
    method: u8,
    financing_interest: String,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let account_channel = crate::auth::account_channel_or_default();
    let cid = symbol_to_counter_id(&symbol);
    if !confirm_action(&format!("submit IPO subscription for {symbol}"))? {
        println!("Cancelled.");
        return Ok(());
    }
    let body = serde_json::json!({
        "counter_id": cid,
        "sub_qty": qty,
        "sub_amount": amount,
        "financing_amount": financing_amount,
        "method": method,
        "financing_interest": financing_interest,
        "account_channel": account_channel,
    });
    let data = http_post("/ipo/submit", body, verbose).await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            println!("IPO subscription submitted.");
            print_json(&data);
        }
    }
    Ok(())
}

/// Withdraw an IPO subscription order by order ID.
pub async fn cmd_ipo_withdraw(
    order_id: String,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let account_channel = crate::auth::account_channel_or_default();
    if !confirm_action(&format!("withdraw IPO order {order_id}"))? {
        println!("Cancelled.");
        return Ok(());
    }
    let body = serde_json::json!({
        "order_id": order_id,
        "account_channel": account_channel,
    });
    let data = http_post("/ipo/withdraw", body, verbose).await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            println!("IPO order withdrawn.");
            print_json(&data);
        }
    }
    Ok(())
}
