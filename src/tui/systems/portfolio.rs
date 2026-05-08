use crate::{
    data::Market,
    openapi,
    tui::app::{AppState, RT},
};
use bevy_ecs::prelude::*;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Table},
};
use rust_decimal::Decimal;
use std::{collections::HashMap, sync::atomic::Ordering, sync::Mutex};

use crate::{
    data::{Account, Counter, STOCKS},
    tui::ui::styles,
    tui::widgets::Terminal,
};

use super::{NavFooter, PopUp};

#[derive(Debug, Resource, Default)]
pub struct Portfolio {
    pub props: Props,
    pub view: View,
}

pub static PORTFOLIO_VIEW: std::sync::LazyLock<
    std::sync::RwLock<Option<crate::data::PortfolioView>>,
> = std::sync::LazyLock::new(|| std::sync::RwLock::new(None));

pub static PORTFOLIO_TABLE: std::sync::LazyLock<Mutex<ratatui::widgets::TableState>> =
    std::sync::LazyLock::new(Mutex::default);

pub fn render_portfolio(
    mut terminal: ResMut<Terminal>,
    mut events: EventReader<super::Key>,
    _portfolio: Res<Portfolio>,
    _accounts: Res<crate::tui::widgets::Select<Account>>,
    _command: Res<super::Command>,
    (state, indexes, ws): NavFooter,
    (mut account, mut currency, mut search, mut watchgroup, mut watchlist_search): PopUp,
    mut log_panel: Local<crate::tui::widgets::LogPanel>,
) {
    for event in &mut events {
        let len = PORTFOLIO_VIEW
            .read()
            .expect("poison")
            .as_ref()
            .map_or(0, |v| v.holdings.len());
        match event {
            super::Key::Up => {
                let mut table = PORTFOLIO_TABLE.lock().expect("poison");
                let idx = table.selected();
                if len > 0 {
                    table.select(Some(idx.map_or(0, |i| if i == 0 { 0 } else { i - 1 })));
                }
            }
            super::Key::Down => {
                let mut table = PORTFOLIO_TABLE.lock().expect("poison");
                let idx = table.selected();
                if len > 0 {
                    table.select(Some(idx.map_or(0, |i| if i + 1 < len { i + 1 } else { i })));
                }
            }
            _ => {}
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

        // Main content area with horizontal margins (1 char on each side)
        let content_rect = Rect {
            x: rect.x + 1,
            y: rect.y + 1,
            width: rect.width.saturating_sub(2),
            height: rect.height - 2,
        };

        // Get Portfolio data
        let portfolio_view_lock = PORTFOLIO_VIEW.read().expect("poison");
        let Some(portfolio_view) = &*portfolio_view_lock else {
            // Show loading message if no data yet
            frame.render_widget(
                Paragraph::new("Loading portfolio data...")
                    .alignment(Alignment::Center)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_style(styles::border()),
                    ),
                content_rect,
            );
            drop(portfolio_view_lock);
            crate::tui::views::popup::render(
                frame,
                rect,
                &mut account,
                &mut currency,
                &mut search,
                &mut watchgroup,
                &mut watchlist_search,
            );
            return;
        };

        let overview = &portfolio_view.overview;
        let holdings = &portfolio_view.holdings;

        let chunks = Layout::default()
            .constraints([Constraint::Length(8), Constraint::Min(10)])
            .direction(Direction::Vertical)
            .split(content_rect);

        {
            let overview_block = Block::default()
                .borders(Borders::ALL)
                .border_style(styles::border())
                .title(format!(
                    " {} ({}) ─── Refresh [r] ",
                    t!("Portfolio.Title"),
                    overview.currency
                ))
                .title_bottom(
                    Line::from(Span::styled(
                        format!(" {} ", t!("Portfolio.QuoteDisclaimer")),
                        styles::dark_gray(),
                    ))
                    .right_aligned(),
                );

            // Calculate styles for P/L
            let pl_style = styles::up(overview.total_pl.cmp(&Decimal::ZERO));
            let today_pl_style = styles::up(overview.total_today_pl.cmp(&Decimal::ZERO));

            // Create three-column layout with horizontal margin (1 char each side)
            let block_inner = overview_block.inner(chunks[0]);
            let inner_area = Rect {
                x: block_inner.x + 1,
                y: block_inner.y,
                width: block_inner.width.saturating_sub(2),
                height: block_inner.height,
            };
            frame.render_widget(overview_block, chunks[0]);

            // Split inner area: 3-column overview + 1 distribution row
            let inner_vertical = Layout::default()
                .constraints([Constraint::Min(3), Constraint::Length(1)])
                .direction(Direction::Vertical)
                .split(inner_area);

            let inner_chunks = Layout::default()
                .constraints([
                    Constraint::Ratio(1, 3),
                    Constraint::Ratio(1, 3),
                    Constraint::Ratio(1, 3),
                ])
                .direction(Direction::Horizontal)
                .split(inner_vertical[0]);

            // Column 1
            let left_items = vec![
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{}: ", t!("Portfolio.Total Asset")),
                        styles::label(),
                    ),
                    Span::styled(
                        format!("{:.2} {}", overview.total_asset, overview.currency),
                        styles::text(),
                    ),
                ])),
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{}: ", t!("Portfolio.Market Cap")), styles::label()),
                    Span::styled(format!("{:.2}", overview.market_cap), styles::text()),
                ])),
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{}: ", t!("Portfolio.Margin Call")),
                        styles::label(),
                    ),
                    Span::styled(format!("{:.2}", overview.margin_call), styles::text()),
                ])),
            ];

            // Column 2
            let middle_items = vec![
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{}: ", t!("Portfolio.P/L")), styles::label()),
                    Span::styled(format!("{:+.2}", overview.total_pl), pl_style),
                ])),
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{}: ", t!("Portfolio.Intraday P/L")),
                        styles::label(),
                    ),
                    Span::styled(format!("{:+.2}", overview.total_today_pl), today_pl_style),
                ])),
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{}: ", t!("Portfolio.Cash Amount")),
                        styles::label(),
                    ),
                    Span::styled(format!("{:.2}", overview.total_cash), styles::text()),
                ])),
            ];

            // Column 3
            let right_items = vec![
                {
                    let (risk_label, risk_style) = styles::risk_level(overview.risk_level);
                    ListItem::new(Line::from(vec![
                        Span::styled(format!("{}: ", t!("Portfolio.Risk Level")), styles::label()),
                        Span::styled(risk_label, risk_style),
                    ]))
                },
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{}: ", t!("Portfolio.Credit Limit")),
                        styles::label(),
                    ),
                    Span::styled(format!("{:.2}", overview.credit_limit), styles::text()),
                ])),
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{}: ", t!("Portfolio.Fund Market Cap")),
                        styles::label(),
                    ),
                    Span::styled(format!("{:.2}", overview.fund_market_value), styles::text()),
                ])),
            ];

            let left_list = List::new(left_items);
            let middle_list = List::new(middle_items);
            let right_list = List::new(right_items);

            frame.render_widget(left_list, inner_chunks[0]);
            frame.render_widget(middle_list, inner_chunks[1]);
            frame.render_widget(right_list, inner_chunks[2]);

            // Distribution row: colored dot + market label + pct, sorted by USD value desc
            if overview.total_asset > Decimal::ZERO {
                let total = overview.total_asset;

                // Aggregate USD market value per market
                let mut map: std::collections::HashMap<Market, Decimal> =
                    std::collections::HashMap::new();
                for h in holdings {
                    let market = if let Some(dot_pos) = h.symbol.rfind('.') {
                        match &h.symbol[dot_pos + 1..] {
                            "US" => Market::US,
                            "SH" | "SZ" => Market::CN,
                            "SG" => Market::SG,
                            _ => Market::HK,
                        }
                    } else {
                        Market::HK
                    };
                    *map.entry(market).or_insert(Decimal::ZERO) += h.market_value_usd;
                }

                // Build sorted list including cash (None = cash)
                let mut entries: Vec<(String, Decimal, Option<Market>)> = map
                    .into_iter()
                    .filter(|(_, v)| *v > Decimal::ZERO)
                    .map(|(m, v)| (format!("{m:?}"), v, Some(m)))
                    .collect();
                entries.push(("Cash".to_string(), overview.total_cash, None));
                entries.sort_by_key(|b| std::cmp::Reverse(b.1));

                let mut spans: Vec<Span> = Vec::new();
                for (label, value, market_opt) in &entries {
                    let pct = value / total * Decimal::ONE_HUNDRED;
                    let dot_style = if let Some(m) = market_opt {
                        styles::market(*m)
                    } else {
                        Style::default().fg(Color::Green)
                    };
                    spans.push(Span::styled("● ", dot_style));
                    spans.push(Span::styled(
                        format!("{label} {pct:.1}%  "),
                        styles::label(),
                    ));
                }
                frame.render_widget(Paragraph::new(Line::from(spans)), inner_vertical[1]);
            }
        }

        // Bottom: Holdings list
        {
            let holdings_block = Block::default()
                .borders(Borders::ALL)
                .border_style(styles::border())
                .title(format!(" {} ", t!("Holding.Holding")))
                .title_bottom(
                    Line::from(vec![
                        Span::styled(format!(" {} ", t!("Trade.BuyKey")), styles::dark_gray()),
                        Span::styled(format!(" {} ", t!("Trade.SellKey")), styles::dark_gray()),
                    ])
                    .right_aligned(),
                );

            if holdings.is_empty() {
                let message = Paragraph::new(vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        t!("Portfolio.No Holdings"),
                        Style::default().fg(Color::Gray),
                    )),
                ])
                .block(holdings_block)
                .alignment(Alignment::Center);
                frame.render_widget(message, chunks[1]);
            } else {
                // Create holdings table
                let header = Row::new(vec![
                    t!("Holding.Code"),
                    t!("Holding.Name"),
                    t!("Holding.Quantity"),
                    t!("Holding.Price"),
                    t!("Holding.Cost Price"),
                    t!("Holding.Market Value"),
                    t!("Holding.P/L"),
                    t!("Holding.P/L%"),
                ])
                .style(styles::header());

                let rows: Vec<Row> = holdings
                    .iter()
                    .map(|holding| {
                        // Parse Counter from symbol string
                        let counter = Counter::from(holding.symbol.as_str());

                        // Calculate P/L
                        let (profit_loss, profit_loss_percent) =
                            if let Some(cost_price) = holding.cost_price {
                                let pl = holding.market_value - (cost_price * holding.quantity);
                                let pl_pct = if cost_price > Decimal::ZERO {
                                    (holding.market_price - cost_price) / cost_price
                                        * Decimal::from(100)
                                } else {
                                    Decimal::ZERO
                                };
                                (pl, pl_pct)
                            } else {
                                (Decimal::ZERO, Decimal::ZERO)
                            };

                        let pl_style = styles::up(profit_loss.cmp(&Decimal::ZERO));

                        // Get currency string
                        let currency_str = match holding.currency {
                            crate::data::Currency::HKD => "HKD",
                            crate::data::Currency::USD => "USD",
                            crate::data::Currency::CNY => "CNY",
                            crate::data::Currency::SGD => "SGD",
                        };

                        Row::new(vec![
                            Cell::from(Line::from(vec![
                                Span::styled(
                                    counter.market().to_string(),
                                    styles::market(counter.region()),
                                ),
                                Span::raw(" "),
                                Span::raw(counter.code().to_string()),
                            ])),
                            Cell::from(holding.name.clone()),
                            Cell::from(format!("{:.0}", holding.quantity)),
                            Cell::from(format!("{:.2} {}", holding.market_price, currency_str)),
                            Cell::from(
                                holding
                                    .cost_price
                                    .map_or("-".to_string(), |p| format!("{p:.2} {currency_str}")),
                            ),
                            Cell::from(format!("{:.2} {}", holding.market_value, currency_str)),
                            Cell::from(format!("{profit_loss:+.2}")).style(pl_style),
                            Cell::from(format!("{profit_loss_percent:+.2}%")).style(pl_style),
                        ])
                    })
                    .collect();

                let table = Table::new(
                    rows,
                    [
                        Constraint::Percentage(10), // Code
                        Constraint::Percentage(10), // Name
                        Constraint::Percentage(8),  // Quantity
                        Constraint::Percentage(14), // Price (with currency)
                        Constraint::Percentage(14), // Cost Price (with currency)
                        Constraint::Percentage(16), // Market Value (with currency)
                        Constraint::Percentage(10), // P/L
                        Constraint::Percentage(10), // P/L%
                    ],
                )
                .header(header)
                .block(holdings_block)
                .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
                .column_spacing(1);

                frame.render_stateful_widget(
                    table,
                    chunks[1],
                    &mut *PORTFOLIO_TABLE.lock().expect("poison"),
                );
                *crate::tui::mouse::PORTFOLIO_TABLE_RECT
                    .lock()
                    .expect("poison") = chunks[1];
            }
        }

        // Render popups
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

