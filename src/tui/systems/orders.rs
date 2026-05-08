use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, RwLock};

use bevy_ecs::{
    prelude::*,
    system::{CommandQueue, InsertResource},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use ratatui::{
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, Paragraph, Row, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Table,
    },
    Frame,
};
use rust_decimal::Decimal;
use std::str::FromStr;

use crate::{
    tui::app::{AppState, POPUP, POPUP_CANCEL_ORDER, POPUP_DATE_FILTER, POPUP_REPLACE_ORDER, RT},
    tui::ui::styles,
    tui::widgets::{
        toast::{set_toast, ToastKind},
        Terminal,
    },
};

use super::{Command, NavFooter, PopUp, StockDetail};

// ──────────────────────────── mode & history state ──────────────────────────

/// false = Today mode, true = History mode
pub static ORDERS_MODE: AtomicBool = AtomicBool::new(false);

pub static ORDERS_VIEW: LazyLock<RwLock<Vec<longbridge::trade::Order>>> =
    LazyLock::new(|| RwLock::new(vec![]));

pub static HISTORY_ORDERS_VIEW: LazyLock<RwLock<Vec<longbridge::trade::Order>>> =
    LazyLock::new(|| RwLock::new(vec![]));

pub static HISTORY_DATE_RANGE: LazyLock<RwLock<HistoryDateRange>> =
    LazyLock::new(|| RwLock::new(HistoryDateRange::default()));

pub static DATE_FILTER_STATE: LazyLock<RwLock<DateFilterState>> =
    LazyLock::new(|| RwLock::new(DateFilterState::default()));

pub static ORDER_ENTRY_STATE: LazyLock<RwLock<Option<OrderEntryState>>> =
    LazyLock::new(|| RwLock::new(None));

pub static REPLACE_ORDER_STATE: LazyLock<RwLock<Option<ReplaceOrderState>>> =
    LazyLock::new(|| RwLock::new(None));

pub static CANCEL_TARGET: LazyLock<RwLock<Option<longbridge::trade::Order>>> =
    LazyLock::new(|| RwLock::new(None));

pub static ORDERS_TABLE: LazyLock<Mutex<TableState>> = LazyLock::new(Mutex::default);
pub static HISTORY_ORDERS_TABLE: LazyLock<Mutex<TableState>> = LazyLock::new(Mutex::default);

// ────────────────────────────── state structs ───────────────────────────────

pub struct HistoryDateRange {
    pub start: String,
    pub end: String,
}

impl Default for HistoryDateRange {
    fn default() -> Self {
        let today = time::OffsetDateTime::now_utc().date();
        let fmt = time::macros::format_description!("[year]-[month]-[day]");
        let end = today.format(&fmt).unwrap_or_else(|_| today.to_string());
        let start_date = today - time::Duration::days(365);
        let start = start_date
            .format(&fmt)
            .unwrap_or_else(|_| start_date.to_string());
        Self { start, end }
    }
}

pub struct DateFilterState {
    pub start_input: tui_input::Input,
    pub end_input: tui_input::Input,
    pub focused: DateFilterField,
}

