use anyhow::Result;
use longbridge::httpclient::Json;
use reqwest::Method;
use serde_json::Value;

use super::OutputFormat;

use crate::utils::counter::{counter_id_to_symbol, symbol_to_counter_id};
use crate::utils::datetime::format_date;
use crate::utils::number::format_financial_value;
use crate::utils::text::strip_html;

async fn http_get(path: &str, params: &[(&str, &str)], verbose: bool) -> Result<Value> {
    if verbose {
        let qs = params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        eprintln!("* GET {path}?{qs}");
    }
    let client = crate::openapi::http_client();
    let params: Vec<(&str, &str)> = params.to_vec();
    let resp = client
        .request(Method::GET, path)
        .query_params(params)
        .response::<Json<Value>>()
        .send()
        .await
        .map_err(anyhow::Error::from)?;
    Ok(resp.0)
}

fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
}

/// Print a JSON value as a human-readable table.
///
/// Objects are split into scalar rows (rendered as a key/value table) and
/// nested sections (printed with a heading and recursed into).  Arrays print
/// each element as a block separated by blank lines.
fn print_kv(value: &Value) {
    print_kv_section(value, 0);
}

fn print_kv_section(value: &Value, depth: usize) {
    let indent = "  ".repeat(depth);
    match value {
        Value::Object(map) => {
            let mut scalar_rows: Vec<Vec<String>> = Vec::new();
            let mut nested: Vec<(&String, &Value)> = Vec::new();

            for (k, v) in map {
                match v {
                    Value::Object(_) | Value::Array(_) => nested.push((k, v)),
                    _ => {
                        let v_str = match v {
                            Value::String(s) => s.clone(),
                            Value::Null => "-".to_string(),
                            other => other.to_string(),
                        };
                        scalar_rows.push(vec![k.clone(), v_str]);
                    }
                }
            }

            if !scalar_rows.is_empty() {
                super::output::print_table(&["key", "value"], scalar_rows, &OutputFormat::Pretty);
            }

            for (k, v) in nested {
                println!("\n{indent}{k}:");
                print_kv_section(v, depth + 1);
            }
        }
        Value::Array(arr) => {
            if let Some(headers) = uniform_object_keys(arr) {
                let rows: Vec<Vec<String>> = arr
                    .iter()
                    .map(|item| {
                        headers
                            .iter()
                            .map(|h| match &item[h.as_str()] {
                                Value::String(s) => s.clone(),
                                Value::Null => "-".to_string(),
                                other => other.to_string(),
                            })
                            .collect()
                    })
                    .collect();
                let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();
                super::output::print_table(&header_refs, rows, &OutputFormat::Pretty);
            } else {
                for (i, item) in arr.iter().enumerate() {
                    if i > 0 {
                        println!();
                    }
                    print_kv_section(item, depth);
                }
            }
        }
        other => println!("{indent}{other}"),
    }
}

/// Returns the ordered key list if every element of `arr` is an object with
/// the same set of keys; otherwise returns `None`.
fn uniform_object_keys(arr: &[Value]) -> Option<Vec<String>> {
    if arr.is_empty() {
        return None;
    }
    let first = arr[0].as_object()?;
    let key_set: std::collections::BTreeSet<&str> = first.keys().map(String::as_str).collect();
    for item in arr.iter().skip(1) {
        let obj = item.as_object()?;
        let keys: std::collections::BTreeSet<&str> = obj.keys().map(String::as_str).collect();
        if keys != key_set {
            return None;
        }
    }
    Some(first.keys().cloned().collect())
}

// ── financials ──────────────────────────────────────────────────────────────

