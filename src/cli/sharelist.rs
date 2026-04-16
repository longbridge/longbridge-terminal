use anyhow::Result;

use super::{
    api::{http_get, http_post},
    output::print_table,
    OutputFormat, SharelistCmd,
};
use crate::utils::counter::{counter_id_to_symbol, symbol_to_counter_id};

pub async fn cmd_sharelist(cmd: Option<SharelistCmd>, format: &OutputFormat) -> Result<()> {
    match cmd {
        None
        | Some(SharelistCmd::List {
            subscription: _,
            own: _,
            size: _,
            tail_mark: _,
        }) => {
            let cmd = cmd.unwrap_or(SharelistCmd::List {
                subscription: false,
                own: false,
                size: 20,
                tail_mark: None,
            });
            if let SharelistCmd::List {
                subscription,
                own,
                size,
                tail_mark,
            } = cmd
            {
                cmd_list(subscription, own, size, tail_mark.as_deref(), format).await
            } else {
                unreachable!()
            }
        }
        Some(SharelistCmd::Hot { size }) => cmd_hot(size, format).await,
        Some(SharelistCmd::Official { size, tail_mark }) => {
            cmd_official(size, tail_mark.as_deref(), format).await
        }
        Some(SharelistCmd::Stock { symbol, count }) => cmd_stock(symbol, count, format).await,
        Some(SharelistCmd::Members {
            id,
            count,
            tail_mark,
        }) => cmd_members(id, count, tail_mark.as_deref(), format).await,
        Some(SharelistCmd::Logs { id, year }) => cmd_logs(id, year.as_deref(), format).await,
        Some(SharelistCmd::MarkRead { id, log_id }) => cmd_mark_read(id, log_id).await,
        Some(SharelistCmd::Sort { id, symbols }) => cmd_sort(id, symbols).await,
        Some(SharelistCmd::RemoveStocks { id, symbols }) => cmd_remove_stocks(id, symbols).await,
        Some(SharelistCmd::Index { symbol }) => cmd_index(symbol, format).await,
    }
}

async fn cmd_list(
    subscription: bool,
    own: bool,
    size: u32,
    tail_mark: Option<&str>,
    format: &OutputFormat,
) -> Result<()> {
    let size_str = size.to_string();
    let mut params: Vec<(&str, &str)> = vec![("size", &size_str)];
    if subscription {
        params.push(("subscription", "true"));
    }
    if own {
        params.push(("self", "true"));
    }
    if let Some(tm) = tail_mark {
        params.push(("tail_mark", tm));
    }

    let resp = http_get("/v1/social/sharelists", &params, false).await?;

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Pretty => {
            let sharelists = resp["sharelists"].as_array().cloned().unwrap_or_default();
            let subscribed = resp["subscribed_sharelists"]
                .as_array()
                .cloned()
                .unwrap_or_default();

            if sharelists.is_empty() && subscribed.is_empty() {
                println!("No sharelists found.");
                return Ok(());
            }

            if !sharelists.is_empty() {
                println!("My Sharelists:");
                print_sharelist_table(&sharelists, format);
            }
            if !subscribed.is_empty() {
                println!("\nSubscribed Sharelists:");
                print_sharelist_table(&subscribed, format);
            }

            if let Some(next) = resp["tail_mark"].as_str() {
                println!("\nNext page cursor: {next}");
            }
        }
    }
    Ok(())
}

async fn cmd_hot(size: u32, format: &OutputFormat) -> Result<()> {
    let size_str = size.to_string();
    let resp = http_get("/v1/social/hot_sharelists", &[("size", &size_str)], false).await?;

    let sharelists = resp["sharelists"].as_array().cloned().unwrap_or_default();

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&sharelists)?);
        }
        OutputFormat::Pretty => {
            if sharelists.is_empty() {
                println!("No hot sharelists found.");
                return Ok(());
            }
            print_sharelist_table(&sharelists, format);
        }
    }
    Ok(())
}

