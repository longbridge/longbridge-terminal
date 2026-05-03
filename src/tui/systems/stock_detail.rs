use std::sync::{atomic::Ordering, Mutex};

use atomic::Atomic;
use bevy_ecs::prelude::*;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, List, ListItem, Paragraph, Row, Table, Tabs},
    Frame,
};
use rust_decimal::Decimal;
use tokio::task::JoinHandle;

use crate::{
    data::{Counter, KlineType, TradeSessionExt, TradeStatusExt, STOCKS},
    openapi,
    tui::app::RT,
    tui::kline::KLINES,
    tui::systems::WS,
    tui::ui::styles::{self, item},
    tui::widgets::Terminal,
    utils::{DecimalExt, Sign},
};

use super::{Key, NavFooter, PopUp, StockDetail, KLINE_INDEX, KLINE_TYPE};

const EMPTY_PLACEHOLDER: &str = "--";

// Debounce state for stock refresh
static REFRESH_STOCK_TASK: std::sync::LazyLock<Mutex<Option<JoinHandle<()>>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));
// Flag to track if a refresh is currently executing
static REFRESH_EXECUTING: Atomic<bool> = Atomic::new(false);
// Flag set when the API returns no data for the current symbol
static STOCK_NOT_FOUND: Atomic<bool> = Atomic::new(false);

// RAII guard to ensure REFRESH_EXECUTING is always cleared
struct RefreshGuard;

impl RefreshGuard {
    fn try_acquire() -> Option<Self> {
        if REFRESH_EXECUTING.swap(true, Ordering::Relaxed) {
            None
        } else {
            Some(RefreshGuard)
        }
    }
}

impl Drop for RefreshGuard {
    fn drop(&mut self) {
        REFRESH_EXECUTING.store(false, Ordering::Relaxed);
    }
}