/// Print financial statements as a transposed table: rows = metrics, columns = periods.
/// Limits to the 5 most recent periods for table-width sanity.
fn print_financials(value: &Value) {
    let Some(list) = value.get("list").and_then(|v| v.as_object()) else {
        print_kv(value);
        return;
    };

    for (kind, kind_data) in list {
        let indicators = match kind_data.get("indicators").and_then(|v| v.as_array()) {
            Some(i) if !i.is_empty() => i,
            _ => continue,
        };

        println!("── {kind} ──");

        // Collect column headers (periods) from the first account of the first indicator.
        let periods: Vec<String> = indicators[0]
            .get("accounts")
            .and_then(|a| a.as_array())
            .and_then(|a| a.first())
            .and_then(|acc| acc.get("values"))
            .and_then(|v| v.as_array())
            .map(|vals| {
                vals.iter()
                    .take(5)
                    .filter_map(|v| v.get("period")?.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();

        if periods.is_empty() {
            continue;
        }

        let period_refs: Vec<&str> = periods.iter().map(String::as_str).collect();
        let mut headers = vec!["metric"];
        headers.extend_from_slice(&period_refs);

        let mut rows: Vec<Vec<String>> = Vec::new();

        for indicator in indicators {
            let accounts = match indicator.get("accounts").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a,
                _ => continue,
            };
            for account in accounts {
                let name = account
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();

                let value_map: std::collections::HashMap<&str, &str> = account
                    .get("values")
                    .and_then(|v| v.as_array())
                    .map(|vals| {
                        vals.iter()
                            .filter_map(|v| {
                                Some((v.get("period")?.as_str()?, v.get("value")?.as_str()?))
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                let is_percent = account
                    .get("percent")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);

                let mut row = vec![name];
                for p in &periods {
                    let raw = value_map.get(p.as_str()).copied().unwrap_or("-");
                    row.push(format_financial_value(raw, is_percent));
                }
                rows.push(row);
            }
        }

        super::output::print_table(&headers, rows, &OutputFormat::Pretty);
    }
}

/// Fetch financial statements for a symbol.
pub async fn cmd_financial_report(
    symbol: String,
    kind: String,
    report: Option<String>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let mut params: Vec<(&str, &str)> = vec![("counter_id", cid.as_str()), ("kind", kind.as_str())];
    if let Some(ref r) = report {
        params.push(("report", r.as_str()));
    }
    let data = http_get("/v1/quote/financial-reports", &params, verbose).await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_financials(&data),
    }
    Ok(())
}

// ── analyst ─────────────────────────────────────────────────────────────────

fn val_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "-".to_string(),
        other => other.to_string(),
    }
}

fn print_institution_rating(ratings: &Value, instratings: &Value) {
    // Consensus: recommend / target price / change / updated_at
    {
        let change_raw = val_str(&instratings["change"]);
        let change = change_raw
            .parse::<f64>()
            .map(|f| format!("{f:.2}%"))
            .unwrap_or(change_raw);
        let target_raw = val_str(&instratings["target"]);
        let target = target_raw
            .parse::<f64>()
            .map(|f| format!("{f:.2}"))
            .unwrap_or(target_raw);
        let headers = ["recommend", "target", "change", "updated_at"];
        let row = vec![
            val_str(&instratings["recommend"]),
            target,
            change,
            val_str(&instratings["updated_at"]),
        ];
        println!("Consensus:");
        super::output::print_table(&headers, vec![row], &OutputFormat::Pretty);
    }

    // Rating breakdown: merge counts from both endpoints
    {
        let ie = &instratings["evaluate"];
        let re = &ratings["evaluate"];
        let headers = [
            "strong_buy",
            "buy",
            "hold",
            "sell",
            "under",
            "no_opinion",
            "total",
        ];
        let row = vec![
            val_str(&ie["strong_buy"]),
            val_str(&ie["buy"]),
            val_str(&ie["hold"]),
            val_str(&ie["sell"]),
            val_str(&ie["under"]),
            val_str(&re["no_opinion"]),
            val_str(&re["total"]),
        ];
        println!("\nRating breakdown:");
        super::output::print_table(&headers, vec![row], &OutputFormat::Pretty);
    }

    // Target price range
    {
        let t = &ratings["target"];
        let headers = ["lowest_price", "highest_price", "prev_close"];
        let row: Vec<String> = headers.iter().map(|k| val_str(&t[k])).collect();
        println!("\nTarget price range:");
        super::output::print_table(&headers, vec![row], &OutputFormat::Pretty);
    }

    // Industry comparison
    {
        let name = val_str(&ratings["industry_name"]);
        if name != "-" {
            let headers = ["industry", "rank", "mean", "median", "total"];
            let row = vec![
                name,
                val_str(&ratings["industry_rank"]),
                val_str(&ratings["industry_mean"]),
                val_str(&ratings["industry_median"]),
                val_str(&ratings["industry_total"]),
            ];
            println!("\nIndustry:");
            super::output::print_table(&headers, vec![row], &OutputFormat::Pretty);
        }
    }
}

const DETAIL_SKIP: &[&str] = &["timestamp"];

fn print_institution_rating_detail(data: &Value) {
    // evaluate.list — monthly rating history (fixed column order: date first)
    if let Some(list) = data["evaluate"]["list"].as_array() {
        if !list.is_empty() {
            println!("Rating history:");
            let ordered = ["date", "strong_buy", "buy", "hold", "sell", "under"];
            let rows: Vec<Vec<String>> = list
                .iter()
                .map(|item| ordered.iter().map(|h| val_str(&item[h])).collect())
                .collect();
            super::output::print_table(&ordered, rows, &OutputFormat::Pretty);
        }
    }

    // target metadata
    let t = &data["target"];
    {
        let accuracy_raw = val_str(&t["prediction_accuracy"]);
        let accuracy = accuracy_raw
            .parse::<f64>()
            .map(|f| format!("{f:.2}%"))
            .unwrap_or(accuracy_raw);
        let data_pct_raw = val_str(&t["data_percent"]);
        let data_pct = data_pct_raw
            .parse::<f64>()
            .map(|f| format!("{:.2}%", f * 100.0))
            .unwrap_or(data_pct_raw);
        let headers = ["data_coverage", "prediction_accuracy", "updated_at"];
        let row = vec![data_pct, accuracy, val_str(&t["updated_at"])];
        println!("\nTarget accuracy:");
        super::output::print_table(&headers, vec![row], &OutputFormat::Pretty);
    }

    // target.list — weekly price target history, skip raw timestamp
    if let Some(list) = t["list"].as_array() {
        if !list.is_empty() {
            println!("\nTarget price history:");
            if let Some(all_headers) = uniform_object_keys(list) {
                let headers: Vec<String> = all_headers
                    .into_iter()
                    .filter(|k| !DETAIL_SKIP.contains(&k.as_str()))
                    .collect();
                let rows = list
                    .iter()
                    .map(|item| headers.iter().map(|h| val_str(&item[h.as_str()])).collect())
                    .collect();
                let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();
                super::output::print_table(&header_refs, rows, &OutputFormat::Pretty);
            }
        }
    }
}

/// Fetch institution rating distribution + current target price summary.
pub async fn cmd_institution_rating(
    symbol: String,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let ratings = http_get(
        "/v1/quote/institution-rating-latest",
        &[("counter_id", cid.as_str())],
        verbose,
    )
    .await?;
    let instratings = http_get(
        "/v1/quote/institution-ratings",
        &[("counter_id", cid.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&serde_json::json!({
            "analyst": ratings,
            "instratings": instratings,
        })),
        OutputFormat::Pretty => print_institution_rating(&ratings, &instratings),
    }
    Ok(())
}

/// Fetch historical institution rating and target price detail.
pub async fn cmd_institution_rating_detail(
    symbol: String,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/quote/institution-ratings/detail",
        &[("counter_id", cid.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_institution_rating_detail(&data),
    }
    Ok(())
}

// ── dividends ───────────────────────────────────────────────────────────────

const DIVIDENDS_SKIP: &[&str] = &["counter_id", "id", "dividend_summary"];

fn print_dividends(value: &Value) {
    let items = match value.get("list").and_then(|v| v.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => {
            println!("No dividend records found.");
            return;
        }
    };

    let headers: Vec<String> = items[0]
        .as_object()
        .map(|m| {
            m.keys()
                .filter(|k| !DIVIDENDS_SKIP.contains(&k.as_str()))
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();
    let mut seen = std::collections::HashSet::new();
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            headers
                .iter()
                .map(|h| match item.get(h) {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Null) | None => "-".to_owned(),
                    Some(other) => other.to_string(),
                })
                .collect::<Vec<_>>()
        })
        .filter(|row| seen.insert(row.clone()))
        .collect();

    super::output::print_table(&header_refs, rows, &OutputFormat::Pretty);
}

/// Fetch dividend history for a symbol.
pub async fn cmd_dividend(
    symbol: String,
    page: u32,
    year: Option<u32>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let page_str = page.to_string();
    let year_str = year.map(|y| y.to_string());
    let mut params = vec![
        ("counter_id", cid.as_str()),
        ("size", "50"),
        ("page", &page_str),
    ];
    if let Some(ref y) = year_str {
        params.push(("year", y.as_str()));
    }
    let data = http_get("/v1/quote/dividends", &params, verbose).await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_dividends(&data),
    }
    Ok(())
}

// ── estimates ───────────────────────────────────────────────────────────────

/// EPS forecast history — most recent 20 snapshots with formatted dates.
fn print_forecast_eps(data: &Value) {
    let all_items = match data["items"].as_array() {
        Some(a) if !a.is_empty() => a,
        _ => {
            println!("No forecast data.");
            return;
        }
    };
    let start = all_items.len().saturating_sub(20);
    let items = &all_items[start..];
    let headers = [
        "end_date", "mean", "median", "highest", "lowest", "up", "down", "total",
    ];
    let rows: Vec<Vec<String>> = items
        .iter()
        .filter_map(|item| {
            let ts = item["forecast_end_date"]
                .as_str()
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);
            if ts == 0 {
                return None;
            }
            Some(vec![
                format_date(ts),
                val_str(&item["forecast_eps_mean"]),
                val_str(&item["forecast_eps_median"]),
                val_str(&item["forecast_eps_highest"]),
                val_str(&item["forecast_eps_lowest"]),
                item["institution_up"].to_string(),
                item["institution_down"].to_string(),
                item["institution_total"].to_string(),
            ])
        })
        .collect();
    println!("EPS Forecasts (recent {}):", rows.len());
    super::output::print_table(&headers, rows, &OutputFormat::Pretty);
}

/// Consensus estimates — rows = metrics, columns = periods.
/// Released values marked ↑ (beat) or ↓ (miss); unreleased prefixed with ~.
fn print_consensus(data: &Value) {
    let periods = match data["list"].as_array() {
        Some(a) if !a.is_empty() => a,
        _ => {
            println!("No consensus data.");
            return;
        }
    };
    let period_texts: Vec<String> = periods.iter().map(|p| val_str(&p["period_text"])).collect();
    let mut headers = vec!["metric".to_owned()];
    headers.extend(period_texts.iter().cloned());
    let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();

    let metric_keys = [
        "revenue",
        "ebit",
        "net_income",
        "normalized_net_income",
        "eps",
        "normalized_eps",
    ];
    let first_details = periods[0]["details"].as_array();
    let mut rows: Vec<Vec<String>> = Vec::new();

    for key in &metric_keys {
        let name = first_details
            .and_then(|d| d.iter().find(|m| m["key"].as_str() == Some(key)))
            .and_then(|m| m["name"].as_str())
            .unwrap_or(key)
            .to_owned();
        let mut row = vec![name];
        for period in periods {
            let details = period["details"].as_array();
            let metric = details.and_then(|d| d.iter().find(|m| m["key"].as_str() == Some(key)));
            let cell = match metric {
                Some(m) => {
                    let is_released = m["is_released"].as_bool().unwrap_or(false);
                    if is_released {
                        let actual = val_str(&m["actual"]);
                        let v = format_financial_value(&actual, false);
                        match val_str(&m["comp"]).as_str() {
                            "beat_est" => format!("{v} ↑"),
                            "miss_est" => format!("{v} ↓"),
                            _ => v,
                        }
                    } else {
                        let est = val_str(&m["estimate"]);
                        format!("~{}", format_financial_value(&est, false))
                    }
                }
                None => "-".to_owned(),
            };
            row.push(cell);
        }
        rows.push(row);
    }

    println!(
        "Currency: {} | Period: {}",
        val_str(&data["currency"]),
        val_str(&data["current_period"])
    );
    super::output::print_table(&header_refs, rows, &OutputFormat::Pretty);
}

/// Fetch EPS forecasts and analyst consensus estimates.
pub async fn cmd_forecast_eps(symbol: String, format: &OutputFormat, verbose: bool) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/quote/forecast-eps",
        &[("counter_id", cid.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_forecast_eps(&data),
    }
    Ok(())
}

/// Fetch financial consensus detail.
pub async fn cmd_consensus(symbol: String, format: &OutputFormat, verbose: bool) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/quote/financial-consensus-detail",
        &[("counter_id", cid.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_consensus(&data),
    }
    Ok(())
}

// ── valuation ───────────────────────────────────────────────────────────────

/// Valuation detail — overview row + peer comparison table.
fn print_valuation_detail(data: &Value) {
    let overview = &data["overview"];
    let indicator = val_str(&overview["indicator"]);
    if indicator == "-" || indicator.is_empty() {
        print_kv(data);
        return;
    }
    let ind = indicator.as_str();

    // Overview: current value vs historical range and industry median
    {
        let ov_m = &overview["metrics"][ind];
        let hist_m = &data["history"]["metrics"][ind];
        let desc_raw = val_str(&ov_m["desc"]);
        let desc = if desc_raw == "-" {
            String::new()
        } else {
            strip_html(&desc_raw)
        };
        let headers = [
            "indicator",
            "current",
            "high",
            "low",
            "median",
            "industry_median",
            "date",
        ];
        let row = vec![
            indicator.to_uppercase(),
            val_str(&ov_m["metric"]),
            val_str(&hist_m["high"]),
            val_str(&hist_m["low"]),
            val_str(&hist_m["median"]),
            val_str(&ov_m["industry_median"]),
            val_str(&overview["date"]),
        ];
        println!("Overview:");
        super::output::print_table(&headers, vec![row], &OutputFormat::Pretty);
        if !desc.is_empty() {
            println!("  {desc}");
        }
    }

    // Peers: up to 10 comparable stocks
    if let Some(peers) = data["peers"][ind]["list"].as_array() {
        if !peers.is_empty() {
            let rows: Vec<Vec<String>> = peers
                .iter()
                .take(10)
                .map(|p| {
                    let v_raw = val_str(&p["value"]);
                    let v = v_raw
                        .parse::<f64>()
                        .map(|f| format!("{f:.2}"))
                        .unwrap_or(v_raw);
                    vec![val_str(&p["name"]), v]
                })
                .collect();
            println!("\nPeers ({}):", rows.len());
            super::output::print_table(&["name", ind], rows, &OutputFormat::Pretty);
        }
    }
}

/// Print valuation history: summary row + recent time-series values.
fn print_valuation_history(data: &Value) {
    let metrics = match data["metrics"].as_object() {
        Some(m) if !m.is_empty() => m,
        _ => {
            println!("No valuation data.");
            return;
        }
    };
    for (key, m) in metrics {
        let desc_raw = val_str(&m["desc"]);
        let desc = if desc_raw == "-" {
            String::new()
        } else {
            strip_html(&desc_raw)
        };
        let headers = ["indicator", "high", "low", "median"];
        let row = vec![
            key.to_uppercase(),
            val_str(&m["high"]),
            val_str(&m["low"]),
            val_str(&m["median"]),
        ];
        println!("{}:", key.to_uppercase());
        super::output::print_table(&headers, vec![row], &OutputFormat::Pretty);
        if !desc.is_empty() {
            println!("  {desc}");
        }

        if let Some(list) = m["list"].as_array() {
            if !list.is_empty() {
                let rows: Vec<Vec<String>> = list
                    .iter()
                    .filter_map(|v| {
                        let ts = val_str(&v["timestamp"]).parse::<i64>().ok()?;
                        Some(vec![format_date(ts), val_str(&v["value"])])
                    })
                    .collect();
                println!();
                super::output::print_table(&["date", "value"], rows, &OutputFormat::Pretty);
            }
        }
    }
}

/// Fetch valuation overview: P/E, P/B, P/S, dividend yield + peer comparison.
pub async fn cmd_valuation(
    symbol: String,
    indicator: Option<String>,
    range: Option<String>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let ind = indicator.as_deref().unwrap_or("pe");
    let range_val = range.as_deref().unwrap_or("1");
    let params: Vec<(&str, &str)> = vec![
        ("counter_id", cid.as_str()),
        ("indicator", ind),
        ("range", range_val),
    ];
    let data = http_get("/v1/quote/valuation", &params, verbose).await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            let has_data = data["metrics"].as_object().is_some_and(|m| {
                m.values()
                    .any(|v| !val_str(&v["median"]).is_empty() && val_str(&v["median"]) != "-")
            });
            if has_data {
                print_valuation_history(&data);
            } else {
                println!("No valuation data.");
            }
        }
    }
    Ok(())
}

