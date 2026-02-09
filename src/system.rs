#![allow(clippy::too_many_arguments, clippy::too_many_lines)]
use std::{collections::HashMap, sync::atomic::Ordering, sync::Mutex};

use atomic::Atomic;
use bevy_ecs::{
    prelude::*,
    system::{CommandQueue, InsertResource},
};
use once_cell::sync::Lazy;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Borders, Cell, List, ListItem, Paragraph, Row, Table, TableState, Tabs,
    },
    Frame,
};
use rust_decimal::Decimal;
use tokio::sync::mpsc;

use crate::{
    app::{AppState, RT, WATCHLIST},
    data::{
        Account, Counter, KlineType, ReadyState, Stock, SubTypes, TradeSessionExt, TradeStatusExt,
        WatchlistGroup, STOCKS,
    },
    helper::{cycle, DecimalExt, Sign},
    kline::KLINES,
    ui::{
        styles::{self, item},
        Content,
    },
    widgets::{Carousel, Loading, LoadingWidget, LocalSearch, Search, Select, Terminal},
};

// Compatibility type alias
pub type Component = ();
const EMPTY_PLACEHOLDER: &str = "--";

// Portfolio stub types
pub mod portfolio {
    #[derive(Clone, Debug, Default)]
    pub struct Props {
        pub account_channel: String,
        pub aaid: String,
    }

    use crate::data::Market;
    use rust_decimal::Decimal;
    use std::collections::HashMap;

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
}

