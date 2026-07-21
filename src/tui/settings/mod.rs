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

use crate::data::StockColorMode;
use crate::tui::keymap::{ActionId, Context};
use crate::tui::popup::{self, PopupKind};
use crate::tui::ui::styles;

/// Identifies a setting.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingId {
    StockColorMode,
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
        }
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

/// The setting registry: the single source of truth for the settings modal.
#[must_use]
pub fn all() -> &'static [SettingMeta] {
    static ALL: &[SettingMeta] = &[SettingMeta {
        id: SettingId::StockColorMode,
        label: "settings.stock_color.label",
        description: "settings.stock_color.description",
        kind: SettingKind::Enum {
            choices: STOCK_COLOR_CHOICES,
        },
    }];
    ALL
}

// ---- Modal selection state ----

static SELECTED: AtomicUsize = AtomicUsize::new(0);

/// Open the settings modal, resetting the selection to the first row.
pub fn open() {
    SELECTED.store(0, Ordering::Relaxed);
    popup::open(PopupKind::Settings);
}

/// The currently highlighted row (clamped to the table length).
#[must_use]
pub fn selected() -> usize {
    SELECTED
        .load(Ordering::Relaxed)
        .min(all().len().saturating_sub(1))
}

fn select_next() {
    let n = all().len();
    if n > 0 {
        SELECTED.store((selected() + 1) % n, Ordering::Relaxed);
    }
}

fn select_prev() {
    let n = all().len();
    if n > 0 {
        SELECTED.store((selected() + n - 1) % n, Ordering::Relaxed);
    }
}

/// Cycle the highlighted setting to its next choice, applying it live and
/// persisting immediately (Grok-style: no separate Save step).
fn cycle_selected() {
    let meta = &all()[selected()];
    match &meta.kind {
        SettingKind::Enum { choices } => {
            let cur = meta.id.current();
            let idx = choices.iter().position(|c| c.canonical == cur).unwrap_or(0);
            let next = &choices[(idx + 1) % choices.len()];
            meta.id.apply(next.canonical);
        }
    }
    persist();
}

/// Snapshot live state into a [`store::Config`] and write it to disk.
fn persist() {
    let config = store::Config {
        stock_color_mode: Some(styles::stock_color_mode()),
    };
    store::save(&config);
}

/// Load persisted settings from disk and apply them to live state. Call once
/// at startup, before the TUI renders.
pub fn load_and_apply() {
    let config = store::load();
    if let Some(mode) = config.stock_color_mode {
        styles::set_stock_color_mode(mode);
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