/// Fetch detailed valuation analysis, optionally focused on one indicator.
pub async fn cmd_valuation_detail(
    symbol: String,
    indicator: Option<String>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let mut params: Vec<(&str, &str)> = vec![("counter_id", cid.as_str())];
    if let Some(ref ind) = indicator {
        params.push(("indicator", ind.as_str()));
    }
    let data = http_get("/v1/quote/valuation/detail", &params, verbose).await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_valuation_detail(&data),
    }
    Ok(())
}

// ── dividend detail ──────────────────────────────────────────────────────────

fn print_dividend_detail(data: &Value) {
    let items = match data.get("list").and_then(|v| v.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => {
            println!("No dividend detail records found.");
            return;
        }
    };
    let headers = ["desc", "ex_date", "payment_date", "record_date"];
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            let raw = val_str(&item["desc"]).replace('\n', " ");
            let desc = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            headers[1..].iter().fold(vec![desc], |mut row, h| {
                row.push(val_str(&item[*h]));
                row
            })
        })
        .collect();
    super::output::print_table(&headers, rows, &OutputFormat::Pretty);
}

/// Fetch dividend distribution scheme details.
pub async fn cmd_dividend_detail(
    symbol: String,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/quote/dividends/details",
        &[("counter_id", cid.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_dividend_detail(&data),
    }
    Ok(())
}

// ── fund holders ─────────────────────────────────────────────────────────────

fn print_fund_holders(data: &Value) {
    let items = match data.get("lists").and_then(|v| v.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => {
            println!("No fund holder records found.");
            return;
        }
    };
    let headers = ["name", "symbol", "currency", "weight", "report_date"];
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            let symbol = counter_id_to_symbol(&val_str(&item["counter_id"]));
            let weight_raw = val_str(&item["position_ratio"]);
            let weight = weight_raw
                .parse::<f64>()
                .map(|f| format!("{f:.2}%"))
                .unwrap_or(weight_raw);
            vec![
                val_str(&item["name"]),
                symbol,
                val_str(&item["currency"]),
                weight,
                val_str(&item["report_date"]),
            ]
        })
        .collect();
    super::output::print_table(&headers, rows, &OutputFormat::Pretty);
}

