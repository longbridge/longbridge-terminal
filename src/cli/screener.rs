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
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let path = if mine {
        "/v1/quote/screener/strategies/mine"
    } else if all {
        "/v1/quote/screener/strategies"
    } else {
        "/v1/quote/screener/strategies/recommend"
    };

    let data = http_get(path, &[], verbose).await?;
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&data).unwrap_or_default()
        ),
        OutputFormat::Pretty => {
            let screeners = match data.get("screeners").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a,
                _ => {
                    println!("No strategies found.");
                    return Ok(());
                }
            };
            let label = if mine {
                "My Strategies"
            } else if all {
                "All Strategies"
            } else {
                "Recommended Strategies"
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
    count: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let strategies = http_get("/v1/quote/screener/strategies/recommend", &[], verbose).await?;
    let strategy_obj = strategies["screeners"]
        .as_array()
        .and_then(|arr| arr.iter().find(|s| s["id"].as_i64() == Some(id)))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let strategy = if strategy_obj.is_null() {
        let id_str = id.to_string();
        http_get(
            "/v1/quote/screener/strategy",
            &[("id", id_str.as_str())],
            verbose,
        )
        .await?
    } else {
        strategy_obj
    };

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
    print_screener_results(id, &mkt, &filters, &returns, count, format, verbose).await
}

pub async fn cmd_screener_filter(
    conditions: &[String],
    market: &str,
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
        let key = if raw_key.starts_with("filter_") {
            raw_key.to_string()
        } else {
            format!("filter_{raw_key}")
        };
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
    print_screener_results(
        0,
        &market.to_uppercase(),
        &filters,
        &returns,
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
    count: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let body = serde_json::json!({
        "market": market,
        "page": 1,
        "size": count,
        "filters": filters,
        "returns": returns,
        "sort_by": 0,
        "sort_order": 1,
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
            headers.extend(returns.iter().take(5).map(|k| k.replace("filter_", "")));
            let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();
            let rows: Vec<Vec<String>> = stocks
                .iter()
                .map(|s| {
                    let sym =
                        crate::utils::counter::counter_id_to_symbol(&val_str(&s["counter_id"]));
                    let mut row = vec![sym, val_str(&s["name"])];
                    if let Some(indicators) = s["indicators"].as_array() {
                        row.extend(indicators.iter().take(5).map(|ind| {
                            let v = val_str(&ind["value"]);
                            let unit = val_str(&ind["unit"]);
                            let (display_v, display_unit) = match unit.as_str() {
                                "亿" => (
                                    v.parse::<f64>()
                                        .map(|f| format!("{:.2}", f / 1e8))
                                        .unwrap_or(v),
                                    "亿".to_string(),
                                ),
                                "万" => v
                                    .parse::<f64>()
                                    .map(|f| {
                                        if f >= 1e8 {
                                            (format!("{:.2}", f / 1e8), "亿".to_string())
                                        } else {
                                            (format!("{:.2}", f / 1e4), "万".to_string())
                                        }
                                    })
                                    .unwrap_or((v, unit)),
                                _ => (v, unit),
                            };
                            if display_unit.is_empty() || display_unit == "-" {
                                display_v
                            } else {
                                format!("{display_v} {display_unit}")
                            }
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
