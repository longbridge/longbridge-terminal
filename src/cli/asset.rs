use anyhow::Result;
use serde_json::Value;

use super::api::http_get;
use super::output::{fmt_datetime, parse_datetime_end, parse_datetime_start, print_table};
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

// ── exchange rate ─────────────────────────────────────────────────────────────

/// Fetch exchange rates for all supported currencies.
pub async fn cmd_exchange_rate(format: &OutputFormat, verbose: bool) -> Result<()> {
    let data = http_get("/v1/asset/exchange_rates", &[], verbose).await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_exchange_rates(&data),
    }
    Ok(())
}

fn print_exchange_rates(data: &Value) {
    if let Some(list) = data["exchanges"].as_array() {
        let rows: Vec<Vec<String>> = list
            .iter()
            .map(|item| {
                vec![
                    format!(
                        "{} → {}",
                        val_str(&item["base_currency"]),
                        val_str(&item["other_currency"])
                    ),
                    val_str(&item["average_rate"]),
                    val_str(&item["bid_rate"]),
                    val_str(&item["offer_rate"]),
                ]
            })
            .collect();
        super::output::print_table(
            &["pair", "average_rate", "bid_rate", "offer_rate"],
            rows,
            &OutputFormat::Pretty,
        );
    } else {
        print_json(data);
    }
}

// ── profit analysis ──────────────────────────────────────────────────────────

pub async fn cmd_profit_analysis(format: &OutputFormat, verbose: bool) -> Result<()> {
    let summary_fut = http_get("/v1/portfolio/profit-analysis-summary", &[], verbose);
    let sublist_fut = http_get(
        "/v1/portfolio/profit-analysis-sublist",
        &[("profit_or_loss", "all")],
        verbose,
    );
    let (summary, sublist) = tokio::join!(summary_fut, sublist_fut);
    let summary = summary?;
    let sublist = sublist?;

    match format {
        OutputFormat::Json => {
            let mut merged = serde_json::Map::new();
            if let Value::Object(m) = &summary {
                merged.extend(m.clone());
            }
            merged.insert("sublist".to_owned(), sublist.clone());
            print_json(&Value::Object(merged));
        }
        OutputFormat::Pretty => {
            print_profit_analysis_summary(&summary);
            print_profit_analysis_sublist(&sublist);
        }
    }
    Ok(())
}

fn print_profit_analysis_summary(data: &Value) {
    let currency = val_str(&data["currency"]);
    let period = format!(
        "{} ~ {}",
        val_str(&data["start_date"]),
        val_str(&data["end_date"])
    );
    println!("P&L Summary ({currency})  {period}\n");

    let fields = [
        ("Total Asset", "current_total_asset"),
        ("Initial Asset", "initial_asset_value"),
        ("Ending Asset", "ending_asset_value"),
        ("Invest Amount", "invest_amount"),
        ("Total P&L", "sum_profit"),
        ("Total P&L Rate", "sum_profit_rate"),
        ("Simple Yield", "total_simple_earning_yield"),
        ("Time-Weighted Yield", "total_time_earning_yield"),
        ("Stocks Traded", "trade_stock_num"),
    ];
    for (label, key) in fields {
        let v = val_str(&data[key]);
        if !v.is_empty() && v != "-" {
            println!("{label:20} {v}");
        }
    }

    if let Some(profits) = data.get("profits") {
        println!();
        let categories = [
            ("Stock P&L", "stock"),
            ("Fund P&L", "fund"),
            ("MMF P&L", "mmf"),
            ("Crypto P&L", "crypto"),
            ("Other P&L", "other"),
            ("IPO Subscription", "ipo_subscription"),
            ("IPO Hit", "ipo_hit"),
        ];
        for (label, key) in categories {
            let v = val_str(&profits[key]);
            if !v.is_empty() && v != "-" && v != "0" {
                println!("{label:20} {v}");
            }
        }
    }
}

/// Build a display symbol from a P&L item.
/// Prefers `security_code.MARKET`, falls back to isin for funds.
fn pnl_item_symbol(item: &Value) -> String {
    let code = val_str(&item["security_code"]);
    if !code.is_empty() && code != "-" {
        let market = val_str(&item["market"]);
        if !market.is_empty() && market != "-" {
            return format!("{code}.{market}");
        }
        return code;
    }
    // Fund items have no security_code; use isin as identifier
    let isin = val_str(&item["isin"]);
    if !isin.is_empty() && isin != "-" {
        return isin;
    }
    val_str(&item["code"])
}