// ── shareholders ─────────────────────────────────────────────────────────────

fn print_shareholders(data: &Value) {
    let items = match data.get("shareholder_list").and_then(|v| v.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => {
            println!("No shareholder records found.");
            return;
        }
    };

    let total = data["total"].as_i64().unwrap_or(0);
    println!("Total shareholders: {total}\n");

    let headers = [
        "shareholder",
        "symbol",
        "% shares",
        "chg shares",
        "report_date",
    ];
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            // Related public stock symbol (institution may itself be listed)
            let symbol = item["stocks"]
                .as_array()
                .and_then(|s| s.first())
                .map_or_else(
                    || "-".to_string(),
                    |s| counter_id_to_symbol(&val_str(&s["counter_id"])),
                );

            let pct_raw = val_str(&item["percent_of_shares"]);
            let pct = pct_raw
                .parse::<f64>()
                .map(|f| format!("{f:.2}%"))
                .unwrap_or(pct_raw);

            let chg_raw = val_str(&item["shares_changed"]);
            let chg = chg_raw
                .parse::<f64>()
                .map(|f| {
                    let sign = if f > 0.0 { "+" } else { "" };
                    format!("{sign}{}", format_financial_value(&f.to_string(), false))
                })
                .unwrap_or(chg_raw);

            vec![
                val_str(&item["shareholder_name"]),
                symbol,
                pct,
                chg,
                val_str(&item["report_date"]),
            ]
        })
        .collect();
    super::output::print_table(&headers, rows, &OutputFormat::Pretty);
}

/// Fetch institutional shareholders for a symbol.
pub async fn cmd_shareholders(
    symbol: String,
    range: String,
    sort_field: String,
    sort_order: String,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/quote/shareholders",
        &[
            ("counter_id", cid.as_str()),
            ("position", "entry"),
            ("range", range.as_str()),
            ("sort_field", sort_field.as_str()),
            ("sort_order", sort_order.as_str()),
        ],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_shareholders(&data),
    }
    Ok(())
}

