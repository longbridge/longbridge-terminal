use std::sync::atomic::Ordering;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use atomic::Atomic;
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_ecs::system::{CommandQueue, InsertResource, SystemState};
use tokio::sync::mpsc;

use crate::data::{Counter, KlineType, User, Watchlist, WatchlistGroup};
use crate::tui::input;
use crate::tui::keys::KeyConfig;
use crate::tui::mouse;
use crate::tui::nav::show_index;
use crate::tui::popup::{self, PopupKind};
use crate::tui::render::{DirtyFlags, RenderState};
use crate::tui::ui::Content;
use crate::tui::widgets::{Carousel, Loading, LocalSearch, Search, Terminal};
use crate::{openapi, tui::systems};

pub static RT: OnceLock<tokio::runtime::Handle> = OnceLock::new();
pub static UPDATE_TX: OnceLock<mpsc::UnboundedSender<CommandQueue>> = OnceLock::new();
pub static LAST_STATE: Atomic<AppState> = Atomic::new(AppState::Watchlist);
pub static QUOTE_BMP: Atomic<bool> = Atomic::new(false);
pub static LOG_PANEL_VISIBLE: Atomic<bool> = Atomic::new(false);
pub static WATCHLIST: std::sync::LazyLock<RwLock<Watchlist>> =
    std::sync::LazyLock::new(Default::default);
pub static USER: std::sync::LazyLock<RwLock<User>> = std::sync::LazyLock::new(Default::default);
pub static ACCOUNT_CHANNEL: std::sync::LazyLock<RwLock<Option<String>>> =
    std::sync::LazyLock::new(|| RwLock::new(None));

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

                    let account = &accounts.status[0];
                    {
                        let mut user = USER.write().expect("poison");
                        user.account_channel.clone_from(&account.account_channel);
                        user.aaid.clone_from(&account.aaid);
                    }

                    let mut queue = CommandQueue::default();
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

                if !LOG_PANEL_VISIBLE.load(Ordering::Relaxed) {
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
        }
    });

    let render_interval = std::time::Duration::from_millis(33); // ~30 FPS
    let mut render_tick = tokio::time::interval(render_interval);
    render_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let keys = KeyConfig::default();
    let mut events = crossterm::event::EventStream::new();
    let mut render_state = RenderState::new();
    render_state.mark_all_dirty();

    loop {
        tokio::select! {
            _ = render_tick.tick() => {
                if render_state.needs_render() {
                    app.update();
                    render_state.clear();
                } else {
                    render_state.skip();
                }
            }
            Some(mut cmd) = update_rx.recv() => {
                cmd.apply(&mut app.world);
                render_state.mark_dirty(DirtyFlags::ALL);
            }
            Some(push_event) = tokio_stream::StreamExt::next(&mut quote_receiver) => {
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
                            stock.update_from_push_quote(&quote);
                        });
                        render_state.mark_dirty(DirtyFlags::NONE.mark_quote_update());
                    }
                    PushEventDetail::Depth(depth) => {
                        tracing::debug!("Update depth: {}", symbol);
                        crate::data::STOCKS.modify(counter, |stock| {
                            use rust_decimal::Decimal;
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
                        render_state.mark_dirty(DirtyFlags::NONE.mark_depth_update());
                    }
                    _ => {}
                }
            }
            Some(event) = tokio_stream::StreamExt::next(&mut events) => {
                let event = match event {
                    Ok(crossterm::event::Event::Key(event)) => event,
                    Ok(crossterm::event::Event::Mouse(mouse_event)) => {
                        let popup = popup::current();
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

                let popup = popup::current();
                let state = *app.world.resource::<State<AppState>>().get();

                // Toggle log panel: global shortcut, works even with popups
                if event == keys.toggle_log {
                    let was_visible = LOG_PANEL_VISIBLE.load(Ordering::Relaxed);
                    LOG_PANEL_VISIBLE.store(!was_visible, Ordering::Relaxed);
                    render_state.mark_dirty(DirtyFlags::ALL);
                    continue;
                }

                if popup.is_open() {
                    input::handle_popup_input(&mut app, popup, event, update_tx.clone());
                    render_state.mark_dirty(DirtyFlags::NONE.mark_popup_change(popup));
                    continue;
                }

                match state {
                    AppState::Error => return,
                    AppState::Loading => {
                        if matches!(event, ctrl!('c') | key!('q')) {
                            return;
                        }
                        continue;
                    }
                    AppState::TradeToken => {
                        match event {
                            ctrl!('c') => return,
                            key!(Esc) => {
                                app.world.insert_resource(NextState(Some(
                                    LAST_STATE.load(Ordering::Relaxed),
                                )));
                                render_state.mark_dirty(DirtyFlags::ALL);
                            }
                            _ => {
                                let evt = crossterm::event::Event::Key(event);
                                if let Some(evt) =
                                    tui_input::backend::crossterm::to_input_request(&evt)
                                {
                                    send_evt(systems::TuiEvent(evt), &mut app.world);
                                    render_state.mark_dirty(DirtyFlags::ALL);
                                }
                            }
                        }
                        continue;
                    }
                    AppState::Portfolio
                    | AppState::Stock
                    | AppState::Watchlist
                    | AppState::WatchlistStock
                    | AppState::Orders => (),
                }

                input::handle_global_keys(
                    &mut app,
                    event,
                    state,
                    update_tx.clone(),
                    &mut render_state,
                    &keys,
                );
            }
        }
    }
}

fn send_evt<T: Event>(evt: T, world: &mut World) {
    let mut state = SystemState::<EventWriter<T>>::new(world);
    state.get_mut(world).send(evt);
}

/// Map a column offset relative to the kline tab bar's left edge to the clicked `KlineType`.
fn kline_tab_at(rel_col: u16) -> Option<KlineType> {
    let mut x = 0u16;
    for kline_type in <KlineType as strum::IntoEnumIterator>::iter() {
        let label = kline_type.to_string();
        let tab_w = (label.chars().count() as u16) + 2;
        if rel_col < x + tab_w {
            return Some(kline_type);
        }
        x += tab_w + 1;
    }
    None
}

#[allow(clippy::too_many_lines)]
fn handle_mouse_event(
    app: &mut bevy_app::App,
    event: crossterm::event::MouseEvent,
    state: AppState,
    popup: PopupKind,
    update_tx: tokio::sync::mpsc::UnboundedSender<CommandQueue>,
    render_state: &mut RenderState,
) {
    use crossterm::event::{MouseButton, MouseEventKind};

    let MouseEventKind::Down(MouseButton::Left) = event.kind else {
        return;
    };

    let col = event.column;
    let row = event.row;

    if popup.is_open() {
        if popup == PopupKind::Help {
            popup::close();
            render_state.mark_dirty(DirtyFlags::ALL);
        } else {
            input::handle_popup_mouse_click(app, popup, col, row, update_tx, render_state);
        }
        return;
    }

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