fn print_profit_analysis_sublist(data: &Value) {
    if let Some(items) = data.get("items").and_then(|v| v.as_array()) {
        if items.is_empty() {
            return;
        }
        println!("\nStock P&L Breakdown\n");
        let headers = &["Symbol", "Name", "Market", "P&L"];
        let rows: Vec<Vec<String>> = items
            .iter()
            .map(|item| {
                vec![
                    pnl_item_symbol(item),
                    val_str(&item["name"]),
                    val_str(&item["market"]),
                    val_str(&item["profit"]),
                ]
            })
            .collect();
        print_table(headers, rows, &OutputFormat::Pretty);
    }
}

pub async fn cmd_profit_analysis_detail(
    symbol: &str,
    start: Option<&str>,
    end: Option<&str>,
    currency: Option<&str>,
    derivative: bool,
    page: u32,
    size: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = crate::utils::counter::symbol_to_counter_id(symbol);

    // Build shared start/end timestamps
    let start_ts = start
        .map(|s| parse_datetime_start(s).map(|d| d.unix_timestamp().to_string()))
        .transpose()?;
    let end_ts = end
        .map(|e| parse_datetime_end(e).map(|d| d.unix_timestamp().to_string()))
        .transpose()?;

    // Build params
    let mut detail_params: Vec<(&str, String)> = vec![("counter_id", cid.clone())];
    if let Some(c) = currency {
        detail_params.push(("currency", c.to_owned()));
    }
    if let Some(ref s) = start_ts {
        detail_params.push(("start", s.clone()));
    }
    if let Some(ref e) = end_ts {
        detail_params.push(("end", e.clone()));
    }
    let detail_pr: Vec<(&str, &str)> = detail_params
        .iter()
        .map(|(k, v)| (*k, v.as_str()))
        .collect();

    let mut flows_params: Vec<(&str, String)> = vec![
        ("counter_id", cid),
        ("page", page.to_string()),
        ("size", size.to_string()),
        ("derivative", derivative.to_string()),
    ];
    if let Some(ref s) = start_ts {
        flows_params.push(("start", s.clone()));
    }
    if let Some(ref e) = end_ts {
        flows_params.push(("end", e.clone()));
    }
    let flows_pr: Vec<(&str, &str)> = flows_params.iter().map(|(k, v)| (*k, v.as_str())).collect();

    // Fetch detail + flows concurrently
    let (detail, flows) = tokio::join!(
        http_get("/v1/portfolio/profit-analysis/detail", &detail_pr, verbose),
        http_get("/v1/portfolio/profit-analysis/flows", &flows_pr, verbose),
    );
    let detail = detail?;
    let flows = flows?;

    match format {
        OutputFormat::Json => {
            let mut merged = serde_json::Map::new();
            if let Value::Object(m) = &detail {
                merged.extend(m.clone());
            }
            merged.insert("flows".to_owned(), flows.clone());
            print_json(&Value::Object(merged));
        }
        OutputFormat::Pretty => {
            print_pnl_detail(&detail, symbol);
            print_pnl_flows(&flows);
        }
    }
    Ok(())
}

fn print_pnl_detail(data: &Value, symbol: &str) {
    let name = val_str(&data["name"]);
    let currency = val_str(&data["currency"]);
    let start_date = val_str(&data["start_date"]);
    let end_date = val_str(&data["end_date"]);

    let title = if name.is_empty() || name == "-" {
        symbol.to_owned()
    } else {
        format!("{name} ({symbol})")
    };
    print!("{title}");
    if !currency.is_empty() && currency != "-" {
        print!("  currency: {currency}");
    }
    if !start_date.is_empty() && start_date != "-" {
        print!("  {start_date} ~ {end_date}");
    }
    println!("\n");

    let mut rows: Vec<Vec<String>> = Vec::new();
    rows.push(vec!["Total P&L".to_owned(), val_str(&data["profit"])]);

    collect_section_rows(&mut rows, "Underlying", &data["underlying_details"]);
    collect_section_rows(&mut rows, "Derivative", &data["derivative_pnl_details"]);

    print_table(&["", "Amount"], rows, &OutputFormat::Pretty);
}

fn collect_section_rows(rows: &mut Vec<Vec<String>>, label: &str, section: &Value) {
    let profit = val_str(&section["profit"]);
    if profit == "0" || profit.is_empty() {
        return;
    }
    rows.push(vec![format!("[{label}]"), String::new()]);
    rows.push(vec!["  P&L".to_owned(), profit]);
    push_if_set(rows, "  Holding Value", &val_str(&section["holding_value"]));
    push_if_set(
        rows,
        "  Total Buy",
        &val_str(&section["cumulative_debited_amount"]),
    );
    push_if_set(
        rows,
        "  Total Sell",
        &val_str(&section["cumulative_credited_amount"]),
    );
    push_if_set(
        rows,
        "  Total Fee",
        &val_str(&section["cumulative_fee_amount"]),
    );
    push_detail_items(rows, section.get("debited_details"));
    push_detail_items(rows, section.get("credited_details"));
    push_detail_items(rows, section.get("fee_details"));
}

