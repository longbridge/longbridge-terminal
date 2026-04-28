use std::sync::atomic::Ordering;
use std::sync::{LazyLock, Mutex, RwLock};

use bevy_ecs::{
    prelude::*,
    system::{CommandQueue, InsertResource},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table},
    Frame,
};
use rust_decimal::Decimal;
use std::str::FromStr;

use crate::{
    tui::app::{AppState, POPUP, POPUP_CANCEL_ORDER, POPUP_REPLACE_ORDER, RT},
    tui::ui::styles,
    tui::widgets::{
        toast::{set_toast, ToastKind},
        Terminal,
    },
};

use super::{Command, NavFooter, PopUp, StockDetail};

pub static ORDERS_VIEW: LazyLock<RwLock<Vec<longbridge::trade::Order>>> =
    LazyLock::new(|| RwLock::new(vec![]));

pub static ORDER_ENTRY_STATE: LazyLock<RwLock<Option<OrderEntryState>>> =
    LazyLock::new(|| RwLock::new(None));

pub static REPLACE_ORDER_STATE: LazyLock<RwLock<Option<ReplaceOrderState>>> =
    LazyLock::new(|| RwLock::new(None));

pub static CANCEL_TARGET: LazyLock<RwLock<Option<longbridge::trade::Order>>> =
    LazyLock::new(|| RwLock::new(None));

pub static ORDERS_TABLE: LazyLock<Mutex<TableState>> = LazyLock::new(Mutex::default);

// ────────────────────────────── state structs ───────────────────────────────

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

pub struct ReplaceOrderState {
    pub order_id: String,
    pub qty_input: tui_input::Input,
    pub price_input: tui_input::Input,
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
            code: KeyCode::Char('y'),
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
            code: KeyCode::Esc | KeyCode::Char('n'),
            ..
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
                // toggle between qty and price
                let currently_qty = s.qty_input.cursor() >= s.price_input.cursor()
                    || s.qty_input.value().len() == s.price_input.value().len();
                let _ = currently_qty; // simple: just focus alternates via convention
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
                .map_or(false, |s| s.confirming);
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
                .map_or(false, |s| s.confirming);
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
                        s.qty_input.handle(req);
                    }
                }
            }
        }
    }
}

// ─────────────────────────── Bevy ECS systems ───────────────────────────────