pub fn render_stock(
    mut terminal: ResMut<Terminal>,
    mut events: EventReader<Key>,
    stock: Res<StockDetail>,
    (state, indexes, ws): NavFooter,
    (mut account, mut currency, mut search, mut watchgroup, mut watchlist_search): PopUp,
    mut last_choose: Local<Counter>,
    mut log_panel: Local<crate::tui::widgets::LogPanel>,
) {
    // workaround bevyengine/bevy#9130
    if *last_choose != stock.0 {
        if !last_choose.is_empty() {
            refresh_stock_debounced(stock.0.clone());
        }
        *last_choose = stock.0.clone();
    }

    for event in &mut events {
        match event {
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
                _ = KLINE_TYPE.fetch_update(Ordering::Acquire, Ordering::Relaxed, |kline_type| {
                    Some(kline_type.next())
                });
            }
            Key::BackTab => {
                _ = KLINE_TYPE.fetch_update(Ordering::Acquire, Ordering::Relaxed, |kline_type| {
                    Some(kline_type.prev())
                });
            }
            Key::Enter
            | Key::Up
            | Key::Down
            | Key::NewsToggle
            | Key::NewsScrollUp
            | Key::NewsScrollDown
            | Key::NewsOpen => {}
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

        stock_detail(
            frame,
            rect,
            &stock.0,
            KLINE_TYPE.load(Ordering::Relaxed),
            KLINE_INDEX.load(Ordering::Relaxed),
        );
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

pub(crate) fn stock_detail(
    frame: &mut Frame,
    rect: Rect,
    counter: &Counter,
    kline_type: KlineType,
    selected: usize,
) {
    fn price_spans(data: &crate::data::QuoteData, counter: &Counter) -> Vec<Span<'static>> {
        // Prefer last_done, fallback to prev_close if not available
        let display_price = data
            .last_done
            .or(data.prev_close)
            .filter(|&p| p > Decimal::ZERO);

        let prev_close = data.prev_close.filter(|&p| p > Decimal::ZERO);

        let (price_str, increase, increase_percent) = match (display_price, prev_close) {
            (Some(price), Some(prev)) => {
                let increase = price - prev;
                (
                    price.format_quote_by_counter(counter),
                    increase.format_quote_by_counter(counter),
                    (increase / prev).format_percent(),
                )
            }
            (Some(price), None) => {
                // Has price but no prev_close, show price without change
                (
                    price.format_quote_by_counter(counter),
                    EMPTY_PLACEHOLDER.to_string(),
                    EMPTY_PLACEHOLDER.to_string(),
                )
            }
            _ => {
                // Neither available, show placeholder
                (
                    EMPTY_PLACEHOLDER.to_string(),
                    EMPTY_PLACEHOLDER.to_string(),
                    EMPTY_PLACEHOLDER.to_string(),
                )
            }
        };

        let trend_style = styles::up(increase.sign());
        vec![
            Span::raw(" "),
            Span::styled(price_str, trend_style),
            Span::raw(" ("),
            Span::styled(format!("{increase_percent}, {increase}"), trend_style),
            Span::raw(") "),
        ]
    }

    let Some(stock) = STOCKS.get(counter) else {
        if STOCK_NOT_FOUND.load(Ordering::Relaxed) {
            let content_height = 2u16;
            let y_offset = rect.height.saturating_sub(content_height) / 2;
            let centered_rect = Rect {
                y: rect.y + y_offset,
                height: content_height,
                ..rect
            };
            let text = ratatui::text::Text::from(vec![
                Line::from(Span::styled(counter.to_string(), styles::primary())),
                Line::from(Span::styled(
                    t!("StockDetail.not_found").to_string(),
                    styles::dark_gray(),
                )),
            ]);
            frame.render_widget(
                Paragraph::new(text).alignment(Alignment::Center),
                centered_rect,
            );
        }
        return;
    };

    // draw title
    let mut titles = vec![Span::styled(
        format!(
            " {} ({}.{})",
            stock.display_name(),
            counter.code(),
            counter.market(),
        ),
        styles::primary(),
    )];
    titles.extend(price_spans(&stock.quote, counter));

    let detail_container = Block::default()
        .title(Line::from(titles))
        .borders(Borders::ALL)
        .border_style(styles::border());

    // draw border
    frame.render_widget(detail_container, rect);

    // Helper function to format optional Decimal (price type)
    let fmt_decimal = |opt: Option<Decimal>| -> String {
        opt.map_or_else(
            || EMPTY_PLACEHOLDER.to_string(),
            |d| d.format_quote_by_counter(counter),
        )
    };

    // Helper function to create ListItem with price and color based on prev_close
    let price_item = |label: &str, price_opt: Option<Decimal>| -> ListItem<'_> {
        let prev_close = stock.quote.prev_close.filter(|&p| p > Decimal::ZERO);
        let price = price_opt.filter(|&p| p > Decimal::ZERO);

        match (price, prev_close) {
            (Some(p), Some(prev)) => {
                let price_str = p.format_quote_by_counter(counter);
                let cmp = p.cmp(&prev);
                let style = styles::up(cmp);
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{label}: "), crate::tui::ui::styles::label()),
                    Span::styled(price_str, style),
                ]))
            }
            (Some(p), None) => {
                // Has price but no prev_close, show without coloring
                let price_str = p.format_quote_by_counter(counter);
                item(label, price_str)
            }
            (None, Some(prev)) => {
                // No price but has prev_close, show prev_close without coloring
                let price_str = prev.format_quote_by_counter(counter);
                item(label, price_str)
            }
            _ => item(label, EMPTY_PLACEHOLDER),
        }
    };

    // Helper function to format u64
    let fmt_unsigned = |val: u64| -> String {
        if val == 0 {
            EMPTY_PLACEHOLDER.to_string()
        } else {
            crate::tui::ui::text::unit(Decimal::from(val), 0)
        }
    };

    // Helper function to format i64
    let fmt_signed = |val: i64| -> String {
        if val == 0 {
            EMPTY_PLACEHOLDER.to_string()
        } else {
            crate::tui::ui::text::unit(Decimal::from(val), 0)
        }
    };

    // Build detail columns - Column 1: Basic trading data
    let column0 = vec![
        ListItem::new(" "),
        item(t!("StockDetail.Trading Status"), {
            let session_label = stock.trade_session.label();
            if session_label.is_empty() {
                stock.trade_status.label()
            } else {
                session_label
            }
        }),
        ListItem::new(" "),
        price_item(&t!("StockDetail.Open"), stock.quote.open),
        item(
            t!("StockDetail.Prev. Close"),
            fmt_decimal(stock.quote.prev_close),
        ),
        ListItem::new(" "),
        price_item(&t!("StockDetail.High"), stock.quote.high),
        price_item(&t!("StockDetail.Low"), stock.quote.low),
        item(t!("StockDetail.Average"), EMPTY_PLACEHOLDER),
        ListItem::new(" "),
        item(t!("StockDetail.Volume"), fmt_unsigned(stock.quote.volume)),
        item(
            t!("StockDetail.Turnover"),
            crate::tui::ui::text::unit(stock.quote.turnover, 2),
        ),
        ListItem::new(" "),
    ];

    // Column 2: Static info (if available)
    let column1 = if let Some(ref info) = stock.static_info {
        vec![
            ListItem::new(" "),
            ListItem::new(" "),
            ListItem::new(" "),
            item(t!("StockDetail.P/E (TTM)"), fmt_decimal(Some(info.eps_ttm))),
            item(t!("StockDetail.EPS (TTM)"), fmt_decimal(Some(info.eps))),
            ListItem::new(" "),
        ]
    } else {
        vec![
            ListItem::new(" "),
            ListItem::new(" "),
            ListItem::new(" "),
            item(t!("StockDetail.P/E (TTM)"), EMPTY_PLACEHOLDER),
            item(t!("StockDetail.EPS (TTM)"), EMPTY_PLACEHOLDER),
            ListItem::new(" "),
        ]
    };

    // Column 3: More static info
    let column2 = if let Some(ref info) = stock.static_info {
        vec![
            ListItem::new(" "),
            ListItem::new(" "),
            ListItem::new(" "),
            item(t!("StockDetail.Shares"), fmt_signed(info.total_shares)),
            item(
                t!("StockDetail.Shares Float"),
                fmt_signed(info.circulating_shares),
            ),
            ListItem::new(" "),
            item(t!("StockDetail.BPS"), fmt_decimal(Some(info.bps))),
            item(
                t!("StockDetail.Dividend (TTM)"),
                fmt_decimal(Some(info.dividend_yield)),
            ),
            ListItem::new(" "),
            ListItem::new(" "),
            item(t!("StockDetail.Min lot size"), info.lot_size.to_string()),
            ListItem::new(" "),
        ]
    } else {
        vec![
            ListItem::new(" "),
            ListItem::new(" "),
            ListItem::new(" "),
            item(t!("StockDetail.Shares"), EMPTY_PLACEHOLDER),
            item(t!("StockDetail.Shares Float"), EMPTY_PLACEHOLDER),
            ListItem::new(" "),
            item(t!("StockDetail.BPS"), EMPTY_PLACEHOLDER),
            item(t!("StockDetail.Dividend Yield (TTM)"), EMPTY_PLACEHOLDER),
            ListItem::new(" "),
            ListItem::new(" "),
            item(t!("StockDetail.Min lot size"), EMPTY_PLACEHOLDER),
            ListItem::new(" "),
        ]
    };

    // Render three-column layout
    let column_height = column0.len().max(column1.len()).max(column2.len()) as u16;

    // Split into upper and lower sections with a divider
    let block_inner = rect.inner(Margin {
        vertical: 1,
        horizontal: 0,
    });
    let inner_rect = Rect {
        x: block_inner.x + 2,
        y: block_inner.y,
        width: block_inner.width.saturating_sub(3),
        height: block_inner.height,
    };
    let chunks = Layout::default()
        .constraints([
            Constraint::Length(column_height),
            Constraint::Length(1),
            Constraint::Min(19),
        ])
        .direction(Direction::Vertical)
        .split(inner_rect);

    // Render horizontal divider line using Block's top border
    let divider = Block::default()
        .borders(Borders::TOP)
        .border_style(styles::border());
    frame.render_widget(divider, chunks[1]);

    let columns_chunks = Layout::default()
        .constraints([
            Constraint::Ratio(2, 9),
            Constraint::Ratio(2, 9),
            Constraint::Ratio(2, 9),
            Constraint::Ratio(3, 9),
        ])
        .direction(Direction::Horizontal)
        .split(chunks[0]);
    frame.render_widget(List::new(column0), columns_chunks[0]);
    frame.render_widget(List::new(column1), columns_chunks[1]);
    frame.render_widget(List::new(column2), columns_chunks[2]);

    // Draw market depth with left border
    let depth_rect = columns_chunks[3];
    frame.render_widget(
        Block::default()
            .borders(Borders::LEFT)
            .border_type(BorderType::Plain)
            .border_style(styles::border()),
        depth_rect,
    );

    if !stock.depth.bids.is_empty() || !stock.depth.asks.is_empty() {
        // Calculate inner area: first remove border (left only), then add margins
        let block_inner = Block::default().borders(Borders::LEFT).inner(depth_rect);
        let depth_inner_rect = Rect {
            x: block_inner.x + 1,
            y: block_inner.y,
            width: block_inner.width.saturating_sub(2),
            height: block_inner.height,
        };

        // Calculate bid/ask ratio
        let total_bid_volume: i64 = stock.depth.bids.iter().map(|d| d.volume).sum();
        let total_ask_volume: i64 = stock.depth.asks.iter().map(|d| d.volume).sum();
        let total_volume = total_bid_volume + total_ask_volume;
        let (bid_ratio, ask_ratio) = if total_volume > 0 {
            let bid_r = Decimal::from(total_bid_volume) / Decimal::from(total_volume);
            let ask_r = Decimal::from(total_ask_volume) / Decimal::from(total_volume);
            (bid_r, ask_r)
        } else {
            (Decimal::ZERO, Decimal::ZERO)
        };

        // Calculate volume column width (adaptive)
        let fixed_width = if counter.is_hk() { 23 } else { 16 };
        let depth_volume_width = (depth_inner_rect.width as usize)
            .saturating_sub(fixed_width)
            .max(10);

        // Format depth row for Table widget
        let format_depth_row = |depth: &crate::data::Depth,
                                counter: &Counter,
                                prev_close: Option<Decimal>,
                                volume_width: usize|
         -> Row<'static> {
            // Position (without colon)
            let position = if depth.position < 10 {
                format!("{}   ", depth.position)
            } else {
                format!("{}  ", depth.position)
            };

            // Price with color
            let price_cmp = prev_close.map_or(std::cmp::Ordering::Equal, |pc| depth.price.cmp(&pc));
            let price_style = styles::up(price_cmp);
            let price_str = depth.price.format_quote_by_counter(counter).clone();

            // Volume (right-aligned to fixed width)
            let volume_str = crate::tui::ui::text::align_right(
                &crate::tui::ui::text::unit(Decimal::from(depth.volume), 0),
                volume_width,
            );

            // Order count (only for HK stocks, right-aligned to 6 chars)
            let order_count_str = if counter.is_hk() {
                crate::tui::ui::text::align_right(
                    &format!("({})", depth.order_num.clamp(0, 999)),
                    6,
                )
            } else {
                String::new()
            };

            Row::new(vec![
                Cell::from(position).style(crate::tui::ui::styles::gray()),
                Cell::from(price_str).style(price_style),
                Cell::from(volume_str),
                Cell::from(order_count_str),
            ])
        };

        // Asks - top section, reverse order (price low to high), max 5 levels
        let asks_rows: Vec<_> = stock
            .depth
            .asks
            .iter()
            .take(5)
            .map(|d| format_depth_row(d, counter, stock.quote.prev_close, depth_volume_width))
            .collect();
        let asks_rows: Vec<_> = asks_rows.into_iter().rev().collect();

        // Bids - bottom section, normal order (price high to low), max 5 levels
        let bids_rows: Vec<_> = stock
            .depth
            .bids
            .iter()
            .take(5)
            .map(|d| format_depth_row(d, counter, stock.quote.prev_close, depth_volume_width))
            .collect();

        // Calculate height based on actual depth levels
        let asks_count = asks_rows.len() as u16;
        let bids_count = bids_rows.len() as u16;
        let total_depth_height = asks_count + 1 + bids_count;
        let available_height = depth_inner_rect.height;
        let top_padding = available_height.saturating_sub(total_depth_height) / 2;

        // Vertical layout: asks -> ratio bar -> bids (dynamic height, vertically centered)
        let depth_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(top_padding),
                Constraint::Length(asks_count),
                Constraint::Length(1),
                Constraint::Length(bids_count),
                Constraint::Min(0),
            ])
            .split(depth_inner_rect);

        // Asks table (borderless, column-aligned)
        let table_widths = if counter.is_hk() {
            vec![
                Constraint::Length(4),
                Constraint::Length(10),
                Constraint::Length(depth_volume_width as u16),
                Constraint::Length(6),
            ]
        } else {
            vec![
                Constraint::Length(4),
                Constraint::Length(10),
                Constraint::Length(depth_volume_width as u16),
                Constraint::Length(0),
            ]
        };

        let asks_table = Table::new(asks_rows, table_widths.clone()).column_spacing(1);

        frame.render_widget(asks_table, depth_layout[1]);

        // Ratio bar: dual-color background using Paragraph (left green right red)
        let (bull_style, bear_style) = styles::bull_bear();
        let green_color = bull_style.fg.unwrap_or(Color::Green);
        let red_color = bear_style.fg.unwrap_or(Color::Red);

        // Calculate width by ratio
        let available_width = depth_layout[2].width as usize;
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let bid_width = ((Decimal::from(available_width) * bid_ratio)
            .to_string()
            .parse::<f64>()
            .unwrap_or(0.0)
            .round() as usize)
            .min(available_width);
        let ask_width = available_width.saturating_sub(bid_width);

        // Build labels: Bid on left, Ask on right
        let bid_label = format!(
            " {}: {:.1}%",
            t!("StockDepth.Bid"),
            bid_ratio * Decimal::from(100)
        );
        let ask_label = format!(
            "{}: {:.1}% ",
            t!("StockDepth.Ask"),
            ask_ratio * Decimal::from(100)
        );

        let bid_label_len = bid_label.chars().count();
        let ask_label_len = ask_label.chars().count();

        // Bid section: green background, label on left
        let bid_padding = bid_width.saturating_sub(bid_label_len);
        let bid_content = format!("{}{}", bid_label, " ".repeat(bid_padding));

        // Ask section: red background, label on right
        let ask_padding = ask_width.saturating_sub(ask_label_len);
        let ask_content = format!("{}{}", " ".repeat(ask_padding), ask_label);

        let ratio_line = Line::from(vec![
            Span::styled(
                bid_content,
                Style::default().fg(Color::White).bg(green_color),
            ),
            Span::styled(ask_content, Style::default().fg(Color::White).bg(red_color)),
        ]);

        frame.render_widget(Paragraph::new(ratio_line), depth_layout[2]);

        // Bids table (borderless, column-aligned)
        let bids_table = Table::new(bids_rows, table_widths).column_spacing(1);

        frame.render_widget(bids_table, depth_layout[3]);
    }

    // Render K-line chart area
    let chart_chunks = Layout::default()
        .constraints([Constraint::Ratio(2, 3), Constraint::Ratio(1, 3)])
        .direction(Direction::Horizontal)
        .split(chunks[2]);

    // Draw chart
    {
        const Y_AXIS_WIDTH: u16 = 17;

        let chart_chunks_inner = Layout::default()
            .constraints([Constraint::Length(2), Constraint::Min(20)])
            .direction(Direction::Vertical)
            .split(chart_chunks[0]);

        let selected_type_index = KlineType::iter()
            .position(|t| t == kline_type)
            .unwrap_or_default();
        let chart_tabs = Tabs::new(
            KlineType::iter()
                .map(|chart_type| {
                    Line::from(vec![
                        Span::raw(" "),
                        Span::raw(chart_type.to_string()),
                        Span::raw(" "),
                    ])
                })
                .collect::<Vec<_>>(),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .select(selected_type_index)
        .padding("", "");
        frame.render_widget(chart_tabs, chart_chunks_inner[0]);
        *crate::tui::mouse::KLINE_TABS_RECT.lock().expect("poison") = chart_chunks_inner[0];

        let area = chart_chunks_inner[1];
        let (width, page, _index) = area
            .width
            .checked_sub(Y_AXIS_WIDTH)
            .filter(|&v| v > 0)
            .map(|width| {
                let width = width as usize;
                (width, selected / width, selected % width)
            })
            .unwrap_or_default();
        let samples = KLINES.by_pagination(
            counter.clone(),
            kline_type,
            crate::data::AdjustType::ForwardAdjust,
            page,
            width,
        );

        // Show loading hint if no data
        if samples.is_empty() {
            frame.render_widget(
                Paragraph::new("Loading...").alignment(Alignment::Center),
                area,
            );
        } else {
            let candles: Vec<cli_candlestick_chart::Candle> = samples
                .iter()
                .filter_map(|sample| {
                    // Safe conversion, filter invalid data
                    let open = f64::try_from(sample.open).ok()?;
                    let high = f64::try_from(sample.high).ok()?;
                    let low = f64::try_from(sample.low).ok()?;
                    let close = f64::try_from(sample.close).ok()?;

                    // Validate data
                    if open <= 0.0 || high <= 0.0 || low <= 0.0 || close <= 0.0 {
                        return None;
                    }
                    if high < low || high < open || high < close || low > open || low > close {
                        return None;
                    }

                    Some(cli_candlestick_chart::Candle {
                        open,
                        high,
                        low,
                        close,
                        volume: Some(
                            #[allow(clippy::cast_precision_loss)]
                            {
                                (sample.amount as f64) / 1_000_000.0
                            },
                        ),
                        timestamp: Some(sample.timestamp),
                    })
                })
                .collect();

            if candles.is_empty() {
                frame.render_widget(
                    Paragraph::new(t!("Error.KlineDataFormat")).alignment(Alignment::Center),
                    area,
                );
            } else {
                let chart_width = area.width.saturating_sub(1);
                let (bull, bear) = styles::bull_bear_color();
                let chart_str = if matches!(
                    kline_type,
                    KlineType::PerDay | KlineType::PerWeek | KlineType::PerYear
                ) {
                    let mut chart = cli_candlestick_chart::Chart::new_with_size(
                        candles,
                        (chart_width, area.height),
                    );
                    chart.set_bull_color(bull);
                    chart.set_vol_bull_color(bull);
                    chart.set_bear_color(bear);
                    chart.set_vol_bear_color(bear);
                    chart.render()
                } else {
                    let mut chart = cli_candlestick_chart::LineChart::new_with_size(
                        candles,
                        (chart_width, area.height),
                    );
                    chart.set_bull_color(bull);
                    chart.set_vol_bull_color(bull);
                    chart.set_bear_color(bear);
                    chart.set_vol_bear_color(bear);
                    chart.render()
                };
                frame.render_widget(crate::tui::widgets::Ansi(&chart_str), area);
            }
        }
    }

    // Render trades area
    {
        let trades_area = chart_chunks[1];
        frame.render_widget(
            Block::default()
                .borders(Borders::LEFT)
                .border_type(BorderType::Plain)
                .border_style(styles::border())
                .title(format!(" {} ", t!("StockQuoteTrades"))),
            trades_area,
        );

        let inner_area = Rect {
            x: trades_area.x + 2,
            y: trades_area.y + 1,
            width: trades_area.width.saturating_sub(3),
            height: trades_area.height.saturating_sub(2),
        };

        if stock.trades.is_empty() {
            frame.render_widget(
                Paragraph::new("Loading...").alignment(Alignment::Center),
                inner_area,
            );
        } else {
            let fixed_width = 21;
            let volume_width = (inner_area.width as usize)
                .saturating_sub(fixed_width)
                .max(8);

            let max_volume = stock
                .trades
                .iter()
                .map(|t| t.volume.abs())
                .max()
                .unwrap_or(1);

            let trade_rows: Vec<Row> = stock
                .trades
                .iter()
                .take(inner_area.height as usize)
                .map(|trade| {
                    let time_str = time::OffsetDateTime::from_unix_timestamp(trade.timestamp)
                        .ok()
                        .and_then(|dt| {
                            let format =
                                time::format_description::parse("[hour]:[minute]:[second]").ok()?;
                            dt.format(&format).ok()
                        })
                        .unwrap_or_else(|| "--:--:--".to_string());

                    let (price_style, direction_symbol, bg_color) = match trade.direction {
                        crate::data::TradeDirection::Up => {
                            let style = styles::up(std::cmp::Ordering::Greater);
                            (style, "↑", style.fg.unwrap_or(Color::Green))
                        }
                        crate::data::TradeDirection::Down => {
                            let style = styles::up(std::cmp::Ordering::Less);
                            (style, "↓", style.fg.unwrap_or(Color::Red))
                        }
                        crate::data::TradeDirection::Neutral => {
                            (Style::default(), " ", Color::DarkGray)
                        }
                    };

                    #[allow(clippy::cast_precision_loss)]
                    let volume_ratio = if max_volume > 0 {
                        let current_volume = trade.volume.abs() as f64;
                        let max_vol_f64 = max_volume as f64;
                        let power = 0.5;
                        let current_pow = current_volume.powf(power);
                        let max_pow = max_vol_f64.powf(power);
                        (current_pow / max_pow).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };

                    let volume_text = crate::tui::ui::text::align_right(
                        &crate::tui::ui::text::unit(Decimal::from(trade.volume), 0),
                        volume_width,
                    );

                    #[allow(
                        clippy::cast_sign_loss,
                        clippy::cast_precision_loss,
                        clippy::cast_possible_truncation
                    )]
                    let bg_width = (volume_width as f64 * volume_ratio).ceil() as usize;
                    let fg_width = volume_width.saturating_sub(bg_width);

                    let volume_chars: Vec<char> = volume_text.chars().collect();
                    let fg_part: String = volume_chars.iter().take(fg_width).collect();
                    let bg_part: String =
                        volume_chars.iter().skip(fg_width).take(bg_width).collect();

                    let volume_cell = if !fg_part.is_empty() && !bg_part.is_empty() {
                        Cell::from(Line::from(vec![
                            Span::styled(fg_part, Style::default()),
                            Span::styled(bg_part, Style::default().bg(bg_color)),
                        ]))
                    } else if !bg_part.is_empty() {
                        Cell::from(Span::styled(bg_part, Style::default().bg(bg_color)))
                    } else {
                        Cell::from(fg_part)
                    };

                    let price_str = format!("{:>8}", trade.price.format_quote_by_counter(counter));

                    Row::new(vec![
                        Cell::from(time_str).style(crate::tui::ui::styles::label()),
                        Cell::from(direction_symbol).style(price_style),
                        Cell::from(price_str).style(price_style),
                        volume_cell,
                    ])
                })
                .collect();

            let widths = [
                Constraint::Length(9),
                Constraint::Length(1),
                Constraint::Length(8),
                Constraint::Length(volume_width as u16),
            ];
            let table = Table::new(trade_rows, widths).column_spacing(1);

            frame.render_widget(table, inner_area);
        }
    }
}

