use anyhow::Result;

use super::{
    api::{http_get, http_post},
    output::{print_json_value, print_table},
    DcaCmd, DcaDayOfWeek, DcaFrequency, DcaReminderHours, OutputFormat,
};

use crate::utils::counter::{counter_id_to_symbol, symbol_to_counter_id};
use crate::utils::datetime::format_timestamp;

pub async fn cmd_dca(
    cmd: Option<DcaCmd>,
    status: Option<&str>,
    symbol: Option<&str>,
    page: u32,
    limit: u32,
    format: &OutputFormat,
) -> Result<()> {
    match cmd {
        None => cmd_list(status, symbol, page, limit, format).await,
        Some(DcaCmd::Create {
            symbol,
            amount,
            frequency,
            day_of_week,
            day_of_month,
            allow_margin,
        }) => {
            cmd_create(
                symbol,
                amount,
                frequency,
                day_of_week,
                day_of_month,
                allow_margin,
            )
            .await
        }
        Some(DcaCmd::Update {
            plan_id,
            amount,
            frequency,
            day_of_week,
            day_of_month,
            allow_margin,
        }) => {
            cmd_update(
                plan_id,
                amount,
                frequency,
                day_of_week,
                day_of_month,
                allow_margin,
            )
            .await
        }
        Some(DcaCmd::Pause { plan_id }) => cmd_toggle(plan_id, "Suspended").await,
        Some(DcaCmd::Resume { plan_id }) => cmd_toggle(plan_id, "Active").await,
        Some(DcaCmd::Stop { plan_id }) => cmd_toggle(plan_id, "Finished").await,
        Some(DcaCmd::History {
            plan_id,
            page,
            limit,
        }) => cmd_records(plan_id, page, limit, format).await,
        Some(DcaCmd::Stats { symbol }) => cmd_stats(symbol.as_deref(), format).await,
        Some(DcaCmd::CalcDate {
            symbol,
            frequency,
            day_of_week,
            day_of_month,
        }) => cmd_calc_date(symbol, frequency, day_of_week, day_of_month, format).await,
        Some(DcaCmd::Check { symbols }) => cmd_check(symbols, format).await,
        Some(DcaCmd::SetReminder { hours }) => cmd_set_reminder(&hours).await,
    }
}

async fn cmd_list(
    status: Option<&str>,
    symbol: Option<&str>,
    page: u32,
    limit: u32,
    format: &OutputFormat,
) -> Result<()> {
    let mut params: Vec<(&str, String)> =
        vec![("page", page.to_string()), ("limit", limit.to_string())];
    if let Some(s) = status {
        params.push(("status", s.to_string()));
    }
    if let Some(s) = symbol {
        params.push(("counter_id", symbol_to_counter_id(s)));
    }

    let param_refs: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let resp = http_get("/v1/dailycoins/query", &param_refs, false).await?;

    let plans = resp["plans"].as_array().cloned().unwrap_or_default();

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&plans)?);
        }
        OutputFormat::Pretty => {
            if plans.is_empty() {
                println!("No recurring investment plans found.");
                return Ok(());
            }
            let headers = &[
                "Plan ID",
                "Symbol",
                "Status",
                "Amount",
                "Frequency",
                "Day of Week",
                "Day of Month",
                "Next Trade Date",
                "Issues",
                "Cum Amount",
                "Cum Profit",
                "Avg Cost",
            ];
            let rows: Vec<Vec<String>> = plans
                .iter()
                .map(|p| {
                    vec![
                        p["plan_id"].as_str().unwrap_or("-").to_string(),
                        p["counter_id"]
                            .as_str()
                            .map_or_else(|| "-".to_string(), counter_id_to_symbol),
                        p["status"].as_str().unwrap_or("-").to_string(),
                        p["per_invest_amount"].as_str().unwrap_or("-").to_string(),
                        p["invest_frequency"].as_str().unwrap_or("-").to_string(),
                        p["invest_day_of_week"].as_str().unwrap_or("-").to_string(),
                        p["invest_day_of_month"].as_str().unwrap_or("-").to_string(),
                        p["next_trd_date"]
                            .as_str()
                            .and_then(|s| s.parse::<i64>().ok())
                            .map_or_else(|| "-".to_string(), format_timestamp),
                        p["issue_number"]
                            .as_u64()
                            .map_or_else(|| "-".to_string(), |n| n.to_string()),
                        p["cum_amount"].as_str().unwrap_or("-").to_string(),
                        p["cum_profit"].as_str().unwrap_or("-").to_string(),
                        p["average_cost"].as_str().unwrap_or("-").to_string(),
                    ]
                })
                .collect();
            print_table(headers, rows, format);
        }
    }
    Ok(())
}

