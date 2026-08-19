use std::sync::{LazyLock, Mutex};

use ratatui::layout::Rect;

use crate::tui::keymap::ActionId;

// Clickable area rects updated every frame during rendering.
// Used by the mouse event handler in app.rs to map clicks to actions.

/// Exact rect of each primary tab pill, in tab order. Written by the navbar
/// renderer so hit-testing uses the drawn geometry instead of re-deriving
/// label widths.
pub static NAVBAR_TAB_RECTS: LazyLock<Mutex<[Rect; 3]>> =
    LazyLock::new(|| Mutex::new([Rect::default(); 3]));

/// Rect of each shortcut hint drawn in the navbar, paired with the action it
/// triggers, so the hints double as buttons.
pub static NAVBAR_HINT_RECTS: LazyLock<Mutex<Vec<(Rect, ActionId)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// Clickable URLs drawn anywhere in the current frame, as (rect, url).
static LINK_RECTS: LazyLock<Mutex<Vec<(Rect, String)>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Drop every link registered by the previous frame. Called once per frame
/// before the screen redraws so stale hit areas never linger.
pub fn clear_links() {
    LINK_RECTS.lock().expect("poison").clear();
}

/// Mark `rect` as a clickable link to `url` for this frame.
pub fn register_link(rect: Rect, url: impl Into<String>) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    LINK_RECTS.lock().expect("poison").push((rect, url.into()));
}

/// The URL under the given cell, if any.
pub fn link_at(col: u16, row: u16) -> Option<String> {
    LINK_RECTS
        .lock()
        .expect("poison")
        .iter()
        .find(|(r, _)| col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height)
        .map(|(_, url)| url.clone())
}

pub static WATCHLIST_TABLE_RECT: LazyLock<Mutex<Rect>> =
    LazyLock::new(|| Mutex::new(Rect::default()));

pub static PORTFOLIO_TABLE_RECT: LazyLock<Mutex<Rect>> =
    LazyLock::new(|| Mutex::new(Rect::default()));

pub static ORDERS_TABLE_RECT: LazyLock<Mutex<Rect>> = LazyLock::new(|| Mutex::new(Rect::default()));

pub static HISTORY_ORDERS_TABLE_RECT: LazyLock<Mutex<Rect>> =
    LazyLock::new(|| Mutex::new(Rect::default()));

/// Rect of the right-hand detail panel on the Portfolio / Orders screens.
/// Clicks inside it must not fall through to the list behind it.
pub static DETAIL_PANEL_RECT: LazyLock<Mutex<Rect>> = LazyLock::new(|| Mutex::new(Rect::default()));

/// Rect of the detail panel's close button (`✕` in its top-right corner).
pub static DETAIL_CLOSE_RECT: LazyLock<Mutex<Rect>> = LazyLock::new(|| Mutex::new(Rect::default()));

pub static POPUP_LIST_RECT: LazyLock<Mutex<Rect>> = LazyLock::new(|| Mutex::new(Rect::default()));

/// Click targets for the settings modal's value chips, as (rect, row, choice).
pub static SETTINGS_CHIP_RECTS: LazyLock<Mutex<Vec<(Rect, usize, usize)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

pub static NEWS_LIST_RECT: LazyLock<Mutex<Rect>> = LazyLock::new(|| Mutex::new(Rect::default()));

/// Body of the news article pane — the wheel scrolls the article here, rather
/// than moving the selection in the list beside it.
pub static NEWS_DETAIL_RECT: LazyLock<Mutex<Rect>> = LazyLock::new(|| Mutex::new(Rect::default()));

/// The `News [n]` affordance on the stock panel's top border.
pub static NEWS_OPEN_RECT: LazyLock<Mutex<Rect>> = LazyLock::new(|| Mutex::new(Rect::default()));

/// The `Back [esc]` affordance on the news list's top border.
pub static NEWS_BACK_RECT: LazyLock<Mutex<Rect>> = LazyLock::new(|| Mutex::new(Rect::default()));

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
