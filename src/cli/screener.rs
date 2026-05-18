use anyhow::Result;
use serde_json::Value;

use super::{api::http_get, api::http_post, output::print_table, OutputFormat};
use crate::utils::counter::symbol_to_counter_id;

fn val_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "-".to_string(),
        other => other.to_string(),
    }
}

pub async fn cmd_screener_strategies(
    mine: bool,
    all: bool,
    id: Option<i64>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    if let Some(sid) = id {
        let sid_str = sid.to_string();
        let data = http_get(
            "/v1/quote/screener/strategy",
            &[("id", sid_str.as_str())],
            verbose,
        )
        .await?;
        match format {
            OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&data).unwrap_or_default()),
            OutputFormat::Pretty => {
                let name = val_str(&data["name"]);
                println!("Strategy #{sid} — {name}\n");
                if let Some(groups) = data["groups"].as_array() {
                    for group in groups {
                        let gname = val_str(&group["group_name"]);
                        println!("  {gname}");
                        if let Some(indicators) = group["indicators"].as_array() {
                            let headers = ["id", "key", "name", "unit", "description"];
                            let rows: Vec<Vec<String>> = indicators.iter().map(|ind| {
                                vec![
                                    val_str(&ind["id"]),
                                    val_str(&ind["key"]),
                                    val_str(&ind["name"]),
                                    val_str(&ind["unit"]),
                                    val_str(&ind["description"]),
                                ]
                            }).collect();
                            print_table(&headers, rows, format);
                        }
                        println!();
                    }
                }
            }
        }
        return Ok(());
    }

    let path = if mine {
        "/v1/quote/screener/strategies/mine"
    } else if all {
        "/v1/quote/screener/strategies"
    } else {
        "/v1/quote/screener/strategies/recommend"
    };

    let data = http_get(path, &[], verbose).await?;
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&data).unwrap_or_default()),
        OutputFormat::Pretty => {
            let screeners = match data.get("screeners").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a,
                _ => {
                    println!("No strategies found.");
                    return Ok(());
                }
            };
            let label = if mine { "My Strategies" } else if all { "All Strategies" } else { "Recommended Strategies" };
            println!("{label}\n");
            let headers = ["ID", "Name", "Avg Day Chg", "Type"];
            let rows: Vec<Vec<String>> = screeners.iter().map(|s| {
                let type_str = match s["type"].as_i64() {
                    Some(1) => "User",
                    _ => "Platform",
                };
                vec![
                    val_str(&s["id"]),
                    val_str(&s["name"]),
                    val_str(&s["average_day_chg"]),
                    type_str.to_string(),
                ]
            }).collect();
            print_table(&headers, rows, format);
        }
    }
    Ok(())
}

pub async fn cmd_screener_search(
    strategy_id: Option<i64>,
    market: &str,
    count: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let mut body = serde_json::json!({
        "market": market.to_uppercase(),
        "page": 1,
        "size": count,
    });
    if let Some(sid) = strategy_id {
        body["id"] = serde_json::json!(sid.to_string());
    }
    let data = http_post("/v1/quote/screener/search", body, verbose).await?;
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&data).unwrap_or_default()),
        OutputFormat::Pretty => {
            let total = data["total"].as_i64().unwrap_or(0);
            let strategy_label = strategy_id
                .map_or_else(|| format!("Custom filter ({market})"), |id| format!("Strategy #{id}"));
            println!("{strategy_label} — {total} stocks found\n");
            let stocks = match data.get("stocks").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a,
                _ => {
                    println!("No stocks found matching the criteria.");
                    return Ok(());
                }
            };
            let headers = ["Symbol", "Name"];
            let rows: Vec<Vec<String>> = stocks.iter().map(|s| {
                let cid = val_str(&s["counter_id"]);
                let sym = crate::utils::counter::counter_id_to_symbol(&cid);
                vec![sym, val_str(&s["name"])]
            }).collect();
            print_table(&headers, rows, format);
        }
    }
    Ok(())
}

pub async fn cmd_screener_indicators(
    symbol: Option<String>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let mut params: Vec<(&str, &str)> = vec![];
    let cid;
    if let Some(ref sym) = symbol {
        cid = symbol_to_counter_id(sym);
        params.push(("counter_id", cid.as_str()));
    }
    let data = http_get("/v1/quote/screener/indicators", &params, verbose).await?;
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&data).unwrap_or_default()),
        OutputFormat::Pretty => {
            let groups = match data.get("groups").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a,
                _ => {
                    println!("No indicator config found.");
                    return Ok(());
                }
            };
            for group in groups {
                let gname = val_str(&group["group_name"]);
                println!("── {gname} ──");
                if let Some(indicators) = group["indicators"].as_array() {
                    let headers = ["ID", "Key", "Name", "Unit", "Min", "Max"];
                    let rows: Vec<Vec<String>> = indicators.iter().map(|ind| {
                        let min = val_str(&ind["default_range"]["min"]);
                        let max = val_str(&ind["default_range"]["max"]);
                        vec![
                            val_str(&ind["id"]),
                            val_str(&ind["key"]),
                            val_str(&ind["name"]),
                            val_str(&ind["unit"]),
                            min,
                            max,
                        ]
                    }).collect();
                    print_table(&headers, rows, format);
                }
                println!();
            }
        }
    }
    Ok(())
}
