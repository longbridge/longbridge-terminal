//! Data-driven keymap.
//!
//! Instead of a flat `KeyConfig` struct compared field-by-field in a long
//! `if event == keys.x` chain, key bindings are declared once as a table of
//! [`ActionDef`] records and resolved at runtime with
//! [`Keymap::lookup`], which matches a key event against the bindings for the
//! current [`Context`] (falling back to [`Context::Always`]).
//!
//! This keeps the binding table as the single source of truth shared by the
//! dispatcher and the navbar shortcut hints, and makes context-aware bindings
//! (e.g. `c` = cancel-order in Orders but select-currency in Portfolio)
//! explicit rather than an accident of `if` ordering.

use std::sync::LazyLock;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::tui::app::AppState;

/// A normalized key chord used to match terminal key events.
///
/// Normalization folds `Shift`+letter and the bare uppercase letter together
/// so that `G` and `Shift`+`g` compare equal regardless of how the terminal
/// reports them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KeyChord {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyChord {
    #[must_use]
    pub const fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    /// Fold case/shift so `Char('G')` and `Shift`+`Char('g')` are equal.
    fn normalized(self) -> Self {
        let KeyChord {
            code,
            mut modifiers,
        } = self;
        let code = match code {
            KeyCode::Char(c) if c.is_ascii_uppercase() => {
                // A bare uppercase letter implies Shift.
                modifiers |= KeyModifiers::SHIFT;
                KeyCode::Char(c)
            }
            KeyCode::Char(c)
                if c.is_ascii_lowercase() && modifiers.contains(KeyModifiers::SHIFT) =>
            {
                // Shift+lowercase is reported as uppercase by some terminals;
                // normalize to the uppercase form.
                KeyCode::Char(c.to_ascii_uppercase())
            }
            other => other,
        };
        KeyChord { code, modifiers }
    }

    /// Whether this chord matches a raw key event (ignoring key-release).
    #[must_use]
    pub fn matches(self, ev: &KeyEvent) -> bool {
        if ev.kind == KeyEventKind::Release {
            return false;
        }
        self.normalized() == KeyChord::new(ev.code, ev.modifiers).normalized()
    }
}

// Terse constructors for the binding table below.
const fn c(ch: char) -> KeyChord {
    KeyChord::new(KeyCode::Char(ch), KeyModifiers::NONE)
}
const fn ctrl(ch: char) -> KeyChord {
    KeyChord::new(KeyCode::Char(ch), KeyModifiers::CONTROL)
}
const fn shift(ch: char) -> KeyChord {
    KeyChord::new(KeyCode::Char(ch), KeyModifiers::SHIFT)
}
const fn code(code: KeyCode) -> KeyChord {
    KeyChord::new(code, KeyModifiers::NONE)
}

/// The context in which a binding applies. Resolution tries the active
/// screen's context first, then [`Context::Always`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Context {
    /// Applies on any interactive screen.
    Always,
    Watchlist,
    WatchlistStock,
    Stock,
    Portfolio,
    Orders,
}

impl Context {
    /// The context-specific bucket for the active application state.
    /// States without their own bindings map to [`Context::Always`].
    #[must_use]
    pub fn from_state(state: AppState) -> Self {
        match state {
            AppState::Watchlist => Context::Watchlist,
            AppState::WatchlistStock => Context::WatchlistStock,
            AppState::Stock => Context::Stock,
            AppState::Portfolio => Context::Portfolio,
            AppState::Orders => Context::Orders,
            _ => Context::Always,
        }
    }
}

/// A semantic action a binding maps to. The dispatcher matches on this instead
/// of on raw key events.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum ActionId {
    // Global / navigation between top-level screens.
    Quit,
    ForceQuit,
    Search,
    Help,
    ToggleLog,
    OpenSettings,
    TabWatchlist,
    TabPortfolio,
    TabOrders,
    // Trading.
    Buy,
    Sell,
    CancelOrder,
    ModifyOrder,
    DateFilter,
    // Selectors / pickers.
    AccountSelector,
    CurrencySelector,
    GroupSelector,
    // Index shortcuts.
    IndexUs,
    IndexHk,
    IndexCn,
    // View controls.
    ToggleLayout,
    Refresh,
    // News (WatchlistStock).
    NewsToggle,
    NewsOpen,
    NewsScrollUp,
    NewsScrollDown,
    // Cursor navigation (behavior is dispatched per screen).
    Up,
    Down,
    Left,
    Right,
    Tab,
    BackTab,
    Enter,
    Escape,
}

