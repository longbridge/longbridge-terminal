use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use atomic::Atomic;
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_ecs::system::{CommandQueue, InsertResource, SystemState};
use tokio::sync::mpsc;

use crate::data::{Counter, KlineType, User, Watchlist, WatchlistGroup};
use crate::tui::mouse;
use crate::tui::render::{DirtyFlags, RenderState};
use crate::tui::ui::Content;
use crate::tui::widgets::{Carousel, Loading, LocalSearch, Search, Terminal};
use crate::{openapi, tui::systems};

pub static RT: OnceLock<tokio::runtime::Handle> = OnceLock::new();
pub static UPDATE_TX: OnceLock<mpsc::UnboundedSender<CommandQueue>> = OnceLock::new();
pub static POPUP: AtomicU16 = AtomicU16::new(0);
pub static LAST_STATE: Atomic<AppState> = Atomic::new(AppState::Watchlist);
pub static QUOTE_BMP: Atomic<bool> = Atomic::new(false);
pub static LOG_PANEL_VISIBLE: Atomic<bool> = Atomic::new(false);
pub static WATCHLIST: std::sync::LazyLock<RwLock<Watchlist>> =
    std::sync::LazyLock::new(Default::default);
pub static USER: std::sync::LazyLock<RwLock<User>> = std::sync::LazyLock::new(Default::default);
pub static ACCOUNT_CHANNEL: std::sync::LazyLock<RwLock<Option<String>>> =
    std::sync::LazyLock::new(|| RwLock::new(None));

pub const POPUP_HELP: u16 = 0b1;
pub const POPUP_SEARCH: u16 = 0b10;
pub const POPUP_ACCOUNT: u16 = 0b100;
pub const POPUP_CURRENCY: u16 = 0b1000;
pub const POPUP_WATCHLIST: u16 = 0b10000;
pub const POPUP_WATCHLIST_SEARCH: u16 = 0b10_0000;
pub const POPUP_ORDER_ENTRY: u16 = 0b0000_0001_0000_0000;
pub const POPUP_CANCEL_ORDER: u16 = 0b0000_0010_0000_0000;
pub const POPUP_REPLACE_ORDER: u16 = 0b0000_0100_0000_0000;
pub const POPUP_DATE_FILTER: u16 = 0b0000_1000_0000_0000;

#[derive(
    Clone, Copy, PartialEq, Eq, Hash, Debug, Default, States, strum::EnumIter, bytemuck::NoUninit,
)]
#[repr(u8)]
pub enum AppState {
    Error,
    #[default]
    Loading,
    TradeToken,
    Portfolio,
    Stock,
    Watchlist,
    WatchlistStock,
    Orders,
}

async fn fetch_account_channel() -> Option<String> {
    let ctx = crate::openapi::trade();
    let resp = ctx.stock_positions(None).await.ok()?;
    resp.channels.into_iter().next().map(|c| c.account_channel)
}