impl Default for DateFilterState {
    fn default() -> Self {
        let range = HISTORY_DATE_RANGE.read().expect("poison");
        Self {
            start_input: tui_input::Input::new(range.start.clone()),
            end_input: tui_input::Input::new(range.end.clone()),
            focused: DateFilterField::Start,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DateFilterField {
    Start,
    End,
}

pub struct OrderEntryState {
    pub symbol: String,
    pub side: longbridge::trade::OrderSide,
    pub order_type: longbridge::trade::OrderType,
    pub quantity_input: tui_input::Input,
    pub price_input: tui_input::Input,
    pub tif: longbridge::trade::TimeInForceType,
    pub focused_field: OrderEntryField,
    pub max_qty: Option<Decimal>,
    pub confirm_button: ConfirmButton,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OrderEntryField {
    OrderType,
    Quantity,
    Price,
    Tif,
    Buttons,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConfirmButton {
    Submit,
    Cancel,
}

const ORDER_TYPES: &[longbridge::trade::OrderType] = &[
    longbridge::trade::OrderType::LO,
    longbridge::trade::OrderType::ELO,
    longbridge::trade::OrderType::MO,
    longbridge::trade::OrderType::AO,
    longbridge::trade::OrderType::ALO,
];

const TIF_TYPES: &[longbridge::trade::TimeInForceType] = &[
    longbridge::trade::TimeInForceType::Day,
    longbridge::trade::TimeInForceType::GoodTilCanceled,
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReplaceOrderField {
    Qty,
    Price,
}

pub struct ReplaceOrderState {
    pub order_id: String,
    pub qty_input: tui_input::Input,
    pub price_input: tui_input::Input,
    pub focused: ReplaceOrderField,
    pub confirming: bool,
}

// ─────────────────────────────── public API ─────────────────────────────────

pub fn open_order_entry(
    symbol: String,
    side: longbridge::trade::OrderSide,
    available_qty: Option<Decimal>,
) {
    let price_str = crate::data::STOCKS
        .get(&crate::data::Counter::new(&symbol))
        .and_then(|s| s.quote.last_done)
        .map(|p| p.to_string())
        .unwrap_or_default();

    let state = OrderEntryState {
        symbol: symbol.clone(),
        side,
        order_type: longbridge::trade::OrderType::LO,
        quantity_input: tui_input::Input::default(),
        price_input: if price_str.is_empty() {
            tui_input::Input::default()
        } else {
            tui_input::Input::new(price_str)
        },
        tif: longbridge::trade::TimeInForceType::Day,
        focused_field: OrderEntryField::Quantity,
        max_qty: available_qty,
        confirm_button: ConfirmButton::Submit,
    };
    *ORDER_ENTRY_STATE.write().expect("poison") = Some(state);

    if side == longbridge::trade::OrderSide::Buy {
        fetch_max_qty(symbol, longbridge::trade::OrderType::LO, None);
    }
}

pub fn refresh_orders() {
    RT.get().unwrap().spawn(async move {
        let ctx = crate::openapi::trade();
        match ctx
            .today_orders(longbridge::trade::GetTodayOrdersOptions::new())
            .await
        {
            Ok(orders) => {
                *ORDERS_VIEW.write().expect("poison") = orders;
            }
            Err(e) => {
                set_toast(
                    ToastKind::Error,
                    format!("{}: {e}", t!("Trade.FailedLoadOrders")),
                );
            }
        }
    });
}

pub fn refresh_history_orders() {
    let range = {
        let r = HISTORY_DATE_RANGE.read().expect("poison");
        (r.start.clone(), r.end.clone())
    };
    RT.get().unwrap().spawn(async move {
        let ctx = crate::openapi::trade();
        let fmt = time::macros::format_description!("[year]-[month]-[day]");
        let mut opts = longbridge::trade::GetHistoryOrdersOptions::new();
        if let Ok(date) = time::Date::parse(&range.0, &fmt) {
            opts = opts.start_at(date.with_time(time::Time::MIDNIGHT).assume_utc());
        }
        if let Ok(date) = time::Date::parse(&range.1, &fmt) {
            let end_time = time::Time::from_hms(23, 59, 59).expect("valid time");
            opts = opts.end_at(date.with_time(end_time).assume_utc());
        }
        match ctx.history_orders(opts).await {
            Ok(orders) => {
                *HISTORY_ORDERS_VIEW.write().expect("poison") = orders;
            }
            Err(e) => {
                set_toast(
                    ToastKind::Error,
                    format!("{}: {e}", t!("Trade.FailedLoadHistoryOrders")),
                );
            }
        }
    });
}

pub fn toggle_orders_mode() {
    let new_history = !ORDERS_MODE.load(Ordering::Relaxed);
    ORDERS_MODE.store(new_history, Ordering::Relaxed);
}

pub fn open_date_filter() {
    let range = HISTORY_DATE_RANGE.read().expect("poison");
    *DATE_FILTER_STATE.write().expect("poison") = DateFilterState {
        start_input: tui_input::Input::new(range.start.clone()),
        end_input: tui_input::Input::new(range.end.clone()),
        focused: DateFilterField::Start,
    };
    POPUP.store(POPUP_DATE_FILTER, Ordering::Relaxed);
}

pub fn apply_date_filter() {
    let (start, end) = {
        let s = DATE_FILTER_STATE.read().expect("poison");
        (
            s.start_input.value().to_string(),
            s.end_input.value().to_string(),
        )
    };
    {
        let mut range = HISTORY_DATE_RANGE.write().expect("poison");
        range.start = start;
        range.end = end;
    }
    POPUP.store(0, Ordering::Relaxed);
    refresh_history_orders();
}

pub fn submit_order() {
    let state = {
        let lock = ORDER_ENTRY_STATE.read().expect("poison");
        lock.as_ref().map(|s| {
            (
                s.symbol.clone(),
                s.side,
                s.order_type,
                s.quantity_input.value().to_string(),
                s.price_input.value().to_string(),
                s.tif,
            )
        })
    };
    let Some((symbol, side, order_type, qty_str, price_str, tif)) = state else {
        return;
    };
    let qty = Decimal::from_str(&qty_str).unwrap_or_default();
    RT.get().unwrap().spawn(async move {
        let mut opts =
            longbridge::trade::SubmitOrderOptions::new(symbol, order_type, side, qty, tif);
        let price_only_for_lo = matches!(
            order_type,
            longbridge::trade::OrderType::LO | longbridge::trade::OrderType::ELO
        );
        if price_only_for_lo {
            if let Ok(price) = Decimal::from_str(&price_str) {
                opts = opts.submitted_price(price);
            }
        }
        let ctx = crate::openapi::trade();
        match ctx.submit_order(opts).await {
            Ok(resp) => {
                set_toast(
                    ToastKind::Success,
                    format!("{}: {}", t!("Trade.OrderSubmitted"), resp.order_id),
                );
                *ORDER_ENTRY_STATE.write().expect("poison") = None;
                POPUP.store(0, Ordering::Relaxed);
                refresh_orders();
            }
            Err(e) => {
                set_toast(ToastKind::Error, e.to_string());
            }
        }
    });
}

pub fn cancel_order(order_id: String) {
    RT.get().unwrap().spawn(async move {
        let ctx = crate::openapi::trade();
        match ctx.cancel_order(order_id.clone()).await {
            Ok(()) => {
                set_toast(
                    ToastKind::Success,
                    format!("{}: {}", t!("Trade.OrderCancelled"), order_id),
                );
                POPUP.store(0, Ordering::Relaxed);
                *CANCEL_TARGET.write().expect("poison") = None;
                refresh_orders();
            }
            Err(e) => {
                set_toast(ToastKind::Error, e.to_string());
                POPUP.store(0, Ordering::Relaxed);
                *CANCEL_TARGET.write().expect("poison") = None;
            }
        }
    });
}

pub fn replace_order() {
    let state = {
        let lock = REPLACE_ORDER_STATE.read().expect("poison");
        lock.as_ref().map(|s| {
            (
                s.order_id.clone(),
                s.qty_input.value().to_string(),
                s.price_input.value().to_string(),
            )
        })
    };
    let Some((order_id, qty_str, price_str)) = state else {
        return;
    };
    RT.get().unwrap().spawn(async move {
        let mut opts = longbridge::trade::ReplaceOrderOptions::new(
            order_id.clone(),
            Decimal::from_str(&qty_str).unwrap_or(Decimal::ONE),
        );
        if let Ok(price) = Decimal::from_str(&price_str) {
            opts = opts.price(price);
        }
        let ctx = crate::openapi::trade();
        match ctx.replace_order(opts).await {
            Ok(()) => {
                set_toast(
                    ToastKind::Success,
                    format!("{}: {}", t!("Trade.OrderReplaced"), order_id),
                );
                POPUP.store(0, Ordering::Relaxed);
                *REPLACE_ORDER_STATE.write().expect("poison") = None;
                refresh_orders();
            }
            Err(e) => {
                set_toast(ToastKind::Error, e.to_string());
                if let Some(s) = REPLACE_ORDER_STATE.write().expect("poison").as_mut() {
                    s.confirming = false;
                }
            }
        }
    });
}

pub fn fetch_max_qty(
    symbol: String,
    order_type: longbridge::trade::OrderType,
    price: Option<Decimal>,
) {
    RT.get().unwrap().spawn(async move {
        let ctx = crate::openapi::trade();
        let mut opts = longbridge::trade::EstimateMaxPurchaseQuantityOptions::new(
            symbol,
            order_type,
            longbridge::trade::OrderSide::Buy,
        );
        if let Some(p) = price {
            opts = opts.price(p);
        }
        if let Ok(resp) = ctx.estimate_max_purchase_quantity(opts).await {
            if let Some(state) = ORDER_ENTRY_STATE.write().expect("poison").as_mut() {
                state.max_qty = Some(resp.cash_max_qty);
            }
        }
    });
}

// ─────────────────────────── keyboard handlers ──────────────────────────────

fn order_entry_next_field(
    current: OrderEntryField,
    order_type: longbridge::trade::OrderType,
) -> OrderEntryField {
    let price_editable = matches!(
        order_type,
        longbridge::trade::OrderType::LO | longbridge::trade::OrderType::ELO
    );
    match current {
        OrderEntryField::OrderType => OrderEntryField::Quantity,
        OrderEntryField::Quantity => {
            if price_editable {
                OrderEntryField::Price
            } else {
                OrderEntryField::Tif
            }
        }
        OrderEntryField::Price => OrderEntryField::Tif,
        OrderEntryField::Tif => OrderEntryField::Buttons,
        OrderEntryField::Buttons => OrderEntryField::OrderType,
    }
}

fn order_entry_prev_field(
    current: OrderEntryField,
    order_type: longbridge::trade::OrderType,
) -> OrderEntryField {
    let price_editable = matches!(
        order_type,
        longbridge::trade::OrderType::LO | longbridge::trade::OrderType::ELO
    );
    match current {
        OrderEntryField::OrderType => OrderEntryField::Buttons,
        OrderEntryField::Quantity => OrderEntryField::OrderType,
        OrderEntryField::Price => OrderEntryField::Quantity,
        OrderEntryField::Tif => {
            if price_editable {
                OrderEntryField::Price
            } else {
                OrderEntryField::Quantity
            }
        }
        OrderEntryField::Buttons => OrderEntryField::Tif,
    }
}

fn cycle_order_type(
    current: longbridge::trade::OrderType,
    forward: bool,
) -> longbridge::trade::OrderType {
    let idx = ORDER_TYPES.iter().position(|&t| t == current).unwrap_or(0);
    let len = ORDER_TYPES.len();
    let new_idx = if forward {
        (idx + 1) % len
    } else {
        (idx + len - 1) % len
    };
    ORDER_TYPES[new_idx]
}

fn cycle_tif(
    current: longbridge::trade::TimeInForceType,
    forward: bool,
) -> longbridge::trade::TimeInForceType {
    let idx = TIF_TYPES.iter().position(|&t| t == current).unwrap_or(0);
    let len = TIF_TYPES.len();
    let new_idx = if forward {
        (idx + 1) % len
    } else {
        (idx + len - 1) % len
    };
    TIF_TYPES[new_idx]
}

pub fn handle_order_entry_key(event: KeyEvent) {
    let close = || {
        POPUP.store(0, Ordering::Relaxed);
        *ORDER_ENTRY_STATE.write().expect("poison") = None;
    };

    match event {
        KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            close();
        }
        KeyEvent {
            code: KeyCode::Tab | KeyCode::Down | KeyCode::Char('j'),
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            if let Some(s) = ORDER_ENTRY_STATE.write().expect("poison").as_mut() {
                s.focused_field = order_entry_next_field(s.focused_field, s.order_type);
            }
        }
        KeyEvent {
            code: KeyCode::BackTab | KeyCode::Up | KeyCode::Char('k'),
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            if let Some(s) = ORDER_ENTRY_STATE.write().expect("poison").as_mut() {
                s.focused_field = order_entry_prev_field(s.focused_field, s.order_type);
            }
        }
        KeyEvent {
            code: KeyCode::Left,
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            if let Some(s) = ORDER_ENTRY_STATE.write().expect("poison").as_mut() {
                match s.focused_field {
                    OrderEntryField::OrderType => {
                        s.order_type = cycle_order_type(s.order_type, false);
                    }
                    OrderEntryField::Tif => {
                        s.tif = cycle_tif(s.tif, false);
                    }
                    OrderEntryField::Buttons => {
                        s.confirm_button = ConfirmButton::Submit;
                    }
                    _ => {}
                }
            }
        }
        KeyEvent {
            code: KeyCode::Right,
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            if let Some(s) = ORDER_ENTRY_STATE.write().expect("poison").as_mut() {
                match s.focused_field {
                    OrderEntryField::OrderType => {
                        s.order_type = cycle_order_type(s.order_type, true);
                    }
                    OrderEntryField::Tif => {
                        s.tif = cycle_tif(s.tif, true);
                    }
                    OrderEntryField::Buttons => {
                        s.confirm_button = ConfirmButton::Cancel;
                    }
                    _ => {}
                }
            }
        }
        KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            let (focused, btn) = ORDER_ENTRY_STATE
                .read()
                .expect("poison")
                .as_ref()
                .map_or((OrderEntryField::Quantity, ConfirmButton::Submit), |s| {
                    (s.focused_field, s.confirm_button)
                });
            if focused == OrderEntryField::Buttons {
                match btn {
                    ConfirmButton::Submit => submit_order(),
                    ConfirmButton::Cancel => close(),
                }
            } else if let Some(s) = ORDER_ENTRY_STATE.write().expect("poison").as_mut() {
                s.focused_field = order_entry_next_field(s.focused_field, s.order_type);
            }
        }
        KeyEvent {
            code: KeyCode::Char('='),
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            if let Some(s) = ORDER_ENTRY_STATE.write().expect("poison").as_mut() {
                let price_str = crate::data::STOCKS
                    .get(&crate::data::Counter::new(&s.symbol))
                    .and_then(|st| st.quote.last_done)
                    .map(|p| p.to_string())
                    .unwrap_or_default();
                if !price_str.is_empty() {
                    s.price_input = tui_input::Input::new(price_str);
                }
            }
        }
        _ => {
            let evt = crossterm::event::Event::Key(event);
            if let Some(req) = tui_input::backend::crossterm::to_input_request(&evt) {
                if let Some(s) = ORDER_ENTRY_STATE.write().expect("poison").as_mut() {
                    match s.focused_field {
                        OrderEntryField::Quantity => {
                            s.quantity_input.handle(req);
                        }
                        OrderEntryField::Price => {
                            s.price_input.handle(req);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

pub fn handle_cancel_order_key(event: KeyEvent) {
    match event {
        KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            let order_id = CANCEL_TARGET
                .read()
                .expect("poison")
                .as_ref()
                .map(|o| o.order_id.clone());
            if let Some(id) = order_id {
                cancel_order(id);
            }
        }
        KeyEvent {
            code: KeyCode::Esc, ..
        } => {
            POPUP.store(0, Ordering::Relaxed);
            *CANCEL_TARGET.write().expect("poison") = None;
        }
        _ => {}
    }
}

pub fn handle_replace_order_key(event: KeyEvent) {
    match event {
        KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            POPUP.store(0, Ordering::Relaxed);
            *REPLACE_ORDER_STATE.write().expect("poison") = None;
        }
        KeyEvent {
            code: KeyCode::Tab,
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            if let Some(s) = REPLACE_ORDER_STATE.write().expect("poison").as_mut() {
                if !s.confirming {
                    s.focused = match s.focused {
                        ReplaceOrderField::Qty => ReplaceOrderField::Price,
                        ReplaceOrderField::Price => ReplaceOrderField::Qty,
                    };
                }
            }
        }
        KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            let confirming = REPLACE_ORDER_STATE
                .read()
                .expect("poison")
                .as_ref()
                .is_some_and(|s| s.confirming);
            if confirming {
                replace_order();
            } else if let Some(s) = REPLACE_ORDER_STATE.write().expect("poison").as_mut() {
                s.confirming = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('y'),
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            let confirming = REPLACE_ORDER_STATE
                .read()
                .expect("poison")
                .as_ref()
                .is_some_and(|s| s.confirming);
            if confirming {
                replace_order();
            }
        }
        KeyEvent {
            code: KeyCode::Char('n'),
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            if let Some(s) = REPLACE_ORDER_STATE.write().expect("poison").as_mut() {
                if s.confirming {
                    s.confirming = false;
                }
            }
        }
        _ => {
            let evt = crossterm::event::Event::Key(event);
            if let Some(req) = tui_input::backend::crossterm::to_input_request(&evt) {
                if let Some(s) = REPLACE_ORDER_STATE.write().expect("poison").as_mut() {
                    if !s.confirming {
                        match s.focused {
                            ReplaceOrderField::Qty => {
                                s.qty_input.handle(req);
                            }
                            ReplaceOrderField::Price => {
                                s.price_input.handle(req);
                            }
                        }
                    }
                }
            }
        }
    }
}

pub fn handle_date_filter_key(event: KeyEvent) {
    match event {
        KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            POPUP.store(0, Ordering::Relaxed);
        }
        KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            apply_date_filter();
        }
        KeyEvent {
            code: KeyCode::Tab | KeyCode::Down | KeyCode::BackTab | KeyCode::Up,
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            let mut s = DATE_FILTER_STATE.write().expect("poison");
            s.focused = match s.focused {
                DateFilterField::Start => DateFilterField::End,
                DateFilterField::End => DateFilterField::Start,
            };
        }
        _ => {
            let evt = crossterm::event::Event::Key(event);
            if let Some(req) = tui_input::backend::crossterm::to_input_request(&evt) {
                let mut s = DATE_FILTER_STATE.write().expect("poison");
                match s.focused {
                    DateFilterField::Start => {
                        s.start_input.handle(req);
                    }
                    DateFilterField::End => {
                        s.end_input.handle(req);
                    }
                }
            }
        }
    }
}

// ─────────────────────────── Bevy ECS systems ───────────────────────────────

pub fn enter_orders() {
    ORDERS_TABLE.lock().expect("poison").select(Some(0));
    refresh_orders();
    refresh_history_orders();
}

pub fn exit_orders() {
    crate::tui::app::LAST_STATE.store(AppState::Orders, Ordering::Relaxed);
}

pub fn render_orders(
    mut terminal: ResMut<Terminal>,
    mut events: EventReader<super::Key>,
    command: Res<Command>,
    (state, indexes, ws): NavFooter,
    (mut account, mut currency, mut search, mut watchgroup, mut watchlist_search): PopUp,
    mut log_panel: Local<crate::tui::widgets::LogPanel>,
) {
    for event in &mut events {
        let is_history = ORDERS_MODE.load(Ordering::Relaxed);
        let orders_len = if is_history {
            HISTORY_ORDERS_VIEW.read().expect("poison").len()
        } else {
            ORDERS_VIEW.read().expect("poison").len()
        };
        match event {
            super::Key::Tab => {
                toggle_orders_mode();
            }
            super::Key::Up => {
                if orders_len > 0 {
                    if is_history {
                        let mut table = HISTORY_ORDERS_TABLE.lock().expect("poison");
                        let cur = table.selected();
                        table.select(Some(cur.map_or(0, |i| i.saturating_sub(1))));
                    } else {
                        let mut table = ORDERS_TABLE.lock().expect("poison");
                        let cur = table.selected();
                        table.select(Some(cur.map_or(0, |i| i.saturating_sub(1))));
                    }
                }
            }
            super::Key::Down => {
                if orders_len > 0 {
                    if is_history {
                        let mut table = HISTORY_ORDERS_TABLE.lock().expect("poison");
                        let cur = table.selected();
                        table.select(Some(cur.map_or(0, |i| {
                            if i + 1 < orders_len {
                                i + 1
                            } else {
                                i
                            }
                        })));
                    } else {
                        let mut table = ORDERS_TABLE.lock().expect("poison");
                        let cur = table.selected();
                        table.select(Some(cur.map_or(0, |i| {
                            if i + 1 < orders_len {
                                i + 1
                            } else {
                                i
                            }
                        })));
                    }
                }
            }
            super::Key::Enter => {
                let selected = if is_history {
                    HISTORY_ORDERS_TABLE.lock().expect("poison").selected()
                } else {
                    ORDERS_TABLE.lock().expect("poison").selected()
                };
                if let Some(idx) = selected {
                    let symbol = if is_history {
                        HISTORY_ORDERS_VIEW
                            .read()
                            .expect("poison")
                            .get(idx)
                            .map(|o| o.symbol.clone())
                    } else {
                        ORDERS_VIEW
                            .read()
                            .expect("poison")
                            .get(idx)
                            .map(|o| o.symbol.clone())
                    };
                    if let Some(symbol) = symbol {
                        let counter = crate::data::Counter::new(&symbol);
                        let mut queue = CommandQueue::default();
                        queue.push(InsertResource {
                            resource: StockDetail(counter),
                        });
                        queue.push(InsertResource {
                            resource: NextState(Some(AppState::WatchlistStock)),
                        });
                        _ = command.0.send(queue);
                    }
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

        let content_rect = Rect {
            x: rect.x + 1,
            y: rect.y + 1,
            width: rect.width.saturating_sub(2),
            height: rect.height - 2,
        };

        render_orders_list(frame, content_rect);

        crate::tui::views::popup::render(
            frame,
            rect,
            &mut account,
            &mut currency,
            &mut search,
            &mut watchgroup,
            &mut watchlist_search,
        );

        crate::tui::widgets::toast::render_toast(frame, rect);

        let log_panel_visible = crate::tui::app::LOG_PANEL_VISIBLE.load(Ordering::Relaxed);
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

fn make_orders_table<'a>(
    orders: &'a [longbridge::trade::Order],
    is_history: bool,
    active: bool,
    title: String,
    bottom_hints: Option<Line<'a>>,
) -> (Table<'a>, bool) {
    let _ = active;
    let border_style = styles::border();
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);
    if let Some(hints) = bottom_hints {
        block = block.title_bottom(hints);
    }

    if orders.is_empty() {
        let empty_msg = if is_history {
            t!("Orders.NoHistoryOrders").to_string()
        } else {
            t!("Orders.NoOrders").to_string()
        };
        let table = Table::new(
            vec![Row::new(vec![Cell::from(Span::styled(
                empty_msg,
                Style::default().fg(Color::DarkGray),
            ))])],
            [Constraint::Percentage(100)],
        )
        .block(block);
        return (table, false);
    }

    let time_header = if is_history {
        t!("Orders.Date")
    } else {
        t!("Orders.SubmittedAt")
    };

    let header = Row::new(vec![
        Cell::from(t!("Orders.Symbol")).style(styles::header()),
        Cell::from(t!("Orders.Side")).style(styles::header()),
        Cell::from(t!("Orders.Type")).style(styles::header()),
        Cell::from(t!("Orders.Status")).style(styles::header()),
        Cell::from(t!("Orders.Qty")).style(styles::header()),
        Cell::from(t!("Orders.ExecQty")).style(styles::header()),
        Cell::from(t!("Orders.Price")).style(styles::header()),
        Cell::from(time_header).style(styles::header()),
    ]);

    let rows: Vec<Row> = orders
        .iter()
        .map(|order| {
            let status_style = order_status_style(order.status);
            let status_label = order_status_label(order.status);
            let side_label = match order.side {
                longbridge::trade::OrderSide::Buy => t!("Trade.Buy"),
                longbridge::trade::OrderSide::Sell => t!("Trade.Sell"),
                longbridge::trade::OrderSide::Unknown => std::borrow::Cow::Borrowed("–"),
            };
            let type_label = order_type_label(order.order_type);
            let price_str = order.price.map_or("–".to_string(), |p| format!("{p:.2}"));
            let t = order.submitted_at;
            let time_str = if is_history {
                format!("{:04}-{:02}-{:02}", t.year(), t.month() as u8, t.day())
            } else {
                format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second())
            };

            Row::new(vec![
                Cell::from(order.symbol.clone()),
                Cell::from(side_label),
                Cell::from(type_label),
                Cell::from(status_label).style(status_style),
                Cell::from(format!("{}", order.quantity)),
                Cell::from(format!("{}", order.executed_quantity)),
                Cell::from(price_str),
                Cell::from(time_str),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(12),
            Constraint::Percentage(6),
            Constraint::Percentage(6),
            Constraint::Percentage(14),
            Constraint::Percentage(8),
            Constraint::Percentage(8),
            Constraint::Percentage(12),
            Constraint::Percentage(10),
        ],
    )
    .header(header)
    .block(block)
    .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
    .column_spacing(1);

    (table, true)
}

fn render_orders_list(frame: &mut Frame, rect: Rect) {
    let is_history_active = ORDERS_MODE.load(Ordering::Relaxed);

    let today_guard = ORDERS_VIEW.read().expect("poison");
    let history_guard = HISTORY_ORDERS_VIEW.read().expect("poison");
    let today_orders: &[longbridge::trade::Order] = &today_guard;
    let history_orders: &[longbridge::trade::Order] = &history_guard;

    // Allocate height: today gets enough for its rows (capped), history gets the rest
    let today_height = {
        let preferred = today_orders.len() as u16 + 3; // 2 borders + 1 header
        preferred.clamp(6, 8) // min 3 data rows, max 5 data rows
    };
    let [today_rect, history_rect] =
        Layout::vertical([Constraint::Length(today_height), Constraint::Min(4)]).areas(rect);

    *crate::tui::mouse::ORDERS_TABLE_RECT.lock().expect("poison") = today_rect;
    *crate::tui::mouse::HISTORY_ORDERS_TABLE_RECT
        .lock()
        .expect("poison") = history_rect;

    // Today table title
    let today_title = if today_orders.is_empty() {
        format!(" {} ", t!("Orders.TodayTab"))
    } else {
        format!(" {} ({}) ", t!("Orders.TodayTab"), today_orders.len())
    };

    // History table title + bottom hints
    let range = HISTORY_DATE_RANGE.read().expect("poison");
    let history_title = if history_orders.is_empty() {
        format!(
            " {} {} ~ {} ",
            t!("Orders.HistoryTab"),
            range.start,
            range.end
        )
    } else {
        format!(
            " {} {} ~ {} ({}) ",
            t!("Orders.HistoryTab"),
            range.start,
            range.end,
            history_orders.len()
        )
    };
    drop(range);

    let bottom_hints = Line::from(vec![
        Span::styled(format!(" {} ", t!("Orders.Refresh")), styles::dark_gray()),
        Span::styled(format!(" {} ", t!("Orders.CancelKey")), styles::dark_gray()),
        Span::styled(
            format!(" {} ", t!("Orders.ReplaceKey")),
            styles::dark_gray(),
        ),
        Span::styled(format!(" {} ", t!("Orders.FilterKey")), styles::dark_gray()),
        Span::styled(format!(" {} ", t!("Orders.TabSwitch")), styles::dark_gray()),
    ])
    .right_aligned();

    let (today_table, today_has_rows) =
        make_orders_table(today_orders, false, !is_history_active, today_title, None);
    let (history_table, history_has_rows) = make_orders_table(
        history_orders,
        true,
        is_history_active,
        history_title,
        Some(bottom_hints),
    );

    let mut today_state = ORDERS_TABLE.lock().expect("poison");
    let mut history_state = HISTORY_ORDERS_TABLE.lock().expect("poison");

    if today_has_rows {
        if is_history_active {
            frame.render_stateful_widget(today_table, today_rect, &mut TableState::default());
        } else {
            frame.render_stateful_widget(today_table, today_rect, &mut *today_state);
        }
        let inner = today_rect.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });
        let scrollbar_area = Rect {
            x: inner.x + inner.width,
            y: inner.y,
            width: 1,
            height: inner.height,
        };
        let mut sb =
            ScrollbarState::new(today_orders.len()).position(today_state.selected().unwrap_or(0));
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(None)
                .thumb_symbol("▐")
                .thumb_style(Style::default().fg(Color::DarkGray)),
            scrollbar_area,
            &mut sb,
        );
    } else {
        frame.render_widget(today_table, today_rect);
    }
    if history_has_rows {
        if is_history_active {
            frame.render_stateful_widget(history_table, history_rect, &mut *history_state);
        } else {
            frame.render_stateful_widget(history_table, history_rect, &mut TableState::default());
        }
        let inner = history_rect.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });
        let scrollbar_area = Rect {
            x: inner.x + inner.width,
            y: inner.y,
            width: 1,
            height: inner.height,
        };
        let mut sb = ScrollbarState::new(history_orders.len())
            .position(history_state.selected().unwrap_or(0));
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(None)
                .thumb_symbol("▐")
                .thumb_style(Style::default().fg(Color::DarkGray)),
            scrollbar_area,
            &mut sb,
        );
    } else {
        frame.render_widget(history_table, history_rect);
    }
}

fn order_status_style(status: longbridge::trade::OrderStatus) -> Style {
    match status {
        longbridge::trade::OrderStatus::NotReported
        | longbridge::trade::OrderStatus::ReplacedNotReported
        | longbridge::trade::OrderStatus::ProtectedNotReported
        | longbridge::trade::OrderStatus::VarietiesNotReported
        | longbridge::trade::OrderStatus::WaitToNew
        | longbridge::trade::OrderStatus::New
        | longbridge::trade::OrderStatus::WaitToReplace
        | longbridge::trade::OrderStatus::PendingReplace
        | longbridge::trade::OrderStatus::WaitToCancel
        | longbridge::trade::OrderStatus::PendingCancel => Style::default().fg(Color::Yellow),
        longbridge::trade::OrderStatus::PartialFilled => Style::default().fg(Color::Cyan),
        longbridge::trade::OrderStatus::Filled => Style::default().fg(Color::Green),
        longbridge::trade::OrderStatus::Canceled
        | longbridge::trade::OrderStatus::Replaced
        | longbridge::trade::OrderStatus::PartialWithdrawal
        | longbridge::trade::OrderStatus::Expired => Style::default().fg(Color::DarkGray),
        longbridge::trade::OrderStatus::Rejected => Style::default().fg(Color::Red),
        longbridge::trade::OrderStatus::Unknown => Style::default(),
    }
}

fn order_status_label(status: longbridge::trade::OrderStatus) -> &'static str {
    match status {
        longbridge::trade::OrderStatus::NotReported => "NotReported",
        longbridge::trade::OrderStatus::ReplacedNotReported => "ReplacedNR",
        longbridge::trade::OrderStatus::ProtectedNotReported => "ProtectedNR",
        longbridge::trade::OrderStatus::VarietiesNotReported => "VarietiesNR",
        longbridge::trade::OrderStatus::WaitToNew => "PendingNew",
        longbridge::trade::OrderStatus::New => "New",
        longbridge::trade::OrderStatus::WaitToReplace => "PendingReplace",
        longbridge::trade::OrderStatus::PendingReplace => "Replacing",
        longbridge::trade::OrderStatus::Replaced => "Replaced",
        longbridge::trade::OrderStatus::PartialFilled => "PartialFill",
        longbridge::trade::OrderStatus::WaitToCancel => "PendingCancel",
        longbridge::trade::OrderStatus::PendingCancel => "Cancelling",
        longbridge::trade::OrderStatus::Filled => "Filled",
        longbridge::trade::OrderStatus::Canceled => "Cancelled",
        longbridge::trade::OrderStatus::Rejected => "Rejected",
        longbridge::trade::OrderStatus::Expired => "Expired",
        longbridge::trade::OrderStatus::PartialWithdrawal => "PartialWithdrawal",
        longbridge::trade::OrderStatus::Unknown => "–",
    }
}

fn order_type_label(order_type: longbridge::trade::OrderType) -> &'static str {
    match order_type {
        longbridge::trade::OrderType::LO => "LO",
        longbridge::trade::OrderType::ELO => "ELO",
        longbridge::trade::OrderType::MO => "MO",
        longbridge::trade::OrderType::AO => "AO",
        longbridge::trade::OrderType::ALO => "ALO",
        _ => "–",
    }
}

// ─────────────────────────── popup render fns ───────────────────────────────

pub fn render_order_entry_popup(frame: &mut Frame, rect: Rect) {
    const W: u16 = 52;
    const H: u16 = 10;
    const INPUT_X_OFFSET: u16 = 12;
    let state_lock = ORDER_ENTRY_STATE.read().expect("poison");
    let Some(state) = &*state_lock else { return };

    let popup_rect = crate::tui::ui::rect::centered(W, H, rect);
    frame.render_widget(Clear, popup_rect);

    let side_label = match state.side {
        longbridge::trade::OrderSide::Buy => t!("Trade.Buy"),
        longbridge::trade::OrderSide::Sell => t!("Trade.Sell"),
        longbridge::trade::OrderSide::Unknown => std::borrow::Cow::Borrowed("–"),
    };

    let side_style = match state.side {
        longbridge::trade::OrderSide::Buy => styles::up(std::cmp::Ordering::Greater),
        longbridge::trade::OrderSide::Sell => styles::up(std::cmp::Ordering::Less),
        longbridge::trade::OrderSide::Unknown => styles::text(),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(side_style)
        .title(Line::from(vec![
            Span::raw(format!(" {} — ", t!("Trade.PlaceOrder"))),
            Span::styled(side_label.to_string(), side_style),
            Span::raw(" "),
        ]))
        .title_bottom(
            Line::from(vec![
                Span::styled(" [←][→] ", styles::dark_gray()),
                Span::styled(t!("Trade.SelectHint").to_string(), styles::dark_gray()),
                Span::styled("  [=] ", styles::dark_gray()),
                Span::styled(t!("Trade.FillPrice").to_string(), styles::dark_gray()),
                Span::raw(" "),
            ])
            .right_aligned(),
        );

    let inner = block.inner(popup_rect);
    frame.render_widget(block, popup_rect);

    let price_editable = matches!(
        state.order_type,
        longbridge::trade::OrderType::LO | longbridge::trade::OrderType::ELO
    );

    let type_label = order_type_label(state.order_type);
    let tif_label = match state.tif {
        longbridge::trade::TimeInForceType::GoodTilCanceled => "GTC",
        _ => "Day",
    };

    let max_str = match state.side {
        longbridge::trade::OrderSide::Buy => state
            .max_qty
            .map_or(String::new(), |q| format!("  {} {q}", t!("Trade.MaxQty"))),
        longbridge::trade::OrderSide::Sell => state.max_qty.map_or(String::new(), |q| {
            format!("  {} {q}", t!("Trade.AvailableQty"))
        }),
        longbridge::trade::OrderSide::Unknown => String::new(),
    };

    let lbl = styles::label();
    let val = styles::text();
    let focused_val = Style::default()
        .add_modifier(Modifier::BOLD)
        .add_modifier(Modifier::UNDERLINED);
    let dim = styles::dark_gray();

    let row_symbol = Line::from(vec![
        Span::styled("  Symbol   ", lbl),
        Span::styled(state.symbol.clone(), styles::primary()),
    ]);

    let row_side = Line::from(vec![
        Span::styled("  Side     ", lbl),
        Span::styled(side_label.to_string(), side_style),
    ]);

    let type_val_style = if state.focused_field == OrderEntryField::OrderType {
        side_style
    } else {
        val
    };
    let type_str = if state.focused_field == OrderEntryField::OrderType {
        format!("◀ {type_label} ▶")
    } else {
        type_label.to_string()
    };
    let row_type = Line::from(vec![
        Span::styled("  Type     ", lbl),
        Span::styled(type_str, type_val_style),
    ]);

    let qty_focused = state.focused_field == OrderEntryField::Quantity;
    let qty_val_style = if qty_focused { focused_val } else { val };
    let mut qty_spans = vec![
        Span::styled("  Qty      ", lbl),
        Span::raw("["),
        Span::styled(state.quantity_input.value().to_string(), qty_val_style),
        Span::raw("]"),
    ];
    if !max_str.is_empty() {
        qty_spans.push(Span::styled(max_str, dim));
    }
    let row_qty = Line::from(qty_spans);

    let price_focused = state.focused_field == OrderEntryField::Price;
    let price_val_style = if price_focused { focused_val } else { val };
    let row_price = if price_editable {
        Line::from(vec![
            Span::styled("  Price    ", lbl),
            Span::raw("["),
            Span::styled(state.price_input.value().to_string(), price_val_style),
            Span::raw("]"),
            Span::styled("  [=] fill", dim),
        ])
    } else {
        Line::from(vec![
            Span::styled("  Price    ", lbl),
            Span::styled("–  (market order)", dim),
        ])
    };

    let tif_val_style = if state.focused_field == OrderEntryField::Tif {
        side_style
    } else {
        val
    };
    let tif_str = if state.focused_field == OrderEntryField::Tif {
        format!("◀ {tif_label} ▶")
    } else {
        tif_label.to_string()
    };
    let row_tif = Line::from(vec![
        Span::styled("  TIF      ", lbl),
        Span::styled(tif_str, tif_val_style),
    ]);

    let row_spacer = Line::from("");

    let on_buttons = state.focused_field == OrderEntryField::Buttons;
    let submit_style = if on_buttons && state.confirm_button == ConfirmButton::Submit {
        styles::text_selected()
    } else {
        val
    };
    let cancel_style = if on_buttons && state.confirm_button == ConfirmButton::Cancel {
        styles::text_selected()
    } else {
        val
    };
    let row_buttons = Line::from(vec![
        Span::raw("  "),
        Span::styled(format!(" {} ", t!("Trade.Submit")), submit_style),
        Span::raw("   "),
        Span::styled(format!(" {} ", t!("Trade.Cancel")), cancel_style),
    ]);

    let rows: Vec<Line> = vec![
        row_symbol,
        row_side,
        row_type,
        row_qty,
        row_price,
        row_tif,
        row_spacer,
        row_buttons,
    ];

    let constraints: Vec<Constraint> = rows.iter().map(|_| Constraint::Length(1)).collect();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    for (line, chunk) in rows.iter().zip(chunks.iter()) {
        frame.render_widget(Paragraph::new(line.clone()), *chunk);
    }

    match state.focused_field {
        OrderEntryField::Quantity => {
            let chunk = chunks[3];
            frame.set_cursor_position((
                chunk.x + INPUT_X_OFFSET + state.quantity_input.visual_cursor() as u16,
                chunk.y,
            ));
        }
        OrderEntryField::Price if price_editable => {
            let chunk = chunks[4];
            frame.set_cursor_position((
                chunk.x + INPUT_X_OFFSET + state.price_input.visual_cursor() as u16,
                chunk.y,
            ));
        }
        _ => {}
    }
}

pub fn render_cancel_order_popup(frame: &mut Frame, rect: Rect) {
    const W: u16 = 44;
    const H: u16 = 10;
    let lock = CANCEL_TARGET.read().expect("poison");
    let Some(order) = &*lock else { return };

    let popup_rect = crate::tui::ui::rect::centered(W, H, rect);
    frame.render_widget(Clear, popup_rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(styles::border())
        .title(format!(" {} ", t!("CancelOrder.Title")));

    let inner = block.inner(popup_rect);
    frame.render_widget(block, popup_rect);

    let price_str = order.price.map_or("–".to_string(), |p| format!("{p:.2}"));

    let rows = [
        format!("  {}: {}", t!("CancelOrder.Order"), order.order_id),
        format!("  {}: {}", t!("CancelOrder.Symbol"), order.symbol),
        format!(
            "  {}: {}  {}: {}  {}: {}",
            t!("CancelOrder.Qty"),
            order.quantity,
            t!("CancelOrder.Price"),
            price_str,
            t!("CancelOrder.Side"),
            match order.side {
                longbridge::trade::OrderSide::Buy => t!("Trade.Buy").to_string(),
                longbridge::trade::OrderSide::Sell => t!("Trade.Sell").to_string(),
                longbridge::trade::OrderSide::Unknown => "–".to_string(),
            }
        ),
        String::new(),
        format!(
            "  {}   {}",
            t!("CancelOrder.Confirm"),
            t!("CancelOrder.Cancel")
        ),
    ];

    let constraints: Vec<Constraint> = rows.iter().map(|_| Constraint::Length(1)).collect();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    for (row, chunk) in rows.iter().zip(chunks.iter()) {
        frame.render_widget(Paragraph::new(row.as_str()).style(styles::text()), *chunk);
    }
}

pub fn render_replace_order_popup(frame: &mut Frame, rect: Rect) {
    const W: u16 = 44;
    const H: u16 = 10;
    let lock = REPLACE_ORDER_STATE.read().expect("poison");
    let Some(state) = &*lock else { return };

    let popup_rect = crate::tui::ui::rect::centered(W, H, rect);
    frame.render_widget(Clear, popup_rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(styles::border())
        .title(format!(" {} ", t!("ReplaceOrder.Title")))
        .title_bottom(
            Line::from(vec![Span::styled(
                format!(
                    " [Tab] {}  [Enter] {}  [Esc] {} ",
                    t!("Orders.DateFilterSwitch"),
                    t!("Trade.Confirm"),
                    t!("Trade.Cancel"),
                ),
                styles::dark_gray(),
            )])
            .right_aligned(),
        );

    let inner = block.inner(popup_rect);
    frame.render_widget(block, popup_rect);

    if state.confirming {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(inner);
        frame.render_widget(
            Paragraph::new(format!(
                "  {}: {}  {}: {}  {}: {}",
                t!("ReplaceOrder.OrderId"),
                state.order_id,
                t!("ReplaceOrder.NewQty"),
                state.qty_input.value(),
                t!("ReplaceOrder.NewPrice"),
                state.price_input.value()
            ))
            .style(styles::text()),
            chunks[0],
        );
        frame.render_widget(
            Paragraph::new(format!(
                "  {}  {}  {}",
                t!("Trade.Confirm"),
                t!("Trade.Yes"),
                t!("Trade.No")
            ))
            .style(styles::label()),
            chunks[1],
        );
        return;
    }

    let lbl = styles::label();
    let val = styles::text();
    let focused_val = Style::default()
        .add_modifier(Modifier::BOLD)
        .add_modifier(Modifier::UNDERLINED);

    let qty_style = if state.focused == ReplaceOrderField::Qty {
        focused_val
    } else {
        val
    };
    let price_style = if state.focused == ReplaceOrderField::Price {
        focused_val
    } else {
        val
    };

    let qty_label = t!("ReplaceOrder.NewQty");
    let price_label = t!("ReplaceOrder.NewPrice");

    let rows: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(format!("  {}: ", t!("ReplaceOrder.OrderId")), lbl),
            Span::styled(state.order_id.clone(), val),
        ]),
        Line::from(vec![
            Span::styled(format!("  {qty_label}: "), lbl),
            Span::raw("["),
            Span::styled(state.qty_input.value().to_string(), qty_style),
            Span::raw("]"),
        ]),
        Line::from(vec![
            Span::styled(format!("  {price_label}: "), lbl),
            Span::raw("["),
            Span::styled(state.price_input.value().to_string(), price_style),
            Span::raw("]"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            format!("  {}   {}", t!("Trade.Submit"), t!("Trade.Cancel")),
            val,
        )]),
    ];

    let constraints: Vec<Constraint> = rows.iter().map(|_| Constraint::Length(1)).collect();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    for (row, chunk) in rows.iter().zip(chunks.iter()) {
        frame.render_widget(Paragraph::new(row.clone()), *chunk);
    }

    // Show cursor on focused input: "  {label}: [" → 2 + label.len + 2 + 1 chars before cursor
    match state.focused {
        ReplaceOrderField::Qty => {
            let prefix = 2 + qty_label.len() as u16 + 2 + 1;
            frame.set_cursor_position((
                chunks[1].x + prefix + state.qty_input.visual_cursor() as u16,
                chunks[1].y,
            ));
        }
        ReplaceOrderField::Price => {
            let prefix = 2 + price_label.len() as u16 + 2 + 1;
            frame.set_cursor_position((
                chunks[2].x + prefix + state.price_input.visual_cursor() as u16,
                chunks[2].y,
            ));
        }
    }
}

pub fn render_date_filter_popup(frame: &mut Frame, rect: Rect) {
    const W: u16 = 44;
    const H: u16 = 8;
    let popup_rect = crate::tui::ui::rect::centered(W, H, rect);
    frame.render_widget(Clear, popup_rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(styles::border())
        .title(format!(" {} ", t!("Orders.DateFilterTitle")))
        .title_bottom(
            Line::from(vec![
                Span::styled(
                    format!(" [Tab] {} ", t!("Orders.DateFilterSwitch")),
                    styles::dark_gray(),
                ),
                Span::styled(
                    format!(" [Enter] {} ", t!("Orders.DateFilterApply")),
                    styles::dark_gray(),
                ),
                Span::styled(" [Esc] ", styles::dark_gray()),
            ])
            .right_aligned(),
        );

    let inner = block.inner(popup_rect);
    frame.render_widget(block, popup_rect);

    let s = DATE_FILTER_STATE.read().expect("poison");

    let lbl = styles::label();
    let focused_val = Style::default()
        .add_modifier(Modifier::BOLD)
        .add_modifier(Modifier::UNDERLINED);
    let val = styles::text();

    let start_style = if s.focused == DateFilterField::Start {
        focused_val
    } else {
        val
    };
    let end_style = if s.focused == DateFilterField::End {
        focused_val
    } else {
        val
    };

    let rows: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(format!("  {} ", t!("Orders.StartDate")), lbl),
            Span::raw("["),
            Span::styled(s.start_input.value().to_string(), start_style),
            Span::raw("]"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(format!("  {} ", t!("Orders.EndDate")), lbl),
            Span::raw("["),
            Span::styled(s.end_input.value().to_string(), end_style),
            Span::raw("]"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            format!("  {}", t!("Orders.DateHint")),
            styles::dark_gray(),
        )]),
    ];

    let constraints: Vec<Constraint> = rows.iter().map(|_| Constraint::Length(1)).collect();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    for (line, chunk) in rows.iter().zip(chunks.iter()) {
        frame.render_widget(Paragraph::new(line.clone()), *chunk);
    }

    // Show cursor on focused input
    let input_prefix_len: u16 = 2 + t!("Orders.StartDate").len() as u16 + 1 + 1; // "  {label} ["
    match s.focused {
        DateFilterField::Start => {
            frame.set_cursor_position((
                chunks[0].x + input_prefix_len + s.start_input.visual_cursor() as u16,
                chunks[0].y,
            ));
        }
        DateFilterField::End => {
            frame.set_cursor_position((
                chunks[2].x + input_prefix_len + s.end_input.visual_cursor() as u16,
                chunks[2].y,
            ));
        }
    }
}

// helper: open cancel popup for selected order in Orders page
pub fn try_open_cancel_for_selected() {
    let idx = ORDERS_TABLE.lock().expect("poison").selected().unwrap_or(0);
    let orders = ORDERS_VIEW.read().expect("poison");
    let Some(order) = orders.get(idx) else {
        set_toast(ToastKind::Error, t!("Trade.NoOrderSelected").to_string());
        return;
    };
    let terminal = matches!(
        order.status,
        longbridge::trade::OrderStatus::Filled
            | longbridge::trade::OrderStatus::Canceled
            | longbridge::trade::OrderStatus::Rejected
            | longbridge::trade::OrderStatus::Expired
            | longbridge::trade::OrderStatus::PartialWithdrawal
    );
    if terminal {
        set_toast(
            ToastKind::Error,
            t!("Trade.OrderNotCancellable").to_string(),
        );
    } else {
        *CANCEL_TARGET.write().expect("poison") = Some(order.clone());
        POPUP.store(POPUP_CANCEL_ORDER, Ordering::Relaxed);
    }
}

// helper: open replace popup for selected order in Orders page
pub fn try_open_replace_for_selected() {
    let idx = ORDERS_TABLE.lock().expect("poison").selected().unwrap_or(0);
    let orders = ORDERS_VIEW.read().expect("poison");
    let Some(order) = orders.get(idx) else {
        set_toast(ToastKind::Error, t!("Trade.NoOrderSelected").to_string());
        return;
    };
    let terminal = matches!(
        order.status,
        longbridge::trade::OrderStatus::Filled
            | longbridge::trade::OrderStatus::Canceled
            | longbridge::trade::OrderStatus::Rejected
            | longbridge::trade::OrderStatus::Expired
            | longbridge::trade::OrderStatus::PartialWithdrawal
    );
    if terminal {
        set_toast(
            ToastKind::Error,
            t!("Trade.OrderNotReplaceable").to_string(),
        );
    } else {
        let state = ReplaceOrderState {
            order_id: order.order_id.clone(),
            qty_input: tui_input::Input::new(format!("{}", order.quantity)),
            price_input: tui_input::Input::new(
                order.price.map(|p| format!("{p:.2}")).unwrap_or_default(),
            ),
            focused: ReplaceOrderField::Qty,
            confirming: false,
        };
        *REPLACE_ORDER_STATE.write().expect("poison") = Some(state);
        POPUP.store(POPUP_REPLACE_ORDER, Ordering::Relaxed);
    }
}