/// One declarative binding: an action available under a context, bound to a
/// default chord plus optional alternates, with metadata for the navbar hints.
pub struct ActionDef {
    pub id: ActionId,
    pub context: Context,
    pub default_chord: KeyChord,
    pub alt_chords: Vec<KeyChord>,
    /// i18n key for the navbar hint label (value carries its own `[key]`).
    pub label: &'static str,
    /// Whether to surface this binding in the navbar hint row.
    pub hint: bool,
}

impl ActionDef {
    fn matches(&self, ev: &KeyEvent) -> bool {
        self.default_chord.matches(ev) || self.alt_chords.iter().copied().any(|ch| ch.matches(ev))
    }
}

/// Terse constructor for a binding-table row.
fn def(
    id: ActionId,
    context: Context,
    default_chord: KeyChord,
    alt: &[KeyChord],
    label: &'static str,
    hint: bool,
) -> ActionDef {
    ActionDef {
        id,
        context,
        default_chord,
        alt_chords: alt.to_vec(),
        label,
        hint,
    }
}

/// The default binding table: the single source of truth for key bindings.
/// Mirrors the behavior the old `KeyConfig` + `input.rs` if-chain encoded,
/// but organized by context. `label` is an i18n key; `hint` marks bindings
/// surfaced in the navbar hint row.
fn default_actions() -> Vec<ActionDef> {
    use ActionId as A;
    use Context::{Always, Orders, Portfolio, Stock, Watchlist, WatchlistStock};

    vec![
        // ---- Global ----
        def(A::ForceQuit, Always, ctrl('c'), &[], "Keyboard.Quit", false),
        def(A::Quit, Always, c('q'), &[], "Keyboard.Quit", false),
        def(A::Search, Always, c('/'), &[], "Keyboard.Search", true),
        def(A::Help, Always, c('?'), &[], "Keyboard.Help", true),
        def(A::ToggleLog, Always, c('`'), &[], "Keyboard.Console", false),
        def(
            A::OpenSettings,
            Always,
            ctrl(','),
            &[],
            "Keyboard.Settings",
            true,
        ),
        def(
            A::TabWatchlist,
            Always,
            c('1'),
            &[],
            "tabs.Watchlist",
            false,
        ),
        def(
            A::TabPortfolio,
            Always,
            c('2'),
            &[],
            "tabs.Portfolio",
            false,
        ),
        def(A::TabOrders, Always, c('3'), &[], "tabs.Orders", false),
        def(A::Buy, Always, c('b'), &[], "Keyboard.Buy", false),
        def(A::Sell, Always, c('s'), &[], "Keyboard.Sell", false),
        def(A::IndexUs, Always, shift('Q'), &[], "-", false),
        def(A::IndexHk, Always, shift('W'), &[], "-", false),
        def(A::IndexCn, Always, shift('E'), &[], "-", false),
        def(
            A::Refresh,
            Always,
            shift('R'),
            &[],
            "Keyboard.Refresh",
            false,
        ),
        // ---- Cursor navigation (Always; behavior dispatched per screen) ----
        def(
            A::Up,
            Always,
            code(KeyCode::Up),
            &[c('k'), shift('k')],
            "-",
            false,
        ),
        def(
            A::Down,
            Always,
            code(KeyCode::Down),
            &[c('j'), shift('j')],
            "-",
            false,
        ),
        def(
            A::Left,
            Always,
            code(KeyCode::Left),
            &[c('h'), shift('h')],
            "-",
            false,
        ),
        def(
            A::Right,
            Always,
            code(KeyCode::Right),
            &[c('l'), shift('l')],
            "-",
            false,
        ),
        def(A::Tab, Always, code(KeyCode::Tab), &[], "-", false),
        def(A::BackTab, Always, code(KeyCode::BackTab), &[], "-", false),
        def(A::Enter, Always, code(KeyCode::Enter), &[], "-", false),
        def(A::Escape, Always, code(KeyCode::Esc), &[], "-", false),
        // ---- Orders ----
        def(
            A::CancelOrder,
            Orders,
            c('c'),
            &[],
            "Keyboard.CancelOrder",
            true,
        ),
        def(
            A::ModifyOrder,
            Orders,
            c('m'),
            &[],
            "Keyboard.ModifyOrder",
            true,
        ),
        def(
            A::DateFilter,
            Orders,
            c('f'),
            &[],
            "Keyboard.DateFilter",
            true,
        ),
        // ---- Portfolio ----
        def(
            A::AccountSelector,
            Portfolio,
            c('a'),
            &[],
            "Keyboard.Account",
            true,
        ),
        def(
            A::CurrencySelector,
            Portfolio,
            c('c'),
            &[],
            "Keyboard.Currency",
            true,
        ),
        // ---- Watchlist group selector (Watchlist + WatchlistStock) ----
        def(
            A::GroupSelector,
            Watchlist,
            c('g'),
            &[shift('g')],
            "Keyboard.Group",
            true,
        ),
        def(
            A::GroupSelector,
            WatchlistStock,
            c('g'),
            &[shift('g')],
            "Keyboard.Group",
            true,
        ),
        // ---- Layout toggle (Stock + WatchlistStock) ----
        def(A::ToggleLayout, Stock, c('t'), &[], "Keyboard.Layout", true),
        def(
            A::ToggleLayout,
            WatchlistStock,
            c('t'),
            &[],
            "Keyboard.Layout",
            true,
        ),
        // ---- News (WatchlistStock) ----
        def(
            A::NewsToggle,
            WatchlistStock,
            c('n'),
            &[],
            "Keyboard.News",
            true,
        ),
        def(
            A::NewsOpen,
            WatchlistStock,
            c('o'),
            &[],
            "Keyboard.NewsOpen",
            true,
        ),
        def(
            A::NewsScrollUp,
            WatchlistStock,
            code(KeyCode::PageUp),
            &[shift('k')],
            "-",
            false,
        ),
        def(
            A::NewsScrollDown,
            WatchlistStock,
            code(KeyCode::PageDown),
            &[shift('j')],
            "-",
            false,
        ),
    ]
}

