//! Data-driven user settings (Grok-style).
//!
//! Settings are declared once as a table of [`SettingMeta`] records
//! ([`all`]); the modal in `views/settings.rs` renders that table and edits
//! live state, and changes are applied immediately and persisted via
//! [`store`]. Adding a setting = adding one row to [`all`] plus a match arm in
//! [`SettingId::current`] / [`SettingId::apply`].

pub mod store;

use std::sync::atomic::{AtomicUsize, Ordering};

use crossterm::event::{KeyCode, KeyEvent};

use crate::ai::settings as chat;
use crate::data::StockColorMode;
use crate::tui::keymap::{ActionId, Context};
use crate::tui::popup::{self, PopupKind};
use crate::tui::ui::styles;

/// Identifies a setting.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingId {
    StockColorMode,
    ToolCalls,
    NotifyOnFinish,
    QuoteCards,
}

/// Where a setting is offered.
///
/// One table, two places that show it: the market TUI's modal and the `ai`
/// chat's Settings view. A row is listed where it means something — offering
/// "show tool calls" in a watchlist would be noise — while [`Scope::Everywhere`]
/// covers what both share, so the up/down colour convention is set once and
/// holds across the whole terminal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    Everywhere,
    Market,
    Chat,
}

impl SettingId {
    /// The current canonical value from live state.
    #[must_use]
    pub fn current(self) -> &'static str {
        match self {
            SettingId::StockColorMode => match styles::stock_color_mode() {
                StockColorMode::RedUp => "red_up",
                StockColorMode::GreenUp => "green_up",
            },
            SettingId::ToolCalls => match chat::tool_calls() {
                chat::ToolCalls::All => "all",
                chat::ToolCalls::Failures => "failures",
                chat::ToolCalls::Off => "off",
            },
            SettingId::NotifyOnFinish => on_off(chat::notify_on_finish()),
            SettingId::QuoteCards => on_off(chat::quote_cards()),
        }
    }

    /// Apply a canonical value to live state (does not persist).
    fn apply(self, canonical: &str) {
        match self {
            SettingId::StockColorMode => {
                let mode = match canonical {
                    "green_up" => StockColorMode::GreenUp,
                    _ => StockColorMode::RedUp,
                };
                styles::set_stock_color_mode(mode);
            }
            SettingId::ToolCalls => chat::set_tool_calls(match canonical {
                "failures" => chat::ToolCalls::Failures,
                "off" => chat::ToolCalls::Off,
                _ => chat::ToolCalls::All,
            }),
            SettingId::NotifyOnFinish => chat::set_notify_on_finish(canonical == "on"),
            SettingId::QuoteCards => chat::set_quote_cards(canonical == "on"),
        }
    }
}

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}

/// A selectable choice for an enum setting.
pub struct EnumChoice {
    /// Canonical persisted value.
    pub canonical: &'static str,
    /// i18n key for the display label.
    pub label: &'static str,
}

/// The kind/domain of a setting (value type + choices).
pub enum SettingKind {
    Enum { choices: &'static [EnumChoice] },
}

/// A declarative setting definition.
pub struct SettingMeta {
    pub id: SettingId,
    /// i18n key for the row label.
    pub label: &'static str,
    /// i18n key for the description line.
    pub description: &'static str,
    pub kind: SettingKind,
    pub scope: Scope,
}

impl SettingMeta {
    /// Whether this row belongs in `scope`'s list.
    #[must_use]
    pub fn shown_in(&self, scope: Scope) -> bool {
        self.scope == Scope::Everywhere || self.scope == scope
    }

    /// The value after this one, wrapping — what one keypress does.
    #[must_use]
    pub fn next_value(&self) -> &'static str {
        match &self.kind {
            SettingKind::Enum { choices } => {
                let cur = self.id.current();
                let i = choices.iter().position(|c| c.canonical == cur).unwrap_or(0);
                choices[(i + 1) % choices.len()].canonical
            }
        }
    }

    /// The i18n key labelling the current value.
    #[must_use]
    pub fn value_label(&self) -> &'static str {
        match &self.kind {
            SettingKind::Enum { choices } => {
                let cur = self.id.current();
                choices
                    .iter()
                    .find(|c| c.canonical == cur)
                    .map_or("", |c| c.label)
            }
        }
    }
}

const STOCK_COLOR_CHOICES: &[EnumChoice] = &[
    EnumChoice {
        canonical: "red_up",
        label: "settings.stock_color.red_up",
    },
    EnumChoice {
        canonical: "green_up",
        label: "settings.stock_color.green_up",
    },
];

const TOOL_CALL_CHOICES: &[EnumChoice] = &[
    EnumChoice {
        canonical: "all",
        label: "settings.tool_calls.all",
    },
    EnumChoice {
        canonical: "failures",
        label: "settings.tool_calls.failures",
    },
    EnumChoice {
        canonical: "off",
        label: "settings.off",
    },
];

