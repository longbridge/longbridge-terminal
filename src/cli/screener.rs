use anyhow::Result;
use serde_json::Value;

use super::{api::http_get, api::http_post, output::print_table, OutputFormat};
use crate::utils::counter::symbol_to_counter_id;
use crate::utils::number::format_financial_value;

fn normalize_key(key: &str) -> String {
    if key.starts_with("filter_") {
        key.to_string()
    } else {
        format!("filter_{key}")
    }
}

fn val_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "-".to_string(),
        other => other.to_string(),
    }
}

pub async fn cmd_screener_strategies(
    mine: bool,
    market: &str,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let path = if mine {
        "/v1/quote/ai/screener/strategies/mine"
    } else {
        "/v1/quote/ai/screener/strategies/recommend"
    };
    let mkt = market.to_uppercase();
    let data = http_get(path, &[("market", mkt.as_str())], verbose).await?;
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&data).unwrap_or_default()
        ),
        OutputFormat::Pretty => {
            let screeners = match data
                .get("strategys")
                .or_else(|| data.get("screeners"))
                .and_then(|v| v.as_array())
            {
                Some(a) if !a.is_empty() => a,
                _ => {
                    println!("No strategies found.");
                    return Ok(());
                }
            };
            let label = if mine {
                "My Strategies"
            } else {
                "Preset Strategies"
            };
            println!("{label}\n");
            let headers = ["ID", "Name", "Avg Day Chg", "Type"];
            let rows: Vec<Vec<String>> = screeners
                .iter()
                .map(|s| {
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
                })
                .collect();
            print_table(&headers, rows, format);
        }
    }
    Ok(())
}

pub async fn cmd_screener_run(
    id: i64,
    sort: Option<&str>,
    order: &str,
    show: &[String],
    count: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let path = format!("/v1/quote/ai/screener/strategy/{id}");
    let strategy = http_get(&path, &[], verbose).await?;

    let mut mkt = "US".to_string();
    let mut filters: Vec<serde_json::Value> = Vec::new();
    let mut returns: Vec<String> = Vec::new();
    if let Some(groups) = strategy["groups"].as_array() {
        for group in groups {
            if let Some(indicators) = group["indicators"].as_array() {
                for ind in indicators {
                    let key = val_str(&ind["key"]);
                    let ind_id = ind["id"].as_i64().unwrap_or(0);
                    if ind_id == -1 && key == "filter_market" {
                        let v = val_str(&ind["value"]);
                        if !v.is_empty() && v != "-" {
                            mkt = v;
                        }
                    } else {
                        let min = val_str(&ind["min"]);
                        let max = val_str(&ind["max"]);
                        let has_range =
                            (!min.is_empty() && min != "-") || (!max.is_empty() && max != "-");
                        if has_range || ind_id > 0 {
                            filters.push(serde_json::json!({
                                "key": key,
                                "min": min,
                                "max": max,
                                "tech_values": {}
                            }));
                            returns.push(key);
                        }
                    }
                }
            }
        }
    }
    for key in show {
        let full_key = normalize_key(key);
        if !returns.contains(&full_key) {
            returns.push(full_key);
        }
    }
    print_screener_results(
        id, &mkt, &filters, &returns, sort, order, count, format, verbose,
    )
    .await
}

pub async fn cmd_screener_filter(
    conditions: &[String],
    market: &str,
    sort: Option<&str>,
    order: &str,
    show: &[String],
    count: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let mut filters: Vec<serde_json::Value> = Vec::new();
    let mut returns: Vec<String> = Vec::new();
    for cond in conditions {
        let parts: Vec<&str> = cond.splitn(3, ':').collect();
        let raw_key = parts.first().copied().unwrap_or("");
        if raw_key.is_empty() {
            continue;
        }
        let key = normalize_key(raw_key);
        let min = parts.get(1).copied().unwrap_or("").to_string();
        let max = parts.get(2).copied().unwrap_or("").to_string();
        filters.push(serde_json::json!({
            "key": key,
            "min": min,
            "max": max,
            "tech_values": {}
        }));
        returns.push(key);
    }
    for key in show {
        let full_key = normalize_key(key);
        if !returns.contains(&full_key) {
            returns.push(full_key);
        }
    }
    print_screener_results(
        0,
        &market.to_uppercase(),
        &filters,
        &returns,
        sort,
        order,
        count,
        format,
        verbose,
    )
    .await
}

async fn print_screener_results(
    strategy_id: i64,
    market: &str,
    filters: &[serde_json::Value],
    returns: &[String],
    sort: Option<&str>,
    order: &str,
    count: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let sort_key = sort.map(normalize_key);
    let mut effective_returns = returns.to_vec();
    if let Some(ref k) = sort_key {
        if !effective_returns.contains(k) {
            effective_returns.push(k.clone());
        }
    }
    let sort_by = sort_key
        .as_deref()
        .and_then(|k| effective_returns.iter().position(|r| r == k))
        .unwrap_or(0);
    let sort_order: u8 = u8::from(order != "asc");
    let body = serde_json::json!({
        "market": market,
        "page": 1,
        "size": count,
        "filters": filters,
        "returns": effective_returns,
        "sort_by": sort_by,
        "sort_order": sort_order,
        "industries": [],
    });
    let data = http_post("/v1/quote/screener/search", body, verbose).await?;
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&data).unwrap_or_default()
        ),
        OutputFormat::Pretty => {
            let total = data
                .get("total")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or_else(|| {
                    data["items"]
                        .as_array()
                        .map_or(0, |a| i64::try_from(a.len()).unwrap_or(0))
                });
            let label = if strategy_id > 0 {
                format!("Strategy #{strategy_id} ({market})")
            } else {
                format!("Custom filter ({market})")
            };
            println!("{label} — {total} stocks found\n");
            let stocks = match data.get("items").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a,
                _ => {
                    println!("No stocks found matching the criteria.");
                    return Ok(());
                }
            };
            let mut headers = vec!["Symbol".to_string(), "Name".to_string()];
            headers.extend(
                effective_returns
                    .iter()
                    .map(|k| k.replace("filter_", "").to_uppercase()),
            );
            let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();
            let rows: Vec<Vec<String>> = stocks
                .iter()
                .map(|s| {
                    let sym =
                        crate::utils::counter::counter_id_to_symbol(&val_str(&s["counter_id"]));
                    let mut row = vec![sym, val_str(&s["name"])];
                    if let Some(indicators) = s["indicators"].as_array() {
                        row.extend(indicators.iter().map(|ind| {
                            let v = val_str(&ind["value"]);
                            if v.is_empty() || v == "-" {
                                return "-".to_string();
                            }
                            let unit = val_str(&ind["unit"]);
                            format_financial_value(&v, unit == "%")
                        }));
                    }
                    row
                })
                .collect();
            print_table(&header_refs, rows, format);
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
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&data).unwrap_or_default()
        ),
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
                    let rows: Vec<Vec<String>> = indicators
                        .iter()
                        .map(|ind| {
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
                        })
                        .collect();
                    print_table(&headers, rows, format);
                }
                println!();
            }
        }
    }
    Ok(())
}
