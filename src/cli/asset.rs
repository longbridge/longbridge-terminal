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