async fn cmd_create(
    symbol: String,
    amount: String,
    frequency: DcaFrequency,
    day_of_week: Option<DcaDayOfWeek>,
    day_of_month: Option<String>,
    allow_margin: bool,
) -> Result<()> {
    let mut body = serde_json::json!({
        "counter_id": symbol_to_counter_id(&symbol),
        "per_invest_amount": amount,
        "invest_frequency": frequency.as_api_str(),
    });

    if let Some(dow) = day_of_week {
        body["invest_day_of_week"] = serde_json::Value::String(dow.as_api_str().to_string());
    }
    if let Some(dom) = day_of_month {
        body["invest_day_of_month"] = serde_json::Value::String(dom);
    }
    body["allow_margin_finance"] = serde_json::Value::Number(i32::from(allow_margin).into());

    let resp = http_post("/v1/dailycoins/create", body, false).await?;
    let plan_id = resp["plan_id"].as_str().unwrap_or("");
    println!("Recurring investment plan created. Plan ID: {plan_id}");
    Ok(())
}

async fn cmd_update(
    plan_id: String,
    amount: Option<String>,
    frequency: Option<DcaFrequency>,
    day_of_week: Option<DcaDayOfWeek>,
    day_of_month: Option<String>,
    allow_margin: Option<bool>,
) -> Result<()> {
    let mut body = serde_json::json!({ "plan_id": plan_id });

    if let Some(a) = amount {
        body["per_invest_amount"] = serde_json::Value::String(a);
    }
    if let Some(f) = frequency {
        body["invest_frequency"] = serde_json::Value::String(f.as_api_str().to_string());
    }
    if let Some(dow) = day_of_week {
        body["invest_day_of_week"] = serde_json::Value::String(dow.as_api_str().to_string());
    }
    if let Some(dom) = day_of_month {
        body["invest_day_of_month"] = serde_json::Value::String(dom);
    }
    if let Some(m) = allow_margin {
        body["allow_margin_finance"] = serde_json::Value::Number(i32::from(m).into());
    }

    http_post("/v1/dailycoins/update", body, false).await?;
    println!("Recurring investment plan {plan_id} updated.");
    Ok(())
}

async fn cmd_toggle(plan_id: String, status: &str) -> Result<()> {
    let body = serde_json::json!({
        "plan_id": plan_id,
        "status": status,
    });
    http_post("/v1/dailycoins/toggle", body, false).await?;
    println!("Recurring investment plan {plan_id} status set to {status}.");
    Ok(())
}

async fn cmd_records(plan_id: String, page: u32, limit: u32, format: &OutputFormat) -> Result<()> {
    let page_str = page.to_string();
    let limit_str = limit.to_string();
    let resp = http_get(
        "/v1/dailycoins/query-records",
        &[
            ("plan_id", plan_id.as_str()),
            ("page", &page_str),
            ("limit", &limit_str),
        ],
        false,
    )
    .await?;

    let records = resp["records"].as_array().cloned().unwrap_or_default();

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&records)?);
        }
        OutputFormat::Pretty => {
            if records.is_empty() {
                println!("No trade history found.");
                return Ok(());
            }
            let headers = &[
                "Date",
                "Symbol",
                "Order ID",
                "Status",
                "Action",
                "Exec Qty",
                "Exec Price",
                "Exec Amount",
                "Reject Reason",
            ];
            let rows: Vec<Vec<String>> = records
                .iter()
                .map(|r| {
                    vec![
                        r["created_at"]
                            .as_str()
                            .and_then(|s| s.parse::<i64>().ok())
                            .map_or_else(|| "-".to_string(), format_timestamp),
                        r["counter_id"]
                            .as_str()
                            .map_or_else(|| "-".to_string(), counter_id_to_symbol),
                        r["order_id"].as_str().unwrap_or("-").to_string(),
                        r["status"].as_str().unwrap_or("-").to_string(),
                        r["action"].as_str().unwrap_or("-").to_string(),
                        r["executed_qty"].as_str().unwrap_or("-").to_string(),
                        r["executed_price"].as_str().unwrap_or("-").to_string(),
                        r["executed_amount"].as_str().unwrap_or("-").to_string(),
                        r["rejected_reason"].as_str().unwrap_or("").to_string(),
                    ]
                })
                .collect();
            print_table(headers, rows, format);
        }
    }
    Ok(())
}

