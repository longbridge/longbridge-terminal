use std::sync::atomic::Ordering;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use atomic::Atomic;
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_ecs::system::{CommandQueue, SystemState};
use tokio::sync::mpsc;

use crate::data::{Counter, User, Watchlist, WatchlistGroup};
use crate::tui::init;
use crate::tui::input;
use crate::tui::keymap::{ActionId, Context};
use crate::tui::mouse_input;
use crate::tui::popup;
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

#[allow(clippy::too_many_lines)]
pub async fn run(
    _args: crate::Args,
    mut quote_receiver: impl tokio_stream::Stream<Item = longbridge::quote::PushEvent> + Unpin,
) {
    // Apply persisted user settings (e.g. up/down colors) before first render.
    crate::tui::settings::load_and_apply();

    let (update_tx, mut update_rx) = mpsc::unbounded_channel();
    UPDATE_TX.set(update_tx.clone()).ok();

    // Initialize index subscriptions
    let indexes: Vec<[Counter; 3]> = vec![
        [".DJI.US".into(), ".IXIC.US".into(), "SPY.US".into()],
        ["HSI.HK".into(), "HSCEI.HK".into(), "HSTECH.HK".into()],
        ["000001.SH".into(), "399001.SZ".into(), "399006.SZ".into()],
    ];
    let subs: Vec<Counter> = indexes.iter().flatten().cloned().collect();
    init::subscribe_indexes(subs);
    init::start_account_init(update_tx.clone());
    init::start_log_watcher(update_tx.clone());

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

    let render_interval = std::time::Duration::from_millis(33); // ~30 FPS
    let mut render_tick = tokio::time::interval(render_interval);
    render_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let keymap = crate::tui::keymap::global();
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
                        mouse_input::handle_mouse_event(&mut app, mouse_event, state, popup, update_tx.clone(), &mut render_state);
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
                if keymap.lookup(&event, Context::Always) == Some(ActionId::ToggleLog) {
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
                    keymap,
                );
            }
        }
    }
}

fn send_evt<T: Event>(evt: T, world: &mut World) {
    let mut state = SystemState::<EventWriter<T>>::new(world);
    state.get_mut(world).send(evt);
}
