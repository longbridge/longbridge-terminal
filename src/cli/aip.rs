use anyhow::{bail, Result};
use serde_json::json;
use time::OffsetDateTime;

use super::{
    api::{http_get, http_post},
    output::print_table,
    AipCmd, OutputFormat,
};

// ── Enum helpers ─────────────────────────────────────────────────────────────

fn plan_status_name(status: u64) -> &'static str {
    match status {
        1 => "Active",
        2 => "Paused",
        3 => "Ended",
        _ => "Unknown",
    }
}

fn task_status_name(status: u64) -> &'static str {
    match status {
        1 => "Processing",
        2 => "Success",
        3 => "Failed",
        _ => "Unknown",
    }
}

fn cycle_name(cycle: u64) -> &'static str {
    match cycle {
        1 => "Daily",
        2 => "Weekly",
        3 => "Biweekly",
        4 => "Monthly",
        _ => "Unknown",
    }
}

fn cycle_day_name(cycle: u64, day: u64) -> String {
    match cycle {
        2 | 3 => {
            let weekday = match day {
                1 => "Mon",
                2 => "Tue",
                3 => "Wed",
                4 => "Thu",
                5 => "Fri",
                _ => "?",
            };
            format!(" ({weekday})")
        }
        4 => {
            if day == 32 {
                " (last day)".to_string()
            } else {
                format!(" ({day}th)")
            }
        }
        _ => String::new(),
    }
}

fn fmt_cycle(cycle: u64, cycle_day: u64) -> String {
    format!("{}{}", cycle_name(cycle), cycle_day_name(cycle, cycle_day))
}

fn fmt_timestamp(ts: u64) -> String {
    if ts == 0 {
        return "—".to_string();
    }
    match i64::try_from(ts)
        .ok()
        .and_then(|s| OffsetDateTime::from_unix_timestamp(s).ok())
    {
        Some(dt) => {
            let fmt = time::macros::format_description!("[year]-[month]-[day]");
            dt.format(&fmt).unwrap_or_else(|_| dt.to_string())
        }
        None => "—".to_string(),
    }
}

fn parse_cycle(s: &str) -> Result<u64> {
    match s.to_lowercase().as_str() {
        "daily" => Ok(1),
        "weekly" => Ok(2),
        "biweekly" => Ok(3),
        "monthly" => Ok(4),
        _ => bail!("Unknown cycle '{s}'. Use: daily | weekly | biweekly | monthly"),
    }
}

fn status_filter_value(s: &str) -> Result<u64> {
    match s.to_lowercase().as_str() {
        "all" => Ok(0),
        "active" => Ok(1),
        "paused" => Ok(2),
        "ended" => Ok(3),
        _ => bail!("Unknown status '{s}'. Use: all | active | paused | ended"),
    }
}

fn check_api_error(resp: &serde_json::Value) -> Result<()> {
    let code = resp
        .get("code")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if code != 0 {
        let msg = resp
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        bail!("API error (code {code}): {msg}");
    }
    Ok(())
}

// ── Main dispatcher ───────────────────────────────────────────────────────────

pub async fn cmd_list_plans(
    status: Option<&str>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    cmd_list(status, format, verbose).await
}

pub async fn cmd_aip(cmd: AipCmd, format: &OutputFormat, verbose: bool) -> Result<()> {
    match cmd {
        AipCmd::Detail { plan_id, limit } => cmd_detail(&plan_id, limit, format, verbose).await,
        AipCmd::Create {
            symbol,
            amount,
            cycle,
            cycle_day,
            invest_now,
            yes,
        } => {
            cmd_create(
                &symbol, &amount, &cycle, cycle_day, invest_now, yes, format, verbose,
            )
            .await
        }
        AipCmd::Update {
            plan_id,
            amount,
            cycle,
            cycle_day,
        } => {
            cmd_update(
                &plan_id,
                amount.as_deref(),
                cycle.as_deref(),
                cycle_day,
                format,
                verbose,
            )
            .await
        }
        AipCmd::Pause { plan_id, yes } => cmd_operate(&plan_id, 1, "Pause", yes, verbose).await,
        AipCmd::Resume { plan_id, yes } => cmd_operate(&plan_id, 2, "Resume", yes, verbose).await,
        AipCmd::Terminate { plan_id, yes } => {
            cmd_operate(&plan_id, 3, "Terminate", yes, verbose).await
        }
        AipCmd::NextTime {
            symbol,
            plan_id,
            cycle,
            cycle_day,
        } => {
            cmd_next_time(
                symbol.as_deref(),
                plan_id.as_deref(),
                &cycle,
                cycle_day,
                format,
                verbose,
            )
            .await
        }
    }
}

