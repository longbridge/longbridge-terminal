use anyhow::{bail, Result};
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

// ── currency convert ──────────────────────────────────────────────────────────

/// Convert an amount between two currencies using live exchange rates.
pub async fn cmd_currency_convert(
    from: String,
    to: String,
    amount: String,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let from = from.to_uppercase();
    let to = to.to_uppercase();
    let qty: f64 = amount
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid amount: {amount}"))?;

    if from == to {
        let result = serde_json::json!({
            "from": from,
            "to": to,
            "amount": qty,
            "rate": 1.0,
            "converted": qty,
        });
        match format {
            OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&result)?),
            OutputFormat::Pretty => println!("{qty} {from} = {qty} {to} (rate: 1)"),
        }
        return Ok(());
    }

    let data = http_get("/v1/asset/exchange_rates", &[], verbose).await?;
    let list = data["exchanges"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("unexpected response format"))?;

    // Try direct pair first, then inverse pair.
    let (rate_str, inverted) = list
        .iter()
        .find_map(|item| {
            let base = val_str(&item["base_currency"]);
            let other = val_str(&item["other_currency"]);
            if base == from && other == to {
                Some((val_str(&item["average_rate"]), false))
            } else if base == to && other == from {
                Some((val_str(&item["average_rate"]), true))
            } else {
                None
            }
        })
        .ok_or_else(|| anyhow::anyhow!("no exchange rate found for {from} → {to}"))?;

    let rate: f64 = rate_str
        .parse()
        .map_err(|_| anyhow::anyhow!("cannot parse rate: {rate_str}"))?;
    if rate == 0.0 {
        bail!("exchange rate is zero for {from} → {to}");
    }

    let (effective_rate, converted) = if inverted {
        (1.0 / rate, qty / rate)
    } else {
        (rate, qty * rate)
    };

    match format {
        OutputFormat::Json => {
            let result = serde_json::json!({
                "from": from,
                "to": to,
                "amount": qty,
                "rate": effective_rate,
                "converted": converted,
            });
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        OutputFormat::Pretty => {
            super::output::print_table(
                &["from", "to", "amount", "rate", "converted"],
                vec![vec![
                    from,
                    to,
                    format!("{qty}"),
                    format!("{effective_rate:.6}"),
                    format!("{converted:.4}"),
                ]],
                &OutputFormat::Pretty,
            );
        }
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
