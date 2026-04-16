use anyhow::Result;

use super::{
    api::{http_delete, http_get, http_post},
    output::print_table,
    OutputFormat, SharelistCmd,
};
use crate::utils::counter::{counter_id_to_symbol, symbol_to_counter_id};

pub async fn cmd_sharelist(
    cmd: Option<SharelistCmd>,
    count: u32,
    format: &OutputFormat,
) -> Result<()> {
    match cmd {
        None => cmd_list(count, format).await,
        Some(SharelistCmd::Detail { id }) => cmd_detail(id, format).await,
        Some(SharelistCmd::Create { name, description }) => cmd_create(name, description).await,
        Some(SharelistCmd::Delete { id }) => cmd_delete(id).await,
        Some(SharelistCmd::Add { id, symbols }) => cmd_add(id, symbols).await,
        Some(SharelistCmd::Remove { id, symbols }) => cmd_remove(id, symbols).await,
        Some(SharelistCmd::Sort { id, symbols }) => cmd_sort(id, symbols).await,
        Some(SharelistCmd::Popular { count }) => cmd_popular(count, format).await,
    }
}

async fn cmd_list(count: u32, format: &OutputFormat) -> Result<()> {
    let size_str = count.to_string();
    let params: Vec<(&str, &str)> = vec![
        ("size", &size_str),
        ("self", "true"),
        ("subscription", "true"),
    ];

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
            println!("ID:          {}", sl["id"].as_str().unwrap_or("-"));
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

async fn cmd_create(name: String, description: String) -> Result<()> {
    let body = serde_json::json!({
        "name": name,
        "description": if description.is_empty() { name } else { description },
        "cover": "https://pub.pbkrs.com/files/202107/kaJSk6BsvPt6NJ3Q/sharelist_v1.png",
    });
    let resp = http_post("/v1/sharelists", body, false).await?;
    let sharelist_id = resp["sharelist_id"].as_str().unwrap_or("");
    println!("Sharelist created. ID: {sharelist_id}");
    Ok(())
}

async fn cmd_delete(id: String) -> Result<()> {
    let path = format!("/v1/sharelists/{id}");
    http_delete(&path, serde_json::Value::Null, false).await?;
    println!("Sharelist {id} deleted.");
    Ok(())
}

async fn cmd_add(id: String, symbols: Vec<String>) -> Result<()> {
    let counter_ids = symbols
        .iter()
        .map(|s| symbol_to_counter_id(s))
        .collect::<Vec<_>>()
        .join(",");
    let path = format!("/v1/sharelists/{id}/items");
    let body = serde_json::json!({ "counter_ids": counter_ids });
    http_post(&path, body, false).await?;
    println!("Added {} stock(s) to sharelist {id}.", symbols.len());
    Ok(())
}

async fn cmd_remove(id: String, symbols: Vec<String>) -> Result<()> {
    let counter_ids = symbols
        .iter()
        .map(|s| symbol_to_counter_id(s))
        .collect::<Vec<_>>()
        .join(",");
    let path = format!("/v1/sharelists/{id}/items");
    let body = serde_json::json!({ "counter_ids": counter_ids });
    http_delete(&path, body, false).await?;
    println!("Removed {} stock(s) from sharelist {id}.", symbols.len());
    Ok(())
}

async fn cmd_sort(id: String, symbols: Vec<String>) -> Result<()> {
    let counter_ids = symbols
        .iter()
        .map(|s| symbol_to_counter_id(s))
        .collect::<Vec<_>>()
        .join(",");
    let path = format!("/v1/sharelists/{id}/items/sort");
    let body = serde_json::json!({ "counter_ids": counter_ids });
    http_post(&path, body, false).await?;
    println!("Stocks reordered in sharelist {id}.");
    Ok(())
}

async fn cmd_popular(count: u32, format: &OutputFormat) -> Result<()> {
    let size_str = count.to_string();
    let resp = http_get("/v1/sharelists/popular", &[("size", &size_str)], false).await?;

    let sharelists = resp["sharelists"].as_array().cloned().unwrap_or_default();

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&sharelists)?);
        }
        OutputFormat::Pretty => {
            if sharelists.is_empty() {
                println!("No popular sharelists found.");
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