#[allow(clippy::too_many_lines)]
pub async fn run(
    _args: crate::Args,
    mut quote_receiver: impl tokio_stream::Stream<Item = longbridge::quote::PushEvent> + Unpin,
) {
    let (update_tx, mut update_rx) = mpsc::unbounded_channel();
    UPDATE_TX.set(update_tx.clone()).ok();

    // Initialize index subscriptions
    let indexes: Vec<[Counter; 3]> = vec![
        [".DJI.US".into(), ".IXIC.US".into(), "SPY.US".into()],
        ["HSI.HK".into(), "HSCEI.HK".into(), "HSTECH.HK".into()],
        ["000001.SH".into(), "399001.SZ".into(), "399006.SZ".into()],
    ];

    // Subscribe to indexes and fetch initial data
    let subs: Vec<Counter> = indexes.iter().flatten().cloned().collect();
    tokio::spawn({
        let subs = subs.clone();
        async move {
            let ctx = crate::openapi::quote();
            let symbols: Vec<String> = subs.iter().map(std::string::ToString::to_string).collect();

            // First, fetch initial quote data (includes prev_close)
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

            // Then subscribe for real-time updates
            if let Err(e) = ctx
                .subscribe(&symbols, longbridge::quote::SubFlags::QUOTE)
                .await
            {
                tracing::error!("Failed to subscribe indexes: {}", e);
            } else {
                tracing::info!("Successfully subscribed to {} indexes", symbols.len());
            }
        }
    });

    // Create search components
    let search_stock = Search::new(update_tx.clone(), |keyword| {
        Box::pin(async move {
            let query = openapi::search::StockQuery {
                keyword,
                market: "HK,SG,SH,SZ,US".to_string(),
                product: "BK,ETF,IX,ST,WT".to_string(),
                account_channel: USER
                    .read()
                    .expect("poison")
                    .get_account_channel()
                    .to_string(),
            };
            openapi::search::fetch_stock(&query)
                .await
                .map(|v| v.product_list)
                .unwrap_or_default()
        })
    });
    let search_watchlist = LocalSearch::new(Vec::<WatchlistGroup>::new(), |_keyword, _group| false);
    let watchlist_search = LocalSearch::new(
        Vec::<crate::data::Counter>::new(),
        |keyword: &str, counter: &crate::data::Counter| {
            let kw = keyword.to_ascii_lowercase();
            if counter.as_str().to_ascii_lowercase().contains(&kw) {
                return true;
            }
            crate::data::STOCKS
                .get(counter)
                .is_some_and(|s| s.name.to_ascii_lowercase().contains(&kw))
        },
    );

    RT.set(tokio::runtime::Handle::current()).unwrap();
    let mut app = bevy_app::App::new();
    app.add_state::<AppState>()
        .add_event::<systems::Key>()
        .add_event::<systems::TuiEvent>()
        .init_resource::<Terminal>()
        .init_resource::<Loading>()
        .insert_resource(search_stock)
        .insert_resource(search_watchlist)
        .insert_resource(watchlist_search)
        .insert_resource(systems::Command(update_tx.clone()))
        .insert_resource(Carousel::new(indexes, Duration::from_secs(5)))
        .insert_resource(systems::WsState(crate::data::ReadyState::Open))
        .add_systems(Update, systems::loading.run_if(in_state(AppState::Loading)))
        .add_systems(Update, systems::error.run_if(in_state(AppState::Error)))
        .add_systems(OnExit(AppState::Watchlist), systems::exit_watchlist)
        .add_systems(
            Update,
            systems::render_watchlist.run_if(in_state(AppState::Watchlist)),
        )
        .add_systems(OnEnter(AppState::Stock), systems::enter_stock)
        .add_systems(OnExit(AppState::Stock), systems::exit_stock)
        .add_systems(
            Update,
            systems::render_stock.run_if(in_state(AppState::Stock)),
        )
        .add_systems(OnEnter(AppState::WatchlistStock), systems::enter_stock)
        .add_systems(OnExit(AppState::WatchlistStock), systems::exit_stock)
        .add_systems(
            Update,
            systems::render_watchlist_stock.run_if(in_state(AppState::WatchlistStock)),
        )
        .add_systems(OnEnter(AppState::Portfolio), systems::enter_portfolio)
        .add_systems(OnExit(AppState::Portfolio), systems::exit_portfolio)
        .add_systems(
            Update,
            systems::render_portfolio.run_if(in_state(AppState::Portfolio)),
        )
        .add_systems(OnEnter(AppState::Orders), systems::enter_orders)
        .add_systems(OnExit(AppState::Orders), systems::exit_orders)
        .add_systems(
            Update,
            systems::render_orders.run_if(in_state(AppState::Orders)),
        );

    // Don't refresh watchlist when transitioning between Watchlist and WatchlistStock
    for v in <AppState as strum::IntoEnumIterator>::iter() {
        if v == AppState::Watchlist || v == AppState::WatchlistStock {
            continue;
        }
        for watch in [AppState::Watchlist, AppState::WatchlistStock] {
            app.add_systems(
                OnTransition { from: v, to: watch },
                systems::enter_watchlist_common,
            );
            app.add_systems(
                OnTransition { from: watch, to: v },
                systems::exit_watchlist_common,
            );
        }
    }

    // Get WebSocket receiver (already initialized in main.rs)
    // We need to re-acquire the receiver or pass it from main.rs
    // Skip WebSocket handling for now, focus on getting code to compile

    // Initialize account information
    tokio::spawn({
        let tx = update_tx.clone();
        async move {
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

                    // Set default account
                    let account = &accounts.status[0];
                    {
                        let mut user = USER.write().expect("poison");
                        user.account_channel.clone_from(&account.account_channel);
                        user.aaid.clone_from(&account.aaid);
                    }

                    let mut queue = CommandQueue::default();

                    // Add Select<Account> resource for Portfolio
                    queue.push(InsertResource {
                        resource: crate::tui::widgets::Select::new(accounts.status.clone()),
                    });

                    queue.push(InsertResource {
                        resource: LocalSearch::new(accounts.status.clone(), |keyword, account| {
                            account
                                .account_name
                                .to_ascii_lowercase()
                                .contains(&keyword.to_ascii_lowercase())
                        }),
                    });

                    // Get currency list
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

                    // Load watchlist data
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
        }
    });

    // Start log file watcher for auto-refresh when log panel is visible
    tokio::spawn({
        let tx = update_tx.clone();
        async move {
            use std::fs;
            use std::path::PathBuf;
            use std::time::SystemTime;

            let mut last_modified: Option<SystemTime> = None;
            let mut last_size: u64 = 0;

            // Helper to get the latest log file
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

                // Only check if log panel is visible
                if !LOG_PANEL_VISIBLE.load(Ordering::Relaxed) {
                    continue;
                }

                if let Some(log_file) = get_latest_log_file() {
                    if let Ok(metadata) = fs::metadata(&log_file) {
                        let modified = metadata.modified().ok();
                        let size = metadata.len();

                        // Check if file has been modified or size changed
                        if modified != last_modified || size != last_size {
                            last_modified = modified;
                            last_size = size;

                            // Trigger UI refresh by sending empty command queue
                            let queue = CommandQueue::default();
                            _ = tx.send(queue);
                        }
                    }
                }
            }
        }
    });

    // FPS-based rendering: 30 FPS for smooth UI updates
    let render_interval = std::time::Duration::from_millis(33); // ~30 FPS
    let mut render_tick = tokio::time::interval(render_interval);
    render_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Wait briefly to ensure terminal is fully ready
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let mut events = crossterm::event::EventStream::new();
    let mut render_state = RenderState::new();
    // Initial render to display UI
    render_state.mark_all_dirty();

    loop {
        tokio::select! {
            // Render at fixed FPS
            _ = render_tick.tick() => {
                if render_state.needs_render() {
                    app.update();
                    render_state.clear();
                } else {
                    render_state.skip();
                }
            }
            // Handle commands (state changes, resource updates)
            Some(mut cmd) = update_rx.recv() => {
                cmd.apply(&mut app.world);
                // State changes typically affect all components
                render_state.mark_dirty(DirtyFlags::ALL);
            }
            // Handle quote push events (data updates)
            Some(push_event) = tokio_stream::StreamExt::next(&mut quote_receiver) => {
                // Handle WebSocket push events
                // PushEvent contains symbol and detail
                use longbridge::quote::PushEventDetail;

                let symbol = push_event.symbol;
                let counter = Counter::new(&symbol);
                 match push_event.detail {
                     PushEventDetail::Quote(quote) => {
                         tracing::debug!(
                             "Update quote: {} = {}, trade_session = {:?}",
                             symbol,
                             quote.last_done,
                             quote.trade_session
                         );
                         crate::data::STOCKS.modify(counter.clone(), |stock| {
                             // Use update_from_push_quote to update all fields including trade_session
                             stock.update_from_push_quote(&quote);
                         });
                         // Quote updates affect watchlist, stock detail, and indexes
                         render_state.mark_dirty(DirtyFlags::NONE.mark_quote_update());
                     }
                     PushEventDetail::Depth(depth) => {
                         tracing::debug!("Update depth: {}", symbol);
                         crate::data::STOCKS.modify(counter, |stock| {
                             use rust_decimal::Decimal;
                             // PushDepth structure may differ from SecurityDepth, update manually
                             stock.depth.asks = depth.asks.iter().map(|d| crate::data::Depth {
                                 position: d.position,
                                 price: d.price.unwrap_or(Decimal::ZERO),
                                 volume: d.volume,
                                 order_num: d.order_num,
                             }).collect();
                             stock.depth.bids = depth.bids.iter().map(|d| crate::data::Depth {
                                 position: d.position,
                                 price: d.price.unwrap_or(Decimal::ZERO),
                                 volume: d.volume,
                                 order_num: d.order_num,
                             }).collect();
                         });
                         // Depth updates only affect stock detail view and depth widget
                         render_state.mark_dirty(DirtyFlags::NONE.mark_depth_update());
                     }
                     _ => {
                         // Other event types not handled yet
                     }
                 }
            }
            // Handle user input events
            Some(event) = tokio_stream::StreamExt::next(&mut events) => {
                let event = match event {
                    Ok(crossterm::event::Event::Key(event)) => event,
                    Ok(crossterm::event::Event::Mouse(mouse_event)) => {
                        let popup = POPUP.load(Ordering::Relaxed);
                        let state = *app.world.resource::<State<AppState>>().get();
                        handle_mouse_event(&mut app, mouse_event, state, popup, update_tx.clone(), &mut render_state);
                        continue;
                    }
                    Ok(_) => continue,
                    Err(err) => {
                        tracing::error!("fail to receive event: {err}");
                        app.world.insert_resource(Content::new(
                            t!("qrcode_view.error.heading"),
                            t!("qrcode_view.error.content"),
                        ));
                        app.world.insert_resource(NextState(Some(AppState::Error)));
                        render_state.mark_dirty(DirtyFlags::ERROR);
                        continue;
                    }
                };

                let popup = POPUP.load(Ordering::Relaxed);
                let state = *app.world.resource::<State<AppState>>().get();

                // Handle global shortcuts that should work even with popups open
                if event.code == crossterm::event::KeyCode::Char('`')
                    && event.modifiers == crossterm::event::KeyModifiers::NONE {
                    // Toggle log panel visibility
                    let was_visible = LOG_PANEL_VISIBLE.load(Ordering::Relaxed);
                    LOG_PANEL_VISIBLE.store(!was_visible, Ordering::Relaxed);
                    render_state.mark_dirty(DirtyFlags::ALL);
                    continue;
                }

                // Handle various popups
                if popup != 0 {
                    handle_popup_input(&mut app, popup, event, update_tx.clone());
                    render_state.mark_dirty(DirtyFlags::NONE.mark_popup_change(popup));
                    continue;
                }

                // Handle input for different states
                match state {
                    AppState::Error => return,
                    AppState::Loading => {
                        if matches!(event, ctrl!('c') | key!('q')) {
                            return;
                        }
                        continue;
                    },
                    AppState::TradeToken => {
                        match event {
                            ctrl!('c') => return,
                            key!(Esc) => {
                                app.world.insert_resource(NextState(Some(LAST_STATE.load(Ordering::Relaxed))));
                                render_state.mark_dirty(DirtyFlags::ALL);
                            }
                            _ => {
                                let evt = crossterm::event::Event::Key(event);
                                if let Some(evt) = tui_input::backend::crossterm::to_input_request(&evt) {
                                    send_evt(systems::TuiEvent(evt), &mut app.world);
                                    render_state.mark_dirty(DirtyFlags::ALL);
                                }
                            }
                        }
                        continue;
                    }
                    AppState::Portfolio | AppState::Stock | AppState::Watchlist | AppState::WatchlistStock | AppState::Orders => (),
                }

                // Handle global keyboard shortcuts
                handle_global_keys(&mut app, event, state, update_tx.clone(), &mut render_state);
            }
        }
    }
}