/// Fetch funds and ETFs that hold a given symbol.
pub async fn cmd_fund_holders(
    symbol: String,
    count: i32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let limit = count.to_string();
    let data = http_get(
        "/v1/quote/fund-holders",
        &[("counter_id", cid.as_str()), ("limit", limit.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_fund_holders(&data),
    }
    Ok(())
}

fn finance_calendar_type_label(t: &str) -> &'static str {
    match t {
        "earning" => "Earnings",
        "financial" => "Financials",
        "report" => "Preview",
        "dividend" => "Dividend",
        "ipo" => "IPO",
        "meeting" => "Meeting",
        "macrodata" => "Macro",
        "split" => "Split",
        "merge" => "Merge",
        "closed" => "Closed",
        _ => "Event",
    }
}

fn print_finance_calendar(payload: &Value) {
    let empty = vec![];
    let list = payload["list"].as_array().unwrap_or(&empty);

    for group in list {
        let group_date = val_str(&group["date"]);
        let infos = group["infos"].as_array().unwrap_or(&empty);
        for info in infos {
            // For macrodata, info["date"] is a time string (e.g. "07:50"); combine with group date.
            // For other types, info["date"] is a full display date or empty (fall back to group date).
            let info_date = val_str(&info["date"]);
            let event_date = if info_date.is_empty() {
                group_date.clone()
            } else if info_date.len() <= 5 {
                // looks like HH:MM — prepend the group date
                format!("{group_date} {info_date}")
            } else {
                info_date
            };

            let event_type = info["type"].as_str().unwrap_or("");
            let type_label = finance_calendar_type_label(event_type);
            let content = val_str(&info["content"]);
            let name = val_str(&info["counter_name"]);
            let symbol = counter_id_to_symbol(info["counter_id"].as_str().unwrap_or(""));
            let market = val_str(&info["market"]);
            let date_type = val_str(&info["date_type"]);
            let star = info["star"].as_u64().unwrap_or(0);

            let mut header = format!("{event_date}  [{type_label}]");
            if !date_type.is_empty() {
                header.push_str("  ");
                header.push_str(&date_type);
            }
            if event_type == "macrodata" && star > 0 {
                let stars: String = (1u64..=3)
                    .map(|i| if i <= star { '★' } else { '☆' })
                    .collect();
                header.push_str("  ");
                header.push_str(&stars);
            }
            if !market.is_empty() {
                header.push_str("  ");
                header.push_str(&market);
            }
            if !name.is_empty() {
                header.push_str("  ");
                header.push_str(&name);
                header.push_str(" (");
                header.push_str(&symbol);
                header.push(')');
            }
            println!("{header}");
            println!("  {content}");

            let kv = info["data_kv"].as_array().unwrap_or(&empty);
            if !kv.is_empty() {
                let find_kv = |type_key: &str| -> String {
                    kv.iter()
                        .find(|e| e["type"].as_str() == Some(type_key))
                        .map(|e| val_str(&e["value"]))
                        .unwrap_or_default()
                };
                let kv_label = |type_key: &str, fallback: &str| -> String {
                    kv.iter()
                        .find(|e| e["type"].as_str() == Some(type_key))
                        .and_then(|e| e["key"].as_str())
                        .filter(|s| !s.is_empty())
                        .map_or_else(|| fallback.to_string(), ToString::to_string)
                };
                // Financial events: EPS / Revenue
                let est_eps = find_kv("estimate_eps");
                let act_eps = find_kv("actual_eps");
                if !est_eps.is_empty() || !act_eps.is_empty() {
                    let est_rev = find_kv("estimate_revenue");
                    let act_rev = find_kv("actual_revenue");
                    println!("  EPS: Est {est_eps} / Act {act_eps}  |  Revenue: Est {est_rev} / Act {act_rev}");
                }
                // Macro events: use API-provided key labels to avoid hardcoded strings
                let prev = find_kv("previous");
                let est = find_kv("estimate");
                let act = find_kv("actual");
                if !prev.is_empty() || !est.is_empty() || !act.is_empty() {
                    let prev_label = kv_label("previous", "Previous");
                    let est_label = kv_label("estimate", "Estimate");
                    let act_label = kv_label("actual", "Actual");
                    println!("  {prev_label}: {prev}  {est_label}: {est}  {act_label}: {act}");
                }
            }
            println!();
        }
    }
}

async fn finance_calendar_request(
    types: &[&str],
    cids: &[String],
    market: Option<&str>,
    start: &str,
    end: Option<&str>,
    count: u32,
    star: &[u32],
    next: &str,
    offset: u32,
    verbose: bool,
) -> Result<serde_json::Value> {
    let count_str = count.to_string();
    let offset_str = offset.to_string();
    let star_strs: Vec<String> = star.iter().map(ToString::to_string).collect();

    let mut params: Vec<(&str, &str)> = vec![
        ("date", start),
        ("count", count_str.as_str()),
        ("offset", offset_str.as_str()),
        ("next", next),
    ];
    for t in types {
        params.push(("types[]", t));
    }
    for c in cids {
        params.push(("counter_ids[]", c.as_str()));
    }
    if let Some(m) = market {
        params.push(("markets[]", m));
    }
    for s in &star_strs {
        params.push(("star[]", s.as_str()));
    }
    if let Some(end) = end {
        params.push(("date_end", end));
    }

    super::api::http_get("/v1/quote/finance_calendar", &params, verbose).await
}

fn merge_finance_calendar_responses(responses: Vec<serde_json::Value>) -> serde_json::Value {
    use std::collections::{BTreeMap, HashMap};
    let empty = vec![];
    let mut groups: BTreeMap<String, HashMap<String, serde_json::Value>> = BTreeMap::new();
    let first_date = responses
        .first()
        .and_then(|r| r["date"].as_str())
        .unwrap_or("")
        .to_string();

    for resp in &responses {
        for group in resp["list"].as_array().unwrap_or(&empty) {
            let date = group["date"].as_str().unwrap_or("").to_string();
            let infos = group["infos"].as_array().unwrap_or(&empty);
            let bucket = groups.entry(date).or_default();
            for info in infos {
                // Use id as dedup key; fall back to datetime+market for id-less events (e.g. closed)
                let key = if let Some(id) = info["id"].as_str().filter(|s| !s.is_empty()) {
                    id.to_string()
                } else {
                    format!(
                        "{}_{}",
                        info["datetime"].as_str().unwrap_or(""),
                        info["market"].as_str().unwrap_or("")
                    )
                };
                bucket.insert(key, info.clone());
            }
        }
    }

    let list: Vec<serde_json::Value> = groups
        .into_iter()
        .map(|(date, infos_map)| {
            let mut infos: Vec<serde_json::Value> = infos_map.into_values().collect();
            infos.sort_by_key(|i| {
                i["datetime"]
                    .as_str()
                    .unwrap_or("")
                    .parse::<u64>()
                    .unwrap_or(0)
            });
            serde_json::json!({ "date": date, "infos": infos })
        })
        .collect();

    serde_json::json!({ "date": first_date, "list": list, "next_date": "", "result": {} })
}

/// Fetch finance calendar events (V2). Optionally filter by symbols, source, market, and star level.
#[allow(clippy::too_many_arguments)]
pub async fn cmd_finance_calendar(
    event_type: String,
    symbols: Vec<String>,
    filter: Option<String>,
    market: Option<String>,
    start: Option<String>,
    end: Option<String>,
    count: u32,
    star: Vec<u32>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let today = time::OffsetDateTime::now_utc().date();
    // Historical types (financial, report) default to 90 days ago; forward-looking types default to today.
    let is_historical = matches!(event_type.as_str(), "financial" | "report");
    let start = start.unwrap_or_else(|| {
        if !symbols.is_empty() || filter.is_some() || is_historical {
            format!("{}", today.saturating_sub(time::Duration::days(90)))
        } else {
            format!("{today}")
        }
    });

    // V2 rule: "report" includes "financial"; "split" includes "merge" (matches app tab behavior)
    let mut types: Vec<&str> = vec![event_type.as_str()];
    if types == ["report"] {
        types.push("financial");
    }
    if types == ["split"] {
        types.push("merge");
    }

    // Resolve symbols from source (watchlist or positions)
    let mut all_symbols = symbols;
    if let Some(ref src) = filter {
        match src.as_str() {
            "watchlist" => {
                let ctx = crate::openapi::quote();
                let groups = ctx.watchlist().await?;
                let mut seen = std::collections::HashSet::new();
                for group in groups {
                    for sec in &group.securities {
                        if seen.insert(sec.symbol.clone()) {
                            all_symbols.push(sec.symbol.clone());
                        }
                    }
                }
            }
            "positions" => {
                let ctx = crate::openapi::trade();
                let resp = ctx.stock_positions(None).await?;
                for channel in &resp.channels {
                    for pos in &channel.positions {
                        all_symbols.push(pos.symbol.clone());
                    }
                }
            }
            other => anyhow::bail!("unknown source '{other}'; use watchlist or positions"),
        }
    }

    let cids: Vec<String> = all_symbols
        .iter()
        .map(|s| symbol_to_counter_id(s))
        .collect();

    let market_ref = market.as_deref();
    let end_ref = end.as_deref();

    // Follow next_date pagination until count events collected or no more pages (max 20 pages).
    let fetch_all_pages = |cids: Vec<String>| {
        let types = types.clone();
        let start = start.clone();
        let star = star.clone();
        async move {
            let mut responses: Vec<serde_json::Value> = Vec::new();
            let mut current_date = start;
            let mut total_events = 0u32;
            for _ in 0..20u32 {
                let r = finance_calendar_request(
                    &types,
                    &cids,
                    market_ref,
                    &current_date,
                    end_ref,
                    count,
                    &star,
                    "later",
                    0,
                    verbose,
                )
                .await?;
                let empty = vec![];
                let page_events: u32 = r["list"]
                    .as_array()
                    .unwrap_or(&empty)
                    .iter()
                    .map(|g| g["infos"].as_array().unwrap_or(&empty).len() as u32)
                    .sum();
                total_events += page_events;
                let next_date = r["next_date"].as_str().unwrap_or("").to_string();
                responses.push(r);
                if next_date.is_empty() || total_events >= count {
                    break;
                }
                current_date = next_date;
            }
            if responses.len() == 1 {
                Ok::<_, anyhow::Error>(responses.remove(0))
            } else {
                Ok(merge_finance_calendar_responses(responses))
            }
        }
    };

    let resp = if cids.len() <= 10 {
        fetch_all_pages(cids).await?
    } else {
        let mut responses = Vec::new();
        for batch in cids.chunks(10) {
            let r = fetch_all_pages(batch.to_vec()).await?;
            responses.push(r);
        }
        merge_finance_calendar_responses(responses)
    };

    match format {
        OutputFormat::Json => print_json(&resp),
        OutputFormat::Pretty => print_finance_calendar(&resp),
    }
    Ok(())
}

// ── Pending commands ─────────────────────────────────────────────────────────

pub async fn cmd_company(symbol: String, format: &OutputFormat, verbose: bool) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/quote/comp-overview",
        &[("counter_id", cid.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_company(&data),
    }
    Ok(())
}

fn print_company(data: &Value) {
    let fields = [
        ("Name", "name"),
        ("Founded", "founded"),
        ("Listing Date", "listing_date"),
        ("Market", "market"),
        ("Category", "category"),
        ("CEO", "manager"),
        ("Chairman", "chairman"),
        ("Employees", "employees"),
        ("Address", "address"),
        ("Website", "website"),
        ("Phone", "Phone"),
        ("Email", "email"),
        ("IPO Price", "issue_price"),
        ("Year End", "year_end"),
        ("Audit", "audit_inst"),
        ("ADS Ratio", "ads_ratio"),
    ];
    for (label, key) in fields {
        let v = val_str(&data[key]);
        if !v.is_empty() && v != "-" {
            println!("{label:15} {v}");
        }
    }
    let profile = val_str(&data["profile"]);
    if !profile.is_empty() && profile != "-" {
        println!();
        println!("{profile}");
    }
}

pub async fn cmd_executive(symbol: String, format: &OutputFormat, verbose: bool) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/quote/company-professionals",
        &[("counter_ids", cid.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_executives(&data),
    }
    Ok(())
}

fn print_executives(data: &Value) {
    let lists = match data.get("professional_list").and_then(|v| v.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => {
            println!("No executive data found.");
            return;
        }
    };
    for entry in lists {
        let professionals = match entry.get("professionals").and_then(|v| v.as_array()) {
            Some(a) if !a.is_empty() => a,
            _ => continue,
        };
        let headers = ["name", "title"];
        let rows: Vec<Vec<String>> = professionals
            .iter()
            .map(|p| vec![val_str(&p["name"]), val_str(&p["title"])])
            .collect();
        super::output::print_table(&headers, rows, &OutputFormat::Pretty);
    }
}

pub async fn cmd_buyback(symbol: String, format: &OutputFormat, verbose: bool) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/quote/buy-backs",
        &[("counter_id", cid.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_buyback(&data),
    }
    Ok(())
}

fn fmt_amount(raw: &str, currency: &str) -> String {
    let v: f64 = raw.parse().unwrap_or(0.0);
    if v == 0.0 {
        return "-".to_string();
    }
    let (val, unit) = if v.abs() >= 1e12 {
        (v / 1e12, "T")
    } else if v.abs() >= 1e8 {
        (v / 1e8, "B")
    } else if v.abs() >= 1e6 {
        (v / 1e6, "M")
    } else {
        (v, "")
    };
    let cur = if currency.is_empty() { "" } else { currency };
    format!("{cur}{val:.2}{unit}")
}

fn fmt_ratio(raw: &str) -> String {
    raw.parse::<f64>()
        .map_or_else(|_| raw.to_string(), |v| format!("{v:.2}"))
}

fn fmt_pct(raw: &str) -> String {
    raw.parse::<f64>()
        .map_or_else(|_| raw.to_string(), |v| format!("{v:.2}%"))
}

fn print_buyback(data: &Value) {
    // Recent buyback summary
    if let Some(recent) = data.get("recent_buybacks") {
        let currency = val_str(&recent["currency"]);
        let cur = if currency.is_empty() || currency == "-" {
            String::new()
        } else {
            currency
        };
        println!("Recent Buyback (TTM)");
        println!(
            "  Net Buyback:       {}",
            fmt_amount(&val_str(&recent["net_buyback_ttm"]), &cur)
        );
        println!(
            "  Net Buyback Yield: {}",
            val_str(&recent["net_buyback_yield_ttm"])
        );
        println!();
    }

    // Buyback history
    let history = data.get("buyback_history").and_then(|v| v.as_array());
    let ratios = data.get("buyback_ratios").and_then(|v| v.as_array());

    let items = match history {
        Some(a) if !a.is_empty() => a,
        _ => {
            println!("No buyback history found.");
            return;
        }
    };

    let headers = [
        "fiscal_year",
        "range",
        "net_buyback",
        "yield",
        "yoy_growth",
        "payout_ratio",
        "cf_ratio",
    ];
    let rows: Vec<Vec<String>> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let currency = val_str(&item["currency"]);
            let cur = if currency.is_empty() || currency == "-" {
                String::new()
            } else {
                currency
            };
            let ratio_item = ratios.and_then(|r| r.get(i));
            let payout = ratio_item.map_or_else(
                || "-".to_string(),
                |r| val_str(&r["net_buyback_payout_ratio"]),
            );
            let cf = ratio_item.map_or_else(
                || "-".to_string(),
                |r| val_str(&r["net_buyback_to_cashflow_ratio"]),
            );
            vec![
                val_str(&item["fiscal_year"]),
                val_str(&item["fiscal_year_range"]),
                fmt_amount(&val_str(&item["net_buyback"]), &cur),
                val_str(&item["net_buyback_yield"]),
                val_str(&item["net_buyback_growth_rate"]),
                payout,
                cf,
            ]
        })
        .collect();
    super::output::print_table(&headers, rows, &OutputFormat::Pretty);
}