// Portfolio stub types

#[derive(Clone, Debug, Default)]
pub struct Props {
    pub account_channel: String,
    pub aaid: String,
}

#[derive(Clone, Debug, Default)]
pub struct StockHold {
    pub total: Decimal,
}

#[derive(Clone, Debug, Default)]
pub struct Overview {
    pub total_assets: Decimal,
}

#[derive(Clone, Debug, Default)]
pub struct MarketPortfolio {
    pub total: Decimal,
}

#[derive(Clone, Debug, Default)]
pub struct CashBalance {
    pub total: Decimal,
}

#[derive(Clone, Debug, Default)]
pub struct View {
    pub stock_hold: StockHold,
    pub props: Props,
    pub overview: Overview,
    pub market_portfolio: HashMap<Market, MarketPortfolio>,
    pub cash_balance: CashBalance,
}

pub async fn fetch_holdings() -> anyhow::Result<Vec<Counter>> {
    let ctx = crate::openapi::trade();

    // Get holdings list
    match ctx.stock_positions(None).await {
        Ok(response) => {
            // StockPositionsResponse contains positions from multiple channels
            let mut counters = Vec::new();
            for channel in &response.channels {
                for position in &channel.positions {
                    #[allow(irrefutable_let_patterns)]
                    if let Ok(counter) = position.symbol.parse() {
                        counters.push(counter);
                    }
                }
            }
            Ok(counters)
        }
        Err(e) => {
            tracing::error!("Failed to fetch holdings: {}", e);
            Ok(vec![])
        }
    }
}

