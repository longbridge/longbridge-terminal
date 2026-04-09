use anyhow::Result;
use longbridge::quote::{
    RequestCreateWatchlistGroup, RequestUpdateWatchlistGroup, SecuritiesUpdateMode,
};

use super::{api::QuoteApi, output::print_table, OutputFormat, WatchlistCmd};

pub async fn cmd_watchlist(cmd: Option<WatchlistCmd>, format: &OutputFormat) -> Result<()> {
    match cmd {
        None => cmd_list(format).await,
        Some(WatchlistCmd::Show { group }) => cmd_show(group, format).await,
        Some(WatchlistCmd::Create { name }) => cmd_create(name).await,
        Some(WatchlistCmd::Delete { id, purge, yes }) => cmd_delete(id, purge, yes).await,
        Some(WatchlistCmd::Update {
            id,
            name,
            add,
            remove,
            mode,
        }) => cmd_update(id, name, add, remove, &mode, format).await,
        Some(WatchlistCmd::Pin { securities, remove }) => cmd_pin(securities, remove).await,
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
                            "is_pinned": s.is_pinned,
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Pretty => {
            for group in &groups {
                println!("\nGroup: {} (ID: {})", group.name, group.id);
                let headers = &["Symbol", "Name", "Market", "Pinned"];
                let rows: Vec<Vec<String>> = group
                    .securities
                    .iter()
                    .map(|s| {
                        vec![
                            s.symbol.clone(),
                            s.name.clone(),
                            format!("{:?}", s.market),
                            if s.is_pinned { "yes" } else { "" }.to_string(),
                        ]
                    })
                    .collect();
                print_table(headers, rows, &OutputFormat::Pretty);
            }
        }
    }
    Ok(())
}