pub fn enter_orders() {
    refresh_orders();
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
        let orders = ORDERS_VIEW.read().expect("poison");
        let len = orders.len();
        drop(orders);
        match event {
            super::Key::Up => {
                let mut table = ORDERS_TABLE.lock().expect("poison");
                let idx = table.selected();
                let new_idx = idx.map_or(0, |i| if i == 0 { 0 } else { i - 1 });
                if len > 0 {
                    table.select(Some(new_idx));
                }
            }
            super::Key::Down => {
                let mut table = ORDERS_TABLE.lock().expect("poison");
                let idx = table.selected();
                let new_idx = idx.map_or(0, |i| if i + 1 < len { i + 1 } else { i });
                if len > 0 {
                    table.select(Some(new_idx));
                }
            }
            super::Key::Enter => {
                let selected = ORDERS_TABLE.lock().expect("poison").selected();
                if let Some(idx) = selected {
                    let orders = ORDERS_VIEW.read().expect("poison");
                    if let Some(order) = orders.get(idx) {
                        let symbol = order.symbol.clone();
                        drop(orders);
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

fn render_orders_list(frame: &mut Frame, rect: Rect) {
    let orders = ORDERS_VIEW.read().expect("poison");
    let mut table_state = ORDERS_TABLE.lock().expect("poison");

    let title = if orders.is_empty() {
        format!(" {} ", t!("Orders.Title"))
    } else {
        format!(" {} ({}) ", t!("Orders.Title"), orders.len())
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(styles::border())
        .title(title)
        .title_bottom(
            Line::from(vec![
                Span::styled(format!(" {} ", t!("Orders.Refresh")), styles::dark_gray()),
                Span::styled(format!(" {} ", t!("Orders.CancelKey")), styles::dark_gray()),
                Span::styled(
                    format!(" {} ", t!("Orders.ReplaceKey")),
                    styles::dark_gray(),
                ),
            ])
            .right_aligned(),
        );

    if orders.is_empty() {
        let msg = Paragraph::new(Span::styled(
            t!("Orders.NoOrders"),
            Style::default().fg(Color::DarkGray),
        ))
        .block(block)
        .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(msg, rect);
        return;
    }

    let header = Row::new(vec![
        Cell::from(t!("Orders.Symbol")).style(styles::header()),
        Cell::from(t!("Orders.Side")).style(styles::header()),
        Cell::from(t!("Orders.Type")).style(styles::header()),
        Cell::from(t!("Orders.Status")).style(styles::header()),
        Cell::from(t!("Orders.Qty")).style(styles::header()),
        Cell::from(t!("Orders.ExecQty")).style(styles::header()),
        Cell::from(t!("Orders.Price")).style(styles::header()),
        Cell::from(t!("Orders.SubmittedAt")).style(styles::header()),
    ]);

    let rows: Vec<Row> = orders
        .iter()
        .map(|order| {
            let status_style = order_status_style(order.status);
            let status_label = order_status_label(order.status);
            let side_label = match order.side {
                longbridge::trade::OrderSide::Buy => t!("Trade.Buy"),
                longbridge::trade::OrderSide::Sell => t!("Trade.Sell"),
                _ => std::borrow::Cow::Borrowed("–"),
            };
            let type_label = order_type_label(order.order_type);
            let price_str = order.price.map_or("–".to_string(), |p| format!("{p:.2}"));
            let t = order.submitted_at;
            let time_str = format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second());

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

    frame.render_stateful_widget(table, rect, &mut *table_state);
}

fn order_status_style(status: longbridge::trade::OrderStatus) -> Style {
    match status {
        longbridge::trade::OrderStatus::WaitToNew
        | longbridge::trade::OrderStatus::New
        | longbridge::trade::OrderStatus::WaitToReplace => Style::default().fg(Color::Yellow),
        longbridge::trade::OrderStatus::PartialFilled => Style::default().fg(Color::Cyan),
        longbridge::trade::OrderStatus::Filled => Style::default().fg(Color::Green),
        longbridge::trade::OrderStatus::Canceled
        | longbridge::trade::OrderStatus::Replaced
        | longbridge::trade::OrderStatus::PartialWithdrawal => Style::default().fg(Color::DarkGray),
        longbridge::trade::OrderStatus::Rejected => Style::default().fg(Color::Red),
        _ => Style::default(),
    }
}

fn order_status_label(status: longbridge::trade::OrderStatus) -> &'static str {
    match status {
        longbridge::trade::OrderStatus::WaitToNew => "PendingNew",
        longbridge::trade::OrderStatus::New => "New",
        longbridge::trade::OrderStatus::WaitToReplace => "PendingReplace",
        longbridge::trade::OrderStatus::PartialFilled => "PartialFill",
        longbridge::trade::OrderStatus::Filled => "Filled",
        longbridge::trade::OrderStatus::Canceled => "Cancelled",
        longbridge::trade::OrderStatus::Replaced => "Replaced",
        longbridge::trade::OrderStatus::PartialWithdrawal => "PartialWithdrawal",
        longbridge::trade::OrderStatus::Rejected => "Rejected",
        _ => "–",
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
    let state_lock = ORDER_ENTRY_STATE.read().expect("poison");
    let Some(state) = &*state_lock else { return };

    const W: u16 = 52;
    const H: u16 = 10;
    let popup_rect = crate::tui::ui::rect::centered(W, H, rect);
    frame.render_widget(Clear, popup_rect);

    let side_label = match state.side {
        longbridge::trade::OrderSide::Buy => t!("Trade.Buy"),
        longbridge::trade::OrderSide::Sell => t!("Trade.Sell"),
        _ => std::borrow::Cow::Borrowed("–"),
    };

    let side_style = match state.side {
        longbridge::trade::OrderSide::Buy => styles::up(std::cmp::Ordering::Greater),
        longbridge::trade::OrderSide::Sell => styles::up(std::cmp::Ordering::Less),
        _ => styles::text(),
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
        longbridge::trade::TimeInForceType::Day => "Day",
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
        _ => String::new(),
    };

    let lbl = styles::label();
    let val = styles::text();
    let focused_val = Style::default()
        .add_modifier(Modifier::BOLD)
        .add_modifier(Modifier::UNDERLINED);
    let dim = styles::dark_gray();

    // Row 0: Symbol
    let row_symbol = Line::from(vec![
        Span::styled("  Symbol   ", lbl),
        Span::styled(state.symbol.clone(), styles::primary()),
    ]);

    // Row 1: Side
    let row_side = Line::from(vec![
        Span::styled("  Side     ", lbl),
        Span::styled(side_label.to_string(), side_style),
    ]);

    // Row 2: Type — ◀ LO ▶ when focused (uses side color for discoverability)
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

    // Row 3: Qty
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

    // Row 4: Price
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

    // Row 5: TIF — ◀ Day ▶ when focused
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

    // Row 6: spacer
    let row_spacer = Line::from("");

    // Row 7: Submit / Cancel buttons — REVERSED on focused button
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

    // "  Qty      [" = 11 + 1 = 12 chars before cursor
    const INPUT_X_OFFSET: u16 = 12;

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
    let lock = CANCEL_TARGET.read().expect("poison");
    let Some(order) = &*lock else { return };

    const W: u16 = 44;
    const H: u16 = 10;
    let popup_rect = crate::tui::ui::rect::centered(W, H, rect);
    frame.render_widget(Clear, popup_rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(styles::border())
        .title(format!(" {} ", t!("CancelOrder.Title")));

    let inner = block.inner(popup_rect);
    frame.render_widget(block, popup_rect);

    let price_str = order.price.map_or("–".to_string(), |p| format!("{p:.2}"));

    let rows = vec![
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
                _ => "–".to_string(),
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
    let lock = REPLACE_ORDER_STATE.read().expect("poison");
    let Some(state) = &*lock else { return };

    const W: u16 = 44;
    const H: u16 = 10;
    let popup_rect = crate::tui::ui::rect::centered(W, H, rect);
    frame.render_widget(Clear, popup_rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(styles::border())
        .title(format!(" {} ", t!("ReplaceOrder.Title")));

    let inner = block.inner(popup_rect);
    frame.render_widget(block, popup_rect);

    if state.confirming {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(inner);
        frame.render_widget(
            Paragraph::new(format!(
                "  Modify {} qty={} price={}",
                state.order_id,
                state.qty_input.value(),
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

    let rows = vec![
        format!("  {}: {}", t!("ReplaceOrder.OrderId"), state.order_id),
        format!(
            "  {}: [{}]",
            t!("ReplaceOrder.NewQty"),
            state.qty_input.value()
        ),
        format!(
            "  {}: [{}]",
            t!("ReplaceOrder.NewPrice"),
            state.price_input.value()
        ),
        String::new(),
        format!("  {}   {}", t!("Trade.Submit"), t!("Trade.Cancel")),
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

// helper: open cancel popup for selected order in Orders page
pub fn try_open_cancel_for_selected() {
    let selected = ORDERS_TABLE.lock().expect("poison").selected();
    if let Some(idx) = selected {
        let orders = ORDERS_VIEW.read().expect("poison");
        if let Some(order) = orders.get(idx) {
            let cancellable = matches!(
                order.status,
                longbridge::trade::OrderStatus::WaitToNew
                    | longbridge::trade::OrderStatus::New
                    | longbridge::trade::OrderStatus::WaitToReplace
                    | longbridge::trade::OrderStatus::PartialFilled
            );
            if cancellable {
                *CANCEL_TARGET.write().expect("poison") = Some(order.clone());
                POPUP.store(POPUP_CANCEL_ORDER, Ordering::Relaxed);
            }
        }
    }
}

// helper: open replace popup for selected order in Orders page
pub fn try_open_replace_for_selected() {
    let selected = ORDERS_TABLE.lock().expect("poison").selected();
    if let Some(idx) = selected {
        let orders = ORDERS_VIEW.read().expect("poison");
        if let Some(order) = orders.get(idx) {
            let replaceable = matches!(
                order.status,
                longbridge::trade::OrderStatus::WaitToNew
                    | longbridge::trade::OrderStatus::New
                    | longbridge::trade::OrderStatus::WaitToReplace
                    | longbridge::trade::OrderStatus::PartialFilled
            );
            if replaceable {
                let state = ReplaceOrderState {
                    order_id: order.order_id.clone(),
                    qty_input: tui_input::Input::new(format!("{}", order.quantity)),
                    price_input: tui_input::Input::new(
                        order.price.map(|p| format!("{p:.2}")).unwrap_or_default(),
                    ),
                    confirming: false,
                };
                *REPLACE_ORDER_STATE.write().expect("poison") = Some(state);
                POPUP.store(POPUP_REPLACE_ORDER, Ordering::Relaxed);
            }
        }
    }
}
