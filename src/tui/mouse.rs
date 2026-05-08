use std::sync::{LazyLock, Mutex};

use ratatui::layout::Rect;

// Clickable area rects updated every frame during rendering.
// Used by the mouse event handler in app.rs to map clicks to actions.

pub static NAVBAR_TABS_RECT: LazyLock<Mutex<Rect>> = LazyLock::new(|| Mutex::new(Rect::default()));

pub static WATCHLIST_TABLE_RECT: LazyLock<Mutex<Rect>> =
    LazyLock::new(|| Mutex::new(Rect::default()));

pub static PORTFOLIO_TABLE_RECT: LazyLock<Mutex<Rect>> =
    LazyLock::new(|| Mutex::new(Rect::default()));

pub static ORDERS_TABLE_RECT: LazyLock<Mutex<Rect>> = LazyLock::new(|| Mutex::new(Rect::default()));

pub static HISTORY_ORDERS_TABLE_RECT: LazyLock<Mutex<Rect>> =
    LazyLock::new(|| Mutex::new(Rect::default()));

pub static POPUP_LIST_RECT: LazyLock<Mutex<Rect>> = LazyLock::new(|| Mutex::new(Rect::default()));

pub static NEWS_LIST_RECT: LazyLock<Mutex<Rect>> = LazyLock::new(|| Mutex::new(Rect::default()));

pub static WATCHLIST_STOCK_TABS_RECT: LazyLock<Mutex<Rect>> =
    LazyLock::new(|| Mutex::new(Rect::default()));

/// Kline period tab bar rect (1m / 5m / … / Year row in stock detail).
pub static KLINE_TABS_RECT: LazyLock<Mutex<Rect>> = LazyLock::new(|| Mutex::new(Rect::default()));

/// Footer index click areas: [Q], [W], [E] regions (one rect per index group).
pub static FOOTER_INDEX_RECTS: LazyLock<Mutex<[Rect; 3]>> =
    LazyLock::new(|| Mutex::new([Rect::default(); 3]));

/// Hit-test a click against a table with NO block border.
/// Header is at rect.y; data row i is at rect.y + 1 + i.
pub fn click_to_row(col: u16, row: u16, rect: Rect) -> Option<usize> {
    if rect.width == 0 || rect.height == 0 {
        return None;
    }
    if col < rect.x || col >= rect.x + rect.width {
        return None;
    }
    // rect.y is the header row — skip it
    if row <= rect.y || row >= rect.y + rect.height {
        return None;
    }
    Some((row - rect.y - 1) as usize)
}

/// Hit-test a click against a table with a block border (1-row top + 1-row header = 2 offset).
/// Data row i is at rect.y + 2 + i.
pub fn click_to_row_with_border(col: u16, row: u16, rect: Rect) -> Option<usize> {
    if rect.width == 0 || rect.height == 0 {
        return None;
    }
    if col < rect.x || col >= rect.x + rect.width {
        return None;
    }
    // rect.y = top border, rect.y+1 = header, data starts at rect.y+2
    if row <= rect.y + 1 || row >= rect.y + rect.height.saturating_sub(1) {
        return None;
    }
    Some((row - rect.y - 2) as usize)
}

/// Hit-test a click against a simple list with a 1-row border on top (no header row).
/// Item i is at rect.y + 1 + i.
pub fn click_to_list_item(col: u16, row: u16, rect: Rect) -> Option<usize> {
    if rect.width == 0 || rect.height == 0 {
        return None;
    }
    if col < rect.x || col >= rect.x + rect.width {
        return None;
    }
    if row <= rect.y || row >= rect.y + rect.height.saturating_sub(1) {
        return None;
    }
    Some((row - rect.y - 1) as usize)
}