// Watchlist API - uses longport SDK
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
            _ => name.to_string(),
        }
    }

    let ctx = crate::openapi::quote();

    // Get watchlist
    match ctx.watchlist().await {
        Ok(watchlist) => {
            // Extract group info and symbols
            let mut groups = Vec::new();
            let mut counters = Vec::new();

            for group in watchlist {
                let group_id_u64 = group.id as u64;

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
                    match security.symbol.parse() {
                        Ok(counter) => {
                            counters.push(counter);
                        }
                        _ => (),
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

pub async fn fetch_holdings() -> anyhow::Result<Vec<Counter>> {
    let ctx = crate::openapi::trade();

    // Get holdings list
    match ctx.stock_positions(None).await {
        Ok(response) => {
            // StockPositionsResponse contains positions from multiple channels
            let mut counters = Vec::new();
            for channel in &response.channels {
                for position in &channel.positions {
                    match position.symbol.parse() {
                        Ok(counter) => {
                            counters.push(counter);
                        }
                        _ => (),
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
                    // Note: longport SDK may not directly provide cost price
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

// WebSocket subscription management (simplified implementation)
pub struct WsManager;

impl WsManager {
    pub async fn unmount(&self, _name: &str) -> anyhow::Result<()> {
        // TODO: Use longport SDK to unsubscribe
        Ok(())
    }

    pub async fn remount(
        &self,
        _name: &str,
        symbols: &[Counter],
        _sub_type: SubTypes,
    ) -> anyhow::Result<()> {
        // TODO: Use longport SDK to resubscribe
        let ctx = crate::openapi::quote();
        let symbol_strings: Vec<String> = symbols.iter().map(|c| c.to_string()).collect();
        let _ = ctx
            .subscribe(&symbol_strings, longport::quote::SubFlags::QUOTE)
            .await;
        Ok(())
    }

    pub async fn quote_detail(&self, _name: &str, symbols: &[Counter]) -> anyhow::Result<()> {
        let ctx = crate::openapi::quote();
        let symbol_strings: Vec<String> = symbols.iter().map(|c| c.to_string()).collect();
        let _ = ctx
            .subscribe(
                &symbol_strings,
                longport::quote::SubFlags::QUOTE | longport::quote::SubFlags::DEPTH,
            )
            .await;
        Ok(())
    }

    pub async fn quote_trade(&self, _name: &str, symbols: &[Counter]) -> anyhow::Result<()> {
        let ctx = crate::openapi::quote();
        let symbol_strings: Vec<String> = symbols.iter().map(|c| c.to_string()).collect();
        let _ = ctx
            .subscribe(&symbol_strings, longport::quote::SubFlags::TRADE)
            .await;
        Ok(())
    }
}

pub static WS: once_cell::sync::Lazy<WsManager> = once_cell::sync::Lazy::new(|| WsManager);

// Other stub types
#[derive(Clone, Debug, Default)]
pub struct DepthView {
    // TODO: Implement
}

#[derive(Clone, Debug, Default)]
pub struct DetailView {
    // TODO: Implement
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RiskLevel {
    Safe,
    Low,
    MiddleLow,
    Middle,
    MiddleHigh,
    Medium,
    High,
    Danger,
    Warning,
}

pub(crate) static KLINE_TYPE: Atomic<KlineType> = Atomic::new(KlineType::PerMinute);
pub(crate) static KLINE_INDEX: Atomic<usize> = Atomic::new(0);

pub(crate) static LAST_DONE: Lazy<Mutex<HashMap<Counter, Decimal>>> = Lazy::new(Mutex::default);
pub(crate) static WATCHLIST_TABLE: Lazy<Mutex<TableState>> = Lazy::new(Mutex::default);

type NavFooter<'w> = (
    Res<'w, State<AppState>>,
    Res<'w, Carousel<[Counter; 3]>>,
    Res<'w, WsState>,
);
type PopUp<'w> = (
    ResMut<'w, LocalSearch<Account>>,
    ResMut<'w, LocalSearch<crate::api::account::CurrencyInfo>>,
    ResMut<'w, Search<crate::api::search::StockItem>>,
    ResMut<'w, LocalSearch<WatchlistGroup>>,
);

#[derive(Event)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Tab,
    BackTab,
    Enter,
}

#[derive(Event)]
pub struct TuiEvent(pub tui_input::InputRequest);

#[derive(Clone, Resource)]
pub struct Command(pub mpsc::UnboundedSender<CommandQueue>);

#[derive(Resource)]
pub struct QrCode(pub String);

#[derive(Resource)]
pub struct WsState(pub ReadyState);

#[derive(Resource)]
pub struct StockDetail(pub Counter);

#[derive(Debug, Resource)]
pub struct Portfolio {
    pub props: portfolio::Props,
    pub view: portfolio::View,
}

pub fn error(mut terminal: ResMut<Terminal>, err: Res<Content<'static>>) {
    _ = terminal.draw(|frame| {
        frame.render_widget(err.clone(), frame.size());
    });
}

pub fn loading(mut terminal: ResMut<Terminal>, loading: Res<Loading>) {
    _ = terminal.draw(|frame| {
        frame.render_widget(LoadingWidget::from(&*loading), frame.size());
    });
}

pub fn qr_code(mut terminal: ResMut<Terminal>, token: Res<QrCode>) {
    _ = terminal.draw(|frame| {
        let content = Content::new(t!("qrcode_view.scan_hint"), Text::raw(&token.0));
        frame.render_widget(content, frame.size());
    });
}

pub fn exit_watchlist() {
    crate::app::LAST_STATE.store(AppState::Watchlist, Ordering::Relaxed);
}

pub fn enter_watchlist_common(command: Res<Command>) {
    refresh_watchlist(command.0.clone());
}

pub fn exit_watchlist_common() {
    RT.get().unwrap().spawn(async move {
        _ = WS.unmount("watchlist").await;
    });
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
                        match quote.symbol.parse() {
                            Ok(counter) => {
                                STOCKS.modify(counter, |stock| {
                                    // Use update_from_security_quote to update all fields including trade_status
                                    stock.update_from_security_quote(&quote);
                                });
                            }
                            _ => (),
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to fetch initial quotes: {}", e);
                }
            }

            // Get stock static info (including name, etc.)
            match ctx.static_info(symbols.iter().map(|s| s.as_str())).await {
                Ok(infos) => {
                    for info in infos {
                        match info.symbol.parse() {
                            Ok(counter) => {
                                STOCKS.modify(counter, |stock| {
                                    stock.name = info.name_cn.clone();
                                    stock.update_from_static_info(&info);
                                });
                            }
                            _ => (),
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

        let local_search = LocalSearch::new(
            WATCHLIST.read().expect("poison").groups().to_vec(),
            |keyword: &str, group: &crate::data::WatchlistGroup| {
                let keyword = &keyword.to_ascii_lowercase();
                group.name.to_ascii_lowercase().contains(keyword)
            },
        );
        let mut queue = CommandQueue::default();
        queue.push(InsertResource {
            resource: local_search,
        });
        _ = update_tx.send(queue);
    });
}

pub fn refresh_stock(counter: Counter) {
    RT.get().unwrap().spawn(async move {
        KLINES.clear();
        let _ = WS.quote_detail("stock_detail", &[counter.clone()]).await;
        let _ = WS.quote_trade("stock_detail", &[counter.clone()]).await;

        // Get full quote data (including prev_close and trade_status)
        let ctx = crate::openapi::quote();
        if let Ok(quotes) = ctx.quote(&[counter.to_string()]).await {
            if let Some(quote) = quotes.first() {
                STOCKS.modify(counter.clone(), |stock| {
                    // Use update_from_security_quote to update all fields including trade_status
                    stock.update_from_security_quote(quote);
                });
            }
        }

        // Get static info (if not already fetched)
        let should_fetch = STOCKS
            .get(&counter)
            .map(|s| s.static_info.is_none())
            .unwrap_or(false);

        if should_fetch {
            // Async fetch static info
            if let Ok(infos) = crate::api::quote::fetch_static_info(&[counter.to_string()]).await {
                if let Some(info) = infos.first() {
                    STOCKS.modify(counter.clone(), |stock| {
                        stock.update_from_static_info(info);
                    });
                }
            }
        }

        // Get trade records
        if let Ok(trades) = crate::api::quote::fetch_trades(&counter.to_string(), 50).await {
            STOCKS.modify(counter.clone(), |stock| {
                stock.update_from_trades(&trades);
            });
        }
    });
}

pub fn enter_stock(counter: Res<StockDetail>) {
    refresh_stock(counter.0.clone());
}

pub fn exit_stock() {
    KLINES.clear();
    RT.get().unwrap().spawn(async move {
        _ = WS.unmount("stock_detail").await;
    });
}

// Portfolio data global storage
pub static PORTFOLIO_VIEW: Lazy<std::sync::RwLock<Option<crate::data::PortfolioView>>> =
    Lazy::new(|| std::sync::RwLock::new(None));

// Refresh Portfolio data
pub fn refresh_portfolio() {
    RT.get().unwrap().spawn(async move {
        tracing::info!("Starting to refresh Portfolio data...");
        match crate::api::account::fetch_portfolio().await {
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
    crate::app::LAST_STATE.store(AppState::Portfolio, Ordering::Relaxed);
}

pub fn render_watchlist_stock(
    mut terminal: ResMut<Terminal>,
    mut events: EventReader<Key>,
    stock: Res<StockDetail>,
    command: Res<Command>,
    (state, indexes, ws): NavFooter,
    (mut account, mut currency, mut search, mut watchgroup): PopUp,
    mut last_choose: Local<Counter>,
) {
    // workaround bevyengine/bevy#9130
    if *last_choose != stock.0 {
        if !last_choose.is_empty() {
            refresh_stock(stock.0.clone());
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
                drop(table); // Explicitly release lock

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
                drop(table); // Explicitly release lock

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
    });
}

pub fn render_stock(
    mut terminal: ResMut<Terminal>,
    mut events: EventReader<Key>,
    stock: Res<StockDetail>,
    (state, indexes, ws): NavFooter,
    (mut account, mut currency, mut search, mut watchgroup): PopUp,
    mut last_choose: Local<Counter>,
) {
    // workaround bevyengine/bevy#9130
    if *last_choose != stock.0 {
        if !last_choose.is_empty() {
            refresh_stock(stock.0.clone());
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
            Key::Enter | Key::Up | Key::Down => {}
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

        stock_detail(
            frame,
            rect,
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
    });
}

fn stock_detail(
    frame: &mut Frame,
    rect: Rect,
    counter: &Counter,
    _kline_type: KlineType,
    _selected: usize,
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
        return;
    };

    // draw title
    let mut titles = vec![Span::raw(format!(
        " {} ({}.{})",
        stock.display_name(),
        counter.code(),
        counter.market(),
    ))];
    titles.extend(price_spans(&stock.quote, counter));
    // Show session or status label if not in normal trading
    let session_label = stock.trade_session.label();
    let status_label = if !session_label.is_empty() {
        session_label
    } else if !stock.trade_status.is_trading() {
        stock.trade_status.label()
    } else {
        String::new()
    };
    if !status_label.is_empty() {
        titles.push(Span::raw(format!(" [{}]", status_label)));
    }

    let detail_container = Block::default()
        .title(Line::from(titles))
        .borders(Borders::ALL);

    // draw border
    frame.render_widget(detail_container, rect);

    // Helper function to format optional Decimal (price type)
    let fmt_decimal = |opt: Option<Decimal>| -> String {
        opt.map(|d| d.format_quote_by_counter(counter))
            .unwrap_or_else(|| EMPTY_PLACEHOLDER.to_string())
    };

    // Helper function to create ListItem with price and color based on prev_close
    let price_item = |label: String, price_opt: Option<Decimal>| -> ListItem<'static> {
        let prev_close = stock.quote.prev_close.filter(|&p| p > Decimal::ZERO);
        let price = price_opt.filter(|&p| p > Decimal::ZERO);

        match (price, prev_close) {
            (Some(p), Some(prev)) => {
                let price_str = p.format_quote_by_counter(counter);
                let cmp = p.cmp(&prev);
                let style = styles::up(cmp);
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{label}: "), crate::ui::styles::label()),
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
    let fmt_u64 = |val: u64| -> String {
        if val == 0 {
            EMPTY_PLACEHOLDER.to_string()
        } else {
            crate::ui::text::unit(Decimal::from(val), 0)
        }
    };

    // Helper function to format i64
    let fmt_i64 = |val: i64| -> String {
        if val == 0 {
            EMPTY_PLACEHOLDER.to_string()
        } else {
            crate::ui::text::unit(Decimal::from(val), 0)
        }
    };

    // Build detail columns - Column 1: Basic trading data
    let column0 = vec![
        ListItem::new(" "),
        item(
            t!("StockDetail.Trading Status"),
            {
                let session_label = stock.trade_session.label();
                if !session_label.is_empty() {
                    session_label
                } else {
                    stock.trade_status.label()
                }
            }
        ),
        ListItem::new(" "),
        price_item(t!("StockDetail.Open"), stock.quote.open),
        item(
            t!("StockDetail.Prev. Close"),
            fmt_decimal(stock.quote.prev_close),
        ),
        ListItem::new(" "),
        price_item(t!("StockDetail.High"), stock.quote.high),
        price_item(t!("StockDetail.Low"), stock.quote.low),
        item(t!("StockDetail.Average"), EMPTY_PLACEHOLDER), // Needs calculation
        ListItem::new(" "),
        item(t!("StockDetail.Volume"), fmt_u64(stock.quote.volume)),
        item(
            t!("StockDetail.Turnover"),
            crate::ui::text::unit(stock.quote.turnover, 2),
        ),
        ListItem::new(" "),
    ];

    // Column 2: Static info (if available)
    let column1 = if let Some(ref info) = stock.static_info {
        vec![
            ListItem::new(" "),
            ListItem::new(" "),
            ListItem::new(" "),
            item(t!("StockDetail.P/E (TTM)"), fmt_decimal(info.eps_ttm)),
            item(t!("StockDetail.EPS (TTM)"), fmt_decimal(info.eps)),
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
            item(t!("StockDetail.Shares"), fmt_i64(info.total_shares)),
            item(
                t!("StockDetail.Shares Float"),
                fmt_i64(info.circulating_shares),
            ),
            ListItem::new(" "),
            item(t!("StockDetail.BPS"), fmt_decimal(info.bps)),
            item(
                t!("StockDetail.Dividend Yield (TTM)"),
                fmt_decimal(info.dividend_yield),
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
    let chunks = Layout::default()
        .constraints([
            Constraint::Length(column_height),
            Constraint::Length(1),
            Constraint::Min(19),
        ])
        .direction(Direction::Vertical)
        .split(rect);
    let columns_chunks = Layout::default()
        .constraints([
            Constraint::Ratio(2, 9),
            Constraint::Ratio(2, 9),
            Constraint::Ratio(2, 9),
            Constraint::Ratio(3, 9),
        ])
        .direction(Direction::Horizontal)
        .split(chunks[0].inner(&Margin {
            vertical: 1,
            horizontal: 2,
        }));
    frame.render_widget(List::new(column0), columns_chunks[0]);
    frame.render_widget(List::new(column1), columns_chunks[1]);
    frame.render_widget(List::new(column2), columns_chunks[2]);

    // Draw market depth
    let depth_rect = columns_chunks[3].inner(&Margin {
        vertical: 1,
        horizontal: 0,
    });
    frame.render_widget(
        Block::default()
            .borders(Borders::LEFT)
            .border_type(BorderType::Plain),
        depth_rect,
    );

    if !stock.depth.bids.is_empty() || !stock.depth.asks.is_empty() {
        // Format single depth level
        let format_depth_line = |depth: &crate::data::Depth,
                                 counter: &Counter,
                                 prev_close: Option<Decimal>|
         -> Line<'static> {
            // Position/Level
            let position = Span::styled(
                format!("{:>2}:", depth.position),
                crate::ui::styles::label(),
            );
            // Price
            let price_cmp = prev_close
                .map(|pc| depth.price.cmp(&pc))
                .unwrap_or(std::cmp::Ordering::Equal);
            let price_style = styles::up(price_cmp);
            let price = Span::styled(
                format!("{:>10} ", depth.price.format_quote_by_counter(counter)),
                price_style,
            );
            // Volume
            let volume = crate::ui::text::align_right(
                &crate::ui::text::unit(Decimal::from(depth.volume), 0),
                6,
            );
            // Order count (only for HK stocks)
            let order_count = if counter.is_hk() {
                crate::ui::text::align_right(&format!("({})", depth.order_num.clamp(0, 999)), 5)
            } else {
                String::new()
            };
            Line::from(vec![position, price, volume.into(), order_count.into()])
        };

        let rect = depth_rect.inner(&Margin {
            vertical: 1,
            horizontal: 2,
        });

        // 计算买卖比例
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

        // 布局
        let chunks = Layout::default()
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(10),
            ])
            .direction(Direction::Vertical)
            .split(rect);

        // Title
        let bidding_title = format!(
            " {}: {:.1}%",
            t!("StockDepth.Bid"),
            bid_ratio * Decimal::from(100)
        );
        let asking_title = format!(
            " {}: {:.1}%",
            t!("StockDepth.Ask"),
            ask_ratio * Decimal::from(100)
        );

        let title_chunks = Layout::default()
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .direction(Direction::Horizontal)
            .split(chunks[0]);
        frame.render_widget(Paragraph::new(bidding_title), title_chunks[0]);
        frame.render_widget(Paragraph::new(asking_title), title_chunks[1]);

        // 比例条
        const BLOCK: &str = "▂";
        let bar_width = rect.width as usize;
        let bid_blocks = ((Decimal::from(bar_width) * bid_ratio)
            .to_string()
            .parse::<f64>()
            .unwrap_or(0.0)
            .round() as usize)
            .min(bar_width);
        let ask_blocks = bar_width.saturating_sub(bid_blocks);

        let (bull_style, bear_style) = styles::bull_bear();
        let ratio_bar = Line::from(vec![
            Span::styled(BLOCK.repeat(bid_blocks), bull_style),
            Span::styled(BLOCK.repeat(ask_blocks), bear_style),
        ]);
        frame.render_widget(Paragraph::new(ratio_bar), chunks[1]);

        // 深度列表：卖盘在上（倒序），买盘在下（正序）
        let mut depth_lines: Vec<Line> = Vec::new();

        // 卖盘（asks）- 倒序显示（从低到高）
        let asks: Vec<Line> = stock
            .depth
            .asks
            .iter()
            .rev()
            .take(10)
            .map(|d| format_depth_line(d, counter, stock.quote.prev_close))
            .collect();
        depth_lines.extend(asks.into_iter().rev());

        // 分隔线（可选）
        if !stock.depth.asks.is_empty() && !stock.depth.bids.is_empty() {
            depth_lines.push(Line::from("―".repeat(rect.width as usize)));
        }

        // 买盘（bids）- 正序显示（从高到低）
        let bids: Vec<Line> = stock
            .depth
            .bids
            .iter()
            .take(10)
            .map(|d| format_depth_line(d, counter, stock.quote.prev_close))
            .collect();
        depth_lines.extend(bids);

        frame.render_widget(Paragraph::new(Text::from(depth_lines)), chunks[2]);
    }

    // 渲染 K线图区域
    let chart_chunks = Layout::default()
        .constraints([Constraint::Ratio(2, 3), Constraint::Ratio(1, 3)])
        .direction(Direction::Horizontal)
        .split(chunks[2].inner(&Margin {
            vertical: 1,
            horizontal: 0,
        }));

    // Draw chart
    {
        const Y_AXIS_WIDTH: u16 = 17;

        let chart_chunks_inner = Layout::default()
            .constraints([Constraint::Length(2), Constraint::Min(20)])
            .direction(Direction::Vertical)
            .split(chart_chunks[0].inner(&Margin {
                vertical: 0,
                horizontal: 2,
            }));

        let selected_type_index = KlineType::iter()
            .position(|t| t == _kline_type)
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
                .collect(),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .select(selected_type_index);
        frame.render_widget(chart_tabs, chart_chunks_inner[0]);

        let area = chart_chunks_inner[1];
        let (width, page, _index) = area
            .width
            .checked_sub(Y_AXIS_WIDTH)
            .filter(|&v| v > 0)
            .map(|width| {
                let width = width as usize;
                (width, _selected / width, _selected % width)
            })
            .unwrap_or_default();
        let samples = crate::kline::KLINES.by_pagination(
            counter.clone(),
            _kline_type,
            crate::data::AdjustType::ForwardAdjust,
            page,
            width,
        );

        // 如果没有数据，显示提示信息
        if samples.is_empty() {
            frame.render_widget(
                Paragraph::new("加载 K 线数据中...").alignment(Alignment::Center),
                area,
            );
        } else {
            let candles: Vec<cli_candlestick_chart::Candle> = samples
                .iter()
                .filter_map(|sample| {
                    // 安全转换，过滤无效数据
                    let open = f64::try_from(sample.open).ok()?;
                    let high = f64::try_from(sample.high).ok()?;
                    let low = f64::try_from(sample.low).ok()?;
                    let close = f64::try_from(sample.close).ok()?;

                    // 验证数据有效性
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
                                sample.amount as f64
                            },
                        ),
                        timestamp: Some(sample.timestamp),
                    })
                })
                .collect();

            if candles.is_empty() {
                frame.render_widget(
                    Paragraph::new("K线数据格式错误").alignment(Alignment::Center),
                    area,
                );
            } else {
                // 调整图表大小，减去边框和信息行的高度
                let chart_height = area.height.saturating_sub(1);
                let mut chart = cli_candlestick_chart::Chart::new_with_size(
                    candles,
                    (area.width, chart_height),
                );
                let (bull, bear) = styles::bull_bear_color();
                chart.set_bull_color(bull);
                chart.set_vol_bull_color(bull);
                chart.set_bear_color(bear);
                chart.set_vol_bear_color(bear);
                chart.set_name(counter.code().to_string());
                // 渲染图表，留出底部空间给信息显示
                frame.render_widget(
                    crate::widgets::Ansi(&chart.render()),
                    Rect {
                        y: area.y + 1,
                        height: area.height.saturating_sub(1),
                        ..area
                    },
                );
            }
        }
    }

    // 渲染成交记录区域
    {
        let trades_area = chart_chunks[1];
        frame.render_widget(
            Block::default()
                .borders(Borders::LEFT)
                .border_type(BorderType::Plain)
                .title(format!(" {} ", t!("StockQuoteTrades"))),
            trades_area,
        );

        let inner_area = trades_area.inner(&Margin {
            vertical: 1,
            horizontal: 2,
        });

        if stock.trades.is_empty() {
            // Show loading hint
            frame.render_widget(
                Paragraph::new("加载成交记录中...").alignment(Alignment::Center),
                inner_area,
            );
        } else {
            // Calculate max volume for progress bar
            let max_volume = stock
                .trades
                .iter()
                .map(|t| t.volume.abs())
                .max()
                .unwrap_or(1);

            // Format trade records
            let trade_lines: Vec<Line> = stock
                .trades
                .iter()
                .take(inner_area.height as usize)
                .map(|trade| {
                    // Simplified time display
                    let time_str = time::OffsetDateTime::from_unix_timestamp(trade.timestamp)
                        .ok()
                        .and_then(|dt| {
                            let format =
                                time::format_description::parse("[hour]:[minute]:[second]").ok()?;
                            dt.format(&format).ok()
                        })
                        .unwrap_or_else(|| "--:--:--".to_string());

                    // Set style based on direction
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

                    // Calculate progress percentage (0.0 to 1.0)
                    let volume_ratio = if max_volume > 0 {
                        trade.volume.abs() as f64 / max_volume as f64
                    } else {
                        0.0
                    };

                    // Create volume text with progress bar background
                    let volume_text = crate::ui::text::align_right(
                        &crate::ui::text::unit(Decimal::from(trade.volume), 0),
                        8,
                    );

                    // Calculate background width (in characters)
                    let volume_width: usize = 8; // Width of volume column
                    let bg_width = (volume_width as f64 * volume_ratio).ceil() as usize;
                    let fg_width = volume_width.saturating_sub(bg_width);

                    // Split text into foreground and background parts (right to left)
                    let volume_chars: Vec<char> = volume_text.chars().collect();
                    // Foreground part (left side, no background)
                    let fg_part: String = volume_chars.iter().take(fg_width).collect();
                    // Background part (right side, with colored background)
                    let bg_part: String =
                        volume_chars.iter().skip(fg_width).take(bg_width).collect();

                    // Create volume span with background color (from right to left)
                    let mut volume_spans = vec![];
                    if !fg_part.is_empty() {
                        volume_spans.push(Span::styled(fg_part, Style::default()));
                    }
                    if !bg_part.is_empty() {
                        volume_spans.push(Span::styled(bg_part, Style::default().bg(bg_color)));
                    }

                    // Combine all spans
                    let mut line_spans = vec![
                        Span::styled(format!("{} ", time_str), crate::ui::styles::label()),
                        Span::styled(direction_symbol, price_style),
                        Span::styled(
                            format!(" {:>8} ", trade.price.format_quote_by_counter(counter)),
                            price_style,
                        ),
                    ];
                    line_spans.extend(volume_spans);

                    Line::from(line_spans)
                })
                .collect();

            frame.render_widget(Paragraph::new(Text::from(trade_lines)), inner_area);
        }
    }
}

pub fn render_watchlist(
    mut terminal: ResMut<Terminal>,
    mut events: EventReader<Key>,
    command: Res<Command>,
    (state, indexes, ws): NavFooter,
    (mut account, mut currency, mut search, mut watchgroup): PopUp,
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
            Key::Left | Key::Right | Key::Tab | Key::BackTab => (),
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
            .constraints([Constraint::Length(81), Constraint::Min(20)])
            .direction(Direction::Horizontal)
            .split(rect);

        watch(frame, chunks[0], true);
        banner(frame, chunks[1]);

        crate::views::popup::render(
            frame,
            rect,
            &mut account,
            &mut currency,
            &mut search,
            &mut watchgroup,
        );
    });
}

fn watch(frame: &mut Frame, rect: Rect, full_mode: bool) {
    // Extract data from watchlist early and release the lock
    let (counters, group_name) = {
        let watchlist = WATCHLIST.read().expect("poison");
        (
            watchlist.counters().to_vec(),
            watchlist
                .group()
                .map(|g| g.name.clone())
                .unwrap_or_else(|| "--".to_string()),
        )
    }; // Lock released here

    let background = Block::default().borders(Borders::ALL).title(format!(
        " {} ─── {} [g] ",
        t!("Watchlist"),
        group_name
    ));
    frame.render_widget(background, rect);

    // Lock WATCHLIST_TABLE once for both reading and rendering
    let mut table_state = WATCHLIST_TABLE.lock().expect("poison");
    let selected = table_state.selected();
    frame.render_stateful_widget(
        watch_group_table(
            &counters,
            selected,
            &mut LAST_DONE.lock().expect("poison"),
            full_mode,
        ),
        rect.inner(&Margin {
            vertical: 2,
            horizontal: 2,
        }),
        &mut *table_state,
    );
}

fn banner(frame: &mut Frame, rect: Rect) {
    frame.render_widget(Block::default().borders(Borders::ALL), rect);

    frame.render_widget(
        crate::ui::assets::banner(crate::ui::styles::text()),
        crate::ui::rect::centered(0, crate::ui::assets::BANNER_HEIGHT, rect),
    );
}

fn watch_group_table(
    counters: &[Counter],
    selected: Option<usize>,
    last_dones: &mut HashMap<Counter, Decimal>,
    full_mode: bool,
) -> Table<'static> {
    // todo: auto scale
    const COLUMN_WIDTHS: [usize; 6] = [9, 21, 10, 8, 10, 14];
    const COLUMN_WIDTHS2: [Constraint; 6] = [
        Constraint::Length(9),
        Constraint::Length(21),
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Length(10),
        // tradeStatus 在 en 下，最长可能有 14 个字符
        Constraint::Length(14),
    ];

    let header = {
        let mut cells = Vec::with_capacity(if full_mode { 6 } else { 4 });
        cells.push(t!("watchlist.CODE"));
        cells.push(t!("watchlist.NAME"));
        cells.push(t!("watchlist.PRICE"));
        cells.push(crate::ui::text::align_right(
            &t!("watchlist.CHG"),
            COLUMN_WIDTHS[3],
        ));
        if full_mode {
            cells.push(crate::ui::text::align_right(
                &t!("watchlist.VOL"),
                COLUMN_WIDTHS[4],
            ));
            cells.push(t!("watchlist.STATUS"));
        };
        Row::new(cells).style(styles::header()).bottom_margin(1)
    };

    let stocks = STOCKS.mget(counters);
    let rows = counters
        .iter()
        .zip(stocks.iter())
        .map(|(counter, stock)| {
            static EMPTY: Lazy<Stock> = Lazy::new(Stock::default);
            let stock = stock.as_deref().unwrap_or(&EMPTY);
            let quote_data = &stock.quote;

            // 优先使用 last_done，如果没有则使用 prev_close
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
                .or(quote_data.open) // Use open price if last_done not available
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

            // Determine status to display:
            // 1. If stock status is abnormal (Halted/Suspended/etc), show trade status
            // 2. If not in normal trading session (Pre/Post/Night), show session status
            // 3. Otherwise show "Trading" for normal trading session with normal status
            let get_status_label = || {
                if !stock.trade_status.is_trading() {
                    // Abnormal status (Halted, Delisted, etc.) - highest priority
                    stock.trade_status.label()
                } else if !stock.trade_session.is_normal_trading() {
                    // Non-Intraday session (Pre, Post, Overnight)
                    stock.trade_session.label()
                } else {
                    // Normal trading: Intraday + Normal status
                    stock.trade_session.label() // Show "Trading" for Intraday
                }
            };

            let status_label = get_status_label();
            // Format: +5.44/5% (no sign for percentage, omit decimal if .00)
            let change_sign = if increase.is_sign_positive() { "+" } else { "" };
            let percent_str = if increase_percent.fract().abs() == Decimal::ZERO {
                // Integer percentage: omit decimal point
                format!("{}", increase_percent.abs().trunc())
            } else {
                format!("{}", increase_percent.abs())
            };
            let increase_percent_str = format!(
                "{}{}/{}%",
                change_sign,
                increase.round_dp(2),
                percent_str
            );
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
                Cell::from(crate::ui::text::align_right(
                    &increase_percent_str,
                    COLUMN_WIDTHS[3],
                ))
                .style(style),
            );
            if full_mode {
                let volume_text = crate::helper::format_volume(quote_data.volume);
                cells.push(Cell::from(crate::ui::text::align_right(
                    &volume_text,
                    COLUMN_WIDTHS[4],
                )));
                // Display session status or trade status in STATUS column
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

    Table::new(rows)
        .header(header)
        .highlight_style(highlight_style)
        .widths(&COLUMN_WIDTHS2)
        .column_spacing(1)
}

pub fn render_portfolio(
    mut terminal: ResMut<Terminal>,
    mut _events: EventReader<Key>,
    _portfolio: Res<Portfolio>,
    _accounts: Res<Select<Account>>,
    _command: Res<Command>,
    (state, indexes, ws): NavFooter,
    (mut account, mut currency, mut search, mut watchgroup): PopUp,
    _table_state: Local<TableState>,
) {
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

        // Main content area with horizontal margins (1 char on each side)
        let content_rect = Rect {
            x: rect.x + 1,
            y: rect.y + 1,
            width: rect.width.saturating_sub(2),
            height: rect.height - 2,
        };

        // Get Portfolio data
        let portfolio_view_lock = PORTFOLIO_VIEW.read().expect("poison");
        let portfolio_view = match &*portfolio_view_lock {
            Some(view) => view,
            None => {
                // Show loading message if no data yet
                frame.render_widget(
                    Paragraph::new("Loading portfolio data...")
                        .alignment(Alignment::Center)
                        .block(Block::default().borders(Borders::ALL)),
                    content_rect,
                );
                drop(portfolio_view_lock);
                crate::views::popup::render(
                    frame,
                    rect,
                    &mut account,
                    &mut currency,
                    &mut search,
                    &mut watchgroup,
                );
                return;
            }
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
                .title(format!(" {} ", t!("Portfolio.Title")));

            // Calculate styles for P/L
            let pl_style = styles::up(overview.total_pl.cmp(&Decimal::ZERO));
            let today_pl_style = styles::up(overview.total_today_pl.cmp(&Decimal::ZERO));

            // Create three-column layout
            let inner_area = overview_block.inner(chunks[0]);
            frame.render_widget(overview_block, chunks[0]);

            let inner_chunks = Layout::default()
                .constraints([
                    Constraint::Ratio(1, 3),
                    Constraint::Ratio(1, 3),
                    Constraint::Ratio(1, 3),
                ])
                .direction(Direction::Horizontal)
                .split(inner_area);

            // Column 1
            let left_items = vec![
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{}: ", t!("Portfolio.Total Asset")),
                        styles::label(),
                    ),
                    Span::styled(format!("{:.2}", overview.total_asset), styles::text()),
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
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{}: ", t!("Portfolio.Health Status")),
                        styles::label(),
                    ),
                    Span::styled(
                        format!("{:.2}%", overview.leverage_ratio * Decimal::from(100)),
                        styles::text(),
                    ),
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
                        format!("{}: ", t!("Portfolio.Total Cash Amount")),
                        styles::label(),
                    ),
                    Span::styled(format!("{:.2}", overview.total_cash), styles::text()),
                ])),
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{}: ", t!("Portfolio.Fund Market Cap")),
                        styles::label(),
                    ),
                    Span::styled(format!("{:.2}", overview.fund_market_value), styles::text()),
                ])),
            ];

            // Column 3
            let right_items = vec![
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{}: ", t!("Portfolio.Risk Level")), styles::label()),
                    Span::styled(format!("{}", overview.risk_level), styles::text()),
                ])),
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{}: ", t!("Portfolio.Credit Limit")),
                        styles::label(),
                    ),
                    Span::styled(format!("{:.2}", overview.credit_limit), styles::text()),
                ])),
                ListItem::new(Line::from(vec![
                    Span::styled("Holdings: ", styles::label()),
                    Span::styled(format!("{}", holdings.len()), styles::text()),
                ])),
                ListItem::new(""),
                ListItem::new(Span::styled(
                    "Press R to refresh",
                    Style::default().fg(Color::Gray),
                )),
            ];

            let left_list = List::new(left_items);
            let middle_list = List::new(middle_items);
            let right_list = List::new(right_items);

            frame.render_widget(left_list, inner_chunks[0]);
            frame.render_widget(middle_list, inner_chunks[1]);
            frame.render_widget(right_list, inner_chunks[2]);
        }

        // 底部：持仓列表
        {
            let holdings_block = Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", t!("Holding.Holding")));

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
                // 创建持仓表格
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
                .style(styles::header())
                .bottom_margin(1);

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
                            Cell::from(format!("{:.2}", holding.market_price)),
                            Cell::from(
                                holding
                                    .cost_price
                                    .map_or("-".to_string(), |p| format!("{:.2}", p)),
                            ),
                            Cell::from(format!("{:.2}", holding.market_value)),
                            Cell::from(format!("{:+.2}", profit_loss)).style(pl_style),
                            Cell::from(format!("{:+.2}%", profit_loss_percent)).style(pl_style),
                        ])
                    })
                    .collect();

                let table = Table::new(rows)
                    .header(header)
                    .block(holdings_block)
                    .widths(&[
                        Constraint::Length(12), // 代码
                        Constraint::Min(15),    // 名称
                        Constraint::Length(10), // 数量
                        Constraint::Length(10), // 现价
                        Constraint::Length(10), // 成本
                        Constraint::Length(12), // 市值
                        Constraint::Length(12), // 盈亏
                        Constraint::Length(10), // 盈亏%
                    ])
                    .column_spacing(1);

                frame.render_widget(table, chunks[1]);
            }
        }

        // 渲染弹窗
        crate::views::popup::render(
            frame,
            rect,
            &mut account,
            &mut currency,
            &mut search,
            &mut watchgroup,
        );
    });
}