async fn cmd_show(group: String, format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::quote();
    let groups = ctx.watchlist().await?;

    // Match by numeric ID first, then fall back to name (case-insensitive)
    let matched = if let Ok(id) = group.parse::<i64>() {
        groups.into_iter().find(|g| g.id == id)
    } else {
        let lower = group.to_lowercase();
        groups.into_iter().find(|g| g.name.to_lowercase() == lower)
    };

    let Some(g) = matched else {
        anyhow::bail!("Watchlist group not found: {group}");
    };

    match format {
        OutputFormat::Json => {
            let val = serde_json::json!({
                "id": g.id,
                "name": g.name,
                "securities": g.securities.iter().map(|s| serde_json::json!({
                    "symbol": s.symbol,
                    "name": s.name,
                    "market": format!("{:?}", s.market),
                    "is_pinned": s.is_pinned,
                })).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Pretty => {
            println!("Group: {} (ID: {})", g.name, g.id);
            let headers = &["Symbol", "Name", "Market", "Pinned"];
            let rows: Vec<Vec<String>> = g
                .securities
                .iter()
                .map(|s| {
                    vec![
                        s.symbol.clone(),
                        s.name.clone(),
                        format!("{:?}", s.market),
                        if s.is_pinned { "yes" } else { "" }.to_string(),
                    ]
                })
                .collect();
            print_table(headers, rows, &OutputFormat::Pretty);
        }
    }
    Ok(())
}

async fn cmd_pin(securities: Vec<String>, remove: Vec<String>) -> Result<()> {
    let (mode, targets) = if remove.is_empty() {
        ("add", securities)
    } else {
        ("remove", remove)
    };

    if targets.is_empty() {
        anyhow::bail!("No securities specified. Use positional args to pin, or --remove to unpin.");
    }

    let body = serde_json::json!({
        "mode": mode,
        "securities": targets,
    });

    super::api::http_post("/v1/watchlist/pinned", body, false).await?;

    if mode == "remove" {
        println!("Unpinned: {}", targets.join(", "));
    } else {
        println!("Pinned: {}", targets.join(", "));
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
    println!("Created watchlist group '{name}' with ID: {id}");
    Ok(())
}

async fn cmd_delete(id: i64, purge: bool, yes: bool) -> Result<()> {
    use std::io::Write;
    if !yes {
        print!("Delete watchlist group {id}? [y/N] ");
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if input.trim().to_lowercase() != "y" {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let ctx = crate::openapi::quote();
    ctx.delete_watchlist_group(id, purge).await?;
    println!("Deleted watchlist group {id}.");
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
        "remove" => SecuritiesUpdateMode::Remove,
        "replace" => SecuritiesUpdateMode::Replace,
        _ => SecuritiesUpdateMode::Add,
    };

    let securities = if remove.is_empty() {
        add.clone()
    } else {
        remove
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
    println!("Watchlist group {id} updated.");
    Ok(())
}

// ─── Testable run_* functions ─────────────────────────────────────────────────

pub async fn run_watchlist_list(api: &dyn QuoteApi, format: &OutputFormat) -> Result<()> {
    let groups = api.watchlist().await?;
    match format {
        OutputFormat::Json => {
            let val: Vec<_> = groups.iter().map(|g| serde_json::json!({
                "id": g.id,
                "name": g.name,
                "securities": g.securities.iter().map(|s| serde_json::json!({"symbol": s.symbol, "name": s.name})).collect::<Vec<_>>(),
            })).collect();
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Pretty => {
            for group in &groups {
                println!("\nGroup: {} (ID: {})", group.name, group.id);
                let headers = &["Symbol", "Name", "Market"];
                let rows: Vec<Vec<String>> = group
                    .securities
                    .iter()
                    .map(|s| vec![s.symbol.clone(), s.name.clone(), format!("{:?}", s.market)])
                    .collect();
                print_table(headers, rows, &OutputFormat::Pretty);
            }
        }
    }
    Ok(())
}

pub async fn run_watchlist_create(api: &dyn QuoteApi, name: String) -> Result<i64> {
    let id = api.create_watchlist_group(name.clone()).await?;
    println!("Created watchlist group '{name}' with ID: {id}");
    Ok(id)
}

pub async fn run_watchlist_delete(api: &dyn QuoteApi, id: i64, _purge: bool) -> Result<()> {
    api.delete_watchlist_group(id).await?;
    println!("Deleted watchlist group {id}.");
    Ok(())
}

pub async fn run_watchlist_update(
    api: &dyn QuoteApi,
    req: RequestUpdateWatchlistGroup,
) -> Result<()> {
    let id = req.id;
    api.update_watchlist_group(req).await?;
    println!("Watchlist group {id} updated.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::api::MockQuoteApi;

    #[tokio::test]
    async fn test_run_watchlist_list_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_watchlist().times(1).returning(|| Ok(vec![]));
        run_watchlist_list(&mock, &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_watchlist_create_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_create_watchlist_group()
            .with(mockall::predicate::eq("Tech Stocks".to_string()))
            .times(1)
            .returning(|_| Ok(123_i64));
        let id = run_watchlist_create(&mock, "Tech Stocks".to_string())
            .await
            .unwrap();
        assert_eq!(id, 123);
    }

    #[tokio::test]
    async fn test_run_watchlist_delete_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_delete_watchlist_group()
            .with(mockall::predicate::eq(42_i64))
            .times(1)
            .returning(|_| Ok(()));
        run_watchlist_delete(&mock, 42, false).await.unwrap();
    }

    #[tokio::test]
    async fn test_run_watchlist_update_dispatches() {
        let mut mock = MockQuoteApi::new();
        mock.expect_update_watchlist_group()
            .times(1)
            .returning(|_| Ok(()));
        let req = RequestUpdateWatchlistGroup {
            id: 42,
            name: Some("New Name".to_string()),
            securities: None,
            mode: SecuritiesUpdateMode::Add,
        };
        run_watchlist_update(&mock, req).await.unwrap();
    }

    #[tokio::test]
    async fn test_run_watchlist_list_json_format() {
        let mut mock = MockQuoteApi::new();
        mock.expect_watchlist().times(1).returning(|| Ok(vec![]));
        run_watchlist_list(&mock, &OutputFormat::Json)
            .await
            .unwrap();
    }
}
