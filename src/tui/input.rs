use bevy_ecs::prelude::*;
use bevy_ecs::system::{CommandQueue, SystemState};
use crossterm::event::KeyEvent;
use tokio::sync::mpsc;

use crate::data::WatchlistGroup;
use crate::tui::app::{AppState, WATCHLIST};
use crate::tui::keymap::{ActionId, Context, Keymap};
use crate::tui::nav::{get_active_symbol, navigate_to_counter, normalize_counter, show_index};
use crate::tui::popup::{self, PopupKind};
use crate::tui::render::{DirtyFlags, RenderState};
use crate::tui::systems;
use crate::tui::widgets::{LocalSearch, Search};
use crate::{openapi, tui::app::USER};

pub fn handle_popup_input(
    app: &mut bevy_app::App,
    popup: PopupKind,
    event: KeyEvent,
    update_tx: mpsc::UnboundedSender<CommandQueue>,
) {
    match popup {
        PopupKind::Account => {
            let mut search = app
                .world
                .resource_mut::<LocalSearch<crate::data::Account>>();
            let (hidden, selected) = search.handle_key(event);
            if hidden {
                popup::close();
            }
            if let Some(account) = selected {
                let mut user = USER.write().expect("poison");
                if user.get_account_channel() != account.account_channel {
                    // TODO: Fetch currency list in background
                }
                user.account_channel = account.account_channel;
                user.aaid = account.aaid;
            }
        }
        PopupKind::Currency => {
            let mut search = app
                .world
                .resource_mut::<LocalSearch<openapi::account::CurrencyInfo>>();
            let (hidden, selected) = search.handle_key(event);
            if hidden {
                popup::close();
            }
            if let Some(currency) = selected {
                popup::close();
                let mut user = USER.write().expect("poison");
                user.base_currency = currency.currency_iso;
            }
        }
        PopupKind::Watchlist => {
            let result = app
                .world
                .get_resource_mut::<LocalSearch<WatchlistGroup>>()
                .map(|mut s| s.handle_key(event));
            if let Some((hidden, selected)) = result {
                if hidden {
                    popup::close();
                }
                if let Some(group) = selected {
                    popup::close();
                    WATCHLIST.write().expect("poison").set_group_id(group.id);
                    systems::refresh_watchlist(update_tx.clone());
                }
            }
        }
        PopupKind::Search => {
            // Check for direct symbol navigation: Enter with typed text but no dropdown selection
            let direct_query = app
                .world
                .get_resource_mut::<Search<openapi::search::StockItem>>()
                .and_then(|mut s| s.consume_direct_enter(event));
            if let Some(query) = direct_query {
                popup::close();
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
                let result = app
                    .world
                    .get_resource_mut::<Search<openapi::search::StockItem>>()
                    .map(|mut s| s.handle_key(event));
                if let Some((hidden, selected)) = result {
                    if hidden {
                        popup::close();
                    }
                    if let Some(selected) = selected {
                        popup::close();
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
            }
        }
        PopupKind::Help => {
            popup::close();
        }
        PopupKind::WatchlistSearch => {
            handle_watchlist_search_input(app, event);
        }
        PopupKind::OrderEntry => {
            systems::handle_order_entry_key(event);
        }
        PopupKind::CancelOrder => {
            systems::handle_cancel_order_key(event);
        }
        PopupKind::ReplaceOrder => {
            systems::handle_replace_order_key(event);
        }
        PopupKind::DateFilter => {
            systems::handle_date_filter_key(event);
        }
        PopupKind::Settings => {
            crate::tui::settings::handle_key(event);
        }
        PopupKind::None => {}
    }
}

fn handle_watchlist_search_input(app: &mut bevy_app::App, event: KeyEvent) {
    let direct_query = {
        let mut search = app
            .world
            .resource_mut::<LocalSearch<crate::data::Counter>>();
        search.consume_direct_enter(event)
    };

    if let Some(query) = direct_query {
        if let Some(symbol) = normalize_counter(&query) {
            popup::close();
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
                popup::close();
            }
            (hidden, selected)
        };
        let _ = hidden;
        if let Some(counter) = selected {
            navigate_to_counter(app, counter);
        }
    }
}

/// Whether the current screen supports opening an order-entry ticket.
fn is_tradeable(state: AppState) -> bool {
    matches!(
        state,
        AppState::Watchlist | AppState::WatchlistStock | AppState::Stock | AppState::Portfolio
    )
}

/// Resolve a key event to an [`ActionId`] via the data-driven [`Keymap`], then
/// execute the action for the current screen. Behavior per action is screen-
/// aware; the keymap only decides *which* action a key triggers.
#[allow(clippy::too_many_lines)]
pub fn handle_global_keys(
    app: &mut bevy_app::App,
    event: KeyEvent,
    state: AppState,
    update_tx: mpsc::UnboundedSender<CommandQueue>,
    render_state: &mut RenderState,
    keymap: &Keymap,
) {
    let Some(action) = keymap.lookup(&event, Context::from_state(state)) else {
        return;
    };

    match action {
        ActionId::ForceQuit => crate::tui::widgets::Terminal::graceful_exit(0),

        ActionId::TabWatchlist => {
            if state != AppState::Watchlist {
                app.world
                    .insert_resource(NextState(Some(AppState::Watchlist)));
                render_state.mark_dirty(DirtyFlags::ALL);
            }
        }
        ActionId::TabPortfolio => {
            if state != AppState::Portfolio {
                if app.world.get_resource::<systems::Portfolio>().is_none() {
                    app.world.insert_resource(systems::Portfolio::default());
                }
                app.world
                    .insert_resource(NextState(Some(AppState::Portfolio)));
                render_state.mark_dirty(DirtyFlags::ALL);
            }
        }
        ActionId::TabOrders => {
            if state != AppState::Orders {
                app.world.insert_resource(NextState(Some(AppState::Orders)));
                render_state.mark_dirty(DirtyFlags::ALL);
            }
        }

        ActionId::Buy => {
            if is_tradeable(state) {
                if let Some(symbol) = get_active_symbol(app, state) {
                    systems::open_order_entry(symbol, longbridge::trade::OrderSide::Buy, None);
                    popup::open(PopupKind::OrderEntry);
                    render_state.mark_dirty(DirtyFlags::ALL);
                }
            }
        }
        ActionId::Sell => {
            if is_tradeable(state) {
                if let Some(symbol) = get_active_symbol(app, state) {
                    systems::open_order_entry(symbol, longbridge::trade::OrderSide::Sell, None);
                    popup::open(PopupKind::OrderEntry);
                    render_state.mark_dirty(DirtyFlags::ALL);
                }
            }
        }

        ActionId::CancelOrder => {
            if !systems::ORDERS_MODE.load(std::sync::atomic::Ordering::Relaxed) {
                systems::try_open_cancel_for_selected();
                render_state.mark_dirty(DirtyFlags::ALL);
            }
        }
        ActionId::ModifyOrder => {
            if !systems::ORDERS_MODE.load(std::sync::atomic::Ordering::Relaxed) {
                systems::try_open_replace_for_selected();
                render_state.mark_dirty(DirtyFlags::ALL);
            }
        }
        ActionId::DateFilter => {
            if systems::ORDERS_MODE.load(std::sync::atomic::Ordering::Relaxed) {
                systems::open_date_filter();
                render_state.mark_dirty(DirtyFlags::ALL);
            }
        }

        ActionId::AccountSelector => {
            if let Some(mut account) = app
                .world
                .get_resource_mut::<LocalSearch<crate::data::Account>>()
            {
                popup::open(PopupKind::Account);
                account.visible();
                render_state.mark_dirty(DirtyFlags::POPUP_ACCOUNT);
            }
        }
        ActionId::CurrencySelector => {
            if let Some(mut currency) = app
                .world
                .get_resource_mut::<LocalSearch<openapi::account::CurrencyInfo>>()
            {
                popup::open(PopupKind::Currency);
                currency.visible();
                render_state.mark_dirty(DirtyFlags::POPUP_CURRENCY);
            }
        }
        ActionId::GroupSelector => {
            if let Some(mut search) = app.world.get_resource_mut::<LocalSearch<WatchlistGroup>>() {
                popup::open(PopupKind::Watchlist);
                search.visible();
                render_state.mark_dirty(DirtyFlags::POPUP_WATCHLIST);
            }
        }

        ActionId::IndexUs => {
            show_index(&mut app.world, 0);
            render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
        }
        ActionId::IndexHk => {
            show_index(&mut app.world, 1);
            render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
        }
        ActionId::IndexCn => {
            show_index(&mut app.world, 2);
            render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
        }

        ActionId::ToggleLayout => {
            if state == AppState::Stock {
                app.world
                    .insert_resource(NextState(Some(AppState::WatchlistStock)));
                render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
            } else if state == AppState::WatchlistStock {
                app.world.insert_resource(NextState(Some(AppState::Stock)));
                render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
            }
        }

        ActionId::Refresh => match state {
            AppState::Portfolio => {
                systems::refresh_portfolio();
                render_state.mark_dirty(DirtyFlags::PORTFOLIO);
            }
            AppState::Watchlist => {
                systems::refresh_watchlist(update_tx.clone());
                render_state.mark_dirty(DirtyFlags::WATCHLIST);
            }
            AppState::WatchlistStock => {
                if let Some(detail) = app.world.get_resource::<systems::StockDetail>() {
                    systems::refresh_stock_debounced(detail.0.clone());
                    render_state.mark_dirty(DirtyFlags::STOCK_DETAIL);
                }
                systems::refresh_watchlist(update_tx.clone());
                render_state.mark_dirty(DirtyFlags::WATCHLIST);
            }
            AppState::Stock => {
                if let Some(detail) = app.world.get_resource::<systems::StockDetail>() {
                    systems::refresh_stock_debounced(detail.0.clone());
                    render_state.mark_dirty(DirtyFlags::STOCK_DETAIL);
                }
            }
            AppState::Orders => {
                systems::refresh_orders();
                systems::refresh_history_orders();
                render_state.mark_dirty(DirtyFlags::ALL);
            }
            _ => {}
        },

        ActionId::Search => {
            if state == AppState::Watchlist || state == AppState::WatchlistStock {
                let mut ws = app
                    .world
                    .resource_mut::<LocalSearch<crate::data::Counter>>();
                ws.visible();
                popup::open(PopupKind::WatchlistSearch);
                render_state.mark_dirty(DirtyFlags::ALL);
            } else {
                let mut search = app
                    .world
                    .resource_mut::<Search<openapi::search::StockItem>>();
                search.visible();
                popup::open(PopupKind::Search);
                render_state.mark_dirty(DirtyFlags::POPUP_SEARCH);
            }
        }

        ActionId::Help => {
            popup::open(PopupKind::Help);
            render_state.mark_dirty(DirtyFlags::POPUP_HELP);
        }

        ActionId::Quit => {
            if state == AppState::WatchlistStock {
                cycle_news_view_back(app, render_state);
            } else {
                crate::tui::widgets::Terminal::graceful_exit(0);
            }
        }

        ActionId::Escape => {
            if state == AppState::WatchlistStock {
                cycle_news_view_back(app, render_state);
            } else if state == AppState::Stock || state == AppState::Orders {
                app.world
                    .insert_resource(NextState(Some(AppState::Watchlist)));
                render_state.mark_dirty(DirtyFlags::ALL);
            } else {
                crate::tui::widgets::Terminal::graceful_exit(0);
            }
        }

        ActionId::NewsToggle => {
            send_evt(systems::Key::NewsToggle, &mut app.world);
            render_state.mark_dirty(DirtyFlags::ALL);
        }
        ActionId::NewsOpen => {
            send_evt(systems::Key::NewsOpen, &mut app.world);
            render_state.mark_dirty(DirtyFlags::ALL);
        }
        ActionId::NewsScrollUp => {
            send_evt(systems::Key::NewsScrollUp, &mut app.world);
            render_state.mark_dirty(DirtyFlags::ALL);
        }
        ActionId::NewsScrollDown => {
            send_evt(systems::Key::NewsScrollDown, &mut app.world);
            render_state.mark_dirty(DirtyFlags::ALL);
        }

        ActionId::Up => {
            send_evt(systems::Key::Up, &mut app.world);
            render_state.mark_dirty(nav_dirty(state));
        }
        ActionId::Down => {
            send_evt(systems::Key::Down, &mut app.world);
            render_state.mark_dirty(nav_dirty(state));
        }
        ActionId::Left => {
            send_evt(systems::Key::Left, &mut app.world);
            render_state.mark_dirty(match state {
                AppState::Stock => DirtyFlags::STOCK_DETAIL,
                _ => DirtyFlags::ALL,
            });
        }
        ActionId::Right => {
            send_evt(systems::Key::Right, &mut app.world);
            render_state.mark_dirty(match state {
                AppState::Stock => DirtyFlags::STOCK_DETAIL,
                _ => DirtyFlags::ALL,
            });
        }
        ActionId::Tab => {
            send_evt(systems::Key::Tab, &mut app.world);
            render_state.mark_dirty(match state {
                AppState::Stock => DirtyFlags::STOCK_DETAIL,
                _ => DirtyFlags::ALL,
            });
        }
        ActionId::BackTab => {
            send_evt(systems::Key::BackTab, &mut app.world);
            render_state.mark_dirty(match state {
                AppState::Stock => DirtyFlags::STOCK_DETAIL,
                _ => DirtyFlags::ALL,
            });
        }
        ActionId::Enter => {
            send_evt(systems::Key::Enter, &mut app.world);
            render_state.mark_dirty(DirtyFlags::ALL);
        }

        ActionId::OpenSettings => {
            crate::tui::settings::open();
            render_state.mark_dirty(DirtyFlags::ALL);
        }

        // Handled globally in the event loop (before the popup check), so it
        // is never reached here; listed for exhaustiveness.
        ActionId::ToggleLog => {}
    }
}

/// Step the `WatchlistStock` news view back one level (Detail -> List -> Quote),
/// finally returning to the Watchlist. Shared by `q` and `Esc`.
fn cycle_news_view_back(app: &mut bevy_app::App, render_state: &mut RenderState) {
    use std::sync::atomic::Ordering;
    match systems::NEWS_VIEW.load(Ordering::Relaxed) {
        systems::NewsView::Detail => {
            systems::NEWS_VIEW.store(systems::NewsView::List, Ordering::Relaxed);
        }
        systems::NewsView::List => {
            systems::NEWS_VIEW.store(systems::NewsView::Quote, Ordering::Relaxed);
        }
        systems::NewsView::Quote => {
            app.world
                .insert_resource(NextState(Some(AppState::Watchlist)));
        }
    }
    render_state.mark_dirty(DirtyFlags::ALL);
}

fn nav_dirty(state: AppState) -> DirtyFlags {
    match state {
        AppState::Watchlist | AppState::WatchlistStock => DirtyFlags::WATCHLIST,
        AppState::Stock => DirtyFlags::STOCK_DETAIL,
        AppState::Portfolio => DirtyFlags::PORTFOLIO,
        _ => DirtyFlags::ALL,
    }
}

fn send_evt<T: Event>(evt: T, world: &mut World) {
    let mut state = SystemState::<EventWriter<T>>::new(world);
    state.get_mut(world).send(evt);
}

pub fn handle_popup_mouse_click(
    app: &mut bevy_app::App,
    popup: PopupKind,
    col: u16,
    row: u16,
    update_tx: tokio::sync::mpsc::UnboundedSender<CommandQueue>,
    render_state: &mut RenderState,
) {
    use crate::tui::mouse;
    let list_rect = *mouse::POPUP_LIST_RECT.lock().expect("poison");

    match popup {
        PopupKind::Account => {
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
                popup::close();
                let mut user = USER.write().expect("poison");
                user.account_channel = account.account_channel;
                user.aaid = account.aaid;
                render_state.mark_dirty(DirtyFlags::ALL);
            }
        }
        PopupKind::Currency => {
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
                popup::close();
                let mut user = USER.write().expect("poison");
                user.base_currency = currency.currency_iso;
                render_state.mark_dirty(DirtyFlags::ALL);
            }
        }
        PopupKind::Watchlist => {
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
                popup::close();
                WATCHLIST.write().expect("poison").set_group_id(group.id);
                systems::refresh_watchlist(update_tx);
                render_state.mark_dirty(DirtyFlags::ALL);
            }
        }
        PopupKind::WatchlistSearch => {
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
                popup::close();
                navigate_to_counter(app, counter);
                render_state.mark_dirty(DirtyFlags::ALL);
            }
        }
        _ => {}
    }
}
