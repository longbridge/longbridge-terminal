use anyhow::Result;
use serde_json::Value;

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
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

// ── withdrawal cards ──────────────────────────────────────────────────────────

/// List withdrawal bank cards for the current user.
pub async fn cmd_withdrawal_cards(format: &OutputFormat, verbose: bool) -> Result<()> {
    let data = http_get("/v3/portfolio/withdraw/cards", &[], verbose).await?;
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
        "/v6/portfolio/withdraw/record",
        &[
            ("page", page_str.as_str()),
            ("size", size_str.as_str()),
            ("account_channel", account_channel.as_str()),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
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
                            val_str(&item["created_at"]),
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
    let data = http_get("/v5/portfolio/deposit/notify", &params, verbose).await?;
    match format {
        OutputFormat::Json => print_json(&data),
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
                let headers = ["date", "amount", "currency", "state", "source"];
                let rows: Vec<Vec<String>> = items
                    .iter()
                    .map(|item| {
                        let state = match val_str(&item["state"]).as_str() {
                            "0" => "Pending".to_string(),
                            "1" => "Credited".to_string(),
                            "2" => "Failed".to_string(),
                            s => s.to_string(),
                        };
                        vec![
                            val_str(&item["created_at"]),
                            val_str(&item["amount"]),
                            val_str(&item["currency"]),
                            state,
                            val_str(&item["fund_source"]),
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
