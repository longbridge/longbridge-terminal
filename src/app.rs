use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use atomic::Atomic;
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_ecs::system::{CommandQueue, InsertResource, SystemState};
use once_cell::sync::Lazy;
use tokio::sync::mpsc;

use crate::data::{Counter, User, Watchlist, WatchlistGroup};
use crate::render::{DirtyFlags, RenderState};
use crate::system;
use crate::ui::Content;
use crate::widgets::{Carousel, Loading, LocalSearch, Search, Terminal};

pub static RT: OnceLock<tokio::runtime::Handle> = OnceLock::new();
pub static POPUP: AtomicU8 = AtomicU8::new(0);
pub static LAST_STATE: Atomic<AppState> = Atomic::new(AppState::Watchlist);
pub static QUOTE_BMP: Atomic<bool> = Atomic::new(false);
pub static WATCHLIST: Lazy<RwLock<Watchlist>> = Lazy::new(Default::default);
pub static USER: Lazy<RwLock<User>> = Lazy::new(Default::default);

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
    mut quote_receiver: impl tokio_stream::Stream<Item = longport::quote::PushEvent> + Unpin,
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
            let symbols: Vec<String> = subs.iter().map(|c| c.to_string()).collect();

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
                .subscribe(&symbols, longport::quote::SubFlags::QUOTE)
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
            let query = crate::api::search::StockQuery {
                keyword,
                market: "HK,SG,SH,SZ,US".to_string(),
                product: "BK,ETF,IX,ST,WT".to_string(),
                account_channel: USER
                    .read()
                    .expect("poison")
                    .get_account_channel()
                    .to_string(),
            };
            crate::api::search::fetch_stock(&query)
                .await
                .map(|v| v.product_list)
                .unwrap_or_default()
        })
    });
    let search_watchlist = LocalSearch::new(Vec::<WatchlistGroup>::new(), |_keyword, _group| false);

    RT.set(tokio::runtime::Handle::current()).unwrap();
    let mut app = bevy_app::App::new();
    app.add_state::<AppState>()
        .add_event::<system::Key>()
        .add_event::<system::TuiEvent>()
        .init_resource::<Terminal>()
        .init_resource::<Loading>()
        .insert_resource(search_stock)
        .insert_resource(search_watchlist)
        .insert_resource(system::Command(update_tx.clone()))
        .insert_resource(Carousel::new(indexes, Duration::from_secs(5)))
        .insert_resource(system::WsState(crate::data::ReadyState::Open))
        .add_systems(Update, system::loading.run_if(in_state(AppState::Loading)))
        .add_systems(Update, system::error.run_if(in_state(AppState::Error)))
        .add_systems(OnExit(AppState::Watchlist), system::exit_watchlist)
        .add_systems(
            Update,
            system::render_watchlist.run_if(in_state(AppState::Watchlist)),
        )
        .add_systems(OnEnter(AppState::Stock), system::enter_stock)
        .add_systems(OnExit(AppState::Stock), system::exit_stock)
        .add_systems(
            Update,
            system::render_stock.run_if(in_state(AppState::Stock)),
        )
        .add_systems(OnEnter(AppState::WatchlistStock), system::enter_stock)
        .add_systems(OnExit(AppState::WatchlistStock), system::exit_stock)
        .add_systems(
            Update,
            system::render_watchlist_stock.run_if(in_state(AppState::WatchlistStock)),
        )
        .add_systems(OnEnter(AppState::Portfolio), system::enter_portfolio)
        .add_systems(OnExit(AppState::Portfolio), system::exit_portfolio)
        .add_systems(
            Update,
            system::render_portfolio.run_if(in_state(AppState::Portfolio)),
        );

    // Don't refresh watchlist when transitioning between Watchlist and WatchlistStock
    for v in <AppState as strum::IntoEnumIterator>::iter() {
        if v == AppState::Watchlist || v == AppState::WatchlistStock {
            continue;
        }
        for watch in [AppState::Watchlist, AppState::WatchlistStock] {
            app.add_systems(
                OnTransition { from: v, to: watch },
                system::enter_watchlist_common,
            );
            app.add_systems(
                OnTransition { from: watch, to: v },
                system::exit_watchlist_common,
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
            match crate::api::account::fetch_account_list().await {
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
                        user.account_channel = account.account_channel.clone();
                        user.aaid = account.aaid.clone();
                    }

                    let mut queue = CommandQueue::default();

                    // Add Select<Account> resource for Portfolio
                    queue.push(InsertResource {
                        resource: crate::widgets::Select::new(accounts.status.clone()),
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
                    if let Ok(currencies) =
                        crate::api::account::currencies(&account.account_channel).await
                    {
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
                    system::refresh_watchlist(tx.clone());
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
                use longport::quote::PushEventDetail;

                let symbol = push_event.symbol;
                let counter = Counter::new(&symbol);
                 match push_event.detail {
                     PushEventDetail::Quote(quote) => {
                         tracing::debug!("Update quote: {} = {}", symbol, quote.last_done);
                         crate::data::STOCKS.modify(counter.clone(), |stock| {
                             // PushQuote only contains partial fields, update available fields
                             stock.quote.last_done = Some(quote.last_done);
                             stock.quote.open = Some(quote.open);
                             stock.quote.high = Some(quote.high);
                             stock.quote.low = Some(quote.low);
                             stock.quote.volume = quote.volume as u64;
                             stock.quote.turnover = quote.turnover;
                             stock.quote.timestamp = quote.timestamp.unix_timestamp();
                             // prev_close keeps original value or obtained from elsewhere

                             // Update trade_status directly from SDK
                             stock.trade_status = quote.trade_status;
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
                                    send_evt(system::TuiEvent(evt), &mut app.world);
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
            if user.get_account_channel() != &account.account_channel {
                // TODO: Fetch currency list in background
            }
            user.account_channel = account.account_channel;
            user.aaid = account.aaid;
        }
    } else if popup == POPUP_CURRENCY {
        let mut search = app
            .world
            .resource_mut::<LocalSearch<crate::api::account::CurrencyInfo>>();
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
            system::refresh_watchlist(update_tx.clone());
        }
    } else if popup == POPUP_SEARCH {
        let mut search = app
            .world
            .resource_mut::<Search<crate::api::search::StockItem>>();
        let (hidden, selected) = search.handle_key(event);
        if hidden {
            POPUP.store(0, Ordering::Relaxed);
        }
        if let Some(selected) = selected {
            POPUP.store(0, Ordering::Relaxed);
            app.world
                .insert_resource(system::StockDetail(selected.counter_id));
            let state = *app.world.resource::<State<AppState>>().get();
            let next_state = if state == AppState::Stock {
                AppState::Stock
            } else {
                AppState::WatchlistStock
            };
            app.world.insert_resource(NextState(Some(next_state)));
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
        ctrl!('c') => crate::widgets::Terminal::graceful_exit(0),
        key!('1') if state != AppState::Watchlist => {
            app.world
                .insert_resource(NextState(Some(AppState::Watchlist)));
            render_state.mark_dirty(DirtyFlags::ALL);
        }
        key!('2') if state != AppState::Portfolio => {
            // Create default Portfolio resource if it doesn't exist
            if app.world.get_resource::<system::Portfolio>().is_none() {
                app.world.insert_resource(system::Portfolio {
                    props: system::portfolio::Props::default(),
                    view: system::portfolio::View::default(),
                });
            }
            app.world
                .insert_resource(NextState(Some(AppState::Portfolio)));
            render_state.mark_dirty(DirtyFlags::ALL);
        }
        key!('a') | shift!('a') if state == AppState::Portfolio => {
            if let Some(mut account) = app
                .world
                .get_resource_mut::<LocalSearch<crate::data::Account>>()
            {
                POPUP.store(POPUP_ACCOUNT, Ordering::Relaxed);
                account.visible();
                render_state.mark_dirty(DirtyFlags::POPUP_ACCOUNT);
            }
        }
        key!('c') | shift!('c') if state == AppState::Portfolio => {
            if let Some(mut currency) = app
                .world
                .get_resource_mut::<LocalSearch<crate::api::account::CurrencyInfo>>()
            {
                POPUP.store(POPUP_CURRENCY, Ordering::Relaxed);
                currency.visible();
                render_state.mark_dirty(DirtyFlags::POPUP_CURRENCY);
            }
        }
        key!('g') | key!('G')
            if state == AppState::Watchlist || state == AppState::WatchlistStock =>
        {
            if let Some(mut search) = app.world.get_resource_mut::<LocalSearch<WatchlistGroup>>() {
                POPUP.store(POPUP_WATCHLIST, Ordering::Relaxed);
                search.visible();
                render_state.mark_dirty(DirtyFlags::POPUP_WATCHLIST);
            };
        }
        key!('Q') | shift!('Q') => {
            show_index(&mut app.world, 0);
            render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
        }
        key!('W') | shift!('W') => {
            show_index(&mut app.world, 1);
            render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
        }
        key!('E') | shift!('E') => {
            show_index(&mut app.world, 2);
            render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
        }
        key!('t') | shift!('t') => {
            if state == AppState::Stock {
                app.world
                    .insert_resource(NextState(Some(AppState::WatchlistStock)));
                render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
            } else if state == AppState::WatchlistStock {
                app.world.insert_resource(NextState(Some(AppState::Stock)));
                render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
            }
        }
        key!('R') | shift!('R') => {
            match state {
                AppState::Portfolio => {
                    system::refresh_portfolio();
                    render_state.mark_dirty(DirtyFlags::PORTFOLIO);
                }
                AppState::Watchlist => {
                    system::refresh_watchlist(update_tx.clone());
                    render_state.mark_dirty(DirtyFlags::WATCHLIST);
                }
                AppState::WatchlistStock => {
                    system::refresh_stock(app.world.resource::<system::StockDetail>().0.clone());
                    system::refresh_watchlist(update_tx.clone());
                    render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
                }
                AppState::Stock => {
                    system::refresh_stock(app.world.resource::<system::StockDetail>().0.clone());
                    render_state.mark_dirty(DirtyFlags::STOCK_DETAIL);
                }
                _ => {}
            };
        }
        key!('?') => {
            POPUP.store(POPUP_HELP, Ordering::Relaxed);
            render_state.mark_dirty(DirtyFlags::POPUP_HELP);
        }
        key!('/') => {
            if let Some(mut search) = app
                .world
                .get_resource_mut::<Search<crate::api::search::StockItem>>()
            {
                POPUP.store(POPUP_SEARCH, Ordering::Relaxed);
                search.visible();
                render_state.mark_dirty(DirtyFlags::POPUP_SEARCH);
            }
        }
        key!(Esc) | key!('q') => {
            let last_state = LAST_STATE.load(Ordering::Relaxed);
            if last_state != state {
                app.world.insert_resource(NextState(Some(last_state)));
                render_state.mark_dirty(DirtyFlags::ALL);
            }
        }
        key!(Up) | key!('k') | shift!('k') => {
            send_evt(system::Key::Up, &mut app.world);
            // Navigation keys affect current view
            render_state.mark_dirty(match state {
                AppState::Watchlist | AppState::WatchlistStock => DirtyFlags::WATCHLIST,
                AppState::Stock => DirtyFlags::STOCK_DETAIL,
                AppState::Portfolio => DirtyFlags::PORTFOLIO,
                _ => DirtyFlags::ALL,
            });
        }
        key!(Down) | key!('j') | shift!('j') => {
            send_evt(system::Key::Down, &mut app.world);
            render_state.mark_dirty(match state {
                AppState::Watchlist | AppState::WatchlistStock => DirtyFlags::WATCHLIST,
                AppState::Stock => DirtyFlags::STOCK_DETAIL,
                AppState::Portfolio => DirtyFlags::PORTFOLIO,
                _ => DirtyFlags::ALL,
            });
        }
        key!(Left) | key!('h') | shift!('h') => {
            send_evt(system::Key::Left, &mut app.world);
            render_state.mark_dirty(match state {
                AppState::Stock => DirtyFlags::STOCK_DETAIL,
                _ => DirtyFlags::ALL,
            });
        }
        key!(Right) | key!('l') | shift!('l') => {
            send_evt(system::Key::Right, &mut app.world);
            render_state.mark_dirty(match state {
                AppState::Stock => DirtyFlags::STOCK_DETAIL,
                _ => DirtyFlags::ALL,
            });
        }
        key!(Tab) => {
            send_evt(system::Key::Tab, &mut app.world);
            render_state.mark_dirty(match state {
                AppState::Stock => DirtyFlags::STOCK_DETAIL,
                _ => DirtyFlags::ALL,
            });
        }
        key!(Enter) => {
            send_evt(system::Key::Enter, &mut app.world);
            render_state.mark_dirty(DirtyFlags::ALL);
        }
        shift!(BackTab) => {
            send_evt(system::Key::BackTab, &mut app.world);
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
    world.insert_resource(system::StockDetail(indexes[index].clone()));
    world.insert_resource(NextState(Some(AppState::WatchlistStock)));
}
