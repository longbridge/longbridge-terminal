#![allow(clippy::too_many_arguments, clippy::too_many_lines)]
use std::{collections::HashMap, sync::atomic::Ordering, sync::Mutex};

use atomic::Atomic;
use bevy_ecs::{
    prelude::*,
    system::{CommandQueue, InsertResource},
};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Borders, Cell, List, ListItem, Paragraph, Row, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Table, TableState, Tabs,
    },
    Frame,
};
use rust_decimal::Decimal;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

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
                let group_id_u64 = group.id.cast_unsigned();

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
    #[allow(clippy::unused_async)]
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
        let symbol_strings: Vec<String> = symbols
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        let _ = ctx
            .subscribe(&symbol_strings, longport::quote::SubFlags::QUOTE)
            .await;
        Ok(())
    }

    pub async fn quote_detail(&self, _name: &str, symbols: &[Counter]) -> anyhow::Result<()> {
        let ctx = crate::openapi::quote();
        let symbol_strings: Vec<String> = symbols
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
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
        let symbol_strings: Vec<String> = symbols
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        let _ = ctx
            .subscribe(&symbol_strings, longport::quote::SubFlags::TRADE)
            .await;
        Ok(())
    }
}

pub static WS: std::sync::LazyLock<WsManager> = std::sync::LazyLock::new(|| WsManager);

// Debounce state for stock refresh
static REFRESH_STOCK_TASK: std::sync::LazyLock<Mutex<Option<JoinHandle<()>>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));
// Flag to track if a refresh is currently executing
static REFRESH_EXECUTING: Atomic<bool> = Atomic::new(false);

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

pub(crate) static KLINE_TYPE: Atomic<KlineType> = Atomic::new(KlineType::PerDay);
pub(crate) static KLINE_INDEX: Atomic<usize> = Atomic::new(0);

pub(crate) static LAST_DONE: std::sync::LazyLock<Mutex<HashMap<Counter, Decimal>>> =
    std::sync::LazyLock::new(Mutex::default);
pub(crate) static WATCHLIST_TABLE: std::sync::LazyLock<Mutex<TableState>> =
    std::sync::LazyLock::new(Mutex::default);

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
                                stock.name.clone_from(&info.name_cn);
                                stock.update_from_static_info(&info);
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
                    // Use update_from_security_quote to update all fields including trade_status
                    stock.update_from_security_quote(quote);
                });
            }
        }

        // Get static info (if not already fetched)
        let should_fetch = STOCKS
            .get(&counter)
            .is_some_and(|s| s.static_info.is_none());

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

