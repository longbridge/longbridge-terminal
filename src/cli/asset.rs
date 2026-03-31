use anyhow::Result;
use serde_json::Value;

use super::api::http_get;
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
        OutputFormat::Table => print_exchange_rates(&data),
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
            &OutputFormat::Table,
        );
    } else {
        print_json(data);
    }
}

// ── my rate ───────────────────────────────────────────────────────────────────

/// Fetch personal commission and fee rates for the current account.
pub async fn cmd_my_rate(format: &OutputFormat, verbose: bool) -> Result<()> {
    let data = http_get("/v1/billing/my_rate", &[], verbose).await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Table => print_my_rate(&data),
    }
    Ok(())
}

fn print_my_rate(data: &Value) {
    if let Some(obj) = data["financing_rates"].as_object() {
        println!("Financing rates:");
        let rows: Vec<Vec<String>> = obj
            .iter()
            .flat_map(|(currency, info)| {
                let divisor = val_str(&info["divisor"]);
                [("dft", "default"), ("member", "member")]
                    .iter()
                    .filter_map(|(key, label)| {
                        let rate_info = &info[key];
                        let rate = rate_info["step_fee_rates"]
                            .as_array()
                            .and_then(|a| a.first())
                            .map(|r| val_str(&r["fee_rate"]))
                            .unwrap_or_default();
                        if rate.is_empty() {
                            return None;
                        }
                        Some(vec![
                            currency.clone(),
                            (*label).to_owned(),
                            rate,
                            divisor.clone(),
                            val_str(&rate_info["min_fee"]),
                            val_str(&rate_info["max_fee"]),
                        ])
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        super::output::print_table(
            &["currency", "type", "rate", "divisor", "min_fee", "max_fee"],
            rows,
            &OutputFormat::Table,
        );
    }
}