const ON_OFF_CHOICES: &[EnumChoice] = &[
    EnumChoice {
        canonical: "on",
        label: "settings.on",
    },
    EnumChoice {
        canonical: "off",
        label: "settings.off",
    },
];

/// The setting registry: the single source of truth for every settings list.
#[must_use]
pub fn all() -> &'static [SettingMeta] {
    static ALL: &[SettingMeta] = &[
        SettingMeta {
            id: SettingId::StockColorMode,
            label: "settings.stock_color.label",
            description: "settings.stock_color.description",
            kind: SettingKind::Enum {
                choices: STOCK_COLOR_CHOICES,
            },
            scope: Scope::Everywhere,
        },
        SettingMeta {
            id: SettingId::ToolCalls,
            label: "settings.tool_calls.label",
            description: "settings.tool_calls.description",
            kind: SettingKind::Enum {
                choices: TOOL_CALL_CHOICES,
            },
            scope: Scope::Chat,
        },
        SettingMeta {
            id: SettingId::NotifyOnFinish,
            label: "settings.notify_on_finish.label",
            description: "settings.notify_on_finish.description",
            kind: SettingKind::Enum {
                choices: ON_OFF_CHOICES,
            },
            scope: Scope::Chat,
        },
        SettingMeta {
            id: SettingId::QuoteCards,
            label: "settings.quote_cards.label",
            description: "settings.quote_cards.description",
            kind: SettingKind::Enum {
                choices: ON_OFF_CHOICES,
            },
            scope: Scope::Chat,
        },
    ];
    ALL
}

/// The rows shown in `scope`, in table order.
#[must_use]
pub fn in_scope(scope: Scope) -> Vec<&'static SettingMeta> {
    all().iter().filter(|m| m.shown_in(scope)).collect()
}

/// Apply a row's next value and persist. The editing convention is shared, so
/// both lists advance a setting the same way.
pub fn cycle(meta: &SettingMeta) {
    meta.id.apply(meta.next_value());
    persist();
}

// ---- Modal selection state ----

static SELECTED: AtomicUsize = AtomicUsize::new(0);

/// Open the settings modal, resetting the selection to the first row.
pub fn open() {
    SELECTED.store(0, Ordering::Relaxed);
    popup::open(PopupKind::Settings);
}

/// The rows the modal shows: the market TUI's own, plus what everything shares.
#[must_use]
pub fn modal_rows() -> Vec<&'static SettingMeta> {
    in_scope(Scope::Market)
}

/// The currently highlighted row (clamped to the modal's length).
#[must_use]
pub fn selected() -> usize {
    SELECTED
        .load(Ordering::Relaxed)
        .min(modal_rows().len().saturating_sub(1))
}

fn select_next() {
    let n = modal_rows().len();
    if n > 0 {
        SELECTED.store((selected() + 1) % n, Ordering::Relaxed);
    }
}

fn select_prev() {
    let n = modal_rows().len();
    if n > 0 {
        SELECTED.store((selected() + n - 1) % n, Ordering::Relaxed);
    }
}

/// Cycle the highlighted setting to its next choice, applying it live and
/// persisting immediately (Grok-style: no separate Save step).
fn cycle_selected() {
    if let Some(meta) = modal_rows().get(selected()) {
        cycle(meta);
    }
}

/// Snapshot live state into a [`store::Config`] and write it to disk.
fn persist() {
    let config = store::Config {
        stock_color_mode: Some(styles::stock_color_mode()),
        chat_tool_calls: Some(chat::tool_calls()),
        chat_notify_on_finish: Some(chat::notify_on_finish()),
        chat_quote_cards: Some(chat::quote_cards()),
    };
    store::save(&config);
}

/// Load persisted settings from disk and apply them to live state. Call once
/// at startup, before anything renders.
pub fn load_and_apply() {
    let config = store::load();
    if let Some(mode) = config.stock_color_mode {
        styles::set_stock_color_mode(mode);
    }
    if let Some(value) = config.chat_tool_calls {
        chat::set_tool_calls(value);
    }
    if let Some(on) = config.chat_notify_on_finish {
        chat::set_notify_on_finish(on);
    }
    if let Some(on) = config.chat_quote_cards {
        chat::set_quote_cards(on);
    }
}

/// Handle a key event while the settings modal is open. Reuses the shared
/// [`crate::tui::keymap`] for navigation so the bindings stay consistent.
pub fn handle_key(event: KeyEvent) {
    // Space toggles the highlighted enum, matching common TUI conventions.
    if event.code == KeyCode::Char(' ') {
        cycle_selected();
        return;
    }
    match crate::tui::keymap::global().lookup(&event, Context::Always) {
        Some(ActionId::Up) => select_prev(),
        Some(ActionId::Down) => select_next(),
        Some(ActionId::Enter) => cycle_selected(),
        Some(ActionId::Escape | ActionId::OpenSettings) => popup::close(),
        _ => {}
    }
}
