use std::sync::atomic::Ordering;

use bevy_ecs::{
    prelude::*,
    system::{CommandQueue, InsertResource},
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Modifier,
    text::Line,
    widgets::Tabs,
};

use crate::{
    data::Counter,
    tui::app::{AppState, WATCHLIST},
    utils::cycle,
};

use super::{
    stock_detail::stock_detail,
    stock_news::{
        fetch_news, fetch_news_detail, news_detail_scroll_down, news_detail_scroll_up,
        news_list_down, news_list_up, render_news_detail_view, render_news_list_view,
        selected_news_id, selected_news_url, NewsView, NEWS_VIEW,
    },
    watchlist::watch,
    Command, Key, NavFooter, PopUp, StockDetail, KLINE_INDEX, KLINE_TYPE, WATCHLIST_TABLE,
};

pub fn render_watchlist_stock(
    mut terminal: ResMut<crate::tui::widgets::Terminal>,
    mut events: EventReader<Key>,
    stock: Res<StockDetail>,
    command: Res<Command>,
    (state, indexes, ws): NavFooter,
    (mut account, mut currency, mut search, mut watchgroup, mut watchlist_search): PopUp,
    mut last_choose: Local<Counter>,
    mut log_panel: Local<crate::tui::widgets::LogPanel>,
) {
    // workaround bevyengine/bevy#9130
    if *last_choose != stock.0 {
        if !last_choose.is_empty() {
            super::stock_detail::refresh_stock_debounced(stock.0.clone());
        }
        *last_choose = stock.0.clone();
    }

    for event in &mut events {
        let news_view = NEWS_VIEW.load(Ordering::Relaxed);

        match event {
            Key::Up => match news_view {
                NewsView::Quote => {
                    let watchlist = WATCHLIST.read().expect("poison");
                    let len = watchlist.counters().len();
                    let mut table = WATCHLIST_TABLE.lock().expect("poison");
                    let idx = table.selected();
                    let new_idx = cycle::prev(idx, len);
                    table.select(new_idx);
                    drop(table);

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
                NewsView::List => news_list_up(),
                NewsView::Detail => {
                    news_list_up();
                    if let Some(id) = selected_news_id() {
                        fetch_news_detail(id, command.0.clone());
                    }
                }
            },

            Key::Down => match news_view {
                NewsView::Quote => {
                    let watchlist = WATCHLIST.read().expect("poison");
                    let len = watchlist.counters().len();
                    let mut table = WATCHLIST_TABLE.lock().expect("poison");
                    let idx = table.selected();
                    let new_idx = cycle::next(idx, len);
                    table.select(new_idx);
                    drop(table);

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
                NewsView::List => news_list_down(),
                NewsView::Detail => {
                    news_list_down();
                    if let Some(id) = selected_news_id() {
                        fetch_news_detail(id, command.0.clone());
                    }
                }
            },

            Key::Enter => match news_view {
                NewsView::Quote => {
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
                NewsView::List => {
                    if let Some(id) = selected_news_id() {
                        fetch_news_detail(id, command.0.clone());
                        NEWS_VIEW.store(NewsView::Detail, Ordering::Relaxed);
                    }
                }
                NewsView::Detail => {}
            },

            Key::Left => {
                if news_view == NewsView::Quote {
                    _ = KLINE_INDEX.fetch_update(Ordering::Acquire, Ordering::Relaxed, |old| {
                        Some(old.saturating_add(1))
                    });
                }
            }
            Key::Right => {
                if news_view == NewsView::Quote {
                    _ = KLINE_INDEX.fetch_update(Ordering::Acquire, Ordering::Relaxed, |old| {
                        Some(old.saturating_sub(1))
                    });
                }
            }
            Key::Tab => {
                if news_view == NewsView::Quote {
                    KLINE_INDEX.store(0, Ordering::Relaxed);
                    _ = KLINE_TYPE.fetch_update(
                        Ordering::Acquire,
                        Ordering::Relaxed,
                        |kline_type| Some(kline_type.next()),
                    );
                }
            }
            Key::BackTab => {
                if news_view == NewsView::Quote {
                    KLINE_INDEX.store(0, Ordering::Relaxed);
                    _ = KLINE_TYPE.fetch_update(
                        Ordering::Acquire,
                        Ordering::Relaxed,
                        |kline_type| Some(kline_type.prev()),
                    );
                }
            }

            Key::NewsToggle => {
                let current = NEWS_VIEW.load(Ordering::Relaxed);
                match current {
                    NewsView::Quote => {
                        NEWS_VIEW.store(NewsView::List, Ordering::Relaxed);
                        fetch_news(stock.0.clone(), command.0.clone());
                    }
                    _ => {
                        NEWS_VIEW.store(NewsView::Quote, Ordering::Relaxed);
                    }
                }
            }
            Key::NewsScrollUp => news_detail_scroll_up(),
            Key::NewsScrollDown => news_detail_scroll_down(),
            Key::NewsOpen => {
                if let Some(url) = selected_news_url() {
                    let _ = open::that(url);
                }
            }
        }
    }

    _ = terminal.draw(|frame| {
        let rect = frame.area();
        let top = Rect { height: 1, ..rect };
        crate::tui::views::navbar::render(frame, top, *state.get());

        let bottom = Rect {
            y: rect.y + rect.height - 1,
            height: 1,
            ..rect
        };
        crate::tui::views::footer::render(frame, bottom, indexes.tick(), &ws);

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

        let news_view = NEWS_VIEW.load(Ordering::Relaxed);

        // Split the right panel: 1-line tab bar + content
        let right_chunks = Layout::default()
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .direction(Direction::Vertical)
            .split(chunks[1]);

        let tab_titles = vec![
            Line::from(t!("Tab.Quote").to_string()),
            Line::from(t!("Tab.News").to_string()),
        ];
        let selected_tab = match news_view {
            NewsView::Quote => 0,
            NewsView::List | NewsView::Detail => 1,
        };
        let tabs = Tabs::new(tab_titles)
            .select(selected_tab)
            .highlight_style(crate::tui::ui::styles::primary().add_modifier(Modifier::BOLD))
            .divider(" ");
        frame.render_widget(tabs, right_chunks[0]);
        *crate::tui::mouse::WATCHLIST_STOCK_TABS_RECT
            .lock()
            .expect("poison") = right_chunks[0];

        match news_view {
            NewsView::Quote => {
                stock_detail(
                    frame,
                    right_chunks[1],
                    &stock.0,
                    KLINE_TYPE.load(Ordering::Relaxed),
                    KLINE_INDEX.load(Ordering::Relaxed),
                );
            }
            NewsView::List => {
                render_news_list_view(frame, right_chunks[1]);
            }
            NewsView::Detail => {
                render_news_detail_view(frame, right_chunks[1]);
            }
        }

        crate::tui::views::popup::render(
            frame,
            rect,
            &mut account,
            &mut currency,
            &mut search,
            &mut watchgroup,
            &mut watchlist_search,
        );

        crate::tui::widgets::render_toast(frame, rect);

        // Render floating log panel if visible
        let log_panel_visible =
            crate::tui::app::LOG_PANEL_VISIBLE.load(std::sync::atomic::Ordering::Relaxed);
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
