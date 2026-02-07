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
        Account, Counter, KlineType, ReadyState, Stock, SubTypes, TradeStatus, WatchlistGroup,
        STOCKS,
    },
    helper::{cycle, DecimalExt, Sign},
    kline::KLINES,
    ui::{
        styles::{self, item},
        Content,
    },
    widgets::{Carousel, Loading, LoadingWidget, LocalSearch, Search, Select, Terminal},
};

// 兼容性类型别名
pub type Component = ();
const EMPTY_PLACEHOLDER: &str = "--";

// Portfolio 相关的存根类型
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

// Watchlist API - 使用 longport SDK
pub async fn fetch_watchlist(
    group_id: Option<u64>,
) -> anyhow::Result<(Vec<Counter>, Vec<crate::data::WatchlistGroup>)> {
    // 翻译默认分组名称
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

    // 获取自选股列表
    match ctx.watchlist().await {
        Ok(watchlist) => {
            // 提取分组信息和自选股
            let mut groups = Vec::new();
            let mut counters = Vec::new();

            for group in watchlist {
                let group_id_u64 = group.id as u64;

                // 添加分组信息,翻译默认分组名称
                groups.push(crate::data::WatchlistGroup {
                    id: group_id_u64,
                    name: translate_group_name(&group.name),
                });

                // 如果指定了分组ID，只返回该分组的股票
                if let Some(filter_id) = group_id {
                    if group_id_u64 != filter_id {
                        continue;
                    }
                }

                // 添加该分组下的股票
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
                "获取到 {} 个分组，共 {} 个股票 (过滤分组: {:?})",
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

    // 获取持仓列表
    match ctx.stock_positions(None).await {
        Ok(response) => {
            // StockPositionsResponse 包含多个渠道的持仓
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
            tracing::error!("获取持仓失败: {}", e);
            Ok(vec![])
        }
    }
}

// 持仓信息
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

// 获取 Portfolio 数据
pub async fn fetch_portfolio_data() -> anyhow::Result<(Vec<PositionInfo>, Decimal, Decimal)> {
    let ctx = crate::openapi::trade();

    // 获取账户余额
    let balance = match ctx.account_balance(None).await {
        Ok(balances) => balances
            .iter()
            .fold(Decimal::ZERO, |acc, b| acc + b.total_cash),
        Err(e) => {
            tracing::error!("获取账户余额失败: {}", e);
            Decimal::ZERO
        }
    };

    // 获取持仓
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
                        cost_price: Decimal::ZERO, // 将在下面通过行情计算
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
            tracing::error!("获取持仓失败: {}", e);
            vec![]
        }
    };

    // 获取实时行情来计算市值和盈亏
    if !positions.is_empty() {
        let quote_ctx = crate::openapi::quote();
        let symbols: Vec<String> = positions.iter().map(|p| p.symbol.to_string()).collect();

        if let Ok(quotes) = quote_ctx.quote(&symbols).await {
            for (pos, quote) in positions.iter_mut().zip(quotes.iter()) {
                // 更新当前价格
                pos.current_price = quote.last_done;

                // 计算市值
                pos.market_value = pos.quantity * pos.current_price;

                // 从 STOCKS 缓存中获取成本价(如果有的话)
                if let Some(_stock) = STOCKS.get(&pos.symbol) {
                    // 注意: longport SDK 可能不直接提供成本价
                    // 这里我们尝试从静态信息或其他来源获取
                    // 临时使用开盘价作为参考
                    pos.cost_price = quote.open;

                    // 计算盈亏
                    if pos.cost_price > Decimal::ZERO {
                        let cost_total = pos.quantity * pos.cost_price;
                        pos.profit_loss = pos.market_value - cost_total;
                        pos.profit_loss_percent =
                            (pos.profit_loss / cost_total * Decimal::from(100)).round_dp(2);
                    }
                } else {
                    // 如果没有缓存,使用昨收价作为成本价的估算
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

    // 计算持仓总市值
    let total_market_value = positions
        .iter()
        .fold(Decimal::ZERO, |acc, p| acc + p.market_value);

    Ok((positions, balance, total_market_value))
}

// WebSocket 订阅管理（简化实现）
pub struct WsManager;

impl WsManager {
    pub async fn unmount(&self, _name: &str) -> anyhow::Result<()> {
        // TODO: 使用 longport SDK 取消订阅
        Ok(())
    }

    pub async fn remount(
        &self,
        _name: &str,
        symbols: &[Counter],
        _sub_type: SubTypes,
    ) -> anyhow::Result<()> {
        // TODO: 使用 longport SDK 重新订阅
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

// 其他存根类型
#[derive(Clone, Debug, Default)]
pub struct DepthView {
    // TODO: 实现
}

#[derive(Clone, Debug, Default)]
pub struct DetailView {
    // TODO: 实现
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
            // 简化实现：使用默认排序
            let mut watchlist = WATCHLIST.write().expect("poison");
            watchlist.set_hidden(true);
            watchlist.set_sortby((0, 0, false)); // (sort_mode, sort_by, reverse)
            watchlist.counters().to_vec()
        };

        // 为每个自选股创建 Stock 条目（如果不存在）
        for counter in &counters {
            if STOCKS.get(counter).is_none() {
                let mut stock = crate::data::Stock::new(counter.clone());
                stock.name = counter.to_string(); // 临时使用 symbol 作为名称
                STOCKS.insert(stock);
            }
        }

        // 获取初始行情数据
        if !counters.is_empty() {
            let ctx = crate::openapi::quote();
            let symbols: Vec<String> = counters.iter().map(|c| c.as_str().to_string()).collect();

            // Use quote() to get full quote data (including prev_close)
            match ctx.quote(&symbols).await {
                Ok(quotes) => {
                    for quote in quotes {
                        match quote.symbol.parse() {
                            Ok(counter) => {
                                STOCKS.modify(counter, |stock| {
                                    // Update quote data with prev_close
                                    stock.quote.last_done = Some(quote.last_done);
                                    stock.quote.prev_close = Some(quote.prev_close);
                                    stock.quote.open = Some(quote.open);
                                    stock.quote.high = Some(quote.high);
                                    stock.quote.low = Some(quote.low);
                                    stock.quote.volume = quote.volume as u64;
                                    stock.quote.turnover = quote.turnover;
                                    stock.quote.timestamp = quote.timestamp.unix_timestamp();
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

            // 获取股票静态信息（包含名称等）
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
                    tracing::error!("获取股票静态信息失败: {}", e);
                }
            }
        }

        // SignalApp 已移除
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

        // Get full quote data (including prev_close)
        let ctx = crate::openapi::quote();
        if let Ok(quotes) = ctx.quote(&[counter.to_string()]).await {
            if let Some(quote) = quotes.first() {
                STOCKS.modify(counter.clone(), |stock| {
                    stock.quote.last_done = Some(quote.last_done);
                    stock.quote.prev_close = Some(quote.prev_close);
                    stock.quote.open = Some(quote.open);
                    stock.quote.high = Some(quote.high);
                    stock.quote.low = Some(quote.low);
                    stock.quote.volume = quote.volume as u64;
                    stock.quote.turnover = quote.turnover;
                    stock.quote.timestamp = quote.timestamp.unix_timestamp();
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

// Portfolio 数据全局存储
pub static PORTFOLIO_POSITIONS: Lazy<std::sync::RwLock<Vec<PositionInfo>>> =
    Lazy::new(|| std::sync::RwLock::new(Vec::new()));
pub static PORTFOLIO_BALANCE: Lazy<std::sync::RwLock<Decimal>> =
    Lazy::new(|| std::sync::RwLock::new(Decimal::ZERO));
pub static PORTFOLIO_MARKET_VALUE: Lazy<std::sync::RwLock<Decimal>> =
    Lazy::new(|| std::sync::RwLock::new(Decimal::ZERO));

// 刷新 Portfolio 数据
pub fn refresh_portfolio() {
    RT.get().unwrap().spawn(async move {
        tracing::info!("开始刷新 Portfolio 数据...");
        match fetch_portfolio_data().await {
            Ok((positions, balance, market_value)) => {
                tracing::info!(
                    "成功获取 Portfolio: {} 个持仓, 余额: {}, 市值: {}",
                    positions.len(),
                    balance,
                    market_value
                );

                *PORTFOLIO_POSITIONS.write().expect("poison") = positions;
                *PORTFOLIO_BALANCE.write().expect("poison") = balance;
                *PORTFOLIO_MARKET_VALUE.write().expect("poison") = market_value;
            }
            Err(e) => {
                tracing::error!("获取 Portfolio 数据失败: {}", e);
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
                let idx = WATCHLIST_TABLE.lock().expect("poison").selected();
                let len = WATCHLIST.read().expect("poison").counters().len();
                let new_idx = cycle::prev(idx, len);
                WATCHLIST_TABLE.lock().expect("poison").select(new_idx);

                // 立即更新股票详情
                if let Some(idx) = new_idx {
                    if let Some(counter) = WATCHLIST
                        .read()
                        .expect("poison")
                        .counters()
                        .get(idx)
                        .cloned()
                    {
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
                let idx = WATCHLIST_TABLE.lock().expect("poison").selected();
                let len = WATCHLIST.read().expect("poison").counters().len();
                let new_idx = cycle::next(idx, len);
                WATCHLIST_TABLE.lock().expect("poison").select(new_idx);

                // 立即更新股票详情
                if let Some(idx) = new_idx {
                    if let Some(counter) = WATCHLIST
                        .read()
                        .expect("poison")
                        .counters()
                        .get(idx)
                        .cloned()
                    {
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
        // 优先使用 last_done，如果没有则使用 prev_close
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
                // 有价格但没有昨收，显示价格但不显示涨跌
                (
                    price.format_quote_by_counter(counter),
                    EMPTY_PLACEHOLDER.to_string(),
                    EMPTY_PLACEHOLDER.to_string(),
                )
            }
            _ => {
                // 都没有，显示占位符
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
    if stock.trade_status.is_us_pre_post() || stock.trade_status.is_us_night() {
        titles.push(Span::raw(stock.trade_status.label()));
        titles.extend(price_spans(&stock.quote, counter));
    }

    let detail_container = Block::default()
        .title(Line::from(titles))
        .borders(Borders::ALL);

    // draw border
    frame.render_widget(detail_container, rect);

    // Helper function to format optional Decimal (价格类)
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
                // 有价格但没有昨收，显示但不着色
                let price_str = p.format_quote_by_counter(counter);
                item(label, price_str)
            }
            (None, Some(prev)) => {
                // 没有价格但有昨收，显示昨收但不着色
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

    // 构建详情列 - 第一列：基本交易数据
    let column0 = vec![
        ListItem::new(" "),
        item("交易状态".to_string(), stock.trade_status.label()),
        ListItem::new(" "),
        price_item("开盘价".to_string(), stock.quote.open),
        item("昨收价".to_string(), fmt_decimal(stock.quote.prev_close)),
        ListItem::new(" "),
        price_item("最高价".to_string(), stock.quote.high),
        price_item("最低价".to_string(), stock.quote.low),
        ListItem::new(" "),
        item("成交量".to_string(), fmt_u64(stock.quote.volume)),
        item(
            "成交额".to_string(),
            crate::ui::text::unit(stock.quote.turnover, 2),
        ),
        ListItem::new(" "),
    ];

    // 第二列：静态信息（如果有）
    let column1 = if let Some(ref info) = stock.static_info {
        vec![
            ListItem::new(" "),
            ListItem::new(" "),
            ListItem::new(" "),
            item("市盈率(TTM)".to_string(), fmt_decimal(info.eps_ttm)),
            item("每股收益".to_string(), fmt_decimal(info.eps)),
            ListItem::new(" "),
        ]
    } else {
        vec![
            ListItem::new(" "),
            ListItem::new(" "),
            ListItem::new(" "),
            item("市盈率(TTM)".to_string(), EMPTY_PLACEHOLDER),
            item("每股收益".to_string(), EMPTY_PLACEHOLDER),
            ListItem::new(" "),
        ]
    };

    // 第三列：更多静态信息
    let column2 = if let Some(ref info) = stock.static_info {
        vec![
            ListItem::new(" "),
            ListItem::new(" "),
            ListItem::new(" "),
            item("总股本".to_string(), fmt_i64(info.total_shares)),
            item("流通股本".to_string(), fmt_i64(info.circulating_shares)),
            ListItem::new(" "),
            item("每股净资产".to_string(), fmt_decimal(info.bps)),
            item("股息率".to_string(), fmt_decimal(info.dividend_yield)),
            ListItem::new(" "),
            ListItem::new(" "),
            item("每手股数".to_string(), info.lot_size.to_string()),
            ListItem::new(" "),
        ]
    } else {
        vec![
            ListItem::new(" "),
            ListItem::new(" "),
            ListItem::new(" "),
            item("总股本".to_string(), EMPTY_PLACEHOLDER),
            item("流通股本".to_string(), EMPTY_PLACEHOLDER),
            ListItem::new(" "),
            item("每股净资产".to_string(), EMPTY_PLACEHOLDER),
            item("股息率".to_string(), EMPTY_PLACEHOLDER),
            ListItem::new(" "),
            ListItem::new(" "),
            item("每手股数".to_string(), EMPTY_PLACEHOLDER),
            ListItem::new(" "),
        ]
    };

    // 渲染三列布局
    let column_height = column0.len().max(column1.len()).max(column2.len()) as u16;
    let chunks = Layout::default()
        .constraints([
            Constraint::Length(column_height),
            Constraint::Length(1),
            Constraint::Min(20),
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

    // 绘制盘口深度
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
        // 格式化单个深度档位
        let format_depth_line = |depth: &crate::data::Depth,
                                 counter: &Counter,
                                 prev_close: Option<Decimal>|
         -> Line<'static> {
            // 档位
            let position = Span::styled(
                format!("{:>2}:", depth.position),
                crate::ui::styles::label(),
            );
            // 价格
            let price_cmp = prev_close
                .map(|pc| depth.price.cmp(&pc))
                .unwrap_or(std::cmp::Ordering::Equal);
            let price_style = styles::up(price_cmp);
            let price = Span::styled(
                format!("{:>10} ", depth.price.format_quote_by_counter(counter)),
                price_style,
            );
            // 数量
            let volume = crate::ui::text::align_right(
                &crate::ui::text::unit(Decimal::from(depth.volume), 0),
                6,
            );
            // 订单数（仅港股显示）
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

        // 标题
        let bidding_title = format!(" 买盘: {:.1}%", bid_ratio * Decimal::from(100));
        let asking_title = format!(" 卖盘: {:.1}%", ask_ratio * Decimal::from(100));

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
                .title(" 成交记录 "),
            trades_area,
        );

        let inner_area = trades_area.inner(&Margin {
            vertical: 1,
            horizontal: 2,
        });

        if stock.trades.is_empty() {
            // 显示加载提示
            frame.render_widget(
                Paragraph::new("加载成交记录中...").alignment(Alignment::Center),
                inner_area,
            );
        } else {
            // 格式化成交记录
            let trade_lines: Vec<Line> = stock
                .trades
                .iter()
                .take(inner_area.height as usize)
                .map(|trade| {
                    // 简化时间显示
                    let time_str = time::OffsetDateTime::from_unix_timestamp(trade.timestamp)
                        .ok()
                        .and_then(|dt| {
                            let format =
                                time::format_description::parse("[hour]:[minute]:[second]").ok()?;
                            dt.format(&format).ok()
                        })
                        .unwrap_or_else(|| "--:--:--".to_string());

                    // 根据方向设置样式
                    let (price_style, direction_symbol) = match trade.direction {
                        crate::data::TradeDirection::Up => {
                            (styles::up(std::cmp::Ordering::Greater), "↑")
                        }
                        crate::data::TradeDirection::Down => {
                            (styles::up(std::cmp::Ordering::Less), "↓")
                        }
                        crate::data::TradeDirection::Neutral => (Style::default(), " "),
                    };

                    Line::from(vec![
                        Span::styled(format!("{} ", time_str), crate::ui::styles::label()),
                        Span::styled(direction_symbol, price_style),
                        Span::styled(
                            format!(" {:>8} ", trade.price.format_quote_by_counter(counter)),
                            price_style,
                        ),
                        Span::styled(
                            crate::ui::text::align_right(
                                &crate::ui::text::unit(Decimal::from(trade.volume), 0),
                                8,
                            ),
                            Style::default(),
                        ),
                    ])
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
                let idx = WATCHLIST_TABLE.lock().expect("poison").selected();
                let len = WATCHLIST.read().expect("poison").counters().len();
                WATCHLIST_TABLE
                    .lock()
                    .expect("poison")
                    .select(cycle::prev(idx, len));
            }
            Key::Down => {
                let idx = WATCHLIST_TABLE.lock().expect("poison").selected();
                let len = WATCHLIST.read().expect("poison").counters().len();
                WATCHLIST_TABLE
                    .lock()
                    .expect("poison")
                    .select(cycle::next(idx, len));
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
    let watchlist = WATCHLIST.read().expect("poison");
    let background = Block::default().borders(Borders::ALL).title(format!(
        " {} ─── {} [g] ",
        t!("Watchlist"),
        watchlist.group().map_or("--", |g| &g.name)
    ));
    frame.render_widget(background, rect);

    let selected = WATCHLIST_TABLE.lock().expect("poison").selected();
    frame.render_stateful_widget(
        watch_group_table(
            &watchlist.counters(),
            selected,
            &mut LAST_DONE.lock().expect("poison"),
            full_mode,
        ),
        rect.inner(&Margin {
            vertical: 2,
            horizontal: 2,
        }),
        &mut WATCHLIST_TABLE.lock().expect("poison"),
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

            // 价格和涨跌幅使用前景色，不使用背景色以避免干扰
            let style = styles::up(increase.sign());
            let trade_status_name = stock.trade_status.label();
            let increase_percent_str = if stock.trade_status == TradeStatus::STOP
                || stock.trade_status == TradeStatus::UsStop
            {
                trade_status_name.to_string()
            } else if increase.positive() {
                format!("+{}", &increase_percent)
            } else {
                increase_percent.to_string()
            };
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
                // 成交量：格式化为简短格式（如 1.23M）
                let volume_text = crate::helper::format_volume(quote_data.volume);
                cells.push(Cell::from(crate::ui::text::align_right(
                    &volume_text,
                    COLUMN_WIDTHS[4],
                )));
                cells.push(Cell::from(trade_status_name));
            }
            Row::new(cells)
        })
        .collect::<Vec<Row<'static>>>();

    let highlight_style = selected
        .map(|i| {
            let increase = if let Some(Some(stock)) = stocks.get(i) {
                let quote_data = &stock.quote;
                // 优先使用 last_done，如果没有则使用 prev_close
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
    mut events: EventReader<Key>,
    _portfolio: Res<Portfolio>,
    _accounts: Res<Select<Account>>,
    _command: Res<Command>,
    (state, indexes, ws): NavFooter,
    (mut account, mut currency, mut search, mut watchgroup): PopUp,
    _table_state: Local<TableState>,
) {
    // 处理按键事件
    for _event in &mut events {
        // 按键处理在 handle_global_keys 中统一处理
    }

    // 渲染界面
    _ = terminal.draw(|frame| {
        let rect = frame.size();

        // 顶部导航栏
        let top = Rect { height: 1, ..rect };
        crate::views::navbar::render(frame, top, *state.get());

        // 底部状态栏
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
        let positions = PORTFOLIO_POSITIONS.read().expect("poison");
        let balance = *PORTFOLIO_BALANCE.read().expect("poison");
        let market_value = *PORTFOLIO_MARKET_VALUE.read().expect("poison");
        let total_assets = balance + market_value;

        // 创建布局
        let chunks = Layout::default()
            .constraints([Constraint::Length(8), Constraint::Min(10)])
            .direction(Direction::Vertical)
            .split(content_rect);

        // 顶部：账户概览
        {
            let overview_block = Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", t!("Portfolio.Title")));

            // 计算总盈亏
            let total_profit_loss: Decimal = positions.iter().map(|p| p.profit_loss).sum();
            let pl_style = styles::up(total_profit_loss.cmp(&Decimal::ZERO));

            // 创建两列布局
            let inner_area = overview_block.inner(chunks[0]);
            frame.render_widget(overview_block, chunks[0]);

            let inner_chunks = Layout::default()
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .direction(Direction::Horizontal)
                .split(inner_area);

            // 左列
            let left_items = vec![
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{}: ", t!("Portfolio.Total Asset")),
                        styles::label(),
                    ),
                    Span::styled(format!("{:.2}", total_assets), styles::text()),
                ])),
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{}: ", t!("Portfolio.Market Cap")), styles::label()),
                    Span::styled(format!("{:.2}", market_value), styles::text()),
                ])),
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{}: ", t!("Portfolio.P/L")), styles::label()),
                    Span::styled(format!("{:+.2}", total_profit_loss), pl_style),
                ])),
            ];

            // 右列
            let right_items = vec![
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{}: ", t!("Portfolio.Total Cash Amount")),
                        styles::label(),
                    ),
                    Span::styled(format!("{:.2}", balance), styles::text()),
                ])),
                ListItem::new(Line::from(vec![
                    Span::styled(format!("持仓数量: "), styles::label()),
                    Span::styled(format!("{}", positions.len()), styles::text()),
                ])),
                ListItem::new(""),
                ListItem::new(Span::styled(
                    "按 R 刷新数据",
                    Style::default().fg(Color::Gray),
                )),
            ];

            let left_list = List::new(left_items);
            let right_list = List::new(right_items);

            frame.render_widget(left_list, inner_chunks[0]);
            frame.render_widget(right_list, inner_chunks[1]);
        }

        // 底部：持仓列表
        {
            let holdings_block = Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", t!("Holding.Holding")));

            if positions.is_empty() {
                let message = Paragraph::new(vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        "暂无持仓数据",
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

                let rows: Vec<Row> = positions
                    .iter()
                    .map(|pos| {
                        let counter = &pos.symbol;

                        // 计算盈亏样式
                        let pl_style = styles::up(pos.profit_loss.cmp(&Decimal::ZERO));

                        Row::new(vec![
                            Cell::from(Line::from(vec![
                                Span::styled(
                                    counter.market().to_string(),
                                    styles::market(counter.region()),
                                ),
                                Span::raw(" "),
                                Span::raw(counter.code().to_string()),
                            ])),
                            Cell::from(pos.symbol_name.clone()),
                            Cell::from(format!("{:.0}", pos.quantity)),
                            Cell::from(format!("{:.2}", pos.current_price)),
                            Cell::from(format!("{:.2}", pos.cost_price)),
                            Cell::from(format!("{:.2}", pos.market_value)),
                            Cell::from(format!("{:+.2}", pos.profit_loss)).style(pl_style),
                            Cell::from(format!("{:+.2}%", pos.profit_loss_percent)).style(pl_style),
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