fn handle_popup_input(
    app: &mut bevy_app::App,
    popup: u16,
    event: crossterm::event::KeyEvent,
    update_tx: mpsc::UnboundedSender<CommandQueue>,
) {
    if popup == POPUP_ACCOUNT {
        let mut search = app
            .world
            .resource_mut::<LocalSearch<crate::data::Account>>();
        let (hidden, selected) = search.handle_key(event);
        if hidden {
            POPUP.store(0, Ordering::Relaxed);
        }
        if let Some(account) = selected {
            let mut user = USER.write().expect("poison");
            if user.get_account_channel() != account.account_channel {
                // TODO: Fetch currency list in background
            }
            user.account_channel = account.account_channel;
            user.aaid = account.aaid;
        }
    } else if popup == POPUP_CURRENCY {
        let mut search = app
            .world
            .resource_mut::<LocalSearch<openapi::account::CurrencyInfo>>();
        let (hidden, selected) = search.handle_key(event);
        if hidden {
            POPUP.store(0, Ordering::Relaxed);
        }
        if let Some(currency) = selected {
            POPUP.store(0, Ordering::Relaxed);
            let mut user = USER.write().expect("poison");
            user.base_currency = currency.currency_iso;
        }
    } else if popup == POPUP_WATCHLIST {
        let mut search = app.world.resource_mut::<LocalSearch<WatchlistGroup>>();
        let (hidden, selected) = search.handle_key(event);
        if hidden {
            POPUP.store(0, Ordering::Relaxed);
        }
        if let Some(group) = selected {
            POPUP.store(0, Ordering::Relaxed);
            WATCHLIST.write().expect("poison").set_group_id(group.id);
            systems::refresh_watchlist(update_tx.clone());
        }
    } else if popup == POPUP_SEARCH {
        // Check for direct symbol navigation: Enter with typed text but no dropdown selection
        let direct_query = {
            let mut search = app
                .world
                .resource_mut::<Search<openapi::search::StockItem>>();
            search.consume_direct_enter(event)
        };
        if let Some(query) = direct_query {
            POPUP.store(0, Ordering::Relaxed);
            let counter = crate::data::Counter::new(&query);
            app.world.insert_resource(systems::StockDetail(counter));
            let state = *app.world.resource::<State<AppState>>().get();
            let next_state = if state == AppState::Stock {
                AppState::Stock
            } else {
                AppState::WatchlistStock
            };
            app.world.insert_resource(NextState(Some(next_state)));
        } else {
            let mut search = app
                .world
                .resource_mut::<Search<openapi::search::StockItem>>();
            let (hidden, selected) = search.handle_key(event);
            if hidden {
                POPUP.store(0, Ordering::Relaxed);
            }
            if let Some(selected) = selected {
                POPUP.store(0, Ordering::Relaxed);
                app.world
                    .insert_resource(systems::StockDetail(selected.counter_id));
                let state = *app.world.resource::<State<AppState>>().get();
                let next_state = if state == AppState::Stock {
                    AppState::Stock
                } else {
                    AppState::WatchlistStock
                };
                app.world.insert_resource(NextState(Some(next_state)));
            }
        }
    } else if popup == POPUP_HELP {
        POPUP.store(0, Ordering::Relaxed);
    } else if popup == POPUP_WATCHLIST_SEARCH {
        handle_watchlist_search_input(app, event);
    } else if popup & POPUP_ORDER_ENTRY != 0 {
        systems::handle_order_entry_key(event);
    } else if popup & POPUP_CANCEL_ORDER != 0 {
        systems::handle_cancel_order_key(event);
    } else if popup & POPUP_REPLACE_ORDER != 0 {
        systems::handle_replace_order_key(event);
    } else if popup & POPUP_DATE_FILTER != 0 {
        systems::handle_date_filter_key(event);
    }
}

