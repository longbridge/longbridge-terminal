use tabled::{builder::Builder, settings::Style};

use super::OutputFormat;

fn print_markdown_table(headers: &[&str], rows: &[Vec<String>]) {
    let mut builder = Builder::default();
    builder.push_record(headers.iter().copied());
    for row in rows {
        builder.push_record(row.iter().map(String::as_str));
    }
    println!("{}", builder.build().with(Style::markdown()));
}

/// Print data as table or JSON depending on format
pub fn print_table(headers: &[&str], rows: Vec<Vec<String>>, format: &OutputFormat) {
    match format {
        OutputFormat::Json => {
            let records: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|row| {
                    let mut map = serde_json::Map::new();
                    for (i, val) in row.into_iter().enumerate() {
                        if let Some(&key) = headers.get(i) {
                            let key = key.to_lowercase().replace(' ', "_");
                            map.insert(key, serde_json::Value::String(val));
                        }
                    }
                    serde_json::Value::Object(map)
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&records).unwrap_or_default()
            );
        }
        OutputFormat::Pretty => {
            print_markdown_table(headers, &rows);
        }
    }
}

/// Print a single JSON value (for commands that return a single object)
pub fn print_json_value(value: &serde_json::Value, format: &OutputFormat) {
    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(value).unwrap_or_default()
            );
        }
        OutputFormat::Pretty => {
            if let serde_json::Value::Object(map) = value {
                let rows: Vec<Vec<String>> = map
                    .iter()
                    .map(|(k, v)| {
                        let val = match v {
                            serde_json::Value::String(s) => s.clone(),
                            serde_json::Value::Null => "-".to_string(),
                            other => other.to_string(),
                        };
                        vec![k.clone(), val]
                    })
                    .collect();
                print_markdown_table(&["Field", "Value"], &rows);
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(value).unwrap_or_default()
                );
            }
        }
    }
}

/// Format optional decimal as string with 3 decimal places
pub fn fmt_decimal(v: &Option<rust_decimal::Decimal>) -> String {
    v.map_or_else(|| "-".to_string(), |d| format!("{d:.3}"))
}

/// Format optional decimal divided by 100 with 3 decimal places (API returns percentage values, e.g. implied volatility, rho)
pub fn fmt_decimal_div100(v: &Option<rust_decimal::Decimal>) -> String {
    v.map_or_else(
        || "-".to_string(),
        |d| format!("{:.3}", d / rust_decimal::Decimal::ONE_HUNDRED),
    )
}

/// Format optional decimal divided by 252 with 3 decimal places (convert annualized greek to per-trading-day, e.g. theta, vega)
pub fn fmt_decimal_div252(v: &Option<rust_decimal::Decimal>) -> String {
    v.map_or_else(
        || "-".to_string(),
        |d| format!("{:.3}", d / rust_decimal::Decimal::from(252u32)),
    )
}

/// Format decimal
pub fn fmt_dec(v: rust_decimal::Decimal) -> String {
    v.to_string()
}

/// Parse a date string (YYYY-MM-DD) into `time::Date`
pub fn parse_date(s: &str) -> anyhow::Result<time::Date> {
    let fmt = time::macros::format_description!("[year]-[month]-[day]");
    time::Date::parse(s, &fmt).map_err(|e| anyhow::anyhow!("Invalid date '{s}': {e}"))
}

/// Parse a date string into `OffsetDateTime` at start of day UTC
pub fn parse_datetime_start(s: &str) -> anyhow::Result<time::OffsetDateTime> {
    let date = parse_date(s)?;
    Ok(date.with_time(time::Time::MIDNIGHT).assume_utc())
}

/// Parse a date string into `OffsetDateTime` at end of day UTC
pub fn parse_datetime_end(s: &str) -> anyhow::Result<time::OffsetDateTime> {
    let date = parse_date(s)?;
    let end_time = time::Time::from_hms(23, 59, 59).unwrap();
    Ok(date.with_time(end_time).assume_utc())
}

/// Format an `OffsetDateTime` as a readable string
pub fn fmt_datetime(dt: time::OffsetDateTime) -> String {
    let fmt = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    dt.format(&fmt).unwrap_or_else(|_| dt.to_string())
}

/// Format a Date as string
pub fn fmt_date(d: time::Date) -> String {
    let fmt = time::macros::format_description!("[year]-[month]-[day]");
    d.format(&fmt).unwrap_or_else(|_| d.to_string())
}

/// Recursively remove non-public internal fields from a JSON value.
///
/// Longbridge API responses may include fields like `aaid` that are internal
/// identifiers not intended for external consumers. This function strips them
/// in-place from any JSON object, at any nesting depth.
pub fn strip_private_fields(v: &mut serde_json::Value) {
    const PRIVATE_FIELDS: &[&str] = &["aaid"];
    match v {
        serde_json::Value::Object(map) => {
            for key in PRIVATE_FIELDS {
                map.remove(*key);
            }
            for val in map.values_mut() {
                strip_private_fields(val);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                strip_private_fields(item);
            }
        }
        _ => {}
    }
}