fn push_if_set(rows: &mut Vec<Vec<String>>, label: &str, val: &str) {
    if !val.is_empty() && val != "-" && val != "0" {
        rows.push(vec![label.to_owned(), val.to_owned()]);
    }
}

fn push_detail_items(rows: &mut Vec<Vec<String>>, items: Option<&Value>) {
    if let Some(arr) = items.and_then(|v| v.as_array()) {
        for item in arr {
            let amount = val_str(&item["amount"]);
            let desc = val_str(&item["describe"]);
            if !amount.is_empty() && amount != "0" {
                rows.push(vec![format!("    {desc}"), amount]);
            }
        }
    }
}

fn print_pnl_flows(data: &Value) {
    if let Some(flows) = data.get("flows_list").and_then(|v| v.as_array()) {
        if flows.is_empty() {
            return;
        }
        println!("\nTransaction Flows\n");
        let headers = &["Time", "Code", "Direction", "Qty", "Price", "Cost", "Desc"];
        let rows: Vec<Vec<String>> = flows
            .iter()
            .map(|f| {
                let exec_time = {
                    let exec_date = val_str(&f["executed_date"]);
                    if exec_date.is_empty() || exec_date == "-" {
                        let raw = val_str(&f["executed_timestamp"]);
                        raw.parse::<i64>()
                            .ok()
                            .or_else(|| f["executed_timestamp"].as_i64())
                            .and_then(|t| time::OffsetDateTime::from_unix_timestamp(t).ok())
                            .map_or(raw, fmt_datetime)
                    } else {
                        exec_date
                    }
                };
                let direction = match val_str(&f["direction"]).as_str() {
                    "0" => "In".to_owned(),
                    "1" => "Out".to_owned(),
                    "-1" => "-".to_owned(),
                    other => other.to_owned(),
                };
                vec![
                    exec_time,
                    val_str(&f["code"]),
                    direction,
                    val_str(&f["executed_quantity"]),
                    val_str(&f["executed_price"]),
                    val_str(&f["executed_cost"]),
                    val_str(&f["describe"]),
                ]
            })
            .collect();
        let has_more = data
            .get("has_more")
            .is_some_and(|v| v.as_bool().unwrap_or(false));
        print_table(headers, rows, &OutputFormat::Pretty);
        if has_more {
            println!("(more results available, use --page to paginate)");
        }
    }
}

pub async fn cmd_profit_analysis_by_market(
    market: Option<&str>,
    start: Option<&str>,
    end: Option<&str>,
    currency: Option<&str>,
    page: u32,
    size: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let mut params: Vec<(&str, String)> =
        vec![("page", page.to_string()), ("size", size.to_string())];
    if let Some(m) = market {
        params.push(("market", m.to_owned()));
    }
    if let Some(s) = start {
        let ts = parse_datetime_start(s)?.unix_timestamp().to_string();
        params.push(("start", ts));
    }
    if let Some(e) = end {
        let ts = parse_datetime_end(e)?.unix_timestamp().to_string();
        params.push(("end", ts));
    }
    if let Some(c) = currency {
        params.push(("currency", c.to_owned()));
    }
    let params_ref: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let data = http_get(
        "/v1/portfolio/profit-analysis/by-market",
        &params_ref,
        verbose,
    )
    .await?;

    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            let total_profit = val_str(&data["profit"]);
            let has_more = data
                .get("has_more")
                .is_some_and(|v| v.as_bool().unwrap_or(false));
            println!("Total P&L: {total_profit}");
            if has_more {
                println!("(more results available, use --page to paginate)\n");
            } else {
                println!();
            }
            if let Some(items) = data.get("stock_items").and_then(|v| v.as_array()) {
                let headers = &["Symbol", "Name", "Market", "P&L"];
                let rows: Vec<Vec<String>> = items
                    .iter()
                    .map(|item| {
                        vec![
                            val_str(&item["code"]),
                            val_str(&item["name"]),
                            val_str(&item["market"]),
                            val_str(&item["profit"]),
                        ]
                    })
                    .collect();
                print_table(headers, rows, format);
            }
        }
    }
    Ok(())
}
