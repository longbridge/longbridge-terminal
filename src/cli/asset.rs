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

pub async fn cmd_profit_analysis_sublist(
    filter: &str,
    start: Option<&str>,
    end: Option<&str>,
    currency: Option<&str>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let mut params: Vec<(&str, String)> = vec![("profit_or_loss", filter.to_owned())];
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
        "/v1/portfolio/profit-analysis-sublist",
        &params_ref,
        verbose,
    )
    .await?;

    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_profit_analysis_sublist(&data),
    }
    Ok(())
}

pub async fn cmd_profit_analysis_detail(
    symbol: &str,
    start: Option<&str>,
    end: Option<&str>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = crate::utils::counter::symbol_to_counter_id(symbol);
    let mut params: Vec<(&str, String)> = vec![("counter_id", cid)];
    if let Some(s) = start {
        let ts = parse_datetime_start(s)?.unix_timestamp().to_string();
        params.push(("start", ts));
    }
    if let Some(e) = end {
        let ts = parse_datetime_end(e)?.unix_timestamp().to_string();
        params.push(("end", ts));
    }
    let params_ref: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let data = http_get("/v1/portfolio/profit-analysis/detail", &params_ref, verbose).await?;

    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => print_pnl_detail(&data, symbol),
    }
    Ok(())
}

fn print_pnl_detail(data: &Value, symbol: &str) {
    let name = val_str(&data["name"]);
    let currency = val_str(&data["currency"]);
    let start_date = val_str(&data["start_date"]);
    let end_date = val_str(&data["end_date"]);
    let profit = val_str(&data["profit"]);

    let title = if name.is_empty() || name == "-" {
        symbol.to_owned()
    } else {
        format!("{name} ({symbol})")
    };
    print!("{title}");
    if !currency.is_empty() && currency != "-" {
        print!("  [{currency}]");
    }
    if !start_date.is_empty() && start_date != "-" {
        print!("  {start_date} ~ {end_date}");
    }
    println!("\n");
    println!("{:24} {profit}", "Total P&L");

    // Underlying details
    let ud = &data["underlying_details"];
    let ud_profit = val_str(&ud["profit"]);
    if ud_profit != "0" && !ud_profit.is_empty() {
        println!("\n  Underlying");
        println!("  {:22} {ud_profit}", "P&L");
        print_amount_if_set("  ", "Holding Value", &val_str(&ud["holding_value"]));
        print_amount_if_set(
            "  ",
            "Total Buy",
            &val_str(&ud["cumulative_debited_amount"]),
        );
        print_amount_if_set(
            "  ",
            "Total Sell",
            &val_str(&ud["cumulative_credited_amount"]),
        );
        print_amount_if_set("  ", "Total Fee", &val_str(&ud["cumulative_fee_amount"]));
        print_detail_items("  ", "Buys", ud.get("debited_details"));
        print_detail_items("  ", "Sells", ud.get("credited_details"));
        print_detail_items("  ", "Fees", ud.get("fee_details"));
    }

    // Derivative details
    let dd = &data["derivative_pnl_details"];
    let dd_profit = val_str(&dd["profit"]);
    if dd_profit != "0" && !dd_profit.is_empty() {
        println!("\n  Derivative");
        println!("  {:22} {dd_profit}", "P&L");
        print_amount_if_set("  ", "Holding Value", &val_str(&dd["holding_value"]));
        print_amount_if_set(
            "  ",
            "Total Buy",
            &val_str(&dd["cumulative_debited_amount"]),
        );
        print_amount_if_set(
            "  ",
            "Total Sell",
            &val_str(&dd["cumulative_credited_amount"]),
        );
        print_amount_if_set("  ", "Total Fee", &val_str(&dd["cumulative_fee_amount"]));
        print_detail_items("  ", "Buys", dd.get("debited_details"));
        print_detail_items("  ", "Sells", dd.get("credited_details"));
        print_detail_items("  ", "Fees", dd.get("fee_details"));
    }
}

fn print_amount_if_set(prefix: &str, label: &str, val: &str) {
    if !val.is_empty() && val != "-" && val != "0" {
        println!("{prefix}{label:22} {val}");
    }
}

fn print_detail_items(prefix: &str, label: &str, items: Option<&Value>) {
    if let Some(arr) = items.and_then(|v| v.as_array()) {
        for item in arr {
            let amount = val_str(&item["amount"]);
            let desc = val_str(&item["describe"]);
            if !amount.is_empty() && amount != "0" {
                println!("{prefix}  {desc:20} {amount}");
            }
        }
    }
    let _ = label; // suppress unused warning — label used for clarity in call sites
}

pub async fn cmd_profit_analysis_flows(
    symbol: &str,
    start: Option<&str>,
    end: Option<&str>,
    derivative: bool,
    page: u32,
    size: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = crate::utils::counter::symbol_to_counter_id(symbol);
    let mut params: Vec<(&str, String)> = vec![
        ("counter_id", cid),
        ("page", page.to_string()),
        ("size", size.to_string()),
        ("derivative", derivative.to_string()),
    ];
    if let Some(s) = start {
        let ts = parse_datetime_start(s)?.unix_timestamp().to_string();
        params.push(("start", ts));
    }
    if let Some(e) = end {
        let ts = parse_datetime_end(e)?.unix_timestamp().to_string();
        params.push(("end", ts));
    }
    let params_ref: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let data = http_get("/v1/portfolio/profit-analysis/flows", &params_ref, verbose).await?;

    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            if let Some(flows) = data.get("flows_list").and_then(|v| v.as_array()) {
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
                print_table(headers, rows, format);
            }
        }
    }
    Ok(())
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
