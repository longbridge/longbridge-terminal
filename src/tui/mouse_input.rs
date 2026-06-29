use std::sync::atomic::Ordering;

use bevy_ecs::prelude::*;
use bevy_ecs::system::{CommandQueue, InsertResource};
use tokio::sync::mpsc;

use crate::data::KlineType;
use crate::tui::app::{AppState, WATCHLIST};
use crate::tui::input;
use crate::tui::mouse;
use crate::tui::nav::show_index;
use crate::tui::popup::{self, PopupKind};
use crate::tui::render::{DirtyFlags, RenderState};
use crate::tui::systems;

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
pub fn handle_mouse_event(
    app: &mut bevy_app::App,
    event: crossterm::event::MouseEvent,
    state: AppState,
    popup: PopupKind,
    update_tx: mpsc::UnboundedSender<CommandQueue>,
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
