use std::sync::atomic::Ordering;

use bevy_ecs::{
    prelude::*,
    system::{CommandQueue, InsertResource},
};
use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::{
    app::{AppState, WATCHLIST},
    data::Counter,
    utils::cycle,
};

use super::{
    stock_detail::stock_detail, watchlist::watch, Command, Key, NavFooter, PopUp, StockDetail,
    KLINE_INDEX, KLINE_TYPE, WATCHLIST_TABLE,
};

pub fn render_watchlist_stock(
    mut terminal: ResMut<crate::widgets::Terminal>,
    mut events: EventReader<Key>,
    stock: Res<StockDetail>,
    command: Res<Command>,
    (state, indexes, ws): NavFooter,
    (mut account, mut currency, mut search, mut watchgroup): PopUp,
    mut last_choose: Local<Counter>,
    mut log_panel: Local<crate::widgets::LogPanel>,
) {
    // workaround bevyengine/bevy#9130
    if *last_choose != stock.0 {
        if !last_choose.is_empty() {
            super::stock_detail::refresh_stock_debounced(stock.0.clone());
        }
        *last_choose = stock.0.clone();
    }

    for event in &mut events {
        match event {
            Key::Up => {
                let watchlist = WATCHLIST.read().expect("poison");
                let len = watchlist.counters().len();
                let mut table = WATCHLIST_TABLE.lock().expect("poison");
                let idx = table.selected();
                let new_idx = cycle::prev(idx, len);
                table.select(new_idx);
                drop(table);

                // Immediately update stock detail
                if let Some(idx) = new_idx {
                    if let Some(counter) = watchlist.counters().get(idx).cloned() {
                        _ = command.0.send({
                            let mut queue = CommandQueue::default();
                            queue.push(InsertResource {
                                resource: StockDetail(counter),
                            });
                            queue
                        });
                    }
                }
            }
            Key::Down => {
                let watchlist = WATCHLIST.read().expect("poison");
                let len = watchlist.counters().len();
                let mut table = WATCHLIST_TABLE.lock().expect("poison");
                let idx = table.selected();
                let new_idx = cycle::next(idx, len);
                table.select(new_idx);
                drop(table);

                // Immediately update stock detail
                if let Some(idx) = new_idx {
                    if let Some(counter) = watchlist.counters().get(idx).cloned() {
                        _ = command.0.send({
                            let mut queue = CommandQueue::default();
                            queue.push(InsertResource {
                                resource: StockDetail(counter),
                            });
                            queue
                        });
                    }
                }
            }
            Key::Left => {
                _ = KLINE_INDEX.fetch_update(Ordering::Acquire, Ordering::Relaxed, |old| {
                    Some(old.saturating_add(1))
                });
            }
            Key::Right => {
                _ = KLINE_INDEX.fetch_update(Ordering::Acquire, Ordering::Relaxed, |old| {
                    Some(old.saturating_sub(1))
                });
            }
            Key::Tab => {
                KLINE_INDEX.store(0, Ordering::Relaxed);
                _ = KLINE_TYPE.fetch_update(Ordering::Acquire, Ordering::Relaxed, |kline_type| {
                    Some(kline_type.next())
                });
            }
            Key::BackTab => {
                KLINE_INDEX.store(0, Ordering::Relaxed);
                _ = KLINE_TYPE.fetch_update(Ordering::Acquire, Ordering::Relaxed, |kline_type| {
                    Some(kline_type.prev())
                });
            }
            Key::Enter => {
                let Some(idx) = WATCHLIST_TABLE.lock().expect("poison").selected() else {
                    continue;
                };
                let counter = WATCHLIST
                    .read()
                    .expect("poison")
                    .counters()
                    .get(idx)
                    .cloned();
                if let Some(counter) = counter {
                    _ = command.0.send({
                        let mut queue = CommandQueue::default();
                        queue.push(InsertResource {
                            resource: StockDetail(counter),
                        });
                        queue.push(InsertResource {
                            resource: NextState(Some(AppState::WatchlistStock)),
                        });
                        queue
                    });
                }
            }
        }
    }

    _ = terminal.draw(|frame| {
        let rect = frame.size();
        let top = Rect { height: 1, ..rect };
        crate::views::navbar::render(frame, top, *state.get());

        let bottom = Rect {
            y: rect.y + rect.height - 1,
            height: 1,
            ..rect
        };
        crate::views::footer::render(frame, bottom, indexes.tick(), &ws);

        let rect = Rect {
            y: rect.y + 1,
            height: rect.height - 2,
            ..rect
        };
        let chunks = Layout::default()
            .constraints([Constraint::Length(57), Constraint::Min(20)])
            .direction(Direction::Horizontal)
            .split(rect);
        watch(frame, chunks[0], false);
        stock_detail(
            frame,
            chunks[1],
            &stock.0,
            KLINE_TYPE.load(Ordering::Relaxed),
            KLINE_INDEX.load(Ordering::Relaxed),
        );

        crate::views::popup::render(
            frame,
            rect,
            &mut account,
            &mut currency,
            &mut search,
            &mut watchgroup,
        );

        // Render floating log panel if visible
        let log_panel_visible =
            crate::app::LOG_PANEL_VISIBLE.load(std::sync::atomic::Ordering::Relaxed);
        if log_panel_visible {
            log_panel.set_visible(true);
            let panel_height = 15;
            let log_rect = Rect {
                x: rect.x,
                y: rect.y + rect.height.saturating_sub(panel_height),
                width: rect.width,
                height: panel_height,
            };
            log_panel.render(frame, log_rect);
        }
    });
}
