use anyhow::{bail, Result};

use super::{
    api::http_post,
    output::{parse_datetime_end, parse_datetime_start, print_json_value},
    OutputFormat,
};
use crate::utils::counter::symbol_to_counter_id;

/// Map CLI period string to the numeric `line_type` expected by the API.
fn period_to_line_type(period: &str) -> Result<i32> {
    match period {
        "1m" | "minute" => Ok(1),
        "5m" => Ok(5),
        "15m" => Ok(15),
        "30m" => Ok(30),
        "1h" | "60m" | "hour" => Ok(60),
        "day" | "d" | "1d" => Ok(1000),
        "week" | "w" => Ok(2000),
        "month" | "m" | "1mo" => Ok(3000),
        "year" | "y" => Ok(4000),
        _ => bail!("Unknown period '{period}'. Use: 1m 5m 15m 30m 1h day week month year"),
    }
}

/// Run a quant indicator script against historical K-line data on the server.
pub async fn cmd_run_script(
    symbol: String,
    period: &str,
    start: &str,
    end: &str,
    script_arg: Option<String>,
    input: Option<String>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let counter_id = symbol_to_counter_id(&symbol);
    let line_type = period_to_line_type(period)?;

    // Parse date strings to seconds-level Unix timestamps.
    let start_time = parse_datetime_start(start)?.unix_timestamp();
    let end_time = parse_datetime_end(end)?.unix_timestamp();

    // Resolve script: inline flag takes priority, then stdin.
    let script = if let Some(s) = script_arg {
        s
    } else {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| anyhow::anyhow!("Failed to read script from stdin: {e}"))?;
        let trimmed = buf.trim().to_string();
        if trimmed.is_empty() {
            bail!("No script provided. Use --script or pipe script content via stdin.");
        }
        trimmed
    };

    // Validate and normalise input_json: default to empty array.
    let input_json = match input {
        Some(s) => {
            let v: serde_json::Value = serde_json::from_str(&s)
                .map_err(|e| anyhow::anyhow!("--input must be a valid JSON array: {e}"))?;
            if !v.is_array() {
                bail!("--input must be a JSON array, e.g. '[100,200,300]'");
            }
            s
        }
        None => "[]".to_string(),
    };

    let body = serde_json::json!({
        "counter_id": counter_id,
        "start_time": start_time,
        "end_time": end_time,
        "script": script,
        "input_json": input_json,
        "line_type": line_type,
    });

    if verbose {
        eprintln!("* counter_id: {counter_id}");
        eprintln!("* line_type: {line_type}");
        eprintln!("* start_time: {start_time}  end_time: {end_time}");
    }

    let resp = http_post("/v1/quant/run_script", body, verbose).await?;
    print_json_value(&resp, format);
    Ok(())
}