// ── 1. List plans ─────────────────────────────────────────────────────────────

async fn cmd_list(status: Option<&str>, format: &OutputFormat, verbose: bool) -> Result<()> {
    let status_val = status_filter_value(status.unwrap_or("all"))?;
    let status_str = status_val.to_string();

    let resp = http_get(
        "/v1/aip/my",
        &[("status", status_str.as_str()), ("page_size", "50")],
        verbose,
    )
    .await?;
    check_api_error(&resp)?;

    let data = &resp;

    if matches!(format, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(data)?);
        return Ok(());
    }

    // Print total invested summary
    if let Some(totals) = data
        .get("total_invested_amounts")
        .and_then(|v| v.as_object())
    {
        let parts: Vec<String> = totals
            .iter()
            .map(|(currency, amount)| format!("{} {}", currency, amount.as_str().unwrap_or("-")))
            .collect();
        if !parts.is_empty() {
            println!("Total Invested: {}", parts.join(" | "));
            println!();
        }
    }

    let plans = data["plans"].as_array().map_or(&[][..], Vec::as_slice);

    let headers = &[
        "ID",
        "Name",
        "Amount",
        "Cycle",
        "Status",
        "Invested",
        "Count",
        "Next Date",
    ];
    let rows: Vec<Vec<String>> = plans
        .iter()
        .map(|p| {
            let id = p["id"].as_str().unwrap_or("-").to_string();
            let name = p["name"].as_str().unwrap_or("-").to_string();
            let currency = p["currency"].as_str().unwrap_or("");
            let invest_amount = p["invest_amount"].as_str().unwrap_or("-");
            let amount = format!("{currency} {invest_amount}").trim().to_string();
            let cycle = p["cycle"].as_u64().unwrap_or(0);
            let cycle_day = p["cycle_day"].as_u64().unwrap_or(0);
            let cycle_str = fmt_cycle(cycle, cycle_day);
            let status = p["status"].as_u64().unwrap_or(0);
            let status_str = plan_status_name(status).to_string();
            let invested = p["invested_amount"].as_str().unwrap_or("-");
            let invested_str = format!("{currency} {invested}").trim().to_string();
            let count = p["invested_count"].as_u64().unwrap_or(0).to_string();
            let next_ts = p["next_invest_time"].as_u64().unwrap_or(0);
            let next_date = if status == 1 {
                fmt_timestamp(next_ts)
            } else {
                "—".to_string()
            };
            vec![
                id,
                name,
                amount,
                cycle_str,
                status_str,
                invested_str,
                count,
                next_date,
            ]
        })
        .collect();

    print_table(headers, rows, format);
    Ok(())
}

// ── 2. Plan detail (includes execution records) ───────────────────────────────