async fn cmd_official(size: u32, tail_mark: Option<&str>, format: &OutputFormat) -> Result<()> {
    let size_str = size.to_string();
    let mut params: Vec<(&str, &str)> = vec![("limit", &size_str)];
    if let Some(tm) = tail_mark {
        params.push(("tail_mark", tm));
    }

    let resp = http_get("/v1/social/lb_sharelists", &params, false).await?;

    let sharelists = resp["sharelists"].as_array().cloned().unwrap_or_default();

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Pretty => {
            if sharelists.is_empty() {
                println!("No official sharelists found.");
                return Ok(());
            }
            print_sharelist_table(&sharelists, format);

            if let Some(tm) = resp["next_params"]["tail_mark"].as_str() {
                println!("\nNext page cursor: {tm}");
            }
        }
    }
    Ok(())
}

async fn cmd_stock(symbol: String, count: u32, format: &OutputFormat) -> Result<()> {
    let counter_id = symbol_to_counter_id(&symbol);
    let count_str = count.to_string();
    let resp = http_get(
        "/v1/social/stock/sharelists",
        &[("counter_id", &counter_id), ("limit", &count_str)],
        false,
    )
    .await?;

    let sharelists = resp["sharelists"].as_array().cloned().unwrap_or_default();

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&sharelists)?);
        }
        OutputFormat::Pretty => {
            if sharelists.is_empty() {
                println!("No sharelists found for {symbol}.");
                return Ok(());
            }
            print_sharelist_table(&sharelists, format);
        }
    }
    Ok(())
}

async fn cmd_members(
    id: String,
    count: u32,
    tail_mark: Option<&str>,
    format: &OutputFormat,
) -> Result<()> {
    let count_str = count.to_string();
    let mut params: Vec<(&str, &str)> = vec![("id", &id), ("limit", &count_str)];
    if let Some(tm) = tail_mark {
        params.push(("tail_mark", tm));
    }

    let resp = http_get("/v1/social/sharelist/profiles", &params, false).await?;

    let profiles = resp["profiles"].as_array().cloned().unwrap_or_default();

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Pretty => {
            if profiles.is_empty() {
                println!("No members found.");
                return Ok(());
            }
            let headers = &["Member ID", "Nickname", "Followers", "Following", "Bio"];
            let rows: Vec<Vec<String>> = profiles
                .iter()
                .map(|p| {
                    vec![
                        p["member_id"].as_str().unwrap_or("-").to_string(),
                        p["nickname"].as_str().unwrap_or("-").to_string(),
                        p["followers_count"].as_str().unwrap_or("-").to_string(),
                        p["following_count"].as_str().unwrap_or("-").to_string(),
                        p["description"].as_str().unwrap_or("").to_string(),
                    ]
                })
                .collect();
            print_table(headers, rows, format);

            if let Some(next) = resp["meta"]["tail_mark"].as_u64() {
                println!("\nNext page cursor: {next}");
            }
        }
    }
    Ok(())
}

async fn cmd_logs(id: String, year: Option<&str>, format: &OutputFormat) -> Result<()> {
    let mut params: Vec<(&str, &str)> = vec![("sharelist_id", &id)];
    if let Some(y) = year {
        params.push(("year", y));
    }

    let resp = http_get("/v1/social/sharelist/logs", &params, false).await?;

    let logs = resp["logs"].as_array().cloned().unwrap_or_default();

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Pretty => {
            if logs.is_empty() {
                println!("No logs found.");
                return Ok(());
            }
            let headers = &["Time", "Action", "Symbols"];
            let rows: Vec<Vec<String>> = logs
                .iter()
                .map(|entry| {
                    let op = match entry["operate"].as_str().unwrap_or("") {
                        "0" => "Add",
                        "1" => "Remove",
                        "2" => "Create",
                        other => other,
                    };
                    let stocks: Vec<String> = entry["stocks"]
                        .as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .filter_map(|s| s.as_str())
                        .map(counter_id_to_symbol)
                        .collect();
                    vec![
                        entry["log_time"].as_str().unwrap_or("-").to_string(),
                        op.to_string(),
                        stocks.join(", "),
                    ]
                })
                .collect();
            print_table(headers, rows, format);

            let add = resp["add_count"].as_u64().unwrap_or(0);
            let remove = resp["remove_count"].as_u64().unwrap_or(0);
            println!("\nAdded: {add}  Removed: {remove}");
            if let Some(prev) = resp["previous_year"].as_str() {
                if !prev.is_empty() {
                    println!("Previous year with data: {prev}");
                }
            }
        }
    }
    Ok(())
}

