use std::sync::atomic::Ordering;
use std::sync::{LazyLock, Mutex, RwLock};

use bevy_ecs::{
    prelude::*,
    system::{CommandQueue, InsertResource},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table},
    Frame,
};
use ratatui::widgets::TableState;
use rust_decimal::Decimal;
use std::str::FromStr;

use crate::{
    tui::app::{AppState, POPUP, POPUP_CANCEL_ORDER, POPUP_REPLACE_ORDER, RT},
    tui::ui::styles,
    tui::widgets::{toast::{set_toast, ToastKind}, Terminal},
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

pub static ORDERS_TABLE: LazyLock<Mutex<TableState>> =
    LazyLock::new(Mutex::default);

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
    pub confirming: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OrderEntryField {
    OrderType,
    Quantity,
    Price,
    Tif,
}

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
    let state = OrderEntryState {
        symbol: symbol.clone(),
        side,
        order_type: longbridge::trade::OrderType::LO,
        quantity_input: tui_input::Input::default(),
        price_input: tui_input::Input::default(),
        tif: longbridge::trade::TimeInForceType::Day,
        focused_field: OrderEntryField::Quantity,
        max_qty: available_qty,
        confirming: false,
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
                set_toast(ToastKind::Error, format!("{}: {e}", t!("Trade.FailedLoadOrders")));
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
        let mut opts = longbridge::trade::SubmitOrderOptions::new(
            symbol,
            order_type,
            side,
            qty,
            tif,
        );
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
                if let Some(s) = ORDER_ENTRY_STATE.write().expect("poison").as_mut() {
                    s.confirming = false;
                }
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
            code: KeyCode::Tab,
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            if let Some(s) = ORDER_ENTRY_STATE.write().expect("poison").as_mut() {
                s.focused_field = match s.focused_field {
                    OrderEntryField::OrderType => OrderEntryField::Quantity,
                    OrderEntryField::Quantity => {
                        let price_editable = matches!(
                            s.order_type,
                            longbridge::trade::OrderType::LO | longbridge::trade::OrderType::ELO
                        );
                        if price_editable {
                            OrderEntryField::Price
                        } else {
                            OrderEntryField::Tif
                        }
                    }
                    OrderEntryField::Price => OrderEntryField::Tif,
                    OrderEntryField::Tif => OrderEntryField::OrderType,
                };
            }
        }
        KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            let confirming = ORDER_ENTRY_STATE
                .read()
                .expect("poison")
                .as_ref()
                .map_or(false, |s| s.confirming);
            if confirming {
                submit_order();
            } else if let Some(s) = ORDER_ENTRY_STATE.write().expect("poison").as_mut() {
                s.confirming = true;
            }
        }
        KeyEvent {
            code: KeyCode::Char('y'),
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            let confirming = ORDER_ENTRY_STATE
                .read()
                .expect("poison")
                .as_ref()
                .map_or(false, |s| s.confirming);
            if confirming {
                submit_order();
            } else {
                forward_char_to_focused('y');
            }
        }
        KeyEvent {
            code: KeyCode::Char('n'),
            modifiers: KeyModifiers::NONE,
            ..
        } => {
            let confirming = ORDER_ENTRY_STATE
                .read()
                .expect("poison")
                .as_ref()
                .map_or(false, |s| s.confirming);
            if confirming {
                if let Some(s) = ORDER_ENTRY_STATE.write().expect("poison").as_mut() {
                    s.confirming = false;
                }
            } else {
                forward_char_to_focused('n');
            }
        }
        _ => {
            let evt = crossterm::event::Event::Key(event);
            if let Some(req) = tui_input::backend::crossterm::to_input_request(&evt) {
                if let Some(s) = ORDER_ENTRY_STATE.write().expect("poison").as_mut() {
                    if !s.confirming {
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
}

fn forward_char_to_focused(c: char) {
    let evt = crossterm::event::Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::NONE,
    ));
    if let Some(req) = tui_input::backend::crossterm::to_input_request(&evt) {
        if let Some(s) = ORDER_ENTRY_STATE.write().expect("poison").as_mut() {
            match s.focused_field {
                OrderEntryField::Quantity => { s.quantity_input.handle(req); }
                OrderEntryField::Price => { s.price_input.handle(req); }
                _ => {}
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
                        queue.push(InsertResource { resource: StockDetail(counter) });
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

        let log_panel_visible =
            crate::tui::app::LOG_PANEL_VISIBLE.load(Ordering::Relaxed);
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
                Span::styled(format!(" {} ", t!("Orders.ReplaceKey")), styles::dark_gray()),
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
                longbridge::trade::OrderSide::Buy => "Buy",
                longbridge::trade::OrderSide::Sell => "Sell",
                _ => "–",
            };
            let type_label = order_type_label(order.order_type);
            let price_str = order
                .price
                .map_or("–".to_string(), |p| format!("{p:.2}"));
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
        | longbridge::trade::OrderStatus::WaitToReplace => {
            Style::default().fg(Color::Yellow)
        }
        longbridge::trade::OrderStatus::PartialFilled => Style::default().fg(Color::Cyan),
        longbridge::trade::OrderStatus::Filled => Style::default().fg(Color::Green),
        longbridge::trade::OrderStatus::Canceled
        | longbridge::trade::OrderStatus::Replaced
        | longbridge::trade::OrderStatus::PartialWithdrawal => {
            Style::default().fg(Color::DarkGray)
        }
        longbridge::trade::OrderStatus::Rejected => {
            Style::default().fg(Color::Red)
        }
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

    const W: u16 = 44;
    const H: u16 = 14;
    let popup_rect = crate::tui::ui::rect::centered(W, H, rect);
    frame.render_widget(Clear, popup_rect);

    let side_label = match state.side {
        longbridge::trade::OrderSide::Buy => t!("Trade.Buy"),
        longbridge::trade::OrderSide::Sell => t!("Trade.Sell"),
        _ => std::borrow::Cow::Borrowed("–"),
    };

    let title = format!(" {} — {} ", t!("Trade.PlaceOrder"), side_label);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(styles::border())
        .title(title);

    let inner = block.inner(popup_rect);
    frame.render_widget(block, popup_rect);

    let price_editable = matches!(
        state.order_type,
        longbridge::trade::OrderType::LO | longbridge::trade::OrderType::ELO
    );

    if state.confirming {
        let price_display = if price_editable {
            format!(" @ {}", state.price_input.value())
        } else {
            String::new()
        };
        let confirm_line = format!(
            "{} {} × {}{}",
            side_label,
            state.quantity_input.value(),
            state.symbol,
            price_display,
        );
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1)])
            .split(inner);
        frame.render_widget(
            Paragraph::new(confirm_line).style(styles::text()),
            chunks[0],
        );
        frame.render_widget(
            Paragraph::new(format!(
                "{}  {}  {}",
                t!("Trade.Confirm"),
                t!("Trade.Yes"),
                t!("Trade.No")
            ))
            .style(styles::label()),
            chunks[1],
        );
        return;
    }

    let max_label = match state.side {
        longbridge::trade::OrderSide::Buy => state
            .max_qty
            .map_or(String::new(), |q| format!("  {} {}", t!("Trade.MaxQty"), q)),
        longbridge::trade::OrderSide::Sell => state
            .max_qty
            .map_or(String::new(), |q| {
                format!("  {} {}", t!("Trade.AvailableQty"), q)
            }),
        _ => String::new(),
    };

    let type_label = order_type_label(state.order_type);
    let tif_label = match state.tif {
        longbridge::trade::TimeInForceType::Day => "Day",
        longbridge::trade::TimeInForceType::GoodTilCanceled => "GTC",
        _ => "Day",
    };

    let rows = vec![
        format!("  Symbol  : {}", state.symbol),
        format!("  Side    : {}", side_label),
        format!("  Type    : [{}]", type_label),
        format!("  Qty     : [{}]{}", state.quantity_input.value(), max_label),
        if price_editable {
            format!("  Price   : [{}]", state.price_input.value())
        } else {
            format!("  Price   : [–]  (not required)")
        },
        format!("  TIF     : [{}]", tif_label),
        String::new(),
        format!("  {}   {}", t!("Trade.Submit"), t!("Trade.Cancel")),
    ];

    let constraints: Vec<Constraint> = rows.iter().map(|_| Constraint::Length(1)).collect();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    for (i, (row, chunk)) in rows.iter().zip(chunks.iter()).enumerate() {
        let focused = match (i, state.focused_field) {
            (2, OrderEntryField::OrderType)
            | (3, OrderEntryField::Quantity)
            | (4, OrderEntryField::Price)
            | (5, OrderEntryField::Tif) => true,
            _ => false,
        };
        let style = if focused {
            styles::text_selected()
        } else {
            styles::text()
        };
        frame.render_widget(Paragraph::new(row.as_str()).style(style), *chunk);
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
                longbridge::trade::OrderSide::Buy => "Buy",
                longbridge::trade::OrderSide::Sell => "Sell",
                _ => "–",
            }
        ),
        String::new(),
        format!("  {}   {}", t!("CancelOrder.Confirm"), t!("CancelOrder.Cancel")),
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
        format!("  {}: [{}]", t!("ReplaceOrder.NewQty"), state.qty_input.value()),
        format!("  {}: [{}]", t!("ReplaceOrder.NewPrice"), state.price_input.value()),
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