pub async fn cmd_industry_valuation(
    symbol: String,
    currency: &str,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/quote/industry-valuation-comparison",
        &[("counter_id", cid.as_str()), ("currency", currency)],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            let items = match data.get("list").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a,
                _ => {
                    println!("No industry valuation data found.");
                    return Ok(());
                }
            };
            let cur = items
                .first()
                .map(|i| val_str(&i["currency"]))
                .unwrap_or_default();
            let headers = [
                "symbol",
                "name",
                "market_cap",
                "price",
                "pe",
                "pb",
                "eps",
                "div_yld",
            ];
            let rows: Vec<Vec<String>> = items
                .iter()
                .map(|item| {
                    let item_cur = val_str(&item["currency"]);
                    let c = if item_cur.is_empty() || item_cur == "-" {
                        &cur
                    } else {
                        &item_cur
                    };
                    vec![
                        counter_id_to_symbol(&val_str(&item["counter_id"])),
                        val_str(&item["name"]),
                        fmt_amount(&val_str(&item["market_value"]), c),
                        format!("{c}{}", val_str(&item["price_close"])),
                        format!("{}x", fmt_ratio(&val_str(&item["pe"]))),
                        format!("{}x", fmt_ratio(&val_str(&item["pb"]))),
                        format!("{c}{}", fmt_ratio(&val_str(&item["eps"]))),
                        fmt_pct(&val_str(&item["div_yld"])),
                    ]
                })
                .collect();
            super::output::print_table(&headers, rows, &OutputFormat::Pretty);
        }
    }
    Ok(())
}