// Position information
#[derive(Clone, Debug)]
pub struct PositionInfo {
    pub symbol: Counter,
    pub symbol_name: String,
    pub quantity: Decimal,
    pub available_quantity: Decimal,
    pub cost_price: Decimal,
    pub current_price: Decimal,
    pub market_value: Decimal,
    pub profit_loss: Decimal,
    pub profit_loss_percent: Decimal,
}

// Fetch Portfolio data
pub async fn fetch_portfolio_data() -> anyhow::Result<(Vec<PositionInfo>, Decimal, Decimal)> {
    let ctx = crate::openapi::trade();

    // Get account balance
    let balance = match ctx.account_balance(None).await {
        Ok(balances) => balances
            .iter()
            .fold(Decimal::ZERO, |acc, b| acc + b.total_cash),
        Err(e) => {
            tracing::error!("Failed to fetch account balance: {}", e);
            Decimal::ZERO
        }
    };

    // Get positions
    let mut positions = match ctx.stock_positions(None).await {
        Ok(response) => {
            let mut positions = Vec::new();
            for channel in &response.channels {
                for position in &channel.positions {
                    let counter: Counter = position.symbol.parse().unwrap();
                    positions.push(PositionInfo {
                        symbol: counter,
                        symbol_name: position.symbol_name.clone(),
                        quantity: position.quantity,
                        available_quantity: position.available_quantity,
                        cost_price: Decimal::ZERO, // Will be calculated below using quotes
                        current_price: Decimal::ZERO,
                        market_value: Decimal::ZERO,
                        profit_loss: Decimal::ZERO,
                        profit_loss_percent: Decimal::ZERO,
                    });
                }
            }
            positions
        }
        Err(e) => {
            tracing::error!("Failed to fetch positions: {}", e);
            vec![]
        }
    };

    // Get real-time quotes to calculate market value and P/L
    if !positions.is_empty() {
        let quote_ctx = crate::openapi::quote();
        let symbols: Vec<String> = positions.iter().map(|p| p.symbol.to_string()).collect();

        if let Ok(quotes) = quote_ctx.quote(&symbols).await {
            for (pos, quote) in positions.iter_mut().zip(quotes.iter()) {
                // Update current price
                pos.current_price = quote.last_done;

                // Calculate market value
                pos.market_value = pos.quantity * pos.current_price;

                // Get cost price from STOCKS cache (if available)
                if let Some(_stock) = STOCKS.get(&pos.symbol) {
                    // Note: Longbridge SDK may not directly provide cost price
                    // We try to get it from static info or other sources
                    // Temporarily use open price as reference
                    pos.cost_price = quote.open;

                    // Calculate P/L
                    if pos.cost_price > Decimal::ZERO {
                        let cost_total = pos.quantity * pos.cost_price;
                        pos.profit_loss = pos.market_value - cost_total;
                        pos.profit_loss_percent =
                            (pos.profit_loss / cost_total * Decimal::from(100)).round_dp(2);
                    }
                } else {
                    // If no cache, use prev_close as cost price estimate
                    pos.cost_price = if quote.prev_close > Decimal::ZERO {
                        quote.prev_close
                    } else {
                        quote.last_done
                    };

                    let cost_total = pos.quantity * pos.cost_price;
                    if cost_total > Decimal::ZERO {
                        pos.profit_loss = pos.market_value - cost_total;
                        pos.profit_loss_percent =
                            (pos.profit_loss / cost_total * Decimal::from(100)).round_dp(2);
                    }
                }
            }
        }
    }

    // Calculate total market value of positions
    let total_market_value = positions
        .iter()
        .fold(Decimal::ZERO, |acc, p| acc + p.market_value);

    Ok((positions, balance, total_market_value))
}

// Refresh Portfolio data
pub fn refresh_portfolio() {
    RT.get().unwrap().spawn(async move {
        tracing::info!("Starting to refresh Portfolio data...");
        match openapi::account::fetch_portfolio().await {
            Ok(view) => {
                tracing::info!(
                    "Successfully fetched Portfolio: {} holdings, total asset: {}",
                    view.holdings.len(),
                    view.overview.total_asset
                );

                *PORTFOLIO_VIEW.write().expect("poison") = Some(view);
            }
            Err(e) => {
                tracing::error!("Failed to fetch Portfolio data: {}", e);
            }
        }
    });
}

pub fn enter_portfolio(_portfolio: Res<Portfolio>) {
    refresh_portfolio();
}

pub fn exit_portfolio() {
    crate::tui::app::LAST_STATE.store(AppState::Portfolio, Ordering::Relaxed);
}
