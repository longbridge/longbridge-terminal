use anyhow::Result;
use longbridge::quote::{
    RequestCreateWatchlistGroup, RequestUpdateWatchlistGroup, SecuritiesUpdateMode,
};

use super::{output::print_table, OutputFormat, WatchlistCmd};

pub async fn cmd_watchlist(cmd: Option<WatchlistCmd>, format: &OutputFormat) -> Result<()> {
    match cmd {
        None => cmd_list(format).await,
        Some(WatchlistCmd::Create { name }) => cmd_create(name).await,
        Some(WatchlistCmd::Delete { id, purge }) => cmd_delete(id, purge).await,
        Some(WatchlistCmd::Update {
            id,
            name,
            add,
            remove,
            mode,
        }) => cmd_update(id, name, add, remove, &mode, format).await,
    }
}

async fn cmd_list(format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::quote();
    let groups = ctx.watchlist().await?;

    match format {
        OutputFormat::Json => {
            let val: Vec<_> = groups
                .iter()
                .map(|g| {
                    serde_json::json!({
                        "id": g.id,
                        "name": g.name,
                        "securities": g.securities.iter().map(|s| serde_json::json!({
                            "symbol": s.symbol,
                            "name": s.name,
                            "market": format!("{:?}", s.market),
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Table => {
            for group in &groups {
                println!("\nGroup: {} (ID: {})", group.name, group.id);
                let headers = &["Symbol", "Name", "Market"];
                let rows: Vec<Vec<String>> = group
                    .securities
                    .iter()
                    .map(|s| {
                        vec![
                            s.symbol.clone(),
                            s.name.clone(),
                            format!("{:?}", s.market),
                        ]
                    })
                    .collect();
                print_table(headers, rows, &OutputFormat::Table);
            }
        }
    }
    Ok(())
}

async fn cmd_create(name: String) -> Result<()> {
    let ctx = crate::openapi::quote();
    let req = RequestCreateWatchlistGroup {
        name: name.clone(),
        securities: None,
    };
    let id = ctx.create_watchlist_group(req).await?;
    println!("Created watchlist group '{}' with ID: {}", name, id);
    Ok(())
}

async fn cmd_delete(id: i64, purge: bool) -> Result<()> {
    print!("Delete watchlist group {}? [y/N] ", id);
    use std::io::Write;
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if input.trim().to_lowercase() != "y" {
        println!("Cancelled.");
        return Ok(());
    }

    let ctx = crate::openapi::quote();
    ctx.delete_watchlist_group(id, purge).await?;
    println!("Deleted watchlist group {}.", id);
    Ok(())
}

async fn cmd_update(
    id: i64,
    name: Option<String>,
    add: Vec<String>,
    remove: Vec<String>,
    mode: &str,
    _format: &OutputFormat,
) -> Result<()> {
    let ctx = crate::openapi::quote();

    let update_mode = match mode {
        "add" => SecuritiesUpdateMode::Add,
        "remove" => SecuritiesUpdateMode::Remove,
        "replace" => SecuritiesUpdateMode::Replace,
        _ => SecuritiesUpdateMode::Add,
    };

    let securities = if !remove.is_empty() {
        remove
    } else {
        add.clone()
    };

    let req = RequestUpdateWatchlistGroup {
        id,
        name,
        securities: if securities.is_empty() {
            None
        } else {
            Some(securities)
        },
        mode: update_mode,
    };
    ctx.update_watchlist_group(req).await?;
    println!("Watchlist group {} updated.", id);
    Ok(())
}