fn handle_watchlist_search_input(app: &mut bevy_app::App, event: crossterm::event::KeyEvent) {
    // First check for direct Enter (typed input, no dropdown selection)
    let direct_query = {
        let mut search = app
            .world
            .resource_mut::<LocalSearch<crate::data::Counter>>();
        search.consume_direct_enter(event)
    };

    if let Some(query) = direct_query {
        if let Some(symbol) = normalize_counter(&query) {
            POPUP.store(0, Ordering::Relaxed);
            let counter = crate::data::Counter::new(&symbol);
            {
                app.world
                    .resource_mut::<LocalSearch<crate::data::Counter>>()
                    .close();
            }
            navigate_to_counter(app, counter);
        } else {
            app.world
                .resource_mut::<LocalSearch<crate::data::Counter>>()
                .set_error(t!("WatchlistSearch.invalid_format").to_string());
        }
    } else {
        let (hidden, selected) = {
            let mut search = app
                .world
                .resource_mut::<LocalSearch<crate::data::Counter>>();
            let (hidden, selected) = search.handle_key(event);
            if hidden {
                POPUP.store(0, Ordering::Relaxed);
            }
            (hidden, selected)
        };
        let _ = hidden;
        if let Some(counter) = selected {
            navigate_to_counter(app, counter);
        }
    }
}

/// Normalizes user input into a full `CODE.MARKET` symbol string.
/// - Input with a dot (e.g. `AAPL.US`, `700.hk`) → validates market, returns uppercased.
/// - All-letter input (e.g. `AAPL`, `tsla`) → appends `.US`.
/// - All-digit input (e.g. `700`, `09988`) → appends `.HK`.
/// - Anything else → `None` (invalid).
fn normalize_counter(query: &str) -> Option<String> {
    let q = query.trim();
    if q.is_empty() {
        return None;
    }
    if q.contains('.') {
        let mut parts = q.splitn(2, '.');
        let code = parts.next().unwrap_or("").trim();
        let market = parts.next().unwrap_or("").trim().to_uppercase();
        if code.is_empty() || !matches!(market.as_str(), "HK" | "US" | "SH" | "SZ" | "SG" | "HAS") {
            return None;
        }
        Some(format!("{}.{}", code.to_uppercase(), market))
    } else if q.chars().all(|c| c.is_ascii_alphabetic()) {
        Some(format!("{}.US", q.to_uppercase()))
    } else if q.chars().all(|c| c.is_ascii_digit()) {
        Some(format!("{q}.HK"))
    } else {
        None
    }
}

fn get_active_symbol(app: &bevy_app::App, state: AppState) -> Option<String> {
    match state {
        AppState::Stock | AppState::WatchlistStock => app
            .world
            .get_resource::<systems::StockDetail>()
            .map(|sd| sd.0.to_string()),
        AppState::Portfolio => {
            let idx = systems::PORTFOLIO_TABLE
                .lock()
                .expect("poison")
                .selected()?;
            let view = systems::PORTFOLIO_VIEW.read().expect("poison");
            view.as_ref()?.holdings.get(idx).map(|h| h.symbol.clone())
        }
        AppState::Watchlist => {
            let idx = systems::WATCHLIST_TABLE
                .lock()
                .expect("poison")
                .selected()?;
            let watchlist = crate::tui::app::WATCHLIST.read().expect("poison");
            let counters = watchlist.counters();
            counters.get(idx).map(std::string::ToString::to_string)
        }
        _ => None,
    }
}

fn navigate_to_counter(app: &mut bevy_app::App, counter: crate::data::Counter) {
    app.world.insert_resource(systems::StockDetail(counter));
    let state = *app.world.resource::<State<AppState>>().get();
    let next_state = if state == AppState::Stock {
        AppState::Stock
    } else {
        AppState::WatchlistStock
    };
    app.world.insert_resource(NextState(Some(next_state)));
}