async fn cmd_detail(plan_id: &str, limit: u32, format: &OutputFormat, verbose: bool) -> Result<()> {
    let page_size_str = limit.to_string();
    let detail_params = [("id", plan_id)];
    let records_params = [("plan_id", plan_id), ("page_size", page_size_str.as_str())];

    // Fetch plan detail and execution records concurrently
    let (detail_resp, records_resp) = tokio::join!(
        http_get("/v1/aip/detail", &detail_params, verbose),
        http_get("/v1/aip/records", &records_params, verbose),
    );
    let detail_resp = detail_resp?;
    let records_resp = records_resp?;
    check_api_error(&detail_resp)?;
    check_api_error(&records_resp)?;

    let plan = &detail_resp["plan"];
    let records_data = &records_resp;

    if matches!(format, OutputFormat::Json) {
        let tasks = records_data["tasks"].clone();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "plan": plan,
                "records": tasks,
            }))?
        );
        return Ok(());
    }

    // Plan info
    let cycle = plan["cycle"].as_u64().unwrap_or(0);
    let cycle_day = plan["cycle_day"].as_u64().unwrap_or(0);
    let status = plan["status"].as_u64().unwrap_or(0);
    let currency = plan["currency"].as_str().unwrap_or("");
    let next_ts = plan["next_invest_time"].as_u64().unwrap_or(0);

    let headers = &["Field", "Value"];
    let rows = vec![
        vec![
            "ID".to_string(),
            plan["id"].as_str().unwrap_or("-").to_string(),
        ],
        vec![
            "Name".to_string(),
            plan["name"].as_str().unwrap_or("-").to_string(),
        ],
        vec![
            "Counter ID".to_string(),
            plan["counter_id"].as_str().unwrap_or("-").to_string(),
        ],
        vec![
            "ISIN".to_string(),
            plan["isin"].as_str().unwrap_or("-").to_string(),
        ],
        vec!["Currency".to_string(), currency.to_string()],
        vec!["Status".to_string(), plan_status_name(status).to_string()],
        vec![
            "Amount / Cycle".to_string(),
            format!(
                "{} {}",
                currency,
                plan["invest_amount"].as_str().unwrap_or("-")
            ),
        ],
        vec!["Cycle".to_string(), fmt_cycle(cycle, cycle_day)],
        vec![
            "Invested Total".to_string(),
            format!(
                "{} {}",
                currency,
                plan["invested_amount"].as_str().unwrap_or("-")
            ),
        ],
        vec![
            "Invested Count".to_string(),
            plan["invested_count"].as_u64().unwrap_or(0).to_string(),
        ],
        vec![
            "Next Invest Date".to_string(),
            if status == 1 {
                fmt_timestamp(next_ts)
            } else {
                "—".to_string()
            },
        ],
    ];
    print_table(headers, rows, format);

    // Execution records
    println!();
    let task_count = records_data["task_count"].as_u64().unwrap_or(0);
    let tasks = records_data["tasks"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    println!("Execution Records (total: {task_count})");
    println!();

    let rec_headers = &["Date", "Amount", "Status", "Order ID"];
    let rec_rows: Vec<Vec<String>> = tasks
        .iter()
        .map(|t| {
            let invest_date = fmt_timestamp(t["invest_time"].as_u64().unwrap_or(0));
            let cur = t["currency"].as_str().unwrap_or("");
            let amount = format!("{cur} {}", t["invest_amount"].as_str().unwrap_or("-"))
                .trim()
                .to_string();
            let status_str = task_status_name(t["status"].as_u64().unwrap_or(0)).to_string();
            let order_id = t["order_id"].as_str().unwrap_or("").to_string();
            let order_id_str = if order_id.is_empty() {
                "—".to_string()
            } else {
                order_id
            };
            vec![invest_date, amount, status_str, order_id_str]
        })
        .collect();
    print_table(rec_headers, rec_rows, format);
    Ok(())
}

// ── 4. Create plan ────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn cmd_create(
    symbol: &str,
    amount: &str,
    cycle: &str,
    cycle_day: Option<u32>,
    invest_now: bool,
    yes: bool,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    use std::io::Write;

    let cycle_freq = parse_cycle(cycle)?;

    // Default cycle_day based on cycle and current date
    let effective_cycle_day = if let Some(d) = cycle_day {
        u64::from(d)
    } else {
        let now = OffsetDateTime::now_utc();
        match cycle_freq {
            2 | 3 => {
                // weekly/biweekly: current weekday (1=Mon..5=Fri)
                u64::from(now.weekday().number_from_monday()).min(5)
            }
            4 => u64::from(now.day()), // monthly: current day of month
            _ => 0,                    // daily: not applicable
        }
    };

    let counter_id = crate::utils::counter::symbol_to_counter_id(symbol);

    let cycle_display = fmt_cycle(cycle_freq, effective_cycle_day);
    let invest_now_display = if invest_now { "Yes" } else { "No" };

    println!("Create AIP Plan:");
    if counter_id == symbol {
        println!("  Fund:       {counter_id}");
    } else {
        println!("  Fund:       {symbol} ({counter_id})");
    }
    println!("  Amount:     {amount}");
    println!("  Cycle:      {cycle_display}");
    println!("  Invest Now: {invest_now_display}");
    println!();

    if !yes {
        print!("Confirm? [y/N] ");
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if input.trim().to_lowercase() != "y" {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let mut body = json!({
        "counter_id": counter_id,
        "invest_amount": amount,
        "cycle": cycle_freq,
    });

    if cycle_freq != 1 && effective_cycle_day > 0 {
        body["cycle_day"] = json!(effective_cycle_day);
    }
    if invest_now {
        body["invest_now"] = json!(true);
    }

    let resp = http_post("/v1/aip/edit", body, verbose).await?;
    check_api_error(&resp)?;

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Pretty => {
            println!("AIP plan created successfully.");
        }
    }
    Ok(())
}

// ── 5. Update plan ────────────────────────────────────────────────────────────

