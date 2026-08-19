use std::sync::atomic::Ordering;

use bevy_ecs::{
    prelude::*,
    system::{CommandQueue, InsertResource},
};
use ratatui::layout::{Constraint, Layout, Rect};

use crate::{
    data::Counter,
    tui::app::{AppState, WATCHLIST},
    utils::cycle,
};

use super::{
    stock_detail::stock_detail,
    stock_news::{
        ensure_news, fetch_news, fetch_news_detail, news_detail_scroll_down, news_detail_scroll_up,
        news_list_down, news_list_up, render_news_detail_view, render_news_list_view,
        selected_news_id, selected_news_url, NewsView, NEWS_VIEW,
    },
    watchlist::watch,
    Command, Key, NavFooter, PopUp, StockDetail, KLINE_INDEX, KLINE_TYPE, WATCHLIST_TABLE,
};

/// Columns the watchlist column occupies when shown.
const WATCHLIST_WIDTH: u16 = 57;
/// Columns the news list occupies when docked beside the quote panel.
const NEWS_DOCK_WIDTH: u16 = 40;
/// Narrowest quote panel still worth drawing beside another pane.
const PANEL_MIN_WIDTH: u16 = 52;

/// Where each pane of the stock screen goes, for a given width and news view.
///
/// News is never on screen until it is asked for. Opening it trades the
/// watchlist away and docks the list to the right of the quote panel; on a
/// terminal too narrow to hold both, the list takes the body outright.
struct Panes {
    watchlist: Option<Rect>,
    quote: Option<Rect>,
    /// The news list, docked beside the quote panel or filling the body.
    news: Option<Rect>,
    /// The list + article split shown while reading (Detail view).
    article: Option<Rect>,
}

fn panes(rect: Rect, view: NewsView) -> Panes {
    let show_watchlist = view == NewsView::Quote && rect.width >= WATCHLIST_WIDTH + PANEL_MIN_WIDTH;
    let (watchlist, right) = if show_watchlist {
        let [w, r] = Layout::horizontal([Constraint::Length(WATCHLIST_WIDTH), Constraint::Min(0)])
            .areas(rect);
        (Some(w), r)
    } else {
        (None, rect)
    };

    // No tab strip above the panes: `News [n]` and `Back [esc]` ride the
    // panels' own borders, which gives this row back to the content.
    let body = right;
    let splits = body.width >= PANEL_MIN_WIDTH + NEWS_DOCK_WIDTH;

    match view {
        NewsView::Quote => Panes {
            watchlist,
            quote: Some(body),
            news: None,
            article: None,
        },
        NewsView::List if splits => {
            // A proportional split rather than a fixed column: headlines need
            // the room, and `splits` already guarantees the dock its minimum.
            let [quote, news] =
                Layout::horizontal([Constraint::Min(0), Constraint::Percentage(45)]).areas(body);
            Panes {
                watchlist,
                quote: Some(quote),
                news: Some(news),
                article: None,
            }
        }
        NewsView::List => Panes {
            watchlist,
            quote: None,
            news: Some(body),
            article: None,
        },
        NewsView::Detail => Panes {
            watchlist,
            quote: None,
            news: None,
            article: Some(body),
        },
    }
}

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
        // Only refresh news that is already open; it is never opened uninvited.
        if NEWS_VIEW.load(Ordering::Relaxed) != NewsView::Quote {
            fetch_news(stock.0.clone(), command.0.clone());
        }
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
                        ensure_news(stock.0.clone(), command.0.clone());
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
        let news_view = NEWS_VIEW.load(Ordering::Relaxed);
        let layout = panes(rect, news_view);

        // Anything not drawn this frame must not keep a stale click area. The
        // news list's `Back [esc]` and the quote panel's `News [n]` both hug
        // the right edge of the same row, so a stale Back rect would sit right
        // on top of the News button and swallow every click on it.
        *crate::tui::mouse::WATCHLIST_TABLE_RECT
            .lock()
            .expect("poison") = Rect::default();
        *crate::tui::mouse::NEWS_LIST_RECT.lock().expect("poison") = Rect::default();
        *crate::tui::mouse::NEWS_BACK_RECT.lock().expect("poison") = Rect::default();

        if let Some(area) = layout.watchlist {
            watch(frame, area, false);
        }

        if let Some(area) = layout.quote {
            stock_detail(
                frame,
                area,
                &stock.0,
                KLINE_TYPE.load(Ordering::Relaxed),
                KLINE_INDEX.load(Ordering::Relaxed),
                true,
            );
        }
        if let Some(area) = layout.news {
            render_news_list_view(frame, area);
        }
        if let Some(area) = layout.article {
            render_news_detail_view(frame, area);
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

#[cfg(test)]
mod tests {
    use super::{panes, NewsView, NEWS_DOCK_WIDTH, PANEL_MIN_WIDTH, WATCHLIST_WIDTH};
    use ratatui::layout::Rect;

    fn area(width: u16) -> Rect {
        Rect::new(0, 0, width, 40)
    }

    #[test]
    fn news_is_never_on_screen_until_it_is_opened() {
        for width in [80u16, 120, 160, 200, 300] {
            let p = panes(area(width), NewsView::Quote);
            assert!(
                p.news.is_none(),
                "news should stay closed at {width} columns"
            );
            assert!(p.quote.is_some());
        }
    }

    #[test]
    fn an_open_list_docks_beside_the_quote_panel_when_there_is_room() {
        let wide = WATCHLIST_WIDTH + PANEL_MIN_WIDTH + NEWS_DOCK_WIDTH;
        let docked = panes(area(wide), NewsView::List);
        assert!(docked.quote.is_some());
        assert!(docked.news.is_some(), "list should dock at {wide} columns");

        let narrow = panes(area(PANEL_MIN_WIDTH + NEWS_DOCK_WIDTH - 1), NewsView::List);
        assert!(narrow.quote.is_none(), "too narrow for both panes");
        assert!(narrow.news.is_some());
    }

    #[test]
    fn opening_news_trades_the_watchlist_for_the_article() {
        for view in [NewsView::List, NewsView::Detail] {
            let p = panes(area(200), view);
            assert!(p.watchlist.is_none(), "{view:?} should hide the watchlist");
        }
        // List keeps the quote panel beside the news; Detail gives the whole
        // body to the list + article split.
        let list = panes(area(200), NewsView::List);
        assert!(list.quote.is_some() && list.news.is_some());
        let detail = panes(area(200), NewsView::Detail);
        assert!(detail.article.is_some() && detail.quote.is_none());
    }

    #[test]
    fn a_narrow_terminal_gives_news_the_whole_body() {
        let p = panes(area(80), NewsView::List);
        assert!(p.quote.is_none());
        assert_eq!(p.news.map(|r| r.width), Some(80));
    }

    #[test]
    fn panes_never_overlap_or_overflow() {
        for width in [60u16, 80, 100, 130, 150, 200] {
            for view in [NewsView::Quote, NewsView::List, NewsView::Detail] {
                let p = panes(area(width), view);
                let mut rects: Vec<Rect> = [p.watchlist, p.quote, p.news, p.article]
                    .into_iter()
                    .flatten()
                    .collect();
                rects.sort_by_key(|r| r.x);
                for pair in rects.windows(2) {
                    assert!(
                        pair[0].x + pair[0].width <= pair[1].x,
                        "{view:?} panes overlap at width {width}"
                    );
                }
                if let Some(last) = rects.last() {
                    assert!(
                        last.x + last.width <= width,
                        "{view:?} overflows at {width}"
                    );
                }
            }
        }
    }
}
