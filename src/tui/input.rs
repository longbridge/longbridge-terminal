use bevy_ecs::prelude::*;
use bevy_ecs::system::{CommandQueue, SystemState};
use crossterm::event::KeyEvent;
use tokio::sync::mpsc;

use crate::data::WatchlistGroup;
use crate::tui::app::{AppState, WATCHLIST};
use crate::tui::keys::KeyConfig;
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
            let mut search = app.world.resource_mut::<LocalSearch<WatchlistGroup>>();
            let (hidden, selected) = search.handle_key(event);
            if hidden {
                popup::close();
            }
            if let Some(group) = selected {
                popup::close();
                WATCHLIST.write().expect("poison").set_group_id(group.id);
                systems::refresh_watchlist(update_tx.clone());
            }
        }
        PopupKind::Search => {
            // Check for direct symbol navigation: Enter with typed text but no dropdown selection
            let direct_query = {
                let mut search = app
                    .world
                    .resource_mut::<Search<openapi::search::StockItem>>();
                search.consume_direct_enter(event)
            };
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
                let mut search = app
                    .world
                    .resource_mut::<Search<openapi::search::StockItem>>();
                let (hidden, selected) = search.handle_key(event);
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

#[allow(clippy::too_many_lines)]
pub fn handle_global_keys(
    app: &mut bevy_app::App,
    event: KeyEvent,
    state: AppState,
    update_tx: mpsc::UnboundedSender<CommandQueue>,
    render_state: &mut RenderState,
    keys: &KeyConfig,
) {
    // Ctrl+C: force quit
    if event == keys.force_quit {
        crate::tui::widgets::Terminal::graceful_exit(0);
    }

    // Number keys: switch tabs
    if event == keys.tab_watchlist && state != AppState::Watchlist {
        app.world
            .insert_resource(NextState(Some(AppState::Watchlist)));
        render_state.mark_dirty(DirtyFlags::ALL);
        return;
    }
    if event == keys.tab_portfolio && state != AppState::Portfolio {
        if app.world.get_resource::<systems::Portfolio>().is_none() {
            app.world.insert_resource(systems::Portfolio::default());
        }
        app.world
            .insert_resource(NextState(Some(AppState::Portfolio)));
        render_state.mark_dirty(DirtyFlags::ALL);
        return;
    }
    if event == keys.tab_orders && state != AppState::Orders {
        app.world.insert_resource(NextState(Some(AppState::Orders)));
        render_state.mark_dirty(DirtyFlags::ALL);
        return;
    }

    // Buy/Sell: open order entry
    let tradeable = matches!(
        state,
        AppState::Watchlist | AppState::WatchlistStock | AppState::Stock | AppState::Portfolio
    );
    if event == keys.buy && tradeable {
        if let Some(symbol) = get_active_symbol(app, state) {
            systems::open_order_entry(symbol, longbridge::trade::OrderSide::Buy, None);
            popup::open(PopupKind::OrderEntry);
            render_state.mark_dirty(DirtyFlags::ALL);
        }
        return;
    }
    if event == keys.sell && tradeable {
        if let Some(symbol) = get_active_symbol(app, state) {
            systems::open_order_entry(symbol, longbridge::trade::OrderSide::Sell, None);
            popup::open(PopupKind::OrderEntry);
            render_state.mark_dirty(DirtyFlags::ALL);
        }
        return;
    }

    // Orders-specific actions
    if state == AppState::Orders {
        let history_mode = systems::ORDERS_MODE.load(std::sync::atomic::Ordering::Relaxed);
        if event == keys.cancel_order && !history_mode {
            systems::try_open_cancel_for_selected();
            render_state.mark_dirty(DirtyFlags::ALL);
            return;
        }
        if event == keys.modify_order && !history_mode {
            systems::try_open_replace_for_selected();
            render_state.mark_dirty(DirtyFlags::ALL);
            return;
        }
        if event == keys.date_filter && history_mode {
            systems::open_date_filter();
            render_state.mark_dirty(DirtyFlags::ALL);
            return;
        }
    }

    // Portfolio-specific popups (a/c keys)
    if state == AppState::Portfolio {
        if event == keys.account_selector {
            if let Some(mut account) = app
                .world
                .get_resource_mut::<LocalSearch<crate::data::Account>>()
            {
                popup::open(PopupKind::Account);
                account.visible();
                render_state.mark_dirty(DirtyFlags::POPUP_ACCOUNT);
            }
            return;
        }
        if event == keys.currency_selector {
            if let Some(mut currency) = app
                .world
                .get_resource_mut::<LocalSearch<openapi::account::CurrencyInfo>>()
            {
                popup::open(PopupKind::Currency);
                currency.visible();
                render_state.mark_dirty(DirtyFlags::POPUP_CURRENCY);
            }
            return;
        }
    }

    // Watchlist group selector (g/G)
    if (state == AppState::Watchlist || state == AppState::WatchlistStock)
        && (event == keys.group_selector || event == keys.group_selector_upper)
    {
        if let Some(mut search) = app.world.get_resource_mut::<LocalSearch<WatchlistGroup>>() {
            popup::open(PopupKind::Watchlist);
            search.visible();
            render_state.mark_dirty(DirtyFlags::POPUP_WATCHLIST);
        }
        return;
    }

    // Index shortcuts (Q/W/E)
    if event == keys.index_us {
        show_index(&mut app.world, 0);
        render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
        return;
    }
    if event == keys.index_hk {
        show_index(&mut app.world, 1);
        render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
        return;
    }
    if event == keys.index_cn {
        show_index(&mut app.world, 2);
        render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
        return;
    }

    // Toggle layout (t): switch between Stock/WatchlistStock
    if event == keys.toggle_layout {
        if state == AppState::Stock {
            app.world
                .insert_resource(NextState(Some(AppState::WatchlistStock)));
            render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
        } else if state == AppState::WatchlistStock {
            app.world.insert_resource(NextState(Some(AppState::Stock)));
            render_state.mark_dirty(DirtyFlags::STOCK_DETAIL | DirtyFlags::WATCHLIST);
        }
        return;
    }

    // Refresh (R)
    if event == keys.refresh {
        match state {
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
        }
        return;
    }

    // Search (/)
    if event == keys.search {
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
        return;
    }

    // Help (?)
    if event == keys.help {
        popup::open(PopupKind::Help);
        render_state.mark_dirty(DirtyFlags::POPUP_HELP);
        return;
    }

    // Quit (q)
    if event == keys.quit {
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
        return;
    }

    // WatchlistStock-specific actions
    if state == AppState::WatchlistStock {
        if event == keys.news_toggle {
            send_evt(systems::Key::NewsToggle, &mut app.world);
            render_state.mark_dirty(DirtyFlags::ALL);
            return;
        }
        if event == keys.news_open {
            send_evt(systems::Key::NewsOpen, &mut app.world);
            render_state.mark_dirty(DirtyFlags::ALL);
            return;
        }
        if is_page_up(event) || is_shift_k(event) {
            send_evt(systems::Key::NewsScrollUp, &mut app.world);
            render_state.mark_dirty(DirtyFlags::ALL);
            return;
        }
        if is_page_down(event) || is_shift_j(event) {
            send_evt(systems::Key::NewsScrollDown, &mut app.world);
            render_state.mark_dirty(DirtyFlags::ALL);
            return;
        }
    }

    // Escape
    if is_esc(event) {
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
        return;
    }

    // Navigation: Up/k/K
    if is_up(event) {
        send_evt(systems::Key::Up, &mut app.world);
        render_state.mark_dirty(nav_dirty(state));
        return;
    }

    // Navigation: Down/j/J
    if is_down(event) {
        send_evt(systems::Key::Down, &mut app.world);
        render_state.mark_dirty(nav_dirty(state));
        return;
    }

    // Navigation: Left/h/H
    if is_left(event) {
        send_evt(systems::Key::Left, &mut app.world);
        render_state.mark_dirty(match state {
            AppState::Stock => DirtyFlags::STOCK_DETAIL,
            _ => DirtyFlags::ALL,
        });
        return;
    }

    // Navigation: Right/l/L
    if is_right(event) {
        send_evt(systems::Key::Right, &mut app.world);
        render_state.mark_dirty(match state {
            AppState::Stock => DirtyFlags::STOCK_DETAIL,
            _ => DirtyFlags::ALL,
        });
        return;
    }

    // Tab
    if is_tab(event) {
        send_evt(systems::Key::Tab, &mut app.world);
        render_state.mark_dirty(match state {
            AppState::Stock => DirtyFlags::STOCK_DETAIL,
            _ => DirtyFlags::ALL,
        });
        return;
    }

    // BackTab (Shift+Tab)
    if is_backtab(event) {
        send_evt(systems::Key::BackTab, &mut app.world);
        render_state.mark_dirty(match state {
            AppState::Stock => DirtyFlags::STOCK_DETAIL,
            _ => DirtyFlags::ALL,
        });
        return;
    }

    // Enter
    if is_enter(event) {
        send_evt(systems::Key::Enter, &mut app.world);
        render_state.mark_dirty(DirtyFlags::ALL);
    }
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

// Key matching helpers to replace the verbose struct literal patterns

fn is_esc(event: KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    event.code == KeyCode::Esc
        && event.modifiers == KeyModifiers::NONE
        && event.kind == crossterm::event::KeyEventKind::Press
}

fn is_up(event: KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    event.kind == crossterm::event::KeyEventKind::Press
        && matches!(event.code, KeyCode::Up | KeyCode::Char('k'))
        && matches!(event.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
}

fn is_down(event: KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    event.kind == crossterm::event::KeyEventKind::Press
        && matches!(event.code, KeyCode::Down | KeyCode::Char('j'))
        && matches!(event.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
}

fn is_left(event: KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    event.kind == crossterm::event::KeyEventKind::Press
        && matches!(event.code, KeyCode::Left | KeyCode::Char('h'))
        && matches!(event.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
}

fn is_right(event: KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    event.kind == crossterm::event::KeyEventKind::Press
        && matches!(event.code, KeyCode::Right | KeyCode::Char('l'))
        && matches!(event.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
}

fn is_tab(event: KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    event.code == KeyCode::Tab
        && event.modifiers == KeyModifiers::NONE
        && event.kind == crossterm::event::KeyEventKind::Press
}

fn is_backtab(event: KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    event.code == KeyCode::BackTab
        && event.modifiers == KeyModifiers::SHIFT
        && event.kind == crossterm::event::KeyEventKind::Press
}

fn is_enter(event: KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    event.code == KeyCode::Enter
        && event.modifiers == KeyModifiers::NONE
        && event.kind == crossterm::event::KeyEventKind::Press
}

fn is_page_up(event: KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    event.code == KeyCode::PageUp
        && event.modifiers == KeyModifiers::NONE
        && event.kind == crossterm::event::KeyEventKind::Press
}

fn is_page_down(event: KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    event.code == KeyCode::PageDown
        && event.modifiers == KeyModifiers::NONE
        && event.kind == crossterm::event::KeyEventKind::Press
}

fn is_shift_k(event: KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    event.code == KeyCode::Char('K')
        && event.modifiers == KeyModifiers::SHIFT
        && event.kind == crossterm::event::KeyEventKind::Press
}

fn is_shift_j(event: KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    event.code == KeyCode::Char('J')
        && event.modifiers == KeyModifiers::SHIFT
        && event.kind == crossterm::event::KeyEventKind::Press
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
