use anyhow::Result;
use serde_json::{Map, Value};

use super::api::http_get;
use super::output::{print_json_value, print_table};
use super::OutputFormat;
use crate::utils::counter::{counter_id_to_symbol, symbol_to_counter_id};

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
        Value::Null => "-".to_string(),
        other => other.to_string(),
    }
}

fn fmt_ts(v: &Value) -> String {
    let ts = match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    };
    ts.map_or_else(|| val_str(v), crate::utils::datetime::format_timestamp)
}

/// Parse a datetime string in `"YYYY.MM.DD HH:MM"` or `"YYYY-MM-DD HH:MM"` format
/// (HKT = UTC+8) and return an RFC 3339 string. Falls back to the original string.
fn fmt_datetime_hkt(v: &Value) -> String {
    use time::{Date, Month, OffsetDateTime, Time, UtcOffset};

    // Try Unix timestamp first.
    let ts = match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    };
    if let Some(ts) = ts {
        return crate::utils::datetime::format_timestamp(ts);
    }

    let s = val_str(v);
    // Accept "YYYY.MM.DD HH:MM" or "YYYY-MM-DD HH:MM"
    let s = s.replace('.', "-");
    let parts: Vec<&str> = s.splitn(2, ' ').collect();
    if parts.len() == 2 {
        let date_parts: Vec<&str> = parts[0].splitn(3, '-').collect();
        let time_parts: Vec<&str> = parts[1].splitn(2, ':').collect();
        if date_parts.len() == 3 && time_parts.len() == 2 {
            if let (Ok(y), Ok(m), Ok(d), Ok(h), Ok(min)) = (
                date_parts[0].parse::<i32>(),
                date_parts[1].parse::<u8>(),
                date_parts[2].parse::<u8>(),
                time_parts[0].parse::<u8>(),
                time_parts[1].parse::<u8>(),
            ) {
                if let Ok(month) = Month::try_from(m) {
                    if let (Ok(date), Ok(time)) = (
                        Date::from_calendar_date(y, month, d),
                        Time::from_hms(h, min, 0),
                    ) {
                        let hkt = UtcOffset::from_hms(8, 0, 0).unwrap_or(UtcOffset::UTC);
                        let dt = OffsetDateTime::new_in_offset(date, time, hkt);
                        return crate::utils::datetime::fmt_rfc3339(dt);
                    }
                }
            }
        }
    }
    val_str(v)
}

fn fmt_date_opt(v: &Value) -> String {
    if let Value::String(s) = v {
        if s.len() == 8 && s.chars().all(|c| c.is_ascii_digit()) {
            return format!("{}-{}-{}", &s[..4], &s[4..6], &s[6..]);
        }
        if s.len() == 10 && s.as_bytes()[4] == b'-' && s.as_bytes()[7] == b'-' {
            return s.clone();
        }
    }
    let ts = match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    };
    match ts {
        Some(n) if n > 0 => crate::utils::datetime::format_date(n),
        Some(_) => "-".to_string(),
        None => val_str(v),
    }
}

fn state_stage_label(v: &Value) -> &'static str {
    match val_str(v).as_str() {
        "0" => "pending",
        "1" => "sub-start",
        "2" => "sub-end",
        "3" => "allotment",
        "4" => "grey-market",
        "5" => "listed",
        _ => "unknown",
    }
}

fn extract_tag(tags: &[Value], keyword: &str) -> String {
    tags.iter()
        .find_map(|t| t.as_str().filter(|s| s.contains(keyword)))
        .map_or_else(|| "-".to_string(), str::to_string)
}

// Transform subscription item: counter_id → symbol, sub_deadline → deadline (RFC 3339),
// state_stage → state (label).
fn transform_subscription(item: &Value) -> Value {
    let mut obj = Map::new();
    if let Some(map) = item.as_object() {
        for (k, v) in map {
            match k.as_str() {
                "counter_id" => {
                    obj.insert(
                        "symbol".to_string(),
                        Value::String(counter_id_to_symbol(&val_str(v))),
                    );
                }
                "sub_deadline" => {
                    obj.insert("deadline".to_string(), Value::String(fmt_ts(v)));
                }
                "state_stage" => {
                    obj.insert(
                        "state".to_string(),
                        Value::String(state_stage_label(v).to_string()),
                    );
                }
                _ => {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }
    }
    Value::Object(obj)
}

fn state_label(v: &Value) -> &'static str {
    match val_str(v).as_str() {
        "0" => "normal",
        "1" => "delayed",
        "2" => "cancelled",
        _ => "unknown",
    }
}

fn sub_state_label(v: &Value) -> &'static str {
    match val_str(v).as_str() {
        "0" => "not-subscribed",
        "1" => "not-won",
        "2" => "won",
        _ => "unknown",
    }
}

// Fields that are unix timestamps (numeric) and should be formatted as RFC 3339.
const TS_FIELDS: &[&str] = &[
    "sub_date",
    "sub_end_date",
    "result_date",
    "mart_date",
    "mart_begin",
    "mart_end",
];

// Internal fields not useful to callers.
const SKIP_FIELDS: &[&str] = &[
    "code",
    "order_id",
    "sort",
    "watched",
    "remaining_second",
    "remaining_day",
];