async fn cmd_mark_read(id: String, log_id: String) -> Result<()> {
    let body = serde_json::json!({
        "sharelist_id": id,
        "log_id": log_id,
    });
    http_post("/v1/social/sharelist/log/mark_read", body, false).await?;
    println!("Log {log_id} marked as read.");
    Ok(())
}

async fn cmd_sort(id: String, symbols: Vec<String>) -> Result<()> {
    let counter_ids: Vec<String> = symbols.iter().map(|s| symbol_to_counter_id(s)).collect();
    let body = serde_json::json!({
        "sharelist_id": id,
        "counter_ids": counter_ids,
    });
    http_post("/v1/social/group_sharelist/sort_stocks", body, false).await?;
    println!("Stocks sorted in sharelist {id}.");
    Ok(())
}

async fn cmd_remove_stocks(id: String, symbols: Vec<String>) -> Result<()> {
    let counter_ids: Vec<String> = symbols.iter().map(|s| symbol_to_counter_id(s)).collect();
    let body = serde_json::json!({
        "sharelist_id": id,
        "counter_ids": counter_ids,
    });
    http_post("/v1/social/group_sharelist/del_stocks", body, false).await?;
    println!("Removed {} stock(s) from sharelist {id}.", symbols.len());
    Ok(())
}

async fn cmd_index(symbol: String, format: &OutputFormat) -> Result<()> {
    let resp = http_get(
        "/v1/stock-info/sharelist-index",
        &[("symbol", &symbol)],
        false,
    )
    .await?;

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Pretty => {
            let headers = &["Name", "Counter ID", "Day Chg", "YTD Chg"];
            let rows = vec![vec![
                resp["name"].as_str().unwrap_or("-").to_string(),
                resp["counter_id"]
                    .as_str()
                    .map_or_else(|| "-".to_string(), counter_id_to_symbol),
                resp["chg"].as_str().unwrap_or("-").to_string(),
                resp["ytd_chg"].as_str().unwrap_or("-").to_string(),
            ]];
            print_table(headers, rows, format);
        }
    }
    Ok(())
}

fn print_sharelist_table(sharelists: &[serde_json::Value], format: &OutputFormat) {
    let headers = &[
        "ID",
        "Name",
        "Type",
        "Day Chg",
        "YTD Chg",
        "Subscribers",
        "Stocks",
    ];
    let rows: Vec<Vec<String>> = sharelists
        .iter()
        .map(|sl| {
            let sl_type = match sl["sharelist_type"].as_u64().unwrap_or(0) {
                0 => "Regular",
                1 => "Group",
                2 => "Auto Group",
                3 => "Official",
                4 => "Industry",
                _ => "-",
            };
            let stock_count = sl["stocks"].as_array().map_or(0, Vec::len).to_string();
            vec![
                sl["id"].as_u64().map_or_else(
                    || sl["id"].as_str().unwrap_or("-").to_string(),
                    |n| n.to_string(),
                ),
                sl["name"].as_str().unwrap_or("-").to_string(),
                sl_type.to_string(),
                sl["chg"].as_str().unwrap_or("-").to_string(),
                sl["this_year_chg"].as_str().unwrap_or("-").to_string(),
                sl["subscribers_count"]
                    .as_u64()
                    .map_or_else(|| "-".to_string(), |n| n.to_string()),
                stock_count,
            ]
        })
        .collect();
    print_table(headers, rows, format);
}
