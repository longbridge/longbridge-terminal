use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use atomic::Atomic;
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_ecs::system::{CommandQueue, InsertResource, SystemState};
use tokio::sync::mpsc;

use crate::data::{Counter, User, Watchlist, WatchlistGroup};
use crate::tui::render::{DirtyFlags, RenderState};
use crate::tui::ui::Content;
use crate::tui::widgets::{Carousel, Loading, LocalSearch, Search, Terminal};
use crate::{openapi, tui::systems};

pub static RT: OnceLock<tokio::runtime::Handle> = OnceLock::new();
pub static POPUP: AtomicU8 = AtomicU8::new(0);
pub static LAST_STATE: Atomic<AppState> = Atomic::new(AppState::Watchlist);
pub static QUOTE_BMP: Atomic<bool> = Atomic::new(false);
pub static LOG_PANEL_VISIBLE: Atomic<bool> = Atomic::new(false);
pub static WATCHLIST: std::sync::LazyLock<RwLock<Watchlist>> =
    std::sync::LazyLock::new(Default::default);
pub static USER: std::sync::LazyLock<RwLock<User>> = std::sync::LazyLock::new(Default::default);

pub const POPUP_HELP: u8 = 0b1;
pub const POPUP_SEARCH: u8 = 0b10;
pub const POPUP_ACCOUNT: u8 = 0b100;
pub const POPUP_CURRENCY: u8 = 0b1000;
pub const POPUP_WATCHLIST: u8 = 0b10000;

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
}

#[allow(clippy::too_many_lines)]
pub async fn run(
    _args: crate::Args,
    mut quote_receiver: impl tokio_stream::Stream<Item = longbridge::quote::PushEvent> + Unpin,
) {
    let (update_tx, mut update_rx) = mpsc::unbounded_channel();

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

    RT.set(tokio::runtime::Handle::current()).unwrap();
    let mut app = bevy_app::App::new();
    app.add_state::<AppState>()
        .add_event::<systems::Key>()
        .add_event::<systems::TuiEvent>()
        .init_resource::<Terminal>()
        .init_resource::<Loading>()
        .insert_resource(search_stock)
        .insert_resource(search_watchlist)
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
                    Ok(_) => {
                        // Non-key events (mouse, resize, etc.) - ignore for now
                        continue
                    },
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
                    AppState::Portfolio | AppState::Stock | AppState::Watchlist | AppState::WatchlistStock => (),
                }

                // Handle global keyboard shortcuts
                handle_global_keys(&mut app, event, state, update_tx.clone(), &mut render_state);
            }
        }
    }
}

fn handle_popup_input(
    app: &mut bevy_app::App,
    popup: u8,
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
    }
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
            _ => {}
        },
        key!('/') => {
            {
                let mut search = app
                    .world
                    .resource_mut::<Search<openapi::search::StockItem>>();
                search.visible();
            }
            POPUP.store(POPUP_SEARCH, Ordering::Relaxed);
            render_state.mark_dirty(DirtyFlags::POPUP_SEARCH);
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
            } else if state == AppState::Stock {
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
