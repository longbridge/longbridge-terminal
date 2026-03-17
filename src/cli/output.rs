use comfy_table::{Cell, ContentArrangement, Table};

use super::OutputFormat;

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
        OutputFormat::Table => {
            let mut table = Table::new();
            table
                .set_content_arrangement(ContentArrangement::Dynamic)
                .set_header(headers.iter().map(Cell::new));
            for row in rows {
                table.add_row(row);
            }
            println!("{table}");
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
        OutputFormat::Table => {
            // For single objects, print as key-value table
            if let serde_json::Value::Object(map) = value {
                let mut table = Table::new();
                table
                    .set_content_arrangement(ContentArrangement::Dynamic)
                    .set_header(["Field", "Value"]);
                for (k, v) in map {
                    let val = match v {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Null => "-".to_string(),
                        other => other.to_string(),
                    };
                    table.add_row([k.as_str(), val.as_str()]);
                }
                println!("{table}");
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(value).unwrap_or_default()
                );
            }
        }
    }
}

/// Format optional decimal as string
pub fn fmt_decimal(v: &Option<rust_decimal::Decimal>) -> String {
    v.map(|d| d.to_string()).unwrap_or_else(|| "-".to_string())
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