#[allow(clippy::too_many_lines)]
fn handle_global_keys(
    app: &mut bevy_app::App,
    event: crossterm::event::KeyEvent,
    state: AppState,
    update_tx: mpsc::UnboundedSender<CommandQueue>,
    render_state: &mut RenderState,
) {
    match event {
        ctrl!('c') => crate::tui::widgets::Terminal::graceful_exit(0),
        key!('1') if state != AppState::Watchlist => {
            app.world
                .insert_resource(NextState(Some(AppState::Watchlist)));
            render_state.mark_dirty(DirtyFlags::ALL);
        }
        key!('2') if state != AppState::Portfolio => {
            // Create default Portfolio resource if it doesn't exist
            if app.world.get_resource::<systems::Portfolio>().is_none() {
                app.world.insert_resource(systems::Portfolio::default());
            }
            app.world
                .insert_resource(NextState(Some(AppState::Portfolio)));
            render_state.mark_dirty(DirtyFlags::ALL);
        }
        key!('3') if state != AppState::Orders => {
            app.world.insert_resource(NextState(Some(AppState::Orders)));
            render_state.mark_dirty(DirtyFlags::ALL);
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('b'),
            modifiers: ::crossterm::event::KeyModifiers::NONE,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } if matches!(
            state,
            AppState::Watchlist | AppState::WatchlistStock | AppState::Stock | AppState::Portfolio
        ) =>
        {
            if let Some(symbol) = get_active_symbol(app, state) {
                systems::open_order_entry(symbol, longbridge::trade::OrderSide::Buy, None);
                POPUP.store(POPUP_ORDER_ENTRY, Ordering::Relaxed);
                render_state.mark_dirty(DirtyFlags::ALL);
            }
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('s'),
            modifiers: ::crossterm::event::KeyModifiers::NONE,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } if matches!(
            state,
            AppState::Watchlist | AppState::WatchlistStock | AppState::Stock | AppState::Portfolio
        ) =>
        {
            if let Some(symbol) = get_active_symbol(app, state) {
                systems::open_order_entry(symbol, longbridge::trade::OrderSide::Sell, None);
                POPUP.store(POPUP_ORDER_ENTRY, Ordering::Relaxed);
                render_state.mark_dirty(DirtyFlags::ALL);
            }
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('c'),
            modifiers: ::crossterm::event::KeyModifiers::NONE,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } if state == AppState::Orders
            && !systems::ORDERS_MODE.load(std::sync::atomic::Ordering::Relaxed) =>
        {
            systems::try_open_cancel_for_selected();
            render_state.mark_dirty(DirtyFlags::ALL);
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('m'),
            modifiers: ::crossterm::event::KeyModifiers::NONE,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } if state == AppState::Orders
            && !systems::ORDERS_MODE.load(std::sync::atomic::Ordering::Relaxed) =>
        {
            systems::try_open_replace_for_selected();
            render_state.mark_dirty(DirtyFlags::ALL);
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('f'),
            modifiers: ::crossterm::event::KeyModifiers::NONE,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } if state == AppState::Orders
            && systems::ORDERS_MODE.load(std::sync::atomic::Ordering::Relaxed) =>
        {
            systems::open_date_filter();
            render_state.mark_dirty(DirtyFlags::ALL);
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('a'),
            modifiers:
                ::crossterm::event::KeyModifiers::NONE | ::crossterm::event::KeyModifiers::SHIFT,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } if state == AppState::Portfolio => {
            if let Some(mut account) = app
                .world
                .get_resource_mut::<LocalSearch<crate::data::Account>>()
            {
                POPUP.store(POPUP_ACCOUNT, Ordering::Relaxed);
                account.visible();
                render_state.mark_dirty(DirtyFlags::POPUP_ACCOUNT);
            }
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('c'),
            modifiers:
                ::crossterm::event::KeyModifiers::NONE | ::crossterm::event::KeyModifiers::SHIFT,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } if state == AppState::Portfolio => {
            if let Some(mut currency) = app
                .world
                .get_resource_mut::<LocalSearch<openapi::account::CurrencyInfo>>()
            {
                POPUP.store(POPUP_CURRENCY, Ordering::Relaxed);
                currency.visible();
                render_state.mark_dirty(DirtyFlags::POPUP_CURRENCY);
            }
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('g' | 'G'),
            modifiers: ::crossterm::event::KeyModifiers::NONE,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } if state == AppState::Watchlist || state == AppState::WatchlistStock => {
            if let Some(mut search) = app.world.get_resource_mut::<LocalSearch<WatchlistGroup>>() {
                POPUP.store(POPUP_WATCHLIST, Ordering::Relaxed);
                search.visible();
                render_state.mark_dirty(DirtyFlags::POPUP_WATCHLIST);
            }
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('Q'),
            modifiers:
                ::crossterm::event::KeyModifiers::NONE | ::crossterm::event::KeyModifiers::SHIFT,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } => {
            show_index(&mut app.world, 0);
            render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('W'),
            modifiers:
                ::crossterm::event::KeyModifiers::NONE | ::crossterm::event::KeyModifiers::SHIFT,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } => {
            show_index(&mut app.world, 1);
            render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('E'),
            modifiers:
                ::crossterm::event::KeyModifiers::NONE | ::crossterm::event::KeyModifiers::SHIFT,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } => {
            show_index(&mut app.world, 2);
            render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('t'),
            modifiers:
                ::crossterm::event::KeyModifiers::NONE | ::crossterm::event::KeyModifiers::SHIFT,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } => {
            if state == AppState::Stock {
                app.world
                    .insert_resource(NextState(Some(AppState::WatchlistStock)));
                render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
            } else if state == AppState::WatchlistStock {
                app.world.insert_resource(NextState(Some(AppState::Stock)));
                render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
            }
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('R'),
            modifiers:
                ::crossterm::event::KeyModifiers::NONE | ::crossterm::event::KeyModifiers::SHIFT,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } => match state {
            AppState::Portfolio => {
                systems::refresh_portfolio();
                render_state.mark_dirty(DirtyFlags::PORTFOLIO);
            }
            AppState::Watchlist => {
                systems::refresh_watchlist(update_tx.clone());
                render_state.mark_dirty(DirtyFlags::WATCHLIST);
            }
            AppState::WatchlistStock => {
                systems::refresh_stock_debounced(
                    app.world.resource::<systems::StockDetail>().0.clone(),
                );
                systems::refresh_watchlist(update_tx.clone());
                render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
            }
            AppState::Stock => {
                systems::refresh_stock_debounced(
                    app.world.resource::<systems::StockDetail>().0.clone(),
                );
                render_state.mark_dirty(DirtyFlags::STOCK_DETAIL);
            }
            AppState::Orders => {
                systems::refresh_orders();
                systems::refresh_history_orders();
                render_state.mark_dirty(DirtyFlags::ALL);
            }
            _ => {}
        },
        key!('/') => {
            if state == AppState::Watchlist || state == AppState::WatchlistStock {
                let mut ws = app
                    .world
                    .resource_mut::<LocalSearch<crate::data::Counter>>();
                ws.visible();
                POPUP.store(POPUP_WATCHLIST_SEARCH, Ordering::Relaxed);
                render_state.mark_dirty(DirtyFlags::ALL);
            } else {
                let mut search = app
                    .world
                    .resource_mut::<Search<openapi::search::StockItem>>();
                search.visible();
                POPUP.store(POPUP_SEARCH, Ordering::Relaxed);
                render_state.mark_dirty(DirtyFlags::POPUP_SEARCH);
            }
        }
        key!('?') => {
            POPUP.store(POPUP_HELP, Ordering::Relaxed);
            render_state.mark_dirty(DirtyFlags::POPUP_HELP);
        }
        key!('q') => {
            if state == AppState::WatchlistStock {
                let news_view = systems::NEWS_VIEW.load(std::sync::atomic::Ordering::Relaxed);
                match news_view {
                    systems::NewsView::Detail => {
                        systems::NEWS_VIEW.store(
                            systems::NewsView::List,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        render_state.mark_dirty(DirtyFlags::ALL);
                    }
                    systems::NewsView::List => {
                        systems::NEWS_VIEW.store(
                            systems::NewsView::Quote,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        render_state.mark_dirty(DirtyFlags::ALL);
                    }
                    systems::NewsView::Quote => {
                        app.world
                            .insert_resource(NextState(Some(AppState::Watchlist)));
                        render_state.mark_dirty(DirtyFlags::ALL);
                    }
                }
            } else {
                crate::tui::widgets::Terminal::graceful_exit(0);
            }
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('n'),
            modifiers: ::crossterm::event::KeyModifiers::NONE,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } if state == AppState::WatchlistStock => {
            send_evt(systems::Key::NewsToggle, &mut app.world);
            render_state.mark_dirty(DirtyFlags::ALL);
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::PageUp,
            modifiers: ::crossterm::event::KeyModifiers::NONE,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        }
        | ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('K'),
            modifiers: ::crossterm::event::KeyModifiers::SHIFT,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } if state == AppState::WatchlistStock => {
            send_evt(systems::Key::NewsScrollUp, &mut app.world);
            render_state.mark_dirty(DirtyFlags::ALL);
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::PageDown,
            modifiers: ::crossterm::event::KeyModifiers::NONE,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        }
        | ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('J'),
            modifiers: ::crossterm::event::KeyModifiers::SHIFT,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } if state == AppState::WatchlistStock => {
            send_evt(systems::Key::NewsScrollDown, &mut app.world);
            render_state.mark_dirty(DirtyFlags::ALL);
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('o'),
            modifiers: ::crossterm::event::KeyModifiers::NONE,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } if state == AppState::WatchlistStock => {
            send_evt(systems::Key::NewsOpen, &mut app.world);
            render_state.mark_dirty(DirtyFlags::ALL);
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Esc,
            modifiers: ::crossterm::event::KeyModifiers::NONE,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } => {
            if state == AppState::WatchlistStock {
                let news_view = systems::NEWS_VIEW.load(std::sync::atomic::Ordering::Relaxed);
                match news_view {
                    systems::NewsView::Detail => {
                        systems::NEWS_VIEW.store(
                            systems::NewsView::List,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        render_state.mark_dirty(DirtyFlags::ALL);
                    }
                    systems::NewsView::List => {
                        systems::NEWS_VIEW.store(
                            systems::NewsView::Quote,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        render_state.mark_dirty(DirtyFlags::ALL);
                    }
                    systems::NewsView::Quote => {
                        app.world
                            .insert_resource(NextState(Some(AppState::Watchlist)));
                        render_state.mark_dirty(DirtyFlags::ALL);
                    }
                }
            } else if state == AppState::Stock || state == AppState::Orders {
                app.world
                    .insert_resource(NextState(Some(AppState::Watchlist)));
                render_state.mark_dirty(DirtyFlags::ALL);
            } else {
                crate::tui::widgets::Terminal::graceful_exit(0);
            }
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Up | ::crossterm::event::KeyCode::Char('k'),
            modifiers: ::crossterm::event::KeyModifiers::NONE,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        }
        | ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('k'),
            modifiers: ::crossterm::event::KeyModifiers::SHIFT,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } => {
            send_evt(systems::Key::Up, &mut app.world);
            // Navigation keys affect current view
            render_state.mark_dirty(match state {
                AppState::Watchlist | AppState::WatchlistStock => DirtyFlags::WATCHLIST,
                AppState::Stock => DirtyFlags::STOCK_DETAIL,
                AppState::Portfolio => DirtyFlags::PORTFOLIO,
                _ => DirtyFlags::ALL,
            });
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Down | ::crossterm::event::KeyCode::Char('j'),
            modifiers: ::crossterm::event::KeyModifiers::NONE,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        }
        | ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('j'),
            modifiers: ::crossterm::event::KeyModifiers::SHIFT,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } => {
            send_evt(systems::Key::Down, &mut app.world);
            render_state.mark_dirty(match state {
                AppState::Watchlist | AppState::WatchlistStock => DirtyFlags::WATCHLIST,
                AppState::Stock => DirtyFlags::STOCK_DETAIL,
                AppState::Portfolio => DirtyFlags::PORTFOLIO,
                _ => DirtyFlags::ALL,
            });
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Left | ::crossterm::event::KeyCode::Char('h'),
            modifiers: ::crossterm::event::KeyModifiers::NONE,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        }
        | ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('h'),
            modifiers: ::crossterm::event::KeyModifiers::SHIFT,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } => {
            send_evt(systems::Key::Left, &mut app.world);
            render_state.mark_dirty(match state {
                AppState::Stock => DirtyFlags::STOCK_DETAIL,
                _ => DirtyFlags::ALL,
            });
        }
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Right | ::crossterm::event::KeyCode::Char('l'),
            modifiers: ::crossterm::event::KeyModifiers::NONE,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        }
        | ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char('l'),
            modifiers: ::crossterm::event::KeyModifiers::SHIFT,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        } => {
            send_evt(systems::Key::Right, &mut app.world);
            render_state.mark_dirty(match state {
                AppState::Stock => DirtyFlags::STOCK_DETAIL,
                _ => DirtyFlags::ALL,
            });
        }
        key!(Tab) => {
            send_evt(systems::Key::Tab, &mut app.world);
            render_state.mark_dirty(match state {
                AppState::Stock => DirtyFlags::STOCK_DETAIL,
                _ => DirtyFlags::ALL,
            });
        }
        key!(Enter) => {
            send_evt(systems::Key::Enter, &mut app.world);
            render_state.mark_dirty(DirtyFlags::ALL);
        }
        shift!(BackTab) => {
            send_evt(systems::Key::BackTab, &mut app.world);
            render_state.mark_dirty(match state {
                AppState::Stock => DirtyFlags::STOCK_DETAIL,
                _ => DirtyFlags::ALL,
            });
        }
        _ => (),
    }
}