async fn cmd_stats(symbol: Option<&str>, format: &OutputFormat) -> Result<()> {
    let counter_id_str;
    let params: Vec<(&str, &str)> = if let Some(s) = symbol {
        counter_id_str = symbol_to_counter_id(s);
        vec![("counter_id", counter_id_str.as_str())]
    } else {
        vec![]
    };
    let resp = http_get("/v1/dailycoins/statistic", &params, false).await?;

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Pretty => {
            print_json_value(
                &serde_json::json!({
                    "total_amount": resp["total_amount"].as_str().unwrap_or("-"),
                    "total_profit": resp["total_profit"].as_str().unwrap_or("-"),
                    "active_count": resp["active_count"].as_str().unwrap_or("-"),
                    "suspended_count": resp["suspended_count"].as_str().unwrap_or("-"),
                    "finished_count": resp["finished_count"].as_str().unwrap_or("-"),
                    "rest_days": resp["rest_days"].as_str().unwrap_or("-"),
                }),
                format,
            );

            let nearest = resp["nearest_plans"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if !nearest.is_empty() {
                println!("\nNearest Plans:");
                let headers = &["Plan ID", "Symbol"];
                let rows: Vec<Vec<String>> = nearest
                    .iter()
                    .map(|p| {
                        vec![
                            p["plan_id"].as_str().unwrap_or("-").to_string(),
                            p["counter_id"]
                                .as_str()
                                .map_or_else(|| "-".to_string(), counter_id_to_symbol),
                        ]
                    })
                    .collect();
                print_table(headers, rows, format);
            }
        }
    }
    Ok(())
}

async fn cmd_calc_date(
    symbol: String,
    frequency: DcaFrequency,
    day_of_week: Option<DcaDayOfWeek>,
    day_of_month: Option<String>,
    format: &OutputFormat,
) -> Result<()> {
    let mut body = serde_json::json!({
        "counter_id": symbol_to_counter_id(&symbol),
        "invest_frequency": frequency.as_api_str(),
    });

    if let Some(dow) = day_of_week {
        body["invest_day_of_week"] = serde_json::Value::String(dow.as_api_str().to_string());
    }
    if let Some(dom) = day_of_month {
        body["invest_day_of_month"] = serde_json::Value::String(dom);
    }

    let resp = http_post("/v1/dailycoins/calc-trd-date", body, false).await?;

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Pretty => {
            let trade_date = resp["trade_date"].as_str().unwrap_or("-");
            let readable = trade_date
                .parse::<i64>()
                .map_or_else(|_| trade_date.to_string(), format_timestamp);
            println!("Next trade date: {readable}");
        }
    }
    Ok(())
}

async fn cmd_check(symbols: Vec<String>, format: &OutputFormat) -> Result<()> {
    let counter_ids: Vec<String> = symbols.iter().map(|s| symbol_to_counter_id(s)).collect();
    let body = serde_json::json!({ "counter_ids": counter_ids });
    let resp = http_post("/v1/dailycoins/batch-check-support", body, false).await?;

    let infos = resp["infos"].as_array().cloned().unwrap_or_default();

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&infos)?);
        }
        OutputFormat::Pretty => {
            if infos.is_empty() {
                println!("No results.");
                return Ok(());
            }
            let headers = &["Symbol", "Supports Recurring Investment"];
            let rows: Vec<Vec<String>> = infos
                .iter()
                .map(|info| {
                    vec![
                        info["counter_id"]
                            .as_str()
                            .map_or_else(|| "-".to_string(), counter_id_to_symbol),
                        if info["support_regular_saving"].as_bool().unwrap_or(false) {
                            "yes"
                        } else {
                            "no"
                        }
                        .to_string(),
                    ]
                })
                .collect();
            print_table(headers, rows, format);
        }
    }
    Ok(())
}

async fn cmd_set_reminder(hours: &DcaReminderHours) -> Result<()> {
    let h = hours.as_api_str();
    let body = serde_json::json!({ "alter_hours": h });
    http_post("/v1/dailycoins/update-alter-hours", body, false).await?;
    println!("Reminder hours updated to {h}h before trade.");
    Ok(())
}