/// Debounced version of `refresh_stock` with 50ms delay
/// Cancels previous pending requests if a new one arrives within the debounce window
/// Also prevents multiple concurrent executions
pub fn refresh_stock_debounced(counter: Counter) {
    // Cancel previous pending task if it exists
    if let Ok(mut task_guard) = REFRESH_STOCK_TASK.lock() {
        if let Some(task) = task_guard.take() {
            task.abort();
        }

        // Reset not-found flag whenever a new refresh starts
        STOCK_NOT_FOUND.store(false, Ordering::Relaxed);

        // Spawn a new debounced task
        let handle = RT.get().unwrap().spawn(async move {
            // Wait 150ms before executing to avoid firing on every stock passed during navigation
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;

            // Try to acquire the execution lock (RAII guard)
            let Some(_guard) = RefreshGuard::try_acquire() else {
                tracing::debug!(
                    "Skipping refresh for {} - another refresh is in progress",
                    counter
                );
                return;
            };

            tracing::debug!("Starting refresh for {}", counter);

            // Execute the actual refresh
            let _ = WS
                .quote_detail("stock_detail", std::slice::from_ref(&counter))
                .await;
            let _ = WS
                .quote_trade("stock_detail", std::slice::from_ref(&counter))
                .await;

            // Get full quote data (including prev_close and trade_status)
            let ctx = crate::openapi::quote();
            if let Ok(quotes) = ctx.quote(&[counter.to_string()]).await {
                if let Some(quote) = quotes.first() {
                    STOCKS.modify(counter.clone(), |stock| {
                        stock.update_from_security_quote(quote);
                    });
                } else {
                    // API returned no data — symbol does not exist
                    STOCK_NOT_FOUND.store(true, Ordering::Relaxed);
                    if let Some(tx) = crate::tui::app::UPDATE_TX.get() {
                        let _ = tx.send(bevy_ecs::system::CommandQueue::default());
                    }
                }
            }

            // Get static info (if not already fetched)
            let should_fetch = STOCKS
                .get(&counter)
                .is_some_and(|s| s.static_info.is_none());

            if should_fetch {
                // Async fetch static info
                if let Ok(infos) = openapi::quote::fetch_static_info(&[counter.to_string()]).await {
                    if let Some(info) = infos.into_iter().next() {
                        STOCKS.modify(counter.clone(), |stock| {
                            stock.update_from_static_info(info);
                        });
                    }
                }
            }

            // Get trade records
            if let Ok(trades) = openapi::quote::fetch_trades(&counter.to_string(), 50).await {
                STOCKS.modify(counter.clone(), |stock| {
                    stock.update_from_trades(&trades);
                });
            }

            tracing::debug!("Completed refresh for {}", counter);

            // The _guard will be dropped here, automatically clearing REFRESH_EXECUTING
        });

        *task_guard = Some(handle);
    }
}

pub fn enter_stock(counter: Res<StockDetail>) {
    refresh_stock_debounced(counter.0.clone());
}

pub fn exit_stock() {
    STOCK_NOT_FOUND.store(false, Ordering::Relaxed);
    crate::tui::systems::stock_news::reset_news_view();
    RT.get().unwrap().spawn(async move {
        _ = WS.unmount("stock_detail").await;
    });
}