fn send_evt<T: Event>(evt: T, world: &mut World) {
    let mut state = SystemState::<EventWriter<T>>::new(world);
    state.get_mut(world).send(evt);
}

fn show_index(world: &mut World, index: usize) {
    let indexes = world.resource::<Carousel<[Counter; 3]>>().current();
    world.insert_resource(systems::StockDetail(indexes[index].clone()));
    world.insert_resource(NextState(Some(AppState::WatchlistStock)));
}

/// Map a column offset (relative to the kline tab bar's left edge) to the clicked `KlineType`.
/// Tabs are rendered as " {label} " with "|" dividers between them.
fn kline_tab_at(rel_col: u16) -> Option<KlineType> {
    let mut x = 0u16;
    for kline_type in <KlineType as strum::IntoEnumIterator>::iter() {
        let label = kline_type.to_string();
        let tab_w = (label.chars().count() as u16) + 2; // " {label} "
        if rel_col < x + tab_w {
            return Some(kline_type);
        }
        x += tab_w + 1; // +1 for "|" divider
    }
    None
}

#[allow(clippy::too_many_lines)]
fn handle_mouse_event(
    app: &mut bevy_app::App,
    event: crossterm::event::MouseEvent,
    state: AppState,
    popup: u16,
    update_tx: tokio::sync::mpsc::UnboundedSender<CommandQueue>,
    render_state: &mut RenderState,
) {
    use crossterm::event::{MouseButton, MouseEventKind};

    let MouseEventKind::Down(MouseButton::Left) = event.kind else {
        return;
    };

    let col = event.column;
    let row = event.row;

    // Popup clicks take priority over everything else
    if popup != 0 {
        if popup == POPUP_HELP {
            POPUP.store(0, Ordering::Relaxed);
            render_state.mark_dirty(DirtyFlags::ALL);
        } else {
            handle_popup_mouse_click(app, popup, col, row, update_tx, render_state);
        }
        return;
    }

    // Navbar tab bar: compute actual tab positions from rendered text widths
    // to avoid the off-by-fraction error that comes from simple width/3 division.
    let navbar_rect = *mouse::NAVBAR_TABS_RECT.lock().expect("poison");
    if navbar_rect.width > 0
        && row == navbar_rect.y
        && col >= navbar_rect.x
        && col < navbar_rect.x + navbar_rect.width
    {
        let tab0_w = format!(" {} [1] ", t!("tabs.Watchlist")).chars().count() as u16;
        let tab1_w = format!(" {} [2] ", t!("tabs.Portfolio")).chars().count() as u16;
        let divider = 1u16;
        let rel = col - navbar_rect.x;
        let tab = if rel < tab0_w {
            0u16
        } else if rel < tab0_w + divider + tab1_w {
            1
        } else {
            2
        };
        match tab {
            0 if !matches!(
                state,
                AppState::Watchlist | AppState::WatchlistStock | AppState::Stock
            ) =>
            {
                app.world
                    .insert_resource(NextState(Some(AppState::Watchlist)));
                render_state.mark_dirty(DirtyFlags::ALL);
            }
            1 if state != AppState::Portfolio => {
                if app.world.get_resource::<systems::Portfolio>().is_none() {
                    app.world.insert_resource(systems::Portfolio::default());
                }
                app.world
                    .insert_resource(NextState(Some(AppState::Portfolio)));
                render_state.mark_dirty(DirtyFlags::ALL);
            }
            2 if state != AppState::Orders => {
                app.world.insert_resource(NextState(Some(AppState::Orders)));
                render_state.mark_dirty(DirtyFlags::ALL);
            }
            _ => {}
        }
        return;
    }

    // Footer index click: Q/W/E index groups
    let footer_rects = *mouse::FOOTER_INDEX_RECTS.lock().expect("poison");
    for (i, frect) in footer_rects.iter().enumerate() {
        if frect.width > 0 && row == frect.y && col >= frect.x && col < frect.x + frect.width {
            show_index(&mut app.world, i);
            render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
            return;
        }
    }

    match state {
        AppState::Watchlist => {
            let table_rect = *mouse::WATCHLIST_TABLE_RECT.lock().expect("poison");
            if let Some(row_idx) = mouse::click_to_row(col, row, table_rect) {
                let offset = systems::WATCHLIST_TABLE.lock().expect("poison").offset();
                let actual_idx = row_idx + offset;
                let len = WATCHLIST.read().expect("poison").counters().len();
                if actual_idx < len {
                    systems::WATCHLIST_TABLE
                        .lock()
                        .expect("poison")
                        .select(Some(actual_idx));
                    let counter = WATCHLIST
                        .read()
                        .expect("poison")
                        .counters()
                        .get(actual_idx)
                        .cloned();
                    if let Some(counter) = counter {
                        let mut queue = CommandQueue::default();
                        queue.push(InsertResource {
                            resource: systems::StockDetail(counter),
                        });
                        queue.push(InsertResource {
                            resource: NextState(Some(AppState::WatchlistStock)),
                        });
                        _ = update_tx.send(queue);
                        render_state.mark_dirty(DirtyFlags::ALL);
                    }
                }
            }
        }
        AppState::WatchlistStock => {
            // Tab bar click (Quote / News toggle)
            let tabs_rect = *mouse::WATCHLIST_STOCK_TABS_RECT.lock().expect("poison");
            if tabs_rect.width > 0
                && row == tabs_rect.y
                && col >= tabs_rect.x
                && col < tabs_rect.x + tabs_rect.width
            {
                let half = tabs_rect.width / 2;
                if col < tabs_rect.x + half {
                    systems::NEWS_VIEW.store(systems::NewsView::Quote, Ordering::Relaxed);
                } else {
                    let news_view = systems::NEWS_VIEW.load(Ordering::Relaxed);
                    if news_view == systems::NewsView::Quote {
                        systems::NEWS_VIEW.store(systems::NewsView::List, Ordering::Relaxed);
                        if let Some(sd) = app.world.get_resource::<systems::StockDetail>() {
                            systems::fetch_news(
                                sd.0.clone(),
                                app.world.resource::<systems::Command>().0.clone(),
                            );
                        }
                    }
                }
                render_state.mark_dirty(DirtyFlags::ALL);
                return;
            }

            // Kline period tab click (only visible in Quote mode)
            let news_view = systems::NEWS_VIEW.load(Ordering::Relaxed);
            if news_view == systems::NewsView::Quote {
                let kline_rect = *mouse::KLINE_TABS_RECT.lock().expect("poison");
                if kline_rect.width > 0
                    && row >= kline_rect.y
                    && row < kline_rect.y + kline_rect.height
                    && col >= kline_rect.x
                    && col < kline_rect.x + kline_rect.width
                {
                    if let Some(kline_type) = kline_tab_at(col - kline_rect.x) {
                        systems::KLINE_TYPE.store(kline_type, Ordering::Relaxed);
                        render_state.mark_dirty(DirtyFlags::STOCK_DETAIL);
                        return;
                    }
                }
            }

            // Watchlist table row click (left sidebar)
            let table_rect = *mouse::WATCHLIST_TABLE_RECT.lock().expect("poison");
            if let Some(row_idx) = mouse::click_to_row(col, row, table_rect) {
                let offset = systems::WATCHLIST_TABLE.lock().expect("poison").offset();
                let actual_idx = row_idx + offset;
                let len = WATCHLIST.read().expect("poison").counters().len();
                if actual_idx < len {
                    systems::WATCHLIST_TABLE
                        .lock()
                        .expect("poison")
                        .select(Some(actual_idx));
                    let counter = WATCHLIST
                        .read()
                        .expect("poison")
                        .counters()
                        .get(actual_idx)
                        .cloned();
                    if let Some(counter) = counter {
                        let mut queue = CommandQueue::default();
                        queue.push(InsertResource {
                            resource: systems::StockDetail(counter),
                        });
                        _ = update_tx.send(queue);
                        render_state.mark_dirty(DirtyFlags::ALL);
                    }
                }
                return;
            }

            // News list item click
            if matches!(
                news_view,
                systems::NewsView::List | systems::NewsView::Detail
            ) {
                let news_rect = *mouse::NEWS_LIST_RECT.lock().expect("poison");
                if news_rect.width > 0
                    && col >= news_rect.x
                    && col < news_rect.x + news_rect.width
                    && row >= news_rect.y
                    && row < news_rect.y + news_rect.height
                {
                    // Each news item is 2 lines tall in non-compact mode
                    let item_idx = ((row - news_rect.y) / 2) as usize;
                    let len = systems::NEWS_ITEMS.lock().expect("poison").len();
                    if item_idx < len {
                        systems::NEWS_LIST_STATE
                            .lock()
                            .expect("poison")
                            .select(Some(item_idx));
                        let id = systems::selected_news_id();
                        if let Some(id) = id {
                            systems::fetch_news_detail(
                                id,
                                app.world.resource::<systems::Command>().0.clone(),
                            );
                            systems::NEWS_VIEW.store(systems::NewsView::Detail, Ordering::Relaxed);
                        }
                        render_state.mark_dirty(DirtyFlags::ALL);
                    }
                }
            }
        }
        AppState::Portfolio => {
            let table_rect = *mouse::PORTFOLIO_TABLE_RECT.lock().expect("poison");
            if let Some(row_idx) = mouse::click_to_row_with_border(col, row, table_rect) {
                let len = systems::PORTFOLIO_VIEW
                    .read()
                    .expect("poison")
                    .as_ref()
                    .map_or(0, |v| v.holdings.len());
                if row_idx < len {
                    systems::PORTFOLIO_TABLE
                        .lock()
                        .expect("poison")
                        .select(Some(row_idx));
                    render_state.mark_dirty(DirtyFlags::PORTFOLIO);
                }
            }
        }
        AppState::Orders => {
            let today_rect = *mouse::ORDERS_TABLE_RECT.lock().expect("poison");
            let history_rect = *mouse::HISTORY_ORDERS_TABLE_RECT.lock().expect("poison");
            if let Some(row_idx) = mouse::click_to_row_with_border(col, row, today_rect) {
                let len = systems::ORDERS_VIEW.read().expect("poison").len();
                if row_idx < len {
                    systems::ORDERS_TABLE
                        .lock()
                        .expect("poison")
                        .select(Some(row_idx));
                    systems::ORDERS_MODE.store(false, Ordering::Relaxed);
                    render_state.mark_dirty(DirtyFlags::ALL);
                }
            } else if let Some(row_idx) = mouse::click_to_row_with_border(col, row, history_rect) {
                let len = systems::HISTORY_ORDERS_VIEW.read().expect("poison").len();
                if row_idx < len {
                    systems::HISTORY_ORDERS_TABLE
                        .lock()
                        .expect("poison")
                        .select(Some(row_idx));
                    systems::ORDERS_MODE.store(true, Ordering::Relaxed);
                    render_state.mark_dirty(DirtyFlags::ALL);
                }
            }
        }
        AppState::Stock => {
            let kline_rect = *mouse::KLINE_TABS_RECT.lock().expect("poison");
            if kline_rect.width > 0
                && row >= kline_rect.y
                && row < kline_rect.y + kline_rect.height
                && col >= kline_rect.x
                && col < kline_rect.x + kline_rect.width
            {
                if let Some(kline_type) = kline_tab_at(col - kline_rect.x) {
                    systems::KLINE_TYPE.store(kline_type, Ordering::Relaxed);
                    render_state.mark_dirty(DirtyFlags::STOCK_DETAIL);
                }
            }
        }
        _ => {}
    }
}