/// Debounced version of `refresh_stock` with 50ms delay
/// Cancels previous pending requests if a new one arrives within the debounce window
/// Also prevents multiple concurrent executions
pub fn refresh_stock_debounced(counter: Counter) {
    // Cancel previous pending task if it exists
    if let Ok(mut task_guard) = REFRESH_STOCK_TASK.lock() {
        if let Some(task) = task_guard.take() {
            task.abort();
        }

        // Spawn a new debounced task
        let handle = RT.get().unwrap().spawn(async move {
            // Wait 50ms before executing
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

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
            KLINES.clear();
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
                }
            }

            // Get static info (if not already fetched)
            let should_fetch = STOCKS
                .get(&counter)
                .is_some_and(|s| s.static_info.is_none());

            if should_fetch {
                // Async fetch static info
                if let Ok(infos) =
                    crate::api::quote::fetch_static_info(&[counter.to_string()]).await
                {
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
    KLINES.clear();
    RT.get().unwrap().spawn(async move {
        _ = WS.unmount("stock_detail").await;
    });
}

// Portfolio data global storage
pub static PORTFOLIO_VIEW: std::sync::LazyLock<
    std::sync::RwLock<Option<crate::data::PortfolioView>>,
> = std::sync::LazyLock::new(|| std::sync::RwLock::new(None));

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
    mut log_panel: Local<crate::widgets::LogPanel>,
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

pub fn render_stock(
    mut terminal: ResMut<Terminal>,
    mut events: EventReader<Key>,
    stock: Res<StockDetail>,
    (state, indexes, ws): NavFooter,
    (mut account, mut currency, mut search, mut watchgroup): PopUp,
    mut last_choose: Local<Counter>,
    mut log_panel: Local<crate::widgets::LogPanel>,
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

fn stock_detail(
    frame: &mut Frame,
    rect: Rect,
    counter: &Counter,
    kline_type: KlineType,
    selected: usize,
) {
    use ratatui::widgets::{Cell, Row, Table};

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
    let fmt_unsigned = |val: u64| -> String {
        if val == 0 {
            EMPTY_PLACEHOLDER.to_string()
        } else {
            crate::ui::text::unit(Decimal::from(val), 0)
        }
    };

    // Helper function to format i64
    let fmt_signed = |val: i64| -> String {
        if val == 0 {
            EMPTY_PLACEHOLDER.to_string()
        } else {
            crate::ui::text::unit(Decimal::from(val), 0)
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
        item(t!("StockDetail.Volume"), fmt_unsigned(stock.quote.volume)),
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
            item(t!("StockDetail.Shares"), fmt_signed(info.total_shares)),
            item(
                t!("StockDetail.Shares Float"),
                fmt_signed(info.circulating_shares),
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

    // Split into upper and lower sections with a divider
    // Use asymmetric margin: left 2 for spacing, right 1
    let block_inner = rect.inner(&Margin {
        vertical: 1,
        horizontal: 0,
    });
    let inner_rect = Rect {
        x: block_inner.x + 2,
        y: block_inner.y,
        width: block_inner.width.saturating_sub(3), // left: 2, right: 1
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
            x: block_inner.x + 1, // left margin
            y: block_inner.y,
            width: block_inner.width.saturating_sub(2), // left: 1, right: 1
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
        let fixed_width = if counter.is_hk() {
            // position (4) + price (10) + order_count (6) + spacing (3) = 23
            23
        } else {
            // position (4) + price (10) + spacing (2) = 16
            16
        };
        let depth_volume_width = (depth_inner_rect.width as usize)
            .saturating_sub(fixed_width)
            .max(10);

        // Format depth row for Table widget
        let format_depth_row = |depth: &crate::data::Depth,
                                counter: &Counter,
                                prev_close: Option<Decimal>,
                                volume_width: usize|
         -> ratatui::widgets::Row<'static> {
            use ratatui::widgets::{Cell, Row};

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
            let volume_str = crate::ui::text::align_right(
                &crate::ui::text::unit(Decimal::from(depth.volume), 0),
                volume_width,
            );

            // Order count (only for HK stocks, right-aligned to 6 chars)
            let order_count_str = if counter.is_hk() {
                crate::ui::text::align_right(&format!("({})", depth.order_num.clamp(0, 999)), 6)
            } else {
                String::new()
            };

            Row::new(vec![
                Cell::from(position).style(crate::ui::styles::gray()),
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
        let total_depth_height = asks_count + 1 + bids_count; // asks + bar + bids
        let available_height = depth_inner_rect.height;
        let top_padding = available_height.saturating_sub(total_depth_height) / 2;

        // Vertical layout: asks -> ratio bar -> bids (dynamic height, vertically centered)
        let depth_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(top_padding), // Top padding
                Constraint::Length(asks_count),  // Asks (actual row count)
                Constraint::Length(1),           // Ratio bar (1 row)
                Constraint::Length(bids_count),  // Bids (actual row count)
                Constraint::Min(0),              // Bottom padding
            ])
            .split(depth_inner_rect);

        // Asks table (borderless, column-aligned)
        let table_widths = if counter.is_hk() {
            vec![
                Constraint::Length(4),                         // Position number
                Constraint::Length(10),                        // Price
                Constraint::Length(depth_volume_width as u16), // Volume (fixed width, right-aligned)
                Constraint::Length(6),                         // Order count (right-aligned)
            ]
        } else {
            vec![
                Constraint::Length(4),                         // Position number
                Constraint::Length(10),                        // Price
                Constraint::Length(depth_volume_width as u16), // Volume (fixed width, right-aligned)
                Constraint::Length(0),                         // No order count
            ]
        };

        let asks_table = Table::new(asks_rows)
            .widths(&table_widths)
            .column_spacing(1);

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
        let bids_table = Table::new(bids_rows)
            .widths(&table_widths)
            .column_spacing(1);

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
                (width, selected / width, selected % width)
            })
            .unwrap_or_default();
        let samples = crate::kline::KLINES.by_pagination(
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
                                // Divide by 1M to shorten display (e.g., 6979570787 -> 6979.57)
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
                // Adjust chart size - reduce width slightly to prevent bottom info line overflow
                let chart_width = area.width.saturating_sub(1);
                let mut chart = cli_candlestick_chart::Chart::new_with_size(
                    candles,
                    (chart_width, area.height),
                );
                let (bull, bear) = styles::bull_bear_color();
                chart.set_bull_color(bull);
                chart.set_vol_bull_color(bull);
                chart.set_bear_color(bear);
                chart.set_vol_bear_color(bear);
                // Don't set name to avoid "CCL |" prefix in the info line
                frame.render_widget(crate::widgets::Ansi(&chart.render()), area);
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

        // Use asymmetric margin: left margin for border, minimal right margin
        let inner_area = Rect {
            x: trades_area.x + 2,
            y: trades_area.y + 1,
            width: trades_area.width.saturating_sub(3), // left: 2, right: 1
            height: trades_area.height.saturating_sub(2),
        };

        if stock.trades.is_empty() {
            // Show loading hint
            frame.render_widget(
                Paragraph::new("Loading...").alignment(Alignment::Center),
                inner_area,
            );
        } else {
            // Calculate available width for volume column (adaptive)
            // Fixed columns: time (9) + space (1) + direction (1) + space (1) + price (8) + space (1) = 21
            let fixed_width = 21;
            let volume_width = (inner_area.width as usize)
                .saturating_sub(fixed_width)
                .max(8);

            // Calculate max volume for progress bar
            let max_volume = stock
                .trades
                .iter()
                .map(|t| t.volume.abs())
                .max()
                .unwrap_or(1);

            // Format trade records as table rows
            let trade_rows: Vec<Row> = stock
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

                    // Calculate progress percentage (0.0 to 1.0) using power scale
                    // This prevents huge differences when some trades have very large volumes
                    // Power of 0.5 (sqrt) compresses less than log10, showing more difference
                    // You can adjust the power value: 0.3 = more compression, 0.7 = less compression
                    #[allow(clippy::cast_precision_loss)]
                    let volume_ratio = if max_volume > 0 {
                        let current_volume = trade.volume.abs() as f64;
                        let max_vol_f64 = max_volume as f64;

                        // Use power scale (0.5 = square root)
                        let power = 0.5;
                        let current_pow = current_volume.powf(power);
                        let max_pow = max_vol_f64.powf(power);

                        (current_pow / max_pow).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };

                    // Create volume text with progress bar background (adaptive width)
                    let volume_text = crate::ui::text::align_right(
                        &crate::ui::text::unit(Decimal::from(trade.volume), 0),
                        volume_width,
                    );

                    // Calculate background width (in characters, using adaptive width)
                    #[allow(
                        clippy::cast_sign_loss,
                        clippy::cast_precision_loss,
                        clippy::cast_possible_truncation
                    )]
                    let bg_width = (volume_width as f64 * volume_ratio).ceil() as usize;
                    let fg_width = volume_width.saturating_sub(bg_width);

                    // Split text into foreground and background parts (right to left)
                    let volume_chars: Vec<char> = volume_text.chars().collect();
                    // Foreground part (left side, no background)
                    let fg_part: String = volume_chars.iter().take(fg_width).collect();
                    // Background part (right side, with colored background)
                    let bg_part: String =
                        volume_chars.iter().skip(fg_width).take(bg_width).collect();

                    // Create volume cell with progress bar effect
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

                    // Format price with fixed width
                    let price_str = format!("{:>8}", trade.price.format_quote_by_counter(counter));

                    Row::new(vec![
                        Cell::from(time_str).style(crate::ui::styles::label()),
                        Cell::from(direction_symbol).style(price_style),
                        Cell::from(price_str).style(price_style),
                        volume_cell,
                    ])
                })
                .collect();

            // Create table with borderless style
            let widths = [
                Constraint::Length(9),                   // time
                Constraint::Length(1),                   // direction
                Constraint::Length(8),                   // price
                Constraint::Length(volume_width as u16), // volume (fixed to match formatted width)
            ];
            let table = Table::new(trade_rows).widths(&widths).column_spacing(1);

            frame.render_widget(table, inner_area);
        }
    }
}

pub fn render_watchlist(
    mut terminal: ResMut<Terminal>,
    mut events: EventReader<Key>,
    command: Res<Command>,
    (state, indexes, ws): NavFooter,
    (mut account, mut currency, mut search, mut watchgroup): PopUp,
    mut log_panel: Local<crate::widgets::LogPanel>,
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

fn watch(frame: &mut Frame, rect: Rect, full_mode: bool) {
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
        .title(format!(" {} ─── {}[g] ", t!("Watchlist"), group_name));
    frame.render_widget(background, rect);

    // Lock WATCHLIST_TABLE once for both reading and rendering
    let mut table_state = WATCHLIST_TABLE.lock().expect("poison");
    let selected = table_state.selected();
    // Use asymmetric margin: left 2 for spacing, right 1
    let block_inner = rect.inner(&Margin {
        vertical: 2,
        horizontal: 0,
    });
    let table_area = Rect {
        x: block_inner.x + 2,
        y: block_inner.y,
        width: block_inner.width.saturating_sub(3), // left: 2, right: 1
        height: block_inner.height,
    };
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
        .end_symbol(None);
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
        // tradeStatus in en locale can be up to 14 characters
        Constraint::Length(14),
    ];

    let header = {
        let mut cells = Vec::with_capacity(if full_mode { 6 } else { 4 });
        cells.push(Cell::from(t!("watchlist.CODE")).style(styles::header()));
        cells.push(Cell::from(t!("watchlist.NAME")).style(styles::header()));
        cells.push(Cell::from(t!("watchlist.PRICE")).style(styles::header()));
        cells.push(
            Cell::from(crate::ui::text::align_right(
                &t!("watchlist.CHG"),
                COLUMN_WIDTHS[3],
            ))
            .style(styles::header()),
        );
        if full_mode {
            cells.push(
                Cell::from(crate::ui::text::align_right(
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
            static EMPTY: std::sync::LazyLock<Stock> = std::sync::LazyLock::new(Stock::default);
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
            // 1. If it's an index (code starts with "IN"), don't show trading status
            // 2. If stock status is abnormal (Halted/Suspended/etc), show trade status
            // 3. If not in normal trading session (Pre/Post/Night), show session status
            // 4. Otherwise show "Trading" for normal trading session with normal status
            let get_status_label = || {
                if !stock.trade_session.is_normal_trading() {
                    // Non-Intraday session (Pre, Post, Overnight)
                    stock.trade_session.label()
                } else if !stock.trade_status.is_trading() {
                    // Abnormal status (Halted, Delisted, etc.) - highest priority
                    stock.trade_status.label()
                } else {
                    // Normal trading: Intraday + Normal status
                    stock.trade_session.label() // Show "Trading" for Intraday
                }
            };

            let status_label = get_status_label();
            // Format: +5% (only percentage with sign)
            let change_sign = if increase.is_sign_positive() { "+" } else { "" };
            let percent_str = if increase_percent.fract().abs() == Decimal::ZERO {
                // Integer percentage: omit decimal point
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
    mut log_panel: Local<crate::widgets::LogPanel>,
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
            crate::views::popup::render(
                frame,
                rect,
                &mut account,
                &mut currency,
                &mut search,
                &mut watchgroup,
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
                    " {} ({}) ",
                    t!("Portfolio.Title"),
                    overview.currency
                ));

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

        // Bottom: Holdings list
        {
            let holdings_block = Block::default()
                .borders(Borders::ALL)
                .border_style(styles::border())
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

                // Render block and get inner area with horizontal margin
                frame.render_widget(holdings_block, chunks[1]);
                let block_inner = Block::default().borders(Borders::ALL).inner(chunks[1]);
                let table_area = Rect {
                    x: block_inner.x + 1,
                    y: block_inner.y,
                    width: block_inner.width.saturating_sub(2),
                    height: block_inner.height,
                };

                let table = Table::new(rows)
                    .header(header)
                    .widths(&[
                        Constraint::Percentage(10), // Code
                        Constraint::Percentage(10), // Name
                        Constraint::Percentage(8),  // Quantity
                        Constraint::Percentage(14), // Price (with currency)
                        Constraint::Percentage(14), // Cost Price (with currency)
                        Constraint::Percentage(16), // Market Value (with currency)
                        Constraint::Percentage(10), // P/L
                        Constraint::Percentage(10), // P/L%
                    ])
                    .column_spacing(1);

                frame.render_widget(table, table_area);
            }
        }

        // Render popups
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