fn transform_ipo_list_item(item: &Value) -> Value {
    let mut obj = Map::new();
    if let Some(map) = item.as_object() {
        for (k, v) in map {
            if SKIP_FIELDS.contains(&k.as_str()) {
                continue;
            }
            if k == "counter_id" {
                obj.insert(
                    "symbol".to_string(),
                    Value::String(counter_id_to_symbol(&val_str(v))),
                );
            } else if k == "state_stage" {
                obj.insert(
                    "state".to_string(),
                    Value::String(state_stage_label(v).to_string()),
                );
            } else if k == "state" {
                obj.insert(k.clone(), Value::String(state_label(v).to_string()));
            } else if k == "sub_state" {
                obj.insert(k.clone(), Value::String(sub_state_label(v).to_string()));
            } else if k == "mart_status" {
                let label = if val_str(v) == "1" { "open" } else { "closed" };
                obj.insert(k.clone(), Value::String(label.to_string()));
            } else if TS_FIELDS.contains(&k.as_str()) && v.is_number() {
                obj.insert(k.clone(), Value::String(fmt_ts(v)));
            } else if k == "ipo_date" {
                let s = val_str(v);
                let formatted = if s.len() == 8 {
                    format!("{}-{}-{}", &s[..4], &s[4..6], &s[6..])
                } else {
                    s
                };
                obj.insert(k.clone(), Value::String(formatted));
            } else {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
    Value::Object(obj)
}

// Transform an IPO order item: counter_id → symbol, format created_at timestamp.
fn transform_order_item(item: &Value) -> Value {
    let mut obj = Map::new();
    if let Some(map) = item.as_object() {
        for (k, v) in map {
            match k.as_str() {
                "counter_id" => {
                    obj.insert(
                        "symbol".to_string(),
                        Value::String(counter_id_to_symbol(&val_str(v))),
                    );
                }
                "created_at" => {
                    obj.insert(k.clone(), Value::String(fmt_ts(v)));
                }
                _ => {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }
    }
    Value::Object(obj)
}

async fn member_id() -> Result<i64> {
    crate::openapi::quote()
        .member_id()
        .await
        .map_err(anyhow::Error::from)
}

// ── read-only IPO list commands ────────────────────────────────────────────────

/// List IPO stocks currently in subscription or pre-filing stage (HK and US).
pub async fn cmd_ipo_subscriptions(format: &OutputFormat, verbose: bool) -> Result<()> {
    let mid = member_id().await?;
    let mid_str = mid.to_string();
    let hk_params = [("memebr_id", mid_str.as_str())];
    let (hk_data, us_data) = tokio::join!(
        http_get("/v1/ipo/subscriptions", &hk_params, verbose),
        http_get("/v1/ipo/us/subscriptions", &[], verbose),
    );
    let hk_data = hk_data?;
    let us_data = us_data?;
    match format {
        OutputFormat::Json => {
            let mut result = serde_json::Map::new();
            if let Some(list) = hk_data["list"].as_array() {
                let transformed: Vec<Value> = list.iter().map(transform_subscription).collect();
                result.insert("hk".to_string(), Value::Array(transformed));
            }
            if let Some(list) = us_data["list"].as_array() {
                let transformed: Vec<Value> = list.iter().map(transform_subscription).collect();
                result.insert("us".to_string(), Value::Array(transformed));
            }
            print_json(&Value::Object(result));
        }
        OutputFormat::Pretty => {
            let mut printed = false;
            if let Some(list) = hk_data["list"].as_array() {
                if !list.is_empty() {
                    println!("── HK ──");
                    let headers = [
                        "name",
                        "symbol",
                        "currency",
                        "entrance_fee",
                        "est_sub",
                        "fin_rate",
                        "max_lev",
                        "issue_price",
                        "deadline",
                        "state",
                    ];
                    let rows: Vec<Vec<String>> = list
                        .iter()
                        .map(|item| {
                            let stage = state_stage_label(&item["state_stage"]).to_string();
                            let tags: &[Value] =
                                item["tags"].as_array().map_or(&[], |a| a.as_slice());
                            let fin_rate = extract_tag(tags, "融资利率");
                            let max_lev = extract_tag(tags, "杠杆");
                            vec![
                                val_str(&item["name"]),
                                counter_id_to_symbol(&val_str(&item["counter_id"])),
                                val_str(&item["currency"]),
                                val_str(&item["entrance_fee"]),
                                val_str(&item["rate_forcast"]),
                                fin_rate,
                                max_lev,
                                val_str(&item["issue_price"]),
                                fmt_ts(&item["sub_deadline"]),
                                stage,
                            ]
                        })
                        .collect();
                    print_table(&headers, rows, &OutputFormat::Pretty);
                    printed = true;
                }
            }
            if let Some(list) = us_data["list"].as_array() {
                if !list.is_empty() {
                    if printed {
                        println!();
                    }
                    println!("── US ──");
                    let headers = [
                        "name",
                        "symbol",
                        "currency",
                        "issue_price",
                        "deadline",
                        "state",
                    ];
                    let rows: Vec<Vec<String>> = list
                        .iter()
                        .map(|item| {
                            let stage = state_stage_label(&item["state_stage"]).to_string();
                            vec![
                                val_str(&item["name"]),
                                counter_id_to_symbol(&val_str(&item["counter_id"])),
                                val_str(&item["currency"]),
                                val_str(&item["issue_price"]),
                                fmt_ts(&item["sub_deadline"]),
                                stage,
                            ]
                        })
                        .collect();
                    print_table(&headers, rows, &OutputFormat::Pretty);
                    printed = true;
                }
            }
            if !printed {
                println!("No active IPO subscriptions.");
            }
        }
    }
    Ok(())
}

/// List IPO stocks in the wait-listing (grey market) stage (HK and US).
pub async fn cmd_ipo_wait_listing(format: &OutputFormat, verbose: bool) -> Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mid = member_id().await?;
    let mid_str = mid.to_string();
    let day_str = now.to_string();
    let hk_params = [
        ("day_time", day_str.as_str()),
        ("memebr_id", mid_str.as_str()),
    ];
    let (hk_data, us_data) = tokio::join!(
        http_get("/v1/ipo/wait-listing", &hk_params, verbose),
        http_get("/v1/ipo/us/wait-listing", &[], verbose),
    );
    let hk_data = hk_data?;
    let us_data = us_data?;
    let wait_list_row = |item: &Value| -> Vec<String> {
        vec![
            val_str(&item["name"]),
            counter_id_to_symbol(&val_str(&item["counter_id"])),
            val_str(&item["issue_price"]),
            fmt_date_opt(&item["ipo_date"]),
            state_stage_label(&item["state_stage"]).to_string(),
        ]
    };
    match format {
        OutputFormat::Json => {
            let mut result = serde_json::Map::new();
            if let Some(list) = hk_data["ipos"].as_array() {
                let transformed: Vec<Value> = list.iter().map(transform_ipo_list_item).collect();
                result.insert("hk".to_string(), Value::Array(transformed));
            }
            if let Some(list) = us_data["ipos"].as_array() {
                let transformed: Vec<Value> = list.iter().map(transform_ipo_list_item).collect();
                result.insert("us".to_string(), Value::Array(transformed));
            }
            print_json(&Value::Object(result));
        }
        OutputFormat::Pretty => {
            let headers = ["name", "symbol", "issue_price", "ipo_date", "state"];
            let mut printed = false;
            if let Some(list) = hk_data["ipos"].as_array() {
                if !list.is_empty() {
                    println!("── HK ──");
                    let rows: Vec<Vec<String>> = list.iter().map(wait_list_row).collect();
                    print_table(&headers, rows, &OutputFormat::Pretty);
                    printed = true;
                }
            }
            if let Some(list) = us_data["ipos"].as_array() {
                if !list.is_empty() {
                    if printed {
                        println!();
                    }
                    println!("── US ──");
                    let rows: Vec<Vec<String>> = list.iter().map(wait_list_row).collect();
                    print_table(&headers, rows, &OutputFormat::Pretty);
                    printed = true;
                }
            }
            if !printed {
                println!("No IPO stocks in wait-listing.");
            }
        }
    }
    Ok(())
}

fn hk_listed_row(item: &Value) -> Vec<String> {
    let date = fmt_date_opt(&item["ipo_date"]);
    let amount = val_str(&item["amount"]).parse::<u64>().map_or_else(
        |_| val_str(&item["amount"]),
        crate::utils::number::format_volume,
    );
    vec![
        val_str(&item["name"]),
        counter_id_to_symbol(&val_str(&item["counter_id"])),
        val_str(&item["issue_price"]),
        val_str(&item["last_done"]),
        val_str(&item["prev_close"]),
        val_str(&item["ipo_change"]),
        amount,
        date,
    ]
}

fn us_listed_row(item: &Value) -> Vec<String> {
    let date = fmt_date_opt(&item["ipo_date"]);
    let amount = val_str(&item["amount"]).parse::<u64>().map_or_else(
        |_| val_str(&item["amount"]),
        crate::utils::number::format_volume,
    );
    vec![
        val_str(&item["name"]),
        counter_id_to_symbol(&val_str(&item["counter_id"])),
        val_str(&item["issue_price"]),
        val_str(&item["last_done"]),
        val_str(&item["prev_close"]),
        val_str(&item["ipo_change"]),
        amount,
        date,
    ]
}

/// List recently listed IPO stocks (HK and US).
pub async fn cmd_ipo_listed(
    page: u32,
    limit: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let mid = member_id().await?;
    let mid_str = mid.to_string();
    let page_str = page.to_string();
    let size_str = limit.to_string();
    let hk_params = [
        ("page", page_str.as_str()),
        ("size", size_str.as_str()),
        ("memebr_id", mid_str.as_str()),
    ];
    let us_params = [("page", page_str.as_str()), ("size", size_str.as_str())];
    let (hk_data, us_data) = tokio::join!(
        http_get("/v1/ipo/listed", &hk_params, verbose),
        http_get("/v1/ipo/us/listed", &us_params, verbose),
    );
    let hk_data = hk_data?;
    let us_data = us_data?;
    match format {
        OutputFormat::Json => {
            let mut result = serde_json::Map::new();
            if let Some(list) = hk_data["list"].as_array() {
                let transformed: Vec<Value> = list.iter().map(transform_ipo_list_item).collect();
                result.insert("hk".to_string(), Value::Array(transformed));
            }
            if let Some(list) = us_data["list"].as_array() {
                let transformed: Vec<Value> = list.iter().map(transform_ipo_list_item).collect();
                result.insert("us".to_string(), Value::Array(transformed));
            }
            print_json(&Value::Object(result));
        }
        OutputFormat::Pretty => {
            let mut printed = false;
            if let Some(list) = hk_data["list"].as_array() {
                if !list.is_empty() {
                    println!("── HK ──");
                    let headers = [
                        "name",
                        "symbol",
                        "issue_price",
                        "last_done",
                        "prev_close",
                        "change%",
                        "amount",
                        "ipo_date",
                    ];
                    let rows: Vec<Vec<String>> = list.iter().map(hk_listed_row).collect();
                    print_table(&headers, rows, &OutputFormat::Pretty);
                    printed = true;
                }
            }
            if let Some(list) = us_data["list"].as_array() {
                if !list.is_empty() {
                    if printed {
                        println!();
                    }
                    println!("── US ──");
                    let headers = [
                        "name",
                        "symbol",
                        "issue_price",
                        "last_done",
                        "prev_close",
                        "change%",
                        "amount",
                        "ipo_date",
                    ];
                    let rows: Vec<Vec<String>> = list.iter().map(us_listed_row).collect();
                    print_table(&headers, rows, &OutputFormat::Pretty);
                    printed = true;
                }
            }
            if !printed {
                println!("No listed IPO stocks found.");
            }
        }
    }
    Ok(())
}

/// Show the IPO calendar (all upcoming and recent IPOs).
pub async fn cmd_ipo_calendar(format: &OutputFormat, verbose: bool) -> Result<()> {
    let data = http_get("/v1/ipo/calendar", &[], verbose).await?;
    match format {
        OutputFormat::Json => {
            if let Some(list) = data["list"].as_array() {
                let transformed: Vec<Value> = list.iter().map(transform_ipo_list_item).collect();
                print_json(&Value::Array(transformed));
            } else {
                print_json(&data);
            }
        }
        OutputFormat::Pretty => {
            if let Some(list) = data["list"].as_array() {
                if list.is_empty() {
                    println!("No IPO calendar entries found.");
                    return Ok(());
                }
                let headers = [
                    "name",
                    "symbol",
                    "state",
                    "sub_date",
                    "sub_end_date",
                    "ipo_date",
                ];
                let mut hk_rows: Vec<Vec<String>> = Vec::new();
                let mut us_rows: Vec<Vec<String>> = Vec::new();
                let mut other_rows: Vec<Vec<String>> = Vec::new();
                for item in list {
                    let cid = val_str(&item["counter_id"]);
                    let row = vec![
                        val_str(&item["name"]),
                        counter_id_to_symbol(&cid),
                        state_stage_label(&item["state_stage"]).to_string(),
                        fmt_date_opt(&item["sub_date"]),
                        fmt_date_opt(&item["sub_end_date"]),
                        fmt_date_opt(&item["ipo_date"]),
                    ];
                    if cid.contains("/HK/") {
                        hk_rows.push(row);
                    } else if cid.contains("/US/") {
                        us_rows.push(row);
                    } else {
                        other_rows.push(row);
                    }
                }
                let mut printed = false;
                if !hk_rows.is_empty() {
                    println!("── HK ──");
                    print_table(&headers, hk_rows, &OutputFormat::Pretty);
                    printed = true;
                }
                if !us_rows.is_empty() {
                    if printed {
                        println!();
                    }
                    println!("── US ──");
                    print_table(&headers, us_rows, &OutputFormat::Pretty);
                    printed = true;
                }
                if !other_rows.is_empty() {
                    if printed {
                        println!();
                    }
                    print_table(&headers, other_rows, &OutputFormat::Pretty);
                }
            } else {
                print_json(&data);
            }
        }
    }
    Ok(())
}

/// Show IPO detail: profile (prospectus summary) + timeline for a symbol.
pub async fn cmd_ipo_detail(
    symbol: String,
    market: &str,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = symbol_to_counter_id(&symbol);
    // Auto-detect market from symbol suffix (e.g. SUJA.US → US); fall back to --market arg.
    let detected = symbol.rsplit_once('.').map(|(_, m)| m.to_uppercase());
    let market = match detected.as_deref() {
        Some("US") => "US",
        Some("HK") => "HK",
        _ => market,
    };
    let account_channel = crate::auth::account_channel_or_default();
    let profile_result =
        http_get("/v1/ipo/profile", &[("counter_id", cid.as_str())], verbose).await;
    let profile_key = if market == "US" { "us" } else { "hk" };
    let profile_data = match profile_result {
        Ok(v) => v,
        Err(e) => {
            // A business-level error from the OpenAPI layer (code != 0) means the
            // stock has no IPO data; surface everything else as a real error.
            if e.downcast_ref::<longbridge::httpclient::HttpClientError>()
                .is_some_and(|he| {
                    matches!(he, longbridge::httpclient::HttpClientError::OpenApi { .. })
                })
            {
                println!("No IPO detail found for {symbol}.");
                return Ok(());
            }
            return Err(e);
        }
    };
    if profile_data[profile_key].is_null() {
        println!("No IPO detail found for {symbol}.");
        return Ok(());
    }
    let timeline_params = [
        ("counter_id", cid.as_str()),
        ("market", market),
        ("flag", "0"),
    ];
    let eligibility_params = [("counter_id", cid.as_str())];
    let holdings_params = [
        ("counter_id", cid.as_str()),
        ("need_realtime", "true"),
        ("account_channel", account_channel.as_str()),
    ];
    let (timeline_data, eligibility_data, holdings_data) = tokio::join!(
        http_get("/v1/ipo/timeline", &timeline_params, verbose),
        http_get("/v1/ipo/eligibility", &eligibility_params, verbose),
        http_get("/v1/ipo/holdings", &holdings_params, verbose),
    );
    let timeline_data = timeline_data?;
    let eligibility_data = eligibility_data?;
    let holdings_data = holdings_data?;
    match format {
        OutputFormat::Json => {
            let mut result = serde_json::Map::new();
            result.insert("profile".to_string(), profile_data);
            result.insert("timeline".to_string(), timeline_data);
            result.insert("eligibility".to_string(), eligibility_data);
            result.insert("holdings".to_string(), holdings_data);
            print_json(&Value::Object(result));
        }
        OutputFormat::Pretty => {
            let kv = |label: &str, value: &str| {
                println!("{:<24}{value}", format!("{label}:"));
            };
            let trunc = |s: String, max: usize| -> String {
                if s.chars().count() > max {
                    format!("{}…", s.chars().take(max).collect::<String>())
                } else {
                    s
                }
            };
            let str_list = |arr: Option<&Vec<Value>>| -> String {
                arr.map(|a| {
                    a.iter()
                        .filter_map(|x| {
                            let s = if x.is_string() {
                                val_str(x)
                            } else {
                                val_str(&x["name"])
                            };
                            if s == "-" || s.is_empty() {
                                None
                            } else {
                                Some(s)
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default()
            };

            let profile = &profile_data[profile_key];
            {
                let ipo_date = fmt_date_opt(&profile["ipo_date"]);
                if ipo_date != "-" {
                    kv("IPO Date", &ipo_date);
                }

                let currency = val_str(&profile["issue_currency"]);
                let issue_price = val_str(&profile["issue_price"]);
                if issue_price != "-" && !issue_price.is_empty() {
                    let price_str = if currency != "-" && !currency.is_empty() {
                        format!("{issue_price} {currency}")
                    } else {
                        issue_price
                    };
                    kv("Issue Price", &price_str);
                }
                if profile["show_mart"].as_bool().unwrap_or(false) {
                    let mart_begin = fmt_ts(&profile["mart_begin"]);
                    let mart_end = fmt_ts(&profile["mart_end"]);
                    if mart_begin != "-" {
                        kv("Grey Market", &format!("{mart_begin} – {mart_end}"));
                    }
                }
                let trade_unit = val_str(&profile["trade_unit"]);
                if trade_unit != "-" && !trade_unit.is_empty() && trade_unit != "0" {
                    kv("Trade Unit (Lot)", &trade_unit);
                }
                let proceeds = val_str(&profile["proceeds_planned"]);
                if proceeds != "-" && !proceeds.is_empty() {
                    kv("Proceeds Planned", &proceeds);
                }

                let industry = val_str(&profile["industry"]);
                if industry != "-" && !industry.is_empty() {
                    kv("Industry", &industry);
                }

                for (key, label) in &[
                    ("margin_multiple", "Margin Multiple"),
                    ("margin_sub", "Margin Sub"),
                ] {
                    let v = val_str(&profile[*key]);
                    if v != "-" && !v.is_empty() {
                        kv(label, &v);
                    }
                }
                let sponsors = str_list(profile["sponsor"].as_array());
                if !sponsors.is_empty() {
                    kv("Sponsor", &sponsors);
                }

                let investors = str_list(profile["investors"].as_array());
                if !investors.is_empty() {
                    kv("Cornerstone Investors", &investors);
                }

                if let Some(uw) = profile["underwriter"].as_array() {
                    if !uw.is_empty() {
                        let names: Vec<String> = uw
                            .iter()
                            .take(5)
                            .filter_map(|x| {
                                let s = if x.is_string() {
                                    val_str(x)
                                } else {
                                    val_str(&x["name"])
                                };
                                if s == "-" || s.is_empty() {
                                    None
                                } else {
                                    Some(s)
                                }
                            })
                            .collect();
                        let mut label = names.join(", ");
                        if uw.len() > 5 {
                            use std::fmt::Write as _;
                            let _ = write!(label, " (+{})", uw.len() - 5);
                        }
                        if !label.is_empty() {
                            kv("Underwriters", &label);
                        }
                    }
                }
                let prospectus = val_str(&profile["prospectus"]);
                if prospectus != "-" && !prospectus.is_empty() {
                    kv("Prospectus", &prospectus);
                }

                let recommend_url = val_str(&profile["recommend_url"]);
                if recommend_url != "-" && !recommend_url.is_empty() {
                    kv("Research", &recommend_url);
                }

                let profile_text = val_str(&profile["profile"]);
                if profile_text != "-" && !profile_text.is_empty() {
                    kv("Description", &trunc(profile_text, 200));
                }
                let rec = val_str(&profile["rec_purposes"]);
                if rec != "-" && !rec.is_empty() {
                    kv("Use of Proceeds", &trunc(rec, 200));
                }
                println!();
            }
            // Eligibility + timeline meta
            let eligible = eligibility_data["can_subscribe"].as_bool();
            let can_sub = timeline_data["can_subscribe"].as_bool().unwrap_or(false);
            let pay_end = fmt_datetime_hkt(&timeline_data["pay_end_date"]);
            let pay_end_valid = pay_end != "-" && !pay_end.is_empty();
            if eligible.is_some() || can_sub || pay_end_valid {
                if let Some(e) = eligible {
                    kv("Can Subscribe", if e { "Yes" } else { "No" });
                }
                if pay_end_valid {
                    kv("Payment Deadline", &pay_end);
                }
                println!();
            }
            if let Some(timeline) = timeline_data["timeline"].as_array() {
                if !timeline.is_empty() {
                    let headers = ["time", "event"];
                    let rows: Vec<Vec<String>> = timeline
                        .iter()
                        .map(|item| {
                            vec![
                                val_str(&item["time"]).replace('\n', " "),
                                val_str(&item["name"]),
                            ]
                        })
                        .collect();
                    print_table(&headers, rows, &OutputFormat::Pretty);
                }
            } else {
                print_json(&timeline_data);
            }
            // Holdings section
            if !holdings_data.is_null() && holdings_data.as_object().is_some() {
                let max_purchase = val_str(&holdings_data["ipo_max_purchase"]);
                let total = val_str(&holdings_data["total_amount"]);
                let current = val_str(&holdings_data["current_amount"]);
                let fee_rate = val_str(&holdings_data["finance_fee_rate"]);
                let has_data = [&max_purchase, &total, &current, &fee_rate]
                    .iter()
                    .any(|v| *v != "-" && !v.is_empty() && *v != "0" && *v != "0.00");
                if has_data {
                    if max_purchase != "-" && !max_purchase.is_empty() && max_purchase != "0" {
                        kv("Max Purchase", &max_purchase);
                    }
                    if total != "-" && !total.is_empty() && total != "0.00" {
                        kv("Total Amount", &total);
                    }
                    if current != "-" && !current.is_empty() && current != "0.00" {
                        kv("Current Amount", &current);
                    }
                    if fee_rate != "-" && !fee_rate.is_empty() {
                        kv("Finance Fee Rate", &fee_rate);
                    }
                }
            }
        }
    }
    Ok(())
}

/// List IPO orders (active + history) for the current account.
pub async fn cmd_ipo_orders(
    symbol: Option<String>,
    market: Option<String>,
    status: Option<String>,
    page: u32,
    count: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let account_channel = crate::auth::account_channel_or_default();
    let mut active_params: Vec<(&str, &str)> = vec![("account_channel", account_channel.as_str())];
    let cid;
    if let Some(ref sym) = symbol {
        cid = symbol_to_counter_id(sym);
        active_params.push(("counter_id", cid.as_str()));
    }
    let page_str = page.to_string();
    let count_str = count.to_string();
    let mut hist_params: Vec<(&str, &str)> =
        vec![("page", page_str.as_str()), ("limit", count_str.as_str())];
    if let Some(ref m) = market {
        hist_params.push(("market", m.as_str()));
    }
    if let Some(ref s) = status {
        hist_params.push(("status", s.as_str()));
    }
    let (active_data, hist_data) = tokio::join!(
        http_get("/v1/ipo/orders", &active_params, verbose),
        http_get("/v1/ipo/orders/history", &hist_params, verbose),
    );
    let active_data = active_data?;
    let hist_data = hist_data?;
    match format {
        OutputFormat::Json => {
            let mut result = serde_json::Map::new();
            if let Some(arr) = active_data["orders"].as_array() {
                let transformed: Vec<Value> = arr.iter().map(transform_order_item).collect();
                result.insert("orders".to_string(), Value::Array(transformed));
            }
            if let Some(arr) = hist_data["orders"].as_array() {
                let transformed: Vec<Value> = arr.iter().map(transform_order_item).collect();
                result.insert("history".to_string(), Value::Array(transformed));
            }
            print_json(&Value::Object(result));
        }
        OutputFormat::Pretty => {
            let mut printed = false;
            if let Some(orders) = active_data["orders"].as_array() {
                if !orders.is_empty() {
                    println!("── Active ──");
                    let headers = ["id", "symbol", "name", "qty", "status", "date"];
                    let rows: Vec<Vec<String>> = orders
                        .iter()
                        .map(|o| {
                            vec![
                                val_str(&o["id"]),
                                counter_id_to_symbol(&val_str(&o["counter_id"])),
                                val_str(&o["name"]),
                                val_str(&o["sub_qty"]),
                                val_str(&o["status"]),
                                fmt_ts(&o["created_at"]),
                            ]
                        })
                        .collect();
                    print_table(&headers, rows, &OutputFormat::Pretty);
                    printed = true;
                }
            }
            if let Some(arr) = hist_data["orders"].as_array() {
                if !arr.is_empty() {
                    if printed {
                        println!();
                    }
                    println!("── History ──");
                    let headers = ["id", "symbol", "name", "qty", "won", "status", "date"];
                    let rows: Vec<Vec<String>> = arr
                        .iter()
                        .map(|o| {
                            vec![
                                val_str(&o["id"]),
                                counter_id_to_symbol(&val_str(&o["counter_id"])),
                                val_str(&o["name"]),
                                val_str(&o["sub_qty"]),
                                val_str(&o["lot_win_qty"]),
                                val_str(&o["status"]),
                                fmt_ts(&o["created_at"]),
                            ]
                        })
                        .collect();
                    print_table(&headers, rows, &OutputFormat::Pretty);
                    printed = true;
                }
            }
            if !printed {
                println!("No IPO orders found.");
            }
        }
    }
    Ok(())
}

/// Show IPO order detail by order ID.
pub async fn cmd_ipo_order_detail(
    order_id: String,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let account_channel = crate::auth::account_channel_or_default();
    let path = format!("/v1/ipo/orders/{order_id}");
    let data = http_get(
        &path,
        &[("account_channel", account_channel.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            let kv = |label: &str, value: &str| {
                println!("{:<24}{value}", format!("{label}:"));
            };
            kv(
                "Symbol",
                &counter_id_to_symbol(&val_str(&data["counter_id"])),
            );
            kv("Name", &val_str(&data["name"]));
            kv("Market", &val_str(&data["market"]));
            let ipo_date = fmt_date_opt(&data["ipo_date"]);
            if ipo_date != "-" {
                kv("IPO Date", &ipo_date);
            }
            let ipo_price = val_str(&data["ipo_price"]);
            if ipo_price != "-" && !ipo_price.is_empty() {
                kv(
                    "IPO Price",
                    &format!("{} {}", ipo_price, val_str(&data["currency"])),
                );
            }
            kv("Status", &val_str(&data["status"]));
            kv("Sub Qty", &val_str(&data["sub_qty"]));
            let won = val_str(&data["lot_win_qty"]);
            if won != "-" && won != "0" {
                kv("Won Qty", &won);
            }
            let sub_amount = val_str(&data["sub_amount"]);
            if sub_amount != "-" && sub_amount != "0.00" {
                kv(
                    "Sub Amount",
                    &format!("{} {}", sub_amount, val_str(&data["currency"])),
                );
            }
            let sub_fee = val_str(&data["sub_fee"]);
            if sub_fee != "-" && sub_fee != "0.00" {
                kv(
                    "Sub Fee",
                    &format!("{} {}", sub_fee, val_str(&data["currency"])),
                );
            }
            let total = val_str(&data["total_amount"]);
            if total != "-" && total != "0.00" {
                kv(
                    "Total Amount",
                    &format!("{} {}", total, val_str(&data["currency"])),
                );
            }
            let need_to_pay = val_str(&data["need_to_pay"]);
            if need_to_pay != "-" && need_to_pay != "0.00" {
                kv(
                    "Need to Pay",
                    &format!("{} {}", need_to_pay, val_str(&data["currency"])),
                );
            }
            let refund = val_str(&data["refund_amount"]);
            if refund != "-" && refund != "0.00" {
                kv(
                    "Refund",
                    &format!("{} {}", refund, val_str(&data["currency"])),
                );
            }
            let mart_begin = fmt_ts(&data["mart_begin"]);
            let mart_end = fmt_ts(&data["mart_end"]);
            if mart_begin != "-" {
                kv("Grey Market", &format!("{mart_begin} – {mart_end}"));
            }
            if let Some(timeline) = data["timeline"].as_array() {
                if !timeline.is_empty() {
                    println!();
                    let headers = ["time", "event"];
                    let rows: Vec<Vec<String>> = timeline
                        .iter()
                        .map(|item| {
                            vec![
                                val_str(&item["time"]).replace('\n', " "),
                                val_str(&item["desc"]),
                            ]
                        })
                        .collect();
                    print_table(&headers, rows, &OutputFormat::Pretty);
                }
            }
        }
    }
    Ok(())
}

/// Show IPO profit/loss summary + items for the given period.
pub async fn cmd_ipo_profit_loss(
    period: &str,
    page: u32,
    limit: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let account_channel = crate::auth::account_channel_or_default();
    let page_str = page.to_string();
    let size_str = limit.to_string();
    let summary_params = [
        ("period", period),
        ("account_channel", account_channel.as_str()),
    ];
    let items_params = [
        ("period", period),
        ("page", page_str.as_str()),
        ("size", size_str.as_str()),
        ("account_channel", account_channel.as_str()),
    ];
    let (summary_data, items_data) = tokio::join!(
        http_get("/v1/ipo/profit-loss", &summary_params, verbose),
        http_get("/v1/ipo/profit-loss/items", &items_params, verbose),
    );
    let summary_data = summary_data?;
    let items_data = items_data?;
    let mut summary = serde_json::Map::new();
    if let Some(obj) = summary_data.as_object() {
        for (k, v) in obj {
            if k == "updated_at" {
                summary.insert(k.clone(), Value::String(fmt_ts(v)));
            } else {
                summary.insert(k.clone(), v.clone());
            }
        }
    }
    let summary_val = Value::Object(summary);
    match format {
        OutputFormat::Json => {
            let mut result = serde_json::Map::new();
            result.insert("summary".to_string(), summary_val);
            result.insert("items".to_string(), items_data);
            print_json(&Value::Object(result));
        }
        OutputFormat::Pretty => {
            print_json_value(&summary_val, format);
            if let Some(items) = items_data["items"].as_array() {
                if !items.is_empty() {
                    println!();
                    let headers = ["symbol", "name", "qty", "cost", "profit", "rate"];
                    let rows: Vec<Vec<String>> = items
                        .iter()
                        .map(|item| {
                            vec![
                                counter_id_to_symbol(&val_str(&item["counter_id"])),
                                val_str(&item["name"]),
                                val_str(&item["qty"]),
                                val_str(&item["cost_price"]),
                                val_str(&item["profit_loss"]),
                                val_str(&item["profit_loss_rate"]),
                            ]
                        })
                        .collect();
                    print_table(&headers, rows, &OutputFormat::Pretty);
                }
            } else if !items_data.is_null() {
                print_json_value(&items_data, format);
            }
        }
    }
    Ok(())
}

// ── US IPO commands ───────────────────────────────────────────────────────────

/// List US IPO stocks currently in subscription stage.
pub async fn cmd_ipo_us_subscriptions(format: &OutputFormat, verbose: bool) -> Result<()> {
    let data = http_get("/v1/ipo/us/subscriptions", &[], verbose).await?;
    match format {
        OutputFormat::Json => {
            if let Some(list) = data["list"].as_array() {
                let transformed: Vec<Value> = list.iter().map(transform_subscription).collect();
                print_json(&Value::Array(transformed));
            } else {
                print_json(&data);
            }
        }
        OutputFormat::Pretty => {
            if let Some(list) = data["list"].as_array() {
                if list.is_empty() {
                    println!("No active US IPO subscriptions.");
                    return Ok(());
                }
                let headers = [
                    "name",
                    "symbol",
                    "currency",
                    "issue_price",
                    "deadline",
                    "state",
                ];
                let rows: Vec<Vec<String>> = list
                    .iter()
                    .map(|item| {
                        let stage = state_stage_label(&item["state_stage"]).to_string();
                        vec![
                            val_str(&item["name"]),
                            counter_id_to_symbol(&val_str(&item["counter_id"])),
                            val_str(&item["currency"]),
                            val_str(&item["issue_price"]),
                            fmt_ts(&item["sub_deadline"]),
                            stage,
                        ]
                    })
                    .collect();
                print_table(&headers, rows, &OutputFormat::Pretty);
            } else {
                print_json(&data);
            }
        }
    }
    Ok(())
}

/// List US IPO stocks in wait-listing stage.
pub async fn cmd_ipo_us_wait_listing(format: &OutputFormat, verbose: bool) -> Result<()> {
    let data = http_get("/v1/ipo/us/wait-listing", &[], verbose).await?;
    match format {
        OutputFormat::Json => {
            if let Some(list) = data["ipos"].as_array() {
                let transformed: Vec<Value> = list.iter().map(transform_ipo_list_item).collect();
                print_json(&Value::Array(transformed));
            } else {
                print_json(&data);
            }
        }
        OutputFormat::Pretty => {
            if let Some(list) = data["ipos"].as_array() {
                if list.is_empty() {
                    println!("No US IPO stocks in wait-listing.");
                    return Ok(());
                }
                let headers = ["name", "symbol", "issue_price", "ipo_date", "state"];
                let rows: Vec<Vec<String>> = list
                    .iter()
                    .map(|item| {
                        vec![
                            val_str(&item["name"]),
                            counter_id_to_symbol(&val_str(&item["counter_id"])),
                            val_str(&item["issue_price"]),
                            fmt_date_opt(&item["ipo_date"]),
                            state_stage_label(&item["state_stage"]).to_string(),
                        ]
                    })
                    .collect();
                print_table(&headers, rows, &OutputFormat::Pretty);
            } else {
                print_json(&data);
            }
        }
    }
    Ok(())
}

/// List recently listed US IPO stocks.
pub async fn cmd_ipo_us_listed(
    page: u32,
    limit: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let page_str = page.to_string();
    let size_str = limit.to_string();
    let data = http_get(
        "/v1/ipo/us/listed",
        &[("page", page_str.as_str()), ("size", size_str.as_str())],
        verbose,
    )
    .await?;
    match format {
        OutputFormat::Json => {
            if let Some(list) = data["list"].as_array() {
                let transformed: Vec<Value> = list.iter().map(transform_ipo_list_item).collect();
                print_json(&Value::Array(transformed));
            } else {
                print_json(&data);
            }
        }
        OutputFormat::Pretty => {
            if let Some(list) = data["list"].as_array() {
                if list.is_empty() {
                    println!("No listed US IPO stocks found.");
                    return Ok(());
                }
                let headers = [
                    "name",
                    "symbol",
                    "issue_price",
                    "last_done",
                    "prev_close",
                    "change%",
                    "amount",
                    "ipo_date",
                ];
                let rows: Vec<Vec<String>> = list.iter().map(us_listed_row).collect();
                print_table(&headers, rows, &OutputFormat::Pretty);
            } else {
                print_json(&data);
            }
        }
    }
    Ok(())
}
