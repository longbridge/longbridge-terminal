use std::{borrow::Cow, collections::HashMap};

use bevy_ecs::{
    prelude::*,
    system::{CommandQueue, InsertResource},
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table},
    Frame,
};
use rust_decimal::Decimal;
use tokio::sync::mpsc;

use crate::{
    data::{Counter, SubTypes, TradeSessionExt, TradeStatusExt, STOCKS},
    tui::app::{AppState, RT, WATCHLIST},
    tui::systems::portfolio::fetch_holdings,
    tui::systems::WS,
    tui::ui::styles,
    tui::widgets::LocalSearch,
    utils::{cycle, DecimalExt, Sign},
};

use super::{Command, Key, NavFooter, PopUp, StockDetail, LAST_DONE, WATCHLIST_TABLE};

pub fn render_watchlist(
    mut terminal: ResMut<crate::tui::widgets::Terminal>,
    mut events: EventReader<Key>,
    command: Res<Command>,
    (state, indexes, ws): NavFooter,
    (mut account, mut currency, mut search, mut watchgroup, mut watchlist_search): PopUp,
    mut log_panel: Local<crate::tui::widgets::LogPanel>,
) {
    for event in &mut events {
        match event {
            Key::Up => {
                let len = WATCHLIST.read().expect("poison").counters().len();
                let mut table = WATCHLIST_TABLE.lock().expect("poison");
                let idx = table.selected();
                table.select(cycle::prev(idx, len));
            }
            Key::Down => {
                let len = WATCHLIST.read().expect("poison").counters().len();
                let mut table = WATCHLIST_TABLE.lock().expect("poison");
                let idx = table.selected();
                table.select(cycle::next(idx, len));
            }
            Key::Left
            | Key::Right
            | Key::Tab
            | Key::BackTab
            | Key::NewsToggle
            | Key::NewsScrollUp
            | Key::NewsScrollDown
            | Key::NewsOpen => (),
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
            .constraints([Constraint::Length(81), Constraint::Min(20)])
            .direction(Direction::Horizontal)
            .split(rect);

        watch(frame, chunks[0], true);
        banner(frame, chunks[1]);

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

pub fn watch(frame: &mut Frame, rect: Rect, full_mode: bool) {
    // Extract data from watchlist early and release the lock
    let (counters, group_name) = {
        let watchlist = WATCHLIST.read().expect("poison");
        (
            watchlist.counters().to_vec(),
            watchlist
                .group()
                .map_or_else(String::new, |g| format!("{} ", g.name)),
        )
    }; // Lock released here

    let background = Block::default()
        .borders(Borders::ALL)
        .border_style(styles::border())
        .title(format!(" {} ─── {}[g] ", t!("Watchlist"), group_name))
        .title_bottom(
            Line::from(vec![
                Span::styled(format!(" {} ", t!("Trade.BuyKey")), styles::dark_gray()),
                Span::styled(format!(" {} ", t!("Trade.SellKey")), styles::dark_gray()),
            ])
            .right_aligned(),
        );
    frame.render_widget(background, rect);

    // Lock WATCHLIST_TABLE once for both reading and rendering
    let mut table_state = WATCHLIST_TABLE.lock().expect("poison");
    let selected = table_state.selected();
    // Use asymmetric margin: left 2 for spacing, right 1
    let block_inner = rect.inner(Margin {
        vertical: 2,
        horizontal: 0,
    });
    let table_area = Rect {
        x: block_inner.x + 2,
        y: block_inner.y,
        width: block_inner.width.saturating_sub(3), // left: 2, right: 1
        height: block_inner.height,
    };
    *crate::tui::mouse::WATCHLIST_TABLE_RECT
        .lock()
        .expect("poison") = table_area;
    frame.render_stateful_widget(
        watch_group_table(
            &counters,
            selected,
            &mut LAST_DONE.lock().expect("poison"),
            full_mode,
        ),
        table_area,
        &mut *table_state,
    );

    // Render scrollbar
    let mut scrollbar_state = ScrollbarState::new(counters.len()).position(selected.unwrap_or(0));
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(None)
        .thumb_symbol("▐")
        .thumb_style(Style::default().fg(Color::DarkGray));
    let scrollbar_area = Rect {
        x: block_inner.x + block_inner.width - 1,
        y: block_inner.y,
        width: 1,
        height: block_inner.height,
    };
    frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
}

fn banner(frame: &mut Frame, rect: Rect) {
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(styles::border()),
        rect,
    );

    frame.render_widget(
        crate::tui::ui::assets::banner(crate::tui::ui::styles::text()),
        crate::tui::ui::rect::centered(0, crate::tui::ui::assets::BANNER_HEIGHT, rect),
    );
}

pub fn watch_group_table(
    counters: &[Counter],
    selected: Option<usize>,
    last_dones: &mut HashMap<Counter, Decimal>,
    full_mode: bool,
) -> Table<'static> {
    const COLUMN_WIDTHS: [usize; 6] = [9, 21, 10, 8, 10, 14];
    const COLUMN_WIDTHS2: [Constraint; 6] = [
        Constraint::Length(9),
        Constraint::Length(21),
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Length(14),
    ];

    let header = {
        let mut cells = Vec::with_capacity(if full_mode { 6 } else { 4 });
        cells.push(Cell::from(t!("watchlist.CODE")).style(styles::header()));
        cells.push(Cell::from(t!("watchlist.NAME")).style(styles::header()));
        cells.push(Cell::from(t!("watchlist.PRICE")).style(styles::header()));
        cells.push(
            Cell::from(crate::tui::ui::text::align_right(
                &t!("watchlist.CHG"),
                COLUMN_WIDTHS[3],
            ))
            .style(styles::header()),
        );
        if full_mode {
            cells.push(
                Cell::from(crate::tui::ui::text::align_right(
                    &t!("watchlist.VOL"),
                    COLUMN_WIDTHS[4],
                ))
                .style(styles::header()),
            );
            cells.push(Cell::from(t!("watchlist.STATUS")).style(styles::header()));
        }
        Row::new(cells)
    };

    let stocks = STOCKS.mget(counters);
    let rows = counters
        .iter()
        .zip(stocks.iter())
        .map(|(counter, stock)| {
            static EMPTY: std::sync::LazyLock<crate::data::Stock> =
                std::sync::LazyLock::new(crate::data::Stock::default);
            let stock = stock.as_deref().unwrap_or(&EMPTY);
            let quote_data = &stock.quote;

            // Prefer last_done, fallback to prev_close if unavailable
            let display_price = quote_data
                .last_done
                .or(quote_data.prev_close)
                .filter(|&p| p > Decimal::ZERO)
                .unwrap_or_default();

            let _last = last_dones.insert(counter.clone(), display_price);

            // Calculate price change: prefer last_done, fallback to open (for after-market display)
            let prev_close = quote_data.prev_close.filter(|&p| p > Decimal::ZERO);
            let current_price = quote_data
                .last_done
                .or(quote_data.open)
                .filter(|&p| p > Decimal::ZERO);

            let (increase, increase_percent) = match (current_price, prev_close) {
                (Some(price), Some(prev)) => {
                    let increase = price - prev;
                    let percent = (increase / prev * Decimal::from(100)).round_dp(2);
                    (increase, percent)
                }
                _ => (Decimal::ZERO, Decimal::ZERO),
            };

            let style = styles::up(increase.sign());

            // Determine status to display
            let get_status_label = || {
                if !stock.trade_session.is_normal_trading() {
                    stock.trade_session.label()
                } else if !stock.trade_status.is_trading() {
                    stock.trade_status.label()
                } else {
                    stock.trade_session.label()
                }
            };

            let status_label = get_status_label();
            let change_sign = if increase.is_sign_positive() { "" } else { "-" };
            let percent_str = if increase_percent.fract().abs() == Decimal::ZERO {
                format!("{}", increase_percent.abs().trunc())
            } else {
                format!("{}", increase_percent.abs())
            };
            let increase_percent_str = format!("{change_sign}{percent_str}%");
            let mut cells = Vec::with_capacity(if full_mode { 6 } else { 4 });
            cells.push(Cell::from(Line::from(vec![
                Span::styled(
                    counter.market().to_string(),
                    styles::market(counter.region()),
                ),
                Span::raw(" "),
                Span::raw(counter.code().to_string()),
            ])));
            cells.push(Cell::from(stock.display_name().to_string()));
            cells.push(Cell::from(display_price.format_quote_by_counter(counter)).style(style));
            cells.push(
                Cell::from(crate::tui::ui::text::align_right(
                    &increase_percent_str,
                    COLUMN_WIDTHS[3],
                ))
                .style(style),
            );
            if full_mode {
                let volume_text = crate::utils::format_volume(quote_data.volume);
                cells.push(Cell::from(crate::tui::ui::text::align_right(
                    &volume_text,
                    COLUMN_WIDTHS[4],
                )));
                cells.push(Cell::from(status_label));
            }
            Row::new(cells)
        })
        .collect::<Vec<Row<'static>>>();

    let highlight_style = selected
        .map(|i| {
            let increase = if let Some(Some(stock)) = stocks.get(i) {
                let quote_data = &stock.quote;
                let display_price = quote_data
                    .last_done
                    .or(quote_data.prev_close)
                    .filter(|&p| p > Decimal::ZERO);
                let prev_close = quote_data.prev_close.filter(|&p| p > Decimal::ZERO);

                match (display_price, prev_close) {
                    (Some(price), Some(prev)) => price.cmp(&prev),
                    _ => std::cmp::Ordering::Equal,
                }
            } else {
                std::cmp::Ordering::Equal
            };
            styles::up(increase).add_modifier(Modifier::REVERSED)
        })
        .unwrap_or_default();

    Table::new(rows, COLUMN_WIDTHS2)
        .header(header)
        .row_highlight_style(highlight_style)
        .column_spacing(1)
}

pub fn exit_watchlist() {
    crate::tui::app::LAST_STATE.store(AppState::Watchlist, std::sync::atomic::Ordering::Relaxed);
}

pub fn enter_watchlist_common(_command: Res<Command>) {
    // Do not reload on every navigation. The initial load is handled by the explicit
    // refresh_watchlist() call during startup, and subsequent reloads are triggered
    // by the R key. Reloading here causes sort jumps because each API response arrives
    // at a different time, potentially seeing different trade_session states.
}

pub fn exit_watchlist_common() {
    RT.get().unwrap().spawn(async move {
        _ = WS.unmount("watchlist").await;
    });
}

// Watchlist API - uses Longbridge SDK
pub async fn fetch_watchlist(
    group_id: Option<u64>,
) -> anyhow::Result<(Vec<Counter>, Vec<crate::data::WatchlistGroup>)> {
    // Translate default group names
    fn translate_group_name(name: &str) -> String {
        match name.to_lowercase().as_str() {
            "all" => t!("watchlist_group.all"),
            "holdings" => t!("watchlist_group.holdings"),
            "us" => t!("watchlist_group.us"),
            "hk" => t!("watchlist_group.hk"),
            "cn" => t!("watchlist_group.cn"),
            "sg" => t!("watchlist_group.sg"),
            "jp" => t!("watchlist_group.jp"),
            "uk" => t!("watchlist_group.uk"),
            "de" => t!("watchlist_group.de"),
            "na" => t!("watchlist_group.na"),
            _ => Cow::Borrowed(name),
        }
        .to_string()
    }

    let ctx = crate::openapi::quote();

    // Get watchlist
    match ctx.watchlist().await {
        Ok(watchlist) => {
            // Extract group info and symbols
            let mut groups = Vec::new();
            let mut counters = Vec::new();

            for group in watchlist {
                let group_id_u64 = group.id.unsigned_abs();

                // Add group info with translated name
                groups.push(crate::data::WatchlistGroup {
                    id: group_id_u64,
                    name: translate_group_name(&group.name),
                });

                // If group_id is specified, only return that group's stocks
                if let Some(filter_id) = group_id {
                    if group_id_u64 != filter_id {
                        continue;
                    }
                }

                // Add stocks from this group
                for security in group.securities {
                    #[allow(irrefutable_let_patterns)]
                    if let Ok(counter) = security.symbol.parse() {
                        counters.push(counter);
                    }
                }
            }

            tracing::info!(
                "Fetched {} groups, {} stocks total (filtered by group: {:?})",
                groups.len(),
                counters.len(),
                group_id
            );
            Ok((counters, groups))
        }
        Err(e) => Err(e.into()),
    }
}

pub fn refresh_watchlist(update_tx: mpsc::UnboundedSender<CommandQueue>) {
    RT.get().unwrap().spawn(async move {
        let group_id = WATCHLIST.read().expect("poison").group_id;
        let (watch_resp, holdings) = tokio::join!(fetch_watchlist(group_id), fetch_holdings());
        match watch_resp {
            Ok((counters, groups)) => {
                let mut watchlist = WATCHLIST.write().expect("poison");
                watchlist.set_groups(groups);
                if let Ok(holdings) = holdings {
                    watchlist.full_load(counters, holdings);
                } else {
                    watchlist.load(counters);
                }
            }
            Err(err) => {
                tracing::error!("fail to fetch watchlist: {err}");
                return;
            }
        }

        let counters = {
            // Simplified implementation: use default sorting
            let mut watchlist = WATCHLIST.write().expect("poison");
            watchlist.set_hidden(true);
            watchlist.set_sortby((0, 0, false)); // (sort_mode, sort_by, reverse)
            watchlist.counters().to_vec()
        };

        // Create Stock entry for each watchlist item (if not exists)
        for counter in &counters {
            if STOCKS.get(counter).is_none() {
                let mut stock = crate::data::Stock::new(counter.clone());
                stock.name = counter.to_string(); // Temporarily use symbol as name
                STOCKS.insert(stock);
            }
        }

        // Get initial quote data
        if !counters.is_empty() {
            let ctx = crate::openapi::quote();
            let symbols: Vec<String> = counters.iter().map(|c| c.as_str().to_string()).collect();

            // Use quote() to get full quote data (including prev_close and trade_status)
            match ctx.quote(&symbols).await {
                Ok(quotes) => {
                    for quote in quotes {
                        // Debug: log trade_status from API
                        tracing::debug!(
                            "API quote for {}: trade_status = {:?}",
                            quote.symbol,
                            quote.trade_status
                        );
                        #[allow(irrefutable_let_patterns)]
                        if let Ok(counter) = quote.symbol.parse() {
                            STOCKS.modify(counter, |stock| {
                                // Use update_from_security_quote to update all fields including trade_status
                                stock.update_from_security_quote(&quote);
                            });
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to fetch initial quotes: {}", e);
                }
            }

            // Get stock static info (including name, etc.)
            match ctx
                .static_info(symbols.iter().map(std::string::String::as_str))
                .await
            {
                Ok(infos) => {
                    for info in infos {
                        #[allow(irrefutable_let_patterns)]
                        if let Ok(counter) = info.symbol.parse() {
                            STOCKS.modify(counter, |stock| {
                                let name = match crate::locale::get() {
                                    "zh-CN" | "zh-HK" => &info.name_cn,
                                    _ => &info.name_en,
                                };
                                stock.name.clone_from(name);
                                stock.update_from_static_info(info);
                            });
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to fetch stock static info: {}", e);
                }
            }
        }

        // SignalApp removed
        let _ = WS.remount("watchlist", &counters, SubTypes::LIST).await;

        // refresh watchlist sort
        WATCHLIST.write().expect("poison").refresh();
        // counter order maybe change, reset table highlight
        WATCHLIST_TABLE.lock().expect("poison").select(None);

        let final_counters = WATCHLIST.read().expect("poison").counters().to_vec();
        let local_search = LocalSearch::new(
            WATCHLIST.read().expect("poison").groups().to_vec(),
            |keyword: &str, group: &crate::data::WatchlistGroup| {
                let keyword = &keyword.to_ascii_lowercase();
                group.name.to_ascii_lowercase().contains(keyword)
            },
        );
        let watchlist_search =
            LocalSearch::new(final_counters, |keyword: &str, counter: &Counter| {
                let kw = keyword.to_ascii_lowercase();
                if counter.as_str().to_ascii_lowercase().contains(&kw) {
                    return true;
                }
                crate::data::STOCKS
                    .get(counter)
                    .is_some_and(|s| s.name.to_ascii_lowercase().contains(&kw))
            });
        let mut queue = CommandQueue::default();
        queue.push(InsertResource {
            resource: local_search,
        });
        queue.push(InsertResource {
            resource: watchlist_search,
        });
        _ = update_tx.send(queue);
    });
}