async fn cmd_update(
    plan_id: &str,
    amount: Option<&str>,
    cycle: Option<&str>,
    cycle_day: Option<u32>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    if amount.is_none() && cycle.is_none() && cycle_day.is_none() {
        bail!("At least one of --amount, --cycle, or --cycle-day must be provided");
    }

    // Fetch current plan for display/validation
    let detail_resp = http_get("/v1/aip/detail", &[("id", plan_id)], verbose).await?;
    check_api_error(&detail_resp)?;
    let plan = &detail_resp["plan"];

    let current_status = plan["status"].as_u64().unwrap_or(0);
    if current_status != 1 {
        bail!(
            "Only active plans (status=Active) can be edited. Current status: {}",
            plan_status_name(current_status)
        );
    }

    let current_cycle = plan["cycle"].as_u64().unwrap_or(0);
    let current_cycle_day = plan["cycle_day"].as_u64().unwrap_or(0);
    let currency = plan["currency"].as_str().unwrap_or("");
    let current_amount = plan["invest_amount"].as_str().unwrap_or("-");

    let new_cycle_freq = cycle.map(parse_cycle).transpose()?;
    let effective_cycle = new_cycle_freq.unwrap_or(current_cycle);
    let effective_cycle_day = cycle_day.map_or(current_cycle_day, u64::from);
    let effective_amount = amount.unwrap_or(current_amount);

    // Show diff
    println!("Update AIP Plan: {plan_id}");
    if let Some(a) = amount {
        println!("  Amount:    {currency} {current_amount}  →  {currency} {a}");
    }
    if cycle.is_some() || cycle_day.is_some() {
        let old = fmt_cycle(current_cycle, current_cycle_day);
        let new = fmt_cycle(effective_cycle, effective_cycle_day);
        println!("  Cycle:     {old}  →  {new}");
    }
    println!();

    {
        use std::io::Write;
        print!("Confirm? [y/N] ");
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if input.trim().to_lowercase() != "y" {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let mut body = json!({
        "plan_id": plan_id,
        "invest_amount": effective_amount,
        "cycle": effective_cycle,
    });
    if effective_cycle != 1 {
        body["cycle_day"] = json!(effective_cycle_day);
    }

    let resp = http_post("/v1/aip/edit", body, verbose).await?;
    check_api_error(&resp)?;

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Pretty => {
            println!("AIP plan updated successfully.");
        }
    }
    Ok(())
}

// ── 6/7/8. Operate (pause / resume / terminate) ───────────────────────────────

async fn cmd_operate(
    plan_id: &str,
    op_type: u64,
    op_name: &str,
    yes: bool,
    verbose: bool,
) -> Result<()> {
    use std::io::Write;

    if op_type == 3 {
        // Termination is irreversible; always show warning
        println!("WARNING: Terminating an AIP plan is irreversible.");
    }
    println!("{op_name} AIP plan: {plan_id}");

    if !yes {
        print!("Confirm? [y/N] ");
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if input.trim().to_lowercase() != "y" {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let body = json!({ "id": plan_id, "type": op_type });
    let resp = http_post("/v1/aip/operate", body, verbose).await?;
    check_api_error(&resp)?;

    println!("{op_name} operation successful.");
    Ok(())
}

// ── 9. Next investment time ───────────────────────────────────────────────────

async fn cmd_next_time(
    symbol: Option<&str>,
    plan_id: Option<&str>,
    cycle: &str,
    cycle_day: Option<u32>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    if plan_id.is_none() && symbol.is_none() {
        bail!("Either a symbol (e.g. QQQ.US) or --plan-id is required");
    }

    let cycle_val = parse_cycle(cycle)?;
    let cycle_val_str = cycle_val.to_string();
    let cycle_day_val = cycle_day.unwrap_or(0);
    let cycle_day_str = cycle_day_val.to_string();

    let counter_id_owned;
    let mut params: Vec<(&str, &str)> =
        vec![("cycle", &cycle_val_str), ("cycle_day", &cycle_day_str)];
    if let Some(pid) = plan_id {
        params.push(("plan_id", pid));
    }
    if let Some(sym) = symbol {
        counter_id_owned = crate::utils::counter::symbol_to_counter_id(sym);
        params.push(("counter_id", counter_id_owned.as_str()));
    }

    let resp = http_get("/v1/aip/next_time", &params, verbose).await?;
    check_api_error(&resp)?;

    let next_ts = resp["next_invest_time"].as_u64().unwrap_or(0);
    let next_date = fmt_timestamp(next_ts);

    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "next_invest_time": next_ts,
                    "next_invest_date": next_date
                }))?
            );
        }
        OutputFormat::Pretty => {
            println!("Next investment date: {next_date}");
        }
    }
    Ok(())
}
