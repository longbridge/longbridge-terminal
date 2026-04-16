use anyhow::Result;

use super::{
    api::{http_get, http_post},
    output::print_table,
    OutputFormat, SharelistCmd,
};
use crate::utils::counter::{counter_id_to_symbol, symbol_to_counter_id};

pub async fn cmd_sharelist(cmd: Option<SharelistCmd>, format: &OutputFormat) -> Result<()> {
    match cmd {
        None => cmd_list(false, false, 20, None, format).await,
        Some(SharelistCmd::List {
            subscription,
            own,
            size,
            tail_mark,
        }) => cmd_list(subscription, own, size, tail_mark.as_deref(), format).await,
        Some(SharelistCmd::Detail { id }) => cmd_detail(id, format).await,
        Some(SharelistCmd::Create {
            name,
            description,
            cover,
            stock_group_id,
        }) => cmd_create(name, description, cover, stock_group_id).await,
        Some(SharelistCmd::Delete { id }) => cmd_delete(id).await,
        Some(SharelistCmd::Add { id, symbols }) => cmd_add(id, symbols).await,
        Some(SharelistCmd::Remove { id, symbols }) => cmd_remove(id, symbols).await,
        Some(SharelistCmd::Sort { id, symbols }) => cmd_sort(id, symbols).await,
        Some(SharelistCmd::Hot { size }) => cmd_hot(size, format).await,
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

    let resp = http_get("/v1/sharelists", &params, false).await?;

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
                if !next.is_empty() {
                    println!("\nNext page cursor: {next}");
                }
            }
        }
    }
    Ok(())
}

async fn cmd_detail(id: String, format: &OutputFormat) -> Result<()> {
    let path = format!("/v1/sharelists/{id}");
    let resp = http_get(
        &path,
        &[
            ("constituent", "true"),
            ("quote", "true"),
            ("subscription", "true"),
        ],
        false,
    )
    .await?;

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Pretty => {
            let sl = &resp["sharelist"];
            let sl_type = match sl["sharelist_type"].as_u64().unwrap_or(0) {
                0 => "Regular",
                1 => "Group",
                2 => "Auto Group",
                3 => "Official",
                4 => "Industry",
                _ => "-",
            };
            println!("ID:          {}", sl["id"].as_u64().unwrap_or(0));
            println!("Name:        {}", sl["name"].as_str().unwrap_or("-"));
            println!("Type:        {sl_type}");
            println!("Description: {}", sl["description"].as_str().unwrap_or("-"));
            println!(
                "Subscribers: {}",
                sl["subscribers_count"].as_u64().unwrap_or(0)
            );
            println!("Day Chg:     {}", sl["chg"].as_str().unwrap_or("-"));
            println!(
                "YTD Chg:     {}",
                sl["this_year_chg"].as_str().unwrap_or("-")
            );
            println!(
                "Subscribed:  {}",
                sl["subscribed"].as_bool().unwrap_or(false)
            );

            let stocks = sl["stocks"].as_array().cloned().unwrap_or_default();
            if !stocks.is_empty() {
                println!("\nConstituents ({}):", stocks.len());
                let headers = &["Symbol", "Name", "Price", "Day Chg"];
                let rows: Vec<Vec<String>> = stocks
                    .iter()
                    .map(|s| {
                        let symbol = s["counter_id"]
                            .as_str()
                            .map_or_else(|| "-".to_string(), counter_id_to_symbol);
                        vec![
                            symbol,
                            s["name"].as_str().unwrap_or("-").to_string(),
                            s["price"].as_str().unwrap_or("-").to_string(),
                            s["chg"].as_str().unwrap_or("-").to_string(),
                        ]
                    })
                    .collect();
                print_table(headers, rows, format);
            }
        }
    }
    Ok(())
}

async fn cmd_create(
    name: String,
    description: String,
    cover: String,
    stock_group_id: String,
) -> Result<()> {
    let body = serde_json::json!({
        "name": name,
        "description": description,
        "cover": cover,
        "stock_group_id": stock_group_id,
    });
    let resp = http_post("/v2/sharelists", body, false).await?;
    let sharelist_id = resp["sharelist_id"].as_str().unwrap_or("");
    println!("Sharelist created. ID: {sharelist_id}");
    Ok(())
}

async fn cmd_delete(id: String) -> Result<()> {
    let body = serde_json::json!({ "id": id });
    http_post("/v2/sharelist/delete", body, false).await?;
    println!("Sharelist {id} deleted.");
    Ok(())
}

async fn cmd_add(id: String, symbols: Vec<String>) -> Result<()> {
    let counter_ids = symbols
        .iter()
        .map(|s| symbol_to_counter_id(s))
        .collect::<Vec<_>>()
        .join(",");
    let body = serde_json::json!({
        "id": id,
        "counter_ids": counter_ids,
    });
    http_post("/v1/sharelist/add_stock", body, false).await?;
    println!("Added {} stock(s) to sharelist {id}.", symbols.len());
    Ok(())
}

async fn cmd_remove(id: String, symbols: Vec<String>) -> Result<()> {
    let counter_ids = symbols
        .iter()
        .map(|s| symbol_to_counter_id(s))
        .collect::<Vec<_>>()
        .join(",");
    let body = serde_json::json!({
        "id": id,
        "counter_ids": counter_ids,
    });
    http_post("/v1/sharelist/delete_stock", body, false).await?;
    println!("Removed {} stock(s) from sharelist {id}.", symbols.len());
    Ok(())
}

async fn cmd_sort(id: String, symbols: Vec<String>) -> Result<()> {
    let counter_ids = symbols
        .iter()
        .map(|s| symbol_to_counter_id(s))
        .collect::<Vec<_>>()
        .join(",");
    let body = serde_json::json!({
        "id": id,
        "counter_ids": counter_ids,
    });
    http_post("/v1/sharelist/sort_stock", body, false).await?;
    println!("Stocks reordered in sharelist {id}.");
    Ok(())
}

async fn cmd_hot(size: u32, format: &OutputFormat) -> Result<()> {
    let size_str = size.to_string();
    let resp = http_get("/v1/sharelists/hot", &[("size", &size_str)], false).await?;

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

fn print_sharelist_table(sharelists: &[serde_json::Value], format: &OutputFormat) {
    let headers = &["ID", "Name", "Type", "Day Chg", "YTD Chg", "Subscribers"];
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
            ]
        })
        .collect();
    print_table(headers, rows, format);
}