/// The runtime binding registry.
pub struct Keymap {
    actions: Vec<ActionDef>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            actions: default_actions(),
        }
    }
}

/// The process-wide keymap. Stateless (default bindings), so a single shared
/// instance serves both input dispatch and rendering. Replace with a
/// `Resource` if per-session user rebinding is added.
pub fn global() -> &'static Keymap {
    static KEYMAP: LazyLock<Keymap> = LazyLock::new(Keymap::default);
    &KEYMAP
}

impl Keymap {
    /// Resolve a key event to an action, trying the given context first and
    /// then [`Context::Always`]. Returns `None` if nothing is bound.
    #[must_use]
    pub fn lookup(&self, ev: &KeyEvent, ctx: Context) -> Option<ActionId> {
        self.find_in(ev, ctx)
            .or_else(|| self.find_in(ev, Context::Always))
    }

    fn find_in(&self, ev: &KeyEvent, ctx: Context) -> Option<ActionId> {
        self.actions
            .iter()
            .find(|a| a.context == ctx && a.matches(ev))
            .map(|a| a.id)
    }

    /// Bindings to show in the navbar hint row for the given context: the
    /// context-specific hints first, then a fixed global tail
    /// (search / help / log / quit).
    #[must_use]
    pub fn navbar_hints(&self, ctx: Context) -> Vec<&ActionDef> {
        let mut out: Vec<&ActionDef> = self
            .actions
            .iter()
            .filter(|a| a.hint && a.context == ctx)
            .collect();
        for id in [
            ActionId::Search,
            ActionId::Help,
            ActionId::OpenSettings,
            ActionId::ToggleLog,
            ActionId::ForceQuit,
        ] {
            if let Some(a) = self
                .actions
                .iter()
                .find(|a| a.id == id && a.context == Context::Always)
            {
                out.push(a);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }
    fn ch(c: char, mods: KeyModifiers) -> KeyEvent {
        ev(KeyCode::Char(c), mods)
    }

    #[test]
    fn context_disambiguates_same_key() {
        let km = Keymap::default();
        // `c` means different things per screen; `Ctrl+C` is always force-quit.
        assert_eq!(
            km.lookup(&ch('c', KeyModifiers::NONE), Context::Orders),
            Some(ActionId::CancelOrder)
        );
        assert_eq!(
            km.lookup(&ch('c', KeyModifiers::NONE), Context::Portfolio),
            Some(ActionId::CurrencySelector)
        );
        // No bare `c` in Watchlist/Always -> unbound.
        assert_eq!(
            km.lookup(&ch('c', KeyModifiers::NONE), Context::Watchlist),
            None
        );
        assert_eq!(
            km.lookup(&ch('c', KeyModifiers::CONTROL), Context::Orders),
            Some(ActionId::ForceQuit)
        );
    }

    #[test]
    fn global_fallback_applies_everywhere() {
        let km = Keymap::default();
        assert_eq!(
            km.lookup(&ch('R', KeyModifiers::SHIFT), Context::Portfolio),
            Some(ActionId::Refresh)
        );
        assert_eq!(
            km.lookup(&ch('1', KeyModifiers::NONE), Context::Orders),
            Some(ActionId::TabWatchlist)
        );
        assert_eq!(
            km.lookup(&ch('q', KeyModifiers::NONE), Context::Stock),
            Some(ActionId::Quit)
        );
    }

    #[test]
    fn case_shift_normalization() {
        let km = Keymap::default();
        // `g` and `G` (as Shift) both open the group selector.
        assert_eq!(
            km.lookup(&ch('g', KeyModifiers::NONE), Context::Watchlist),
            Some(ActionId::GroupSelector)
        );
        assert_eq!(
            km.lookup(&ch('G', KeyModifiers::SHIFT), Context::Watchlist),
            Some(ActionId::GroupSelector)
        );
        assert_eq!(
            km.lookup(&ch('G', KeyModifiers::NONE), Context::Watchlist),
            Some(ActionId::GroupSelector)
        );
    }

    #[test]
    fn navigation_letters_and_arrows() {
        let km = Keymap::default();
        assert_eq!(
            km.lookup(&ch('j', KeyModifiers::NONE), Context::Stock),
            Some(ActionId::Down)
        );
        assert_eq!(
            km.lookup(&ev(KeyCode::Up, KeyModifiers::NONE), Context::Stock),
            Some(ActionId::Up)
        );
    }

    #[test]
    fn shift_jk_is_news_scroll_only_in_watchlist_stock() {
        let km = Keymap::default();
        // Shift+K scrolls news in WatchlistStock, but is Up elsewhere.
        assert_eq!(
            km.lookup(&ch('K', KeyModifiers::SHIFT), Context::WatchlistStock),
            Some(ActionId::NewsScrollUp)
        );
        assert_eq!(
            km.lookup(&ch('K', KeyModifiers::SHIFT), Context::Stock),
            Some(ActionId::Up)
        );
    }

    #[test]
    fn key_release_is_ignored() {
        let km = Keymap::default();
        let mut release = ch('q', KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert_eq!(km.lookup(&release, Context::Watchlist), None);
    }

    #[test]
    fn navbar_hints_lead_with_context_then_global_tail() {
        let km = Keymap::default();
        let orders: Vec<ActionId> = km
            .navbar_hints(Context::Orders)
            .iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(
            orders,
            vec![
                ActionId::CancelOrder,
                ActionId::ModifyOrder,
                ActionId::DateFilter,
                ActionId::Search,
                ActionId::Help,
                ActionId::OpenSettings,
                ActionId::ToggleLog,
                ActionId::ForceQuit,
            ]
        );
    }

    #[test]
    fn no_duplicate_binding_within_a_context() {
        let actions = default_actions();
        for (i, a) in actions.iter().enumerate() {
            for b in &actions[i + 1..] {
                if a.context == b.context {
                    assert!(
                        a.default_chord != b.default_chord,
                        "duplicate chord {:?} in context {:?}: {:?} vs {:?}",
                        a.default_chord,
                        a.context,
                        a.id,
                        b.id
                    );
                }
            }
        }
    }
}
