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
            OutputFormat::Json => println!(
                "{}",
                serde_json::to_string_pretty(&data).unwrap_or_default()
            ),
            OutputFormat::Pretty => {
                let name = val_str(&data["name"]);
                println!("Strategy #{sid} — {name}\n");
                if let Some(groups) = data["groups"].as_array() {
                    for group in groups {
                        let gname = val_str(&group["group_name"]);
                        println!("  {gname}");
                        if let Some(indicators) = group["indicators"].as_array() {
                            let headers = ["id", "key", "name", "unit", "description"];
                            let rows: Vec<Vec<String>> = indicators
                                .iter()
                                .map(|ind| {
                                    vec![
                                        val_str(&ind["id"]),
                                        val_str(&ind["key"]),
                                        val_str(&ind["name"]),
                                        val_str(&ind["unit"]),
                                        val_str(&ind["description"]),
                                    ]
                                })
                                .collect();
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

pub async fn cmd_screener_search(
    strategy_id: Option<i64>,
    filter_args: &[String],
    market: &str,
    count: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    // When a strategy ID is given, fetch recommend list to get its groups (no extra request needed).
    let (mkt, filters, returns) = if let Some(sid) = strategy_id {
        let strategies = http_get("/v1/quote/screener/strategies/recommend", &[], verbose).await?;
        // Also try user strategies if not found in recommend
        let strategy_obj = strategies["screeners"]
            .as_array()
            .and_then(|arr| arr.iter().find(|s| s["id"].as_i64() == Some(sid)))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        // Fall back to fetching by ID if not in list
        let strategy = if strategy_obj.is_null() {
            let sid_str = sid.to_string();
            http_get(
                "/v1/quote/screener/strategy",
                &[("id", sid_str.as_str())],
                verbose,
            )
            .await?
        } else {
            strategy_obj
        };
        let mut mkt = market.to_uppercase();
        let mut filters: Vec<serde_json::Value> = Vec::new();
        let mut returns: Vec<String> = Vec::new();
        if let Some(groups) = strategy["groups"].as_array() {
            for group in groups {
                if let Some(indicators) = group["indicators"].as_array() {
                    for ind in indicators {
                        let key = val_str(&ind["key"]);
                        let id = ind["id"].as_i64().unwrap_or(0);
                        if id == -1 && key == "filter_market" {
                            // Market indicator — extract market value
                            let v = val_str(&ind["value"]);
                            if !v.is_empty() && v != "-" {
                                mkt = v;
                            }
                        } else {
                            let min = val_str(&ind["min"]);
                            let max = val_str(&ind["max"]);
                            let has_range =
                                (!min.is_empty() && min != "-") || (!max.is_empty() && max != "-");
                            if has_range || id > 0 {
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
        (mkt, filters, returns)
    } else {
        // Mode B: parse --filter key:min:max args
        let mut filters: Vec<serde_json::Value> = Vec::new();
        let mut returns: Vec<String> = Vec::new();
        for arg in filter_args {
            let parts: Vec<&str> = arg.splitn(3, ':').collect();
            let key = parts.first().copied().unwrap_or("").to_string();
            let min = parts.get(1).copied().unwrap_or("").to_string();
            let max = parts.get(2).copied().unwrap_or("").to_string();
            if key.is_empty() {
                continue;
            }
            filters.push(serde_json::json!({
                "key": key,
                "min": min,
                "max": max,
                "tech_values": {}
            }));
            returns.push(key);
        }
        (market.to_uppercase(), filters, returns)
    };

    let body = serde_json::json!({
        "market": mkt,
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
            let label = strategy_id.map_or_else(
                || format!("Custom filter ({mkt})"),
                |id| format!("Strategy #{id} ({mkt})"),
            );
            println!("{label} — {total} stocks found\n");
            let stocks = match data.get("items").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a,
                _ => {
                    println!("No stocks found matching the criteria.");
                    return Ok(());
                }
            };
            // Build column headers from returns keys
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
                            let display_v = match unit.as_str() {
                                "亿" => v
                                    .parse::<f64>()
                                    .map(|f| format!("{:.2}", f / 1e8))
                                    .unwrap_or(v),
                                "万" => v
                                    .parse::<f64>()
                                    .map(|f| format!("{:.2}", f / 1e4))
                                    .unwrap_or(v),
                                _ => v,
                            };
                            if unit.is_empty() || unit == "-" {
                                display_v
                            } else {
                                format!("{display_v} {unit}")
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