fn handle_popup_mouse_click(
    app: &mut bevy_app::App,
    popup: u16,
    col: u16,
    row: u16,
    update_tx: tokio::sync::mpsc::UnboundedSender<CommandQueue>,
    render_state: &mut RenderState,
) {
    let list_rect = *mouse::POPUP_LIST_RECT.lock().expect("poison");

    if popup == POPUP_ACCOUNT {
        let Some(row_idx) = mouse::click_to_list_item(col, row, list_rect) else {
            return;
        };
        let selected = {
            let search = app
                .world
                .resource_mut::<LocalSearch<crate::data::Account>>();
            search.options().get(row_idx).cloned()
        };
        if let Some(account) = selected {
            {
                app.world
                    .resource_mut::<LocalSearch<crate::data::Account>>()
                    .table
                    .select(Some(row_idx));
            }
            POPUP.store(0, Ordering::Relaxed);
            let mut user = USER.write().expect("poison");
            user.account_channel = account.account_channel;
            user.aaid = account.aaid;
            render_state.mark_dirty(DirtyFlags::ALL);
        }
    } else if popup == POPUP_CURRENCY {
        let Some(row_idx) = mouse::click_to_list_item(col, row, list_rect) else {
            return;
        };
        let selected = {
            let search = app
                .world
                .resource_mut::<LocalSearch<openapi::account::CurrencyInfo>>();
            search.options().get(row_idx).cloned()
        };
        if let Some(currency) = selected {
            {
                app.world
                    .resource_mut::<LocalSearch<openapi::account::CurrencyInfo>>()
                    .table
                    .select(Some(row_idx));
            }
            POPUP.store(0, Ordering::Relaxed);
            let mut user = USER.write().expect("poison");
            user.base_currency = currency.currency_iso;
            render_state.mark_dirty(DirtyFlags::ALL);
        }
    } else if popup == POPUP_WATCHLIST {
        let Some(row_idx) = mouse::click_to_list_item(col, row, list_rect) else {
            return;
        };
        let selected = {
            let search = app.world.resource_mut::<LocalSearch<WatchlistGroup>>();
            search.options().get(row_idx).cloned()
        };
        if let Some(group) = selected {
            {
                app.world
                    .resource_mut::<LocalSearch<WatchlistGroup>>()
                    .table
                    .select(Some(row_idx));
            }
            POPUP.store(0, Ordering::Relaxed);
            WATCHLIST.write().expect("poison").set_group_id(group.id);
            systems::refresh_watchlist(update_tx);
            render_state.mark_dirty(DirtyFlags::ALL);
        }
    } else if popup == POPUP_WATCHLIST_SEARCH {
        let Some(row_idx) = mouse::click_to_list_item(col, row, list_rect) else {
            return;
        };
        let selected = {
            let search = app
                .world
                .resource_mut::<LocalSearch<crate::data::Counter>>();
            search.options().get(row_idx).cloned()
        };
        if let Some(counter) = selected {
            {
                app.world
                    .resource_mut::<LocalSearch<crate::data::Counter>>()
                    .table
                    .select(Some(row_idx));
            }
            POPUP.store(0, Ordering::Relaxed);
            navigate_to_counter(app, counter);
            render_state.mark_dirty(DirtyFlags::ALL);
        }
    }
}
