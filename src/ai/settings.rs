//! Live state for the chat preferences.
//!
//! The declarations live in the shared registry ([`crate::tui::settings`]) so
//! there is one table, one persistence file and one editing convention for the
//! whole terminal. What lives here is only the state those rows read and write —
//! process-wide atomics, like [`crate::tui::ui::styles`]'s colour mode, because
//! the renderers that consult them are plain functions reached from several
//! places (the TUI, `agent chat`) and threading a settings handle through all of
//! them would buy nothing.

use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

/// How much of a turn's tool activity the transcript shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCalls {
    /// Every tool the turn ran.
    All,
    /// Only the ones that failed — a failure changes how much to trust the
    /// answer, a success rarely does.
    Failures,
    /// None: the answer alone.
    Off,
}

impl ToolCalls {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => ToolCalls::Failures,
            2 => ToolCalls::Off,
            _ => ToolCalls::All,
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            ToolCalls::All => 0,
            ToolCalls::Failures => 1,
            ToolCalls::Off => 2,
        }
    }
}

static TOOL_CALLS: AtomicU8 = AtomicU8::new(0);
static NOTIFY_ON_FINISH: AtomicU8 = AtomicU8::new(1);
static QUOTE_CARDS: AtomicU8 = AtomicU8::new(1);
static TAPE: AtomicU8 = AtomicU8::new(1);

pub fn tool_calls() -> ToolCalls {
    ToolCalls::from_u8(TOOL_CALLS.load(Ordering::Relaxed))
}

pub fn set_tool_calls(value: ToolCalls) {
    TOOL_CALLS.store(value.as_u8(), Ordering::Relaxed);
}

/// Whether a finished turn rings the terminal bell when the window is unfocused.
pub fn notify_on_finish() -> bool {
    NOTIFY_ON_FINISH.load(Ordering::Relaxed) == 1
}

pub fn set_notify_on_finish(on: bool) {
    NOTIFY_ON_FINISH.store(u8::from(on), Ordering::Relaxed);
}

/// Whether a security the answer references is drawn as a live quote card.
///
/// Off is not only about looks: a card costs two quote requests per answer, and
/// a reader who does not want them should not pay for them.
pub fn quote_cards() -> bool {
    QUOTE_CARDS.load(Ordering::Relaxed) == 1
}

pub fn set_quote_cards(on: bool) {
    QUOTE_CARDS.store(u8::from(on), Ordering::Relaxed);
}

/// Whether the title bar carries the session's securities and their quotes.
pub fn tape() -> bool {
    TAPE.load(Ordering::Relaxed) == 1
}

pub fn set_tape(on: bool) {
    TAPE.store(u8::from(on), Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_calls_round_trips_through_its_wire_form() {
        for value in [ToolCalls::All, ToolCalls::Failures, ToolCalls::Off] {
            assert_eq!(ToolCalls::from_u8(value.as_u8()), value);
            let json = serde_json::to_string(&value).unwrap();
            assert_eq!(serde_json::from_str::<ToolCalls>(&json).unwrap(), value);
        }
        // Persisted as readable snake_case, not as an opaque number.
        assert_eq!(
            serde_json::to_string(&ToolCalls::Failures).unwrap(),
            "\"failures\""
        );
    }

    /// The defaults are the behaviour before there was a setting: everything on.
    #[test]
    fn defaults_keep_the_previous_behaviour() {
        assert_eq!(ToolCalls::from_u8(0), ToolCalls::All);
        assert!(notify_on_finish());
        assert!(quote_cards());
    }
}
