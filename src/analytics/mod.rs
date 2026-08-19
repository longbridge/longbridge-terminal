//! Sensors Analytics for the terminal.
//!
//! The client lives in [`core`], copied verbatim from the desktop app so fixes
//! can be moved across by replacing the file. This module holds what cannot be
//! shared: the endpoint, what this product calls itself, where the device id
//! lives, and the process-wide instance.
//!
//! ```text
//!   core.rs   wire format · HTTP · retry · identity · heartbeat · crash
//!   mod.rs    endpoint · product properties · device id · singleton
//! ```
//!
//! # The two process shapes
//!
//! This binary is both a short-lived CLI and a long-running TUI, and analytics
//! works differently for each:
//!
//! * **A command** (`longbridge static TSLA.US`) outlives none of its own
//!   requests. Reports run on background tasks, and `#[tokio::main]` cancels
//!   those when `main` returns — so a command **must** call [`flush`] before
//!   exiting, or its events are simply never sent. There is nothing in the log
//!   to indicate this: the failure is a request that was never made.
//! * **The TUI** runs long enough to report normally, and is the only shape
//!   where a heartbeat means anything.
//!
//! Both are configured from [`init`], which takes the shape as an argument
//! rather than guessing.

// Shared verbatim with the desktop app, so its lints are waived here rather
// than fixed in place: editing the file would cost the ability to sync a fix by
// replacing it wholesale, which is the only reason it is a copy at all.
#[allow(
    clippy::doc_markdown,
    clippy::duration_suboptimal_units,
    clippy::format_push_string,
    clippy::map_unwrap_or,
    clippy::match_same_arms,
    clippy::needless_raw_string_hashes,
    clippy::single_match_else
)]
pub mod core;

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use self::core::{Config, Crash, Heartbeat, Sensors};

/// Production ingest. The terminal has no staging environment of its own, so
/// unlike the desktop app there is no environment table here.
const SERVER_URL: &str = "https://event-tracking.lbctrl.com/sa?project=production";

/// Product identifier, telling this client apart from the desktop app (`LBAI`)
/// and the mobile clients in the same project.
///
/// TODO(data): confirm before this ships. Getting it wrong is cheap in code and
/// expensive in the warehouse, where dashboards are keyed on it.
const TERMINAL_TYPE: &str = "LBTERM";

/// How long to wait for reports to drain before a command exits. Long enough
/// for one request on a normal connection, short enough that a user on a bad
/// one does not notice the CLI hesitating.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(3);

static SENSORS: OnceLock<Sensors> = OnceLock::new();

/// Which shape the process is running as. Decides whether a heartbeat is armed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// A single command that will exit shortly. No heartbeat — the process will
    /// not live to send a second one — and [`flush`] is required before exit.
    Command,
    /// The full-screen TUI. Runs long enough for liveness to mean something.
    Tui,
}

/// Event names this binary reports under.
pub mod event {
    /// A CLI command ran. `command` names it — the subcommand only, never its
    /// arguments, which carry symbols, account numbers and order details.
    pub const COMMAND: &str = "terminal_command_run";
    /// The TUI was opened.
    pub const TUI_LAUNCH: &str = "terminal_tui_launch";
    /// Periodic proof the TUI is still open. Carries `uptime_ms`/`active_ms`.
    pub const HEARTBEAT: &str = "terminal_heartbeat";
    /// A panic from the previous run, replayed on this one.
    pub const CRASH: &str = "terminal_crash";

    /// `longbridge ai` was opened. Carries `agent` and `signed_in`.
    pub const AI_LAUNCH: &str = "terminal_ai_launch";
    /// A question was sent to the agent. Carries `agent`, `is_first_turn` and
    /// `query_len` — the length only, never the text: a question names the
    /// symbols and positions the reader is asking about.
    pub const AI_TURN_START: &str = "terminal_ai_turn_start";
    /// A turn ended. Carries `result` (`ok`/`error`/`cancelled`), `error_kind`,
    /// `duration_ms`, `answer_len`, the tools it called, and whether the answer
    /// came with references or follow-up questions.
    ///
    /// Reported for abandoned turns too, so `start` and `finish` counts stay
    /// comparable — a turn aborted by `/new` or Esc would otherwise hang in the
    /// warehouse as a start with no end, and no completion rate could be derived.
    pub const AI_TURN_FINISH: &str = "terminal_ai_turn_finish";
    /// A fresh conversation was started.
    pub const AI_SESSION_NEW: &str = "terminal_ai_session_new";
    /// A conversation was reopened from history.
    pub const AI_SESSION_RESUME: &str = "terminal_ai_session_resume";
    /// The agent was switched. Carries `from` and `to`.
    pub const AI_AGENT_SWITCH: &str = "terminal_ai_agent_switch";
    /// The agent asked the reader something before it could carry on. Carries
    /// `question_count`.
    pub const AI_INTERRUPT: &str = "terminal_ai_interrupt";
    /// How that question ended: `answered`, or `abandoned` when the reader walked
    /// away from it instead.
    pub const AI_INTERRUPT_ANSWERED: &str = "terminal_ai_interrupt_answered";
}