pub async fn cmd_industry_valuation_dist(
    symbol: String,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/quote/industry-valuation-distribution",
        &[("counter_id", cid.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            let metrics = [("PE", "pe"), ("PB", "pb"), ("PS", "ps")];
            let mut found = false;
            let headers = [
                "metric",
                "current",
                "low",
                "median",
                "high",
                "rank",
                "percentile",
            ];
            let mut rows: Vec<Vec<String>> = Vec::new();
            for (label, key) in metrics {
                if let Some(m) = data.get(key) {
                    found = true;
                    let rank_idx = val_str(&m["rank_index"]);
                    let rank_total = val_str(&m["rank_total"]);
                    let rank = if rank_idx != "-" && rank_total != "-" {
                        format!("{rank_idx}/{rank_total}")
                    } else {
                        "-".to_string()
                    };
                    let ranking = val_str(&m["ranking"]);
                    let pct = ranking
                        .parse::<f64>()
                        .map(|v| format!("{:.1}%", v * 100.0))
                        .unwrap_or(ranking);
                    let suffix = "x";
                    rows.push(vec![
                        label.to_string(),
                        format!("{}{suffix}", fmt_ratio(&val_str(&m["value"]))),
                        format!("{}{suffix}", fmt_ratio(&val_str(&m["low"]))),
                        format!("{}{suffix}", fmt_ratio(&val_str(&m["median"]))),
                        format!("{}{suffix}", fmt_ratio(&val_str(&m["high"]))),
                        rank,
                        pct,
                    ]);
                }
            }
            if found {
                super::output::print_table(&headers, rows, &OutputFormat::Pretty);
            } else {
                println!("No valuation distribution data found.");
            }
        }
    }
    Ok(())
}

pub async fn cmd_operating(
    symbol: String,
    report: Option<String>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let mut params = vec![("counter_id", cid.as_str())];
    let report_val;
    if let Some(ref r) = report {
        report_val = r.clone();
        params.push(("report", report_val.as_str()));
    }
    let data = http_get("/v1/quote/operatings", &params, verbose).await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_operating(&data),
    }
    Ok(())
}

