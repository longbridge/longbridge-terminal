use anyhow::Result;
use serde_json::{Map, Value};

use super::api::http_get;
use super::output::print_table;
use super::OutputFormat;

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

fn transform_ts_field(item: &Value, ts_fields: &[&str]) -> Value {
    let mut obj = Map::new();
    if let Some(map) = item.as_object() {
        for (k, v) in map {
            if ts_fields.contains(&k.as_str()) {
                obj.insert(k.clone(), Value::String(fmt_ts(v)));
            } else {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    Value::Object(obj)
}

const DEPOSIT_SKIP: &[&str] = &[
    "vouchers",
    "state_code",
    "sub_state_code",
    "bank_operation_type_id",
    "disable_link",
    "can_cancel",
];

fn transform_deposit_item(item: &Value) -> Value {
    let mut obj = Map::new();
    if let Some(map) = item.as_object() {
        for (k, v) in map {
            if DEPOSIT_SKIP.contains(&k.as_str()) {
                continue;
            }
            obj.insert(k.clone(), v.clone());
        }
    }
    Value::Object(obj)
}

// ── withdrawal cards ──────────────────────────────────────────────────────────

/// List withdrawal bank cards for the current user.
pub async fn cmd_withdrawal_cards(format: &OutputFormat, verbose: bool) -> Result<()> {
    let data = http_get("/v1/account/bank-cards", &[], verbose).await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            if let Some(cards) = data["cards"].as_array() {
                if cards.is_empty() {
                    println!("No withdrawal cards found.");
                    return Ok(());
                }
                let headers = ["bank_name", "account_number", "currency", "status"];
                let rows: Vec<Vec<String>> = cards
                    .iter()
                    .map(|card| {
                        vec![
                            val_str(&card["bank_name"]),
                            val_str(&card["account_number"]),
                            val_str(&card["currency"]),
                            val_str(&card["status"]),
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

// ── withdrawals ───────────────────────────────────────────────────────────────

/// List withdrawal history for the current account.
pub async fn cmd_withdrawals(
    page: u32,
    limit: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let account_channel = crate::auth::account_channel_or_default();
    let page_str = page.to_string();
    let size_str = limit.to_string();
    let data = http_get(
        "/v1/account/withdrawals",
        &[
            ("page", page_str.as_str()),
            ("size", size_str.as_str()),
            ("account_channel", account_channel.as_str()),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => {
            let mut result = serde_json::Map::new();
            if let Some(obj) = data.as_object() {
                for (k, v) in obj {
                    if k == "list" {
                        if let Some(list) = v.as_array() {
                            let transformed: Vec<Value> = list
                                .iter()
                                .map(|item| transform_ts_field(item, &["created_at"]))
                                .collect();
                            result.insert(k.clone(), Value::Array(transformed));
                        }
                    } else {
                        result.insert(k.clone(), v.clone());
                    }
                }
            }
            print_json(&Value::Object(result));
        }
        OutputFormat::Pretty => {
            let total = val_str(&data["total"]);
            if !total.is_empty() && total != "0" {
                println!("Total: {total}\n");
            }
            if let Some(list) = data["list"].as_array() {
                if list.is_empty() {
                    println!("No withdrawal records.");
                    return Ok(());
                }
                let headers = ["date", "amount", "currency", "status", "bank"];
                let rows: Vec<Vec<String>> = list
                    .iter()
                    .map(|item| {
                        vec![
                            fmt_ts(&item["created_at"]),
                            val_str(&item["amount"]),
                            val_str(&item["currency"]),
                            val_str(&item["status"]),
                            val_str(&item["bank_name"]),
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

// ── deposits ──────────────────────────────────────────────────────────────────

/// List deposit history for the current account.
pub async fn cmd_deposits(
    page: u32,
    limit: u32,
    states: Option<&str>,
    currencies: Option<&str>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let account_channel = crate::auth::account_channel_or_default();
    let page_str = page.to_string();
    let size_str = limit.to_string();
    let mut params: Vec<(&str, &str)> = vec![
        ("page", page_str.as_str()),
        ("size", size_str.as_str()),
        ("account_channel", account_channel.as_str()),
    ];
    if let Some(s) = states {
        params.push(("states", s));
    }
    if let Some(c) = currencies {
        params.push(("currencies", c));
    }
    let data = http_get("/v1/account/deposits", &params, verbose).await?;
    match format {
        OutputFormat::Json => {
            let mut result = serde_json::Map::new();
            if let Some(obj) = data.as_object() {
                for (k, v) in obj {
                    if k == "items" {
                        if let Some(items) = v.as_array() {
                            let transformed: Vec<Value> =
                                items.iter().map(transform_deposit_item).collect();
                            result.insert(k.clone(), Value::Array(transformed));
                        }
                    } else {
                        result.insert(k.clone(), v.clone());
                    }
                }
            }
            print_json(&Value::Object(result));
        }
        OutputFormat::Pretty => {
            let total = val_str(&data["total"]);
            if !total.is_empty() && total != "0" {
                println!("Total: {total}\n");
            }
            if let Some(items) = data["items"].as_array() {
                if items.is_empty() {
                    println!("No deposit records.");
                    return Ok(());
                }
                let headers = ["id", "date", "amount", "currency", "type", "state"];
                let rows: Vec<Vec<String>> = items
                    .iter()
                    .map(|item| {
                        vec![
                            val_str(&item["id"]),
                            val_str(&item["created_at"]),
                            val_str(&item["amount"]),
                            val_str(&item["currency"]),
                            val_str(&item["bank_operation_type_name"]),
                            val_str(&item["state"]),
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