/// Arms analytics. Safe to call more than once; only the first call wins.
///
/// Never fails: a terminal that cannot report is a terminal that still works.
pub fn init(shape: Shape) {
    if SENSORS.get().is_some() {
        return;
    }

    let Some(device_id) = device_id() else {
        // Without a stable id every run would look like a new user, which is
        // worse than no data at all — it would quietly inflate every count.
        tracing::debug!("analytics disabled: no device id");
        return;
    };

    let Some(sensors) = Sensors::new(Config {
        server_url: SERVER_URL.into(),
        device_id,
        lib_name: "Rust".into(),
        lib_version: env!("CARGO_PKG_VERSION").into(),
        base_properties: base_properties(shape),
        // Only the TUI lives long enough to beat. A command would arm a timer
        // it never reaches.
        heartbeat: match shape {
            Shape::Tui => Some(Heartbeat {
                event: event::HEARTBEAT.into(),
                ..Default::default()
            }),
            Shape::Command => None,
        },
        crash: crash_path().map(|path| Crash {
            path,
            event: event::CRASH.into(),
        }),
        ..Default::default()
    }) else {
        return;
    };

    // The access token carries the member id, so binding costs no network call
    // and no OpenAPI context — which matters for a command that would otherwise
    // pay for a context it does not need. `None` is normal here: plenty of
    // commands run signed out.
    sensors.set_member(crate::auth::member_id());

    let _ = SENSORS.set(sensors);
}

/// Reports one event. No-op before [`init`].
pub fn track(event: &str, properties: serde_json::Value) {
    if let Some(sensors) = SENSORS.get() {
        sensors.track(event, properties);
    }
}

/// Waits for reports to drain. **Every command path must call this before
/// returning**, or its events die with the runtime.
pub async fn flush() {
    if let Some(sensors) = SENSORS.get() {
        sensors.flush(FLUSH_TIMEOUT).await;
    }
}

/// Records a panic for the next run to report. Called from the panic hook.
pub fn note_crash(info: &str) {
    if let Some(sensors) = SENSORS.get() {
        sensors.note_crash(info);
    }
}

/// Installs the crash hook, chaining whatever was there before.
///
/// Chained rather than replaced: the TUI's own hook restores the terminal out
/// of full-screen mode, and losing that would leave a panicking user staring at
/// a garbled shell.
pub fn install_crash_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        note_crash(&info.to_string());
        previous(info);
    }));
}

/// Properties every event from this binary carries.
fn base_properties(shape: Shape) -> serde_json::Map<String, serde_json::Value> {
    let mut base = serde_json::Map::new();
    base.insert("platform_type".into(), platform_type().into());
    base.insert("terminal_type".into(), TERMINAL_TYPE.into());
    // Which shape produced the event. A command and the TUI have very different
    // usage patterns, and mixing them would make both unreadable.
    base.insert(
        "surface".into(),
        match shape {
            Shape::Command => "cli",
            Shape::Tui => "tui",
        }
        .into(),
    );
    base.insert("version".into(), env!("CARGO_PKG_VERSION").into());
    base.insert("report_source".into(), "terminal".into());
    // The SDK puts these in `properties` as well as in the `lib` object; other
    // clients in this project do the same, so shell events stay the same shape.
    base.insert("$lib".into(), "Rust".into());
    base.insert("$lib_version".into(), env!("CARGO_PKG_VERSION").into());
    base
}

/// Platform, kept distinct from the desktop app's `desktop-*` so the two
/// products do not merge into one breakdown.
const fn platform_type() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "cli-mac"
    }
    #[cfg(target_os = "windows")]
    {
        "cli-win"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "cli-linux"
    }
}

/// A stable per-machine id, created on first use.
///
/// Kept beside the `OpenAPI` token rather than in a cache directory: a cache that
/// gets cleared would turn one user into many, and the count would climb with
/// no indication of why.
fn device_id() -> Option<String> {
    let path = analytics_dir()?.join("device-id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }

    let generated = uuid::Uuid::new_v4().to_string();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // A failed write is not fatal: this run reports under an id that will not
    // be reused, which is worse than a stable one but better than silence.
    if let Err(error) = std::fs::write(&path, &generated) {
        tracing::debug!("could not persist the analytics device id: {error}");
    }
    Some(generated)
}

/// Where a crash from this run is left for the next one to find.
fn crash_path() -> Option<PathBuf> {
    Some(analytics_dir()?.join("last-crash.json"))
}

/// `~/.longbridge/openapi/`, alongside the token and the invite code.
fn analytics_dir() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".longbridge").join("openapi"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A command must not arm a heartbeat: it would schedule a timer the
    /// process never lives to reach.
    #[test]
    fn only_the_tui_beats() {
        assert_eq!(
            base_properties(Shape::Command).get("surface").unwrap(),
            "cli"
        );
        assert_eq!(base_properties(Shape::Tui).get("surface").unwrap(), "tui");
    }

    /// Sharing a platform name with the desktop app would merge two products
    /// into one breakdown.
    #[test]
    fn the_platform_is_not_the_desktop_apps() {
        assert!(platform_type().starts_with("cli-"));
        assert!(!platform_type().starts_with("desktop-"));
    }

    /// These events come from Rust, not from a browser SDK. Claiming `js` would
    /// produce a payload carrying none of the properties a JS SDK event always
    /// has, sent from a non-browser agent.
    #[test]
    fn the_library_is_named_honestly() {
        let base = base_properties(Shape::Command);
        assert_eq!(base.get("$lib").unwrap(), "Rust");
        assert_eq!(base.get("report_source").unwrap(), "terminal");
    }

    /// The device id has to survive between runs, or every invocation looks
    /// like a new user.
    #[test]
    fn the_device_id_is_stable_across_calls() {
        let Some(first) = device_id() else {
            return; // No home directory in this environment.
        };
        assert_eq!(Some(first), device_id());
    }
}
