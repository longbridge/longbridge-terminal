use std::time::Duration;

use bevy_ecs::prelude::*;
use bevy_ecs::system::{CommandQueue, InsertResource};
use tokio::sync::mpsc;

use crate::data::Counter;
use crate::tui::app::{AppState, ACCOUNT_CHANNEL, LOG_PANEL_VISIBLE, USER};
use crate::tui::ui::Content;
use crate::tui::widgets::{LocalSearch, Select};
use crate::{openapi, tui::systems};

async fn fetch_account_channel() -> Option<String> {
    let ctx = crate::openapi::trade();
    let resp = ctx.stock_positions(None).await.ok()?;
    resp.channels.into_iter().next().map(|c| c.account_channel)
}

pub fn subscribe_indexes(subs: Vec<Counter>) {
    tokio::spawn(async move {
        let ctx = crate::openapi::quote();
        let symbols: Vec<String> = subs.iter().map(std::string::ToString::to_string).collect();

        match ctx.quote(&symbols).await {
            Ok(quotes) => {
                tracing::info!("Fetched {} index quotes", quotes.len());
                for quote in quotes {
                    let counter = Counter::new(&quote.symbol);
                    let mut stock = crate::data::Stock::new(counter);
                    stock.update_from_security_quote(&quote);
                    crate::data::STOCKS.insert(stock);
                }
            }
            Err(e) => {
                tracing::error!("Failed to fetch index quotes: {}", e);
            }
        }

        if let Err(e) = ctx
            .subscribe(&symbols, longbridge::quote::SubFlags::QUOTE)
            .await
        {
            tracing::error!("Failed to subscribe indexes: {}", e);
        } else {
            tracing::info!("Successfully subscribed to {} indexes", symbols.len());
        }
    });
}

pub fn start_account_init(tx: mpsc::UnboundedSender<CommandQueue>) {
    tokio::spawn(async move {
        tracing::info!("Fetching account list...");
        match openapi::account::fetch_account_list().await {
            Ok(accounts) => {
                tracing::info!("Successfully fetched {} accounts", accounts.status.len());
                if accounts.status.is_empty() {
                    tracing::error!("no account found");
                    let mut queue = CommandQueue::default();
                    queue.push(InsertResource {
                        resource: Content::new(
                            t!("user.open_account.heading"),
                            t!("user.open_account.content"),
                        ),
                    });
                    queue.push(InsertResource {
                        resource: NextState(Some(AppState::Error)),
                    });
                    _ = tx.send(queue);
                    return;
                }

                let account = &accounts.status[0];
                {
                    let mut user = USER.write().expect("poison");
                    user.account_channel.clone_from(&account.account_channel);
                    user.aaid.clone_from(&account.aaid);
                }

                let mut queue = CommandQueue::default();
                queue.push(InsertResource {
                    resource: Select::new(accounts.status.clone()),
                });
                queue.push(InsertResource {
                    resource: LocalSearch::new(accounts.status.clone(), |keyword, account| {
                        account
                            .account_name
                            .to_ascii_lowercase()
                            .contains(&keyword.to_ascii_lowercase())
                    }),
                });

                if let Ok(currencies) = openapi::account::currencies(&account.account_channel) {
                    queue.push(InsertResource {
                        resource: LocalSearch::new(currencies.clone(), |keyword, currency| {
                            currency
                                .currency_iso
                                .contains(&keyword.to_ascii_uppercase())
                        }),
                    });
                }

                queue.push(InsertResource {
                    resource: NextState(Some(AppState::Watchlist)),
                });
                _ = tx.send(queue);

                tokio::spawn(async move {
                    if let Some(channel) = fetch_account_channel().await {
                        *ACCOUNT_CHANNEL.write().expect("poison") = Some(channel);
                    }
                });

                tracing::info!("Loading watchlist data...");
                systems::refresh_watchlist(tx.clone());
            }
            Err(e) => {
                tracing::error!("Failed to fetch account list: {}", e);
                let mut queue = CommandQueue::default();
                queue.push(InsertResource {
                    resource: Content::new(t!("error.api.heading"), e.to_string()),
                });
                queue.push(InsertResource {
                    resource: NextState(Some(AppState::Error)),
                });
                _ = tx.send(queue);
            }
        }
    });
}

pub fn start_log_watcher(tx: mpsc::UnboundedSender<CommandQueue>) {
    tokio::spawn(async move {
        use std::fs;
        use std::path::PathBuf;
        use std::time::SystemTime;

        let mut last_modified: Option<SystemTime> = None;
        let mut last_size: u64 = 0;

        let get_latest_log_file = || -> Option<PathBuf> {
            let log_dir = crate::logger::default_log_dir();
            let mut log_files: Vec<PathBuf> = fs::read_dir(&log_dir)
                .ok()?
                .filter_map(std::result::Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.is_file()
                        && path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                            n.starts_with("longbridge")
                                && std::path::Path::new(n)
                                    .extension()
                                    .is_some_and(|ext| ext.eq_ignore_ascii_case("log"))
                        })
                })
                .collect();

            log_files.sort_by(|a, b| {
                let time_a = fs::metadata(a).and_then(|m| m.modified()).ok();
                let time_b = fs::metadata(b).and_then(|m| m.modified()).ok();
                match (time_a, time_b) {
                    (Some(ta), Some(tb)) => tb.cmp(&ta),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                }
            });

            log_files.into_iter().next()
        };

        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;

            if !LOG_PANEL_VISIBLE.load(std::sync::atomic::Ordering::Relaxed) {
                continue;
            }

            if let Some(log_file) = get_latest_log_file() {
                if let Ok(metadata) = fs::metadata(&log_file) {
                    let modified = metadata.modified().ok();
                    let size = metadata.len();

                    if modified != last_modified || size != last_size {
                        last_modified = modified;
                        last_size = size;
                        let queue = CommandQueue::default();
                        _ = tx.send(queue);
                    }
                }
            }
        }
    });
}
