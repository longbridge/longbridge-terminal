use anyhow::Result;
use serde_json::Value;

use super::{api::http_get, api::http_post, output::print_table, OutputFormat};
use crate::utils::counter::symbol_to_counter_id;
use crate::utils::number::format_financial_value;

const DEFAULT_RETURNS: &[&str] = &[
    "filter_prevclose",
    "filter_prevchg",
    "filter_marketcap",
    "filter_salesgrowthyoy",
    "filter_pettm",
    "filter_pbmrq",
    "filter_industry",
];

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
    let screeners = match data
        .get("strategys")
        .or_else(|| data.get("screeners"))
        .and_then(|v| v.as_array())
    {
        Some(a) if !a.is_empty() => a,
        _ => {
            match format {
                OutputFormat::Json => println!("[]"),
                OutputFormat::Pretty => println!("No strategies found."),
            }
            return Ok(());
        }
    };
    if matches!(format, OutputFormat::Json) {
        let items: Vec<serde_json::Value> = screeners
            .iter()
            .map(|s| {
                let type_str = match s["type"].as_i64() {
                    Some(1) => "user",
                    _ => "platform",
                };
                let id = s["id"]
                    .as_i64()
                    .unwrap_or_else(|| val_str(&s["id"]).parse::<i64>().unwrap_or(0));
                serde_json::json!({
                    "id": id,
                    "name": val_str(&s["name"]),
                    "type": type_str,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&items).unwrap_or_default()
        );
        return Ok(());
    }
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
    Ok(())
}

pub async fn cmd_screener_run(
    id: i64,
    sort: Option<&str>,
    order: &str,
    show: &[String],
    page: u32,
    count: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let path = format!("/v1/quote/ai/screener/strategy/{id}");
    let strategy = http_get(&path, &[], verbose).await?;
    let mkt = val_str(&strategy["market"]);
    let mkt = if mkt.is_empty() || mkt == "-" {
        "US".to_string()
    } else {
        mkt.to_uppercase()
    };

    let mut filters: Vec<serde_json::Value> = Vec::new();
    if let Some(f) = strategy["filter"]["filters"].as_array() {
        for ind in f {
            let key = val_str(&ind["key"]);
            if key.is_empty() || key == "-" {
                continue;
            }
            let min = val_str(&ind["min"]);
            let max = val_str(&ind["max"]);
            let tech_values = ind["tech_values"].clone();
            let tech_values = if tech_values.is_object() {
                tech_values
            } else {
                serde_json::json!({})
            };
            filters.push(serde_json::json!({
                "key": key,
                "min": min,
                "max": max,
                "tech_values": tech_values,
            }));
        }
    }
    let mut returns: Vec<String> = DEFAULT_RETURNS.iter().map(ToString::to_string).collect();
    for key in show {
        let full_key = normalize_key(key);
        if !returns.contains(&full_key) {
            returns.push(full_key);
        }
    }
    print_screener_results(
        id, &mkt, &filters, &returns, sort, order, page, count, format, verbose,
    )
    .await
}

pub async fn cmd_screener_filter(
    conditions: &[String],
    market: &str,
    sort: Option<&str>,
    order: &str,
    show: &[String],
    page: u32,
    count: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let mut filters: Vec<serde_json::Value> = Vec::new();
    for cond in conditions {
        let parts: Vec<&str> = cond.splitn(4, ':').collect();
        let raw_key = parts.first().copied().unwrap_or("");
        if raw_key.is_empty() {
            continue;
        }
        let key = normalize_key(raw_key);
        let min = parts.get(1).copied().unwrap_or("").to_string();
        let max = parts.get(2).copied().unwrap_or("").to_string();
        let tech_values: serde_json::Map<String, serde_json::Value> = parts
            .get(3)
            .map(|s| {
                s.split(',')
                    .filter_map(|kv| {
                        let mut it = kv.splitn(2, '=');
                        let k = it.next()?.trim();
                        let v = it.next()?.trim();
                        if k.is_empty() || v.is_empty() {
                            return None;
                        }
                        Some((k.to_string(), serde_json::Value::String(v.to_string())))
                    })
                    .collect()
            })
            .unwrap_or_default();
        filters.push(serde_json::json!({
            "key": key,
            "min": min,
            "max": max,
            "tech_values": tech_values,
        }));
    }
    let mut returns: Vec<String> = DEFAULT_RETURNS.iter().map(ToString::to_string).collect();
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
        page,
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
    page: u32,
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
        .unwrap_or(1);
    let sort_order: u8 = u8::from(order != "asc");
    let body = serde_json::json!({
        "market": market,
        "page": page,
        "size": count,
        "filters": filters,
        "returns": effective_returns,
        "sort_by": sort_by,
        "sort_order": sort_order,
        "industries": [],
    });
    let data = http_post("/v1/quote/ai/screener/search", body, verbose).await?;
    let total = data
        .get("total")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_else(|| {
            data["items"]
                .as_array()
                .map_or(0, |a| i64::try_from(a.len()).unwrap_or(0))
        });
    let stocks = match data.get("items").and_then(|v| v.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => {
            match format {
                OutputFormat::Json => println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "market": market, "total": total, "page": page, "items": []
                    }))
                    .unwrap_or_default()
                ),
                OutputFormat::Pretty => println!("No stocks found matching the criteria."),
            }
            return Ok(());
        }
    };
    match format {
        OutputFormat::Pretty => {
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
            let label = if strategy_id > 0 {
                format!("Strategy #{strategy_id} ({market})")
            } else {
                format!("Custom filter ({market})")
            };
            println!("{label} — {total} stocks found\n");
            print_table(&header_refs, rows, format);
        }
        OutputFormat::Json => {
            let items: Vec<serde_json::Value> = stocks
                .iter()
                .map(|s| {
                    let sym =
                        crate::utils::counter::counter_id_to_symbol(&val_str(&s["counter_id"]));
                    let mut map = serde_json::Map::new();
                    map.insert("symbol".to_string(), serde_json::Value::String(sym));
                    map.insert(
                        "name".to_string(),
                        serde_json::Value::String(val_str(&s["name"])),
                    );
                    if let Some(indicators) = s["indicators"].as_array() {
                        for ind in indicators {
                            let key = val_str(&ind["key"]).replace("filter_", "");
                            let raw = val_str(&ind["value"]);
                            let json_val = raw
                                .parse::<f64>()
                                .map(|n| serde_json::json!(n))
                                .unwrap_or(serde_json::Value::String(raw));
                            map.insert(key, json_val);
                        }
                    }
                    serde_json::Value::Object(map)
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "market": market,
                    "total": total,
                    "page": page,
                    "items": items,
                }))
                .unwrap_or_default()
            );
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
    let data = http_get("/v1/quote/ai/screener/indicators", &params, verbose).await?;
    let groups = match data.get("groups").and_then(|v| v.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => {
            match format {
                OutputFormat::Json => println!("[]"),
                OutputFormat::Pretty => println!("No indicator config found."),
            }
            return Ok(());
        }
    };
    let headers = ["ID", "Key", "Name", "Unit", "Min", "Max"];
    match format {
        OutputFormat::Json => {
            let items: Vec<serde_json::Value> = groups
                .iter()
                .flat_map(|group| {
                    group["indicators"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|ind| {
                            let min = val_str(&ind["default_range"]["min"]);
                            let max = val_str(&ind["default_range"]["max"]);
                            let mut obj = serde_json::json!({
                                "id": val_str(&ind["id"]),
                                "key": val_str(&ind["key"]).replace("filter_", ""),
                                "name": val_str(&ind["name"]),
                                "unit": val_str(&ind["unit"]),
                                "min": if min == "-" { serde_json::Value::Null } else { serde_json::Value::String(min) },
                                "max": if max == "-" { serde_json::Value::Null } else { serde_json::Value::String(max) },
                            });
                            if let Some(tech_inds) = ind["tech_indicators"].as_array() {
                                let tv: serde_json::Map<String, serde_json::Value> = tech_inds
                                    .iter()
                                    .map(|ti| {
                                        let key = val_str(&ti["tech_key"]);
                                        let opts: Vec<serde_json::Value> = ti["tech_items"]
                                            .as_array()
                                            .unwrap_or(&vec![])
                                            .iter()
                                            .map(|item| serde_json::json!({
                                                "value": val_str(&item["item_value"]),
                                                "label": val_str(&item["item_name"]),
                                            }))
                                            .collect();
                                        (key, serde_json::Value::Array(opts))
                                    })
                                    .collect();
                                if !tv.is_empty() {
                                    obj["tech_values"] = serde_json::Value::Object(tv);
                                }
                            }
                            obj
                        })
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&items).unwrap_or_default()
            );
        }
        OutputFormat::Pretty => {
            for group in groups {
                let gname = val_str(&group["group_name"]);
                println!("── {gname} ──");
                if let Some(indicators) = group["indicators"].as_array() {
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
