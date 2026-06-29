use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub struct KeyConfig {
    pub quit: KeyEvent,
    pub force_quit: KeyEvent,
    pub search: KeyEvent,
    pub help: KeyEvent,
    pub tab_watchlist: KeyEvent,
    pub tab_portfolio: KeyEvent,
    pub tab_orders: KeyEvent,
    pub toggle_log: KeyEvent,
    pub buy: KeyEvent,
    pub sell: KeyEvent,
    pub cancel_order: KeyEvent,
    pub modify_order: KeyEvent,
    pub date_filter: KeyEvent,
    pub account_selector: KeyEvent,
    pub currency_selector: KeyEvent,
    pub group_selector: KeyEvent,
    pub group_selector_upper: KeyEvent,
    pub index_us: KeyEvent,
    pub index_hk: KeyEvent,
    pub index_cn: KeyEvent,
    pub toggle_layout: KeyEvent,
    pub refresh: KeyEvent,
    pub news_toggle: KeyEvent,
    pub news_open: KeyEvent,
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn shift_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
}

fn ctrl_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

impl Default for KeyConfig {
    fn default() -> Self {
        Self {
            quit: key(KeyCode::Char('q')),
            force_quit: ctrl_key(KeyCode::Char('c')),
            search: key(KeyCode::Char('/')),
            help: key(KeyCode::Char('?')),
            tab_watchlist: key(KeyCode::Char('1')),
            tab_portfolio: key(KeyCode::Char('2')),
            tab_orders: key(KeyCode::Char('3')),
            toggle_log: key(KeyCode::Char('`')),
            buy: key(KeyCode::Char('b')),
            sell: key(KeyCode::Char('s')),
            cancel_order: key(KeyCode::Char('c')),
            modify_order: key(KeyCode::Char('m')),
            date_filter: key(KeyCode::Char('f')),
            account_selector: key(KeyCode::Char('a')),
            currency_selector: key(KeyCode::Char('c')),
            group_selector: key(KeyCode::Char('g')),
            group_selector_upper: shift_key(KeyCode::Char('G')),
            index_us: shift_key(KeyCode::Char('Q')),
            index_hk: shift_key(KeyCode::Char('W')),
            index_cn: shift_key(KeyCode::Char('E')),
            toggle_layout: key(KeyCode::Char('t')),
            refresh: shift_key(KeyCode::Char('R')),
            news_toggle: key(KeyCode::Char('n')),
            news_open: key(KeyCode::Char('o')),
        }
    }
}