fn print_operating(data: &Value) {
    let items = match data.get("list").and_then(|v| v.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => {
            println!("No operating data found.");
            return;
        }
    };

    // Collect rows for financial indicators table
    let mut currency = String::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    for item in items {
        let report = val_str(&item["report"]);
        let latest = item["latest"].as_bool().unwrap_or(false);
        let marker = if latest { " *" } else { "" };

        if let Some(fin) = item.get("financial") {
            if currency.is_empty() {
                currency = val_str(&fin["currency"]);
            }
            if let Some(indicators) = fin.get("indicators").and_then(|v| v.as_array()) {
                let mut row = vec![format!("{report}{marker}")];
                for ind in indicators {
                    let value = val_str(&ind["indicator_value"]);
                    let yoy = val_str(&ind["yoy"]);
                    row.push(value);
                    row.push(if yoy.is_empty() || yoy == "-" {
                        "-".to_string()
                    } else {
                        format!("{yoy}%")
                    });
                }
                rows.push(row);
            }
        }
    }

    // Build dynamic headers from first item's indicators
    let mut headers: Vec<String> = vec!["period".to_string()];
    if let Some(first) = items.first() {
        if let Some(indicators) = first
            .get("financial")
            .and_then(|f| f.get("indicators"))
            .and_then(|v| v.as_array())
        {
            for ind in indicators {
                let name = val_str(&ind["indicator_name"]);
                headers.push(name.clone());
                headers.push(format!("{name}_yoy"));
            }
        }
    }

    if !rows.is_empty() {
        if !currency.is_empty() {
            println!("Currency: {currency}\n");
        }
        let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();
        super::output::print_table(&header_refs, rows, &OutputFormat::Pretty);
    }

    // Print latest period's management review
    if let Some(latest) = items
        .iter()
        .find(|i| i["latest"].as_bool().unwrap_or(false))
    {
        let txt = val_str(&latest["txt"]);
        if !txt.is_empty() {
            let clean = strip_html(&txt);
            let truncated = if clean.chars().count() > 300 {
                let s: String = clean.chars().take(300).collect();
                format!("{s}...")
            } else {
                clean
            };
            println!("\nLatest Review:\n{truncated}");
        }
    }
}

pub async fn cmd_rating_history(
    symbol: String,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/quote/ratings",
        &[("counter_id", cid.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_rating_history(&data),
    }
    Ok(())
}

fn chg_arrow(v: &Value) -> &'static str {
    match v.as_i64() {
        Some(1) => "↑",
        Some(-1) => "↓",
        _ => "→",
    }
}

fn print_rating_history(data: &Value) {
    // Header: style + scale + report period
    let style = val_str(&data["style_txt_name"]);
    let scale = val_str(&data["scale_txt_name"]);
    let period = val_str(&data["report_period_txt"]);
    println!("{style} / {scale}  ({period})");
    println!(
        "Multi-Score: {} ({}) {}  Industry: {} (rank {}/{}, mean {} median {})",
        val_str(&data["multi_score"]),
        val_str(&data["multi_letter"]),
        chg_arrow(&data["multi_score_change"]),
        val_str(&data["industry_name"]),
        val_str(&data["industry_rank"]),
        val_str(&data["industry_total"]),
        val_str(&data["industry_mean_score"]),
        val_str(&data["industry_median_score"]),
    );
    println!();

    // Flatten ratings into a table with sub-indicators
    if let Some(ratings) = data.get("ratings").and_then(|v| v.as_array()) {
        let headers = ["indicator", "value", "score", "grade"];
        let mut rows: Vec<Vec<String>> = Vec::new();

        for r in ratings {
            // Skip type=1 (style) and type=2 (scale) — only show type=3 (multi-score)
            if r["type"].as_i64() != Some(3) {
                continue;
            }
            if let Some(subs) = r.get("sub_indicators").and_then(|v| v.as_array()) {
                for sub in subs {
                    let Some(ind) = sub.get("indicator") else {
                        continue;
                    };
                    // Category row (e.g. 盈利评分)
                    rows.push(vec![
                        val_str(&ind["name"]),
                        String::new(),
                        val_str(&ind["score"]),
                        val_str(&ind["letter"]),
                    ]);
                    // Sub-indicator rows
                    if let Some(leaf_subs) = sub.get("sub_indicators").and_then(|v| v.as_array()) {
                        for leaf in leaf_subs {
                            let name = val_str(&leaf["name"]);
                            let value = val_str(&leaf["value"]);
                            let display_val = match val_str(&leaf["value_type"]).as_str() {
                                "percent" => format!("{value}%"),
                                "bignumber" => format_financial_value(&value, true),
                                _ => value,
                            };
                            rows.push(vec![
                                format!("  {name}"),
                                display_val,
                                val_str(&leaf["score"]),
                                val_str(&leaf["letter"]),
                            ]);
                        }
                    }
                }
            }
        }
        super::output::print_table(&headers, rows, &OutputFormat::Pretty);
    }
}

pub async fn cmd_corp_action(
    symbol: String,
    all: bool,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let mut data = http_get(
        "/v1/quote/company-act",
        &[
            ("counter_id", cid.as_str()),
            ("req_type", "1"),
            ("version", "3"),
        ],
        verbose,
    )
    .await?;
    if !all {
        let key = if data.get("items").is_some() {
            "items"
        } else {
            "CompanyActItem"
        };
        if let Some(arr) = data.get_mut(key).and_then(|v| v.as_array_mut()) {
            arr.truncate(30);
        }
    }
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_corp_action(&data),
    }
    Ok(())
}

fn print_corp_action(data: &Value) {
    let items = match data
        .get("items")
        .or_else(|| data.get("CompanyActItem"))
        .and_then(|v| v.as_array())
    {
        Some(a) if !a.is_empty() => a,
        _ => {
            println!("No corporate action records found.");
            return;
        }
    };

    let headers = ["date", "date_type", "action", "description"];
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            vec![
                val_str(&item["date"]),
                val_str(&item["date_type"]),
                val_str(&item["act_type"]),
                val_str(&item["act_desc"]),
            ]
        })
        .collect();
    super::output::print_table(&headers, rows, &OutputFormat::Pretty);
}

pub async fn cmd_invest_relation(
    symbol: String,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    let data = http_get(
        "/v1/quote/invest-relations",
        &[("counter_id", cid.as_str()), ("count", "0")],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_invest_relation(&data),
    }
    Ok(())
}

fn print_invest_relation(data: &Value) {
    let items = match data.get("invest_securities").and_then(|v| v.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => {
            println!("No investment relations found.");
            return;
        }
    };

    let api_total = data["total"].as_u64().unwrap_or(0);
    let total = if api_total > 0 {
        api_total
    } else {
        items.len() as u64
    };
    println!("Total: {total}\n");

    let headers = ["company", "symbol", "% shares", "value", "currency", "rank"];
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            let sym = val_str(&item["counter_id"]);
            let display_sym = if sym.is_empty() || sym == "-" {
                "-".to_string()
            } else {
                counter_id_to_symbol(&sym)
            };
            let raw_val = val_str(&item["shares_value"]);
            let cur = val_str(&item["currency"]);
            vec![
                val_str(&item["company_name"]),
                display_sym,
                fmt_pct(&val_str(&item["percent_of_shares"])),
                fmt_amount(&raw_val, ""),
                cur,
                val_str(&item["shares_rank"]),
            ]
        })
        .collect();
    super::output::print_table(&headers, rows, &OutputFormat::Pretty);
}
