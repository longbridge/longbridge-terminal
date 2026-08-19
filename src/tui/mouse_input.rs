use std::sync::atomic::Ordering;

use bevy_ecs::prelude::*;
use bevy_ecs::system::{CommandQueue, InsertResource};
use tokio::sync::mpsc;

use crate::data::KlineType;
use crate::tui::app::{AppState, WATCHLIST};
use crate::tui::input;
use crate::tui::keymap::ActionId;
use crate::tui::mouse;
use crate::tui::nav::show_index;
use crate::tui::popup::{self, PopupKind};
use crate::tui::render::{DirtyFlags, RenderState};
use crate::tui::systems;

/// Send a navigation `Key` event into the ECS world (same path the keyboard
/// uses), so mouse scrolling reuses each screen's existing cursor handling.
fn send_key(world: &mut World, key: systems::Key) {
    use bevy_ecs::system::SystemState;
    let mut state = SystemState::<EventWriter<systems::Key>>::new(world);
    state.get_mut(world).send(key);
}

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

/// Whether the pointer is over the body of an open news article.
fn over_news_article(state: AppState, col: u16, row: u16) -> bool {
    if state != AppState::WatchlistStock
        || systems::NEWS_VIEW.load(Ordering::Relaxed) != systems::NewsView::Detail
    {
        return false;
    }
    let r = *mouse::NEWS_DETAIL_RECT.lock().expect("poison");
    r.width > 0 && col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

/// Classify a click against the open detail panel: `None` if it landed
/// elsewhere, `Some(true)` on the close button, `Some(false)` anywhere else in
/// the panel (absorbed, so it does not reach the list behind it).
fn detail_panel_click(col: u16, row: u16, open: bool) -> Option<bool> {
    if !open {
        return None;
    }
    let hit = |r: ratatui::layout::Rect| {
        r.width > 0 && col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
    };
    if hit(*mouse::DETAIL_CLOSE_RECT.lock().expect("poison")) {
        return Some(true);
    }
    hit(*mouse::DETAIL_PANEL_RECT.lock().expect("poison")).then_some(false)
}

#[allow(clippy::too_many_lines)]
pub fn handle_mouse_event(
    app: &mut bevy_app::App,
    event: crossterm::event::MouseEvent,
    state: AppState,
    popup: PopupKind,
    update_tx: mpsc::UnboundedSender<CommandQueue>,
    render_state: &mut RenderState,
) {
    use crossterm::event::{MouseButton, MouseEventKind};

    // Scroll wheel: map to cursor up/down on the active screen's list, reusing
    // the same Key events as the keyboard so every list scrolls with no
    // per-list plumbing. (Popups manage their own scrolling elsewhere.)
    match event.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            if popup.is_open() {
                return;
            }
            let up = event.kind == MouseEventKind::ScrollUp;
            // Over the open article, the wheel scrolls the text; anywhere else
            // it moves the cursor in the screen's list.
            if over_news_article(state, event.column, event.row) {
                systems::news_detail_scroll_by(if up { -3 } else { 3 });
            } else {
                send_key(
                    &mut app.world,
                    if up {
                        systems::Key::Up
                    } else {
                        systems::Key::Down
                    },
                );
            }
            render_state.mark_dirty(DirtyFlags::ALL);
            return;
        }
        MouseEventKind::Down(MouseButton::Left) => {}
        _ => return,
    }

    let col = event.column;
    let row = event.row;

    // Links win over everything else under the cursor: they are drawn on top
    // (help popup, logo banner) and clicking one should open the browser.
    if let Some(url) = mouse::link_at(col, row) {
        let _ = open::that(url);
        return;
    }

    if popup.is_open() {
        if popup == PopupKind::Help {
            popup::close();
            render_state.mark_dirty(DirtyFlags::ALL);
        } else {
            input::handle_popup_mouse_click(app, popup, col, row, update_tx, render_state);
        }
        return;
    }

    // Primary tabs: hit-test against the pill rects the navbar recorded, so the
    // click areas always match what was drawn.
    let tab_rects = *mouse::NAVBAR_TAB_RECTS.lock().expect("poison");
    for (i, trect) in tab_rects.iter().enumerate() {
        if trect.width == 0 || row != trect.y || col < trect.x || col >= trect.x + trect.width {
            continue;
        }
        match i {
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

    // Shortcut hints double as buttons — a click runs the same action the key
    // would.
    let hint_hit = mouse::NAVBAR_HINT_RECTS
        .lock()
        .expect("poison")
        .iter()
        .find(|(r, _)| row == r.y && col >= r.x && col < r.x + r.width)
        .map(|(_, action)| *action);
    if let Some(action) = hint_hit {
        input::run_action(app, action, state, update_tx, render_state);
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
            // `News [n]` on the quote panel's border opens the list; `Back
            // [esc]` on the list's border steps back out, the same way the key
            // does.
            let hit = |r: ratatui::layout::Rect| {
                r.width > 0 && row == r.y && col >= r.x && col < r.x + r.width
            };
            if hit(*mouse::NEWS_BACK_RECT.lock().expect("poison")) {
                input::run_action(app, ActionId::Escape, state, update_tx, render_state);
                return;
            }
            if hit(*mouse::NEWS_OPEN_RECT.lock().expect("poison")) {
                input::run_action(app, ActionId::NewsToggle, state, update_tx, render_state);
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

            // The list is clickable wherever it is drawn — including the dock
            // beside the quote panel, where picking a headline opens it.
            {
                let news_rect = *mouse::NEWS_LIST_RECT.lock().expect("poison");
                if news_rect.width > 0
                    && col >= news_rect.x
                    && col < news_rect.x + news_rect.width
                    && row >= news_rect.y
                    && row < news_rect.y + news_rect.height
                {
                    let item_idx = ((row - news_rect.y) / systems::NEWS_ITEM_ROWS) as usize;
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
            if let Some(closed) = detail_panel_click(
                col,
                row,
                systems::HOLDING_DETAIL_OPEN.load(Ordering::Relaxed),
            ) {
                if closed {
                    systems::close_holding_detail();
                }
                render_state.mark_dirty(DirtyFlags::ALL);
                return;
            }
            let table_rect = *mouse::PORTFOLIO_TABLE_RECT.lock().expect("poison");
            if let Some(row_idx) = mouse::click_to_row_with_border(col, row, table_rect) {
                let len = systems::PORTFOLIO_VIEW
                    .read()
                    .expect("poison")
                    .as_ref()
                    .map_or(0, |v| v.holdings.len());
                if row_idx < len {
                    systems::open_holding_detail(row_idx);
                    render_state.mark_dirty(DirtyFlags::ALL);
                }
            }
        }
        AppState::Orders => {
            if let Some(closed) =
                detail_panel_click(col, row, systems::ORDER_DETAIL_OPEN.load(Ordering::Relaxed))
            {
                if closed {
                    systems::close_order_detail();
                }
                render_state.mark_dirty(DirtyFlags::ALL);
                return;
            }
            let today_rect = *mouse::ORDERS_TABLE_RECT.lock().expect("poison");
            let history_rect = *mouse::HISTORY_ORDERS_TABLE_RECT.lock().expect("poison");
            if let Some(row_idx) = mouse::click_to_row_with_border(col, row, today_rect) {
                let len = systems::ORDERS_VIEW.read().expect("poison").len();
                if row_idx < len {
                    systems::open_order_detail(row_idx, false);
                    render_state.mark_dirty(DirtyFlags::ALL);
                }
            } else if let Some(row_idx) = mouse::click_to_row_with_border(col, row, history_rect) {
                let len = systems::HISTORY_ORDERS_VIEW.read().expect("poison").len();
                if row_idx < len {
                    systems::open_order_detail(row_idx, true);
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
