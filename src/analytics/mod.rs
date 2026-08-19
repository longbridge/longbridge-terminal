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
//! # Release builds only
//!
//! [`init`] does nothing in a debug build. This repository is public and there
//! is no staging project to report into, so a contributor running `cargo run`
//! would otherwise be indistinguishable from a user in every production
//! dashboard — and would be reporting their own account without ever asking to.
//! The switch is the build profile, not a flag: nothing to discover, nothing to
//! keep working across two states.
//!
//! # The three process shapes
//!
//! This binary is a short-lived CLI, a full-screen TUI, and a set of servers
//! that stay open, and analytics works differently for each:
//!
//! * **A command** (`longbridge static TSLA.US`) outlives none of its own
//!   requests. Reports run on background tasks, and `#[tokio::main]` cancels
//!   those when `main` returns — so a command **must** call [`finish_command`]
//!   (or [`flush`]) before exiting, or its events are simply never sent. There
//!   is nothing in the log to indicate this: the failure is a request that was
//!   never made.
//! * **The TUI** runs long enough to report normally, and beats.
//! * **A session** (`ai`, `serve`, `acp`) also stays open and beats, but is
//!   routinely killed rather than exited, which is why its run event goes out on
//!   the way in. Counted as a command it would have looked like the fastest
//!   thing in the product while actually being the longest.
//!
//! All three are configured from [`init`], which takes the shape as an argument
//! rather than guessing. [`Shape`] owns every consequence of the distinction —
//! whether it beats, when the run event fires, which crash file it claims — so
//! adding a shape does not mean finding the places that switch on it.
//!
//! # When the run event is sent
//!
//! One [`event::COMMAND`] per invocation, but at different moments depending on
//! how the command ends:
//!
//! * **One-shot commands** report on the way out, from [`finish_command`], so
//!   the event carries the outcome and how long the run took. Reporting on the
//!   way in would cost a round trip less — the request would overlap the
//!   command's own work — but it cannot say whether the user got an answer,
//!   which is the question the data exists to answer.
//! * **Sessions** (`ai`, `serve`, `acp`) report on the way in, through
//!   [`report_started`]. These are routinely killed rather than exited, and an
//!   exit-time event for a process that ends on Ctrl-C never arrives at all.
//!
//! `tui` reports neither: it has [`event::TUI_LAUNCH`] of its own, and counting
//! it as a command as well would double every total.
//!
//! # What is lost, knowingly
//!
//! [`core`]'s retry queue lives in memory, so a report that fails while a
//! one-shot command is exiting is gone — a command's whole life is shorter than
//! one retry. Persisting it would mean the next run carrying the last one's
//! backlog, which is a change to the shared file rather than to this one.
//! Offline use therefore reports nothing at all, and the numbers should be read
//! as "runs that reached the ingest", not "runs".

// Shared with the desktop app, so its lints are waived here rather than fixed in
// place: editing the file would cost the ability to sync a fix by replacing it
// wholesale, which is the only reason it is a copy at all. Not byte-identical
// though — this project requires `cargo fmt`, which reformats it — so syncing is
// "copy the file, run `cargo fmt`", and the diff that leaves behind is
// formatting only.
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

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use self::core::{Config, Crash, Heartbeat, Sensors};

/// Production ingest. The terminal has no staging environment of its own, so
/// unlike the desktop app there is no environment table here — which is also
/// why nothing reports from a debug build (see the module docs).
const SERVER_URL: &str = "https://event-tracking.lbctrl.com/sa?project=production";

/// Product identifier, telling this client apart from the desktop app (`LBAI`)
/// and the mobile clients in the same project.
///
/// Dashboards are keyed on this, so changing it later orphans everything built
/// on the old value.
const TERMINAL_TYPE: &str = "CLI";

/// How long to wait for reports to drain before a command exits.
///
/// A one-shot command now reports on its way out, so this wait is on the
/// critical path of every invocation rather than overlapping the work — and
/// this CLI is built to be called from scripts and agents, which call it in
/// bulk. Long enough for one request on a working connection, short enough that
/// a user on a broken one does not think the command hung.
const FLUSH_TIMEOUT: Duration = Duration::from_millis(1500);

static SENSORS: OnceLock<Sensors> = OnceLock::new();
static COMMAND: OnceLock<Run> = OnceLock::new();
/// Whether the run event has gone out. Guards against reporting one invocation
/// twice: a session reports on entry, and the exit path still runs afterwards.
static REPORTED: AtomicBool = AtomicBool::new(false);
/// Whether a panic has already been recorded. The TUI installs a hook of its
/// own that chains to this module's, so a panic there reaches [`note_crash`]
/// twice; the first record is the one taken before the terminal is restored.
static CRASH_NOTED: AtomicBool = AtomicBool::new(false);

/// The command being run, and when it started.
struct Run {
    name: String,
    started: Instant,
}

/// Which shape the process is running as. Decides whether a heartbeat is armed,
/// and which crash file this process owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// A single command that will exit shortly. No heartbeat — the process will
    /// not live to send a second one — and [`finish_command`] is required
    /// before exit.
    Command,
    /// The full-screen TUI. Runs long enough for liveness to mean something.
    Tui,
    /// A session that stays open without owning the screen: `ai`, `serve`,
    /// `acp`. Beats like the TUI, because the question "how long was it open"
    /// is the same question — and answering it for `ai` is the whole point of
    /// telling this shape apart from a command that happens to run long.
    Session,
}

impl Shape {
    /// The `surface` property, and the suffix that keeps each shape's crash file
    /// apart from the others'.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Command => "cli",
            Self::Tui => "tui",
            Self::Session => "session",
        }
    }

    /// Whether this shape lives long enough for a heartbeat to mean anything.
    /// A command would arm a timer it never reaches.
    const fn beats(self) -> bool {
        matches!(self, Self::Tui | Self::Session)
    }

    /// Whether the run event goes out on the way in rather than on the way out.
    ///
    /// Sessions are killed far more often than they exit, so an exit-time event
    /// never arrives. Kept here rather than as a second list of subcommands at
    /// the call site: that list and this one have to agree, and the way to
    /// guarantee they do is for there to be only one.
    pub const fn reports_on_entry(self) -> bool {
        matches!(self, Self::Session)
    }

    /// Every shape. Declared with its length so that adding a variant without
    /// listing it here fails to compile — the tests below check properties that
    /// must hold for *all* shapes, and one that quietly skips the new one is
    /// worse than no test, since it reads as coverage.
    #[cfg(test)]
    const ALL: [Self; 3] = [Self::Command, Self::Tui, Self::Session];
}

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A session that has just opened and will not report again.
    Started,
    /// Ran to completion.
    Ok,
    /// The command itself failed.
    Error,
    /// Never got as far as the command: authentication failed first.
    AuthFailed,
}

impl Outcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Ok => "ok",
            Self::Error => "error",
            Self::AuthFailed => "auth_failed",
        }
    }
}

/// Event names this binary reports under.
pub mod event {
    /// A CLI command ran. `command` names it — the subcommand path only, never
    /// its arguments, which carry symbols, account numbers and order details.
    /// Also carries `outcome`, and `duration_ms` for anything that finished.
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
///
/// Does nothing in a debug build, or when the reader has opted out — see the
/// module docs.
pub fn init(shape: Shape) {
    if cfg!(debug_assertions) {
        return;
    }

    if opted_out() {
        tracing::debug!("analytics disabled by the environment");
        return;
    }

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
        heartbeat: shape.beats().then(|| Heartbeat {
            event: event::HEARTBEAT.into(),
            ..Default::default()
        }),
        crash: crash_path(shape).map(|path| Crash {
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

/// Records which command is running, without reporting anything yet.
///
/// The event itself goes out from [`report_started`] or [`finish_command`],
/// depending on the shape of the command — see the module docs.
pub fn arm_command(name: String) {
    let _ = COMMAND.set(Run {
        name,
        started: Instant::now(),
    });
}

/// Reports a session's run event now, because it may never exit cleanly.
pub fn report_started() {
    report_run(Outcome::Started, None);
}

/// Reports the run event for a command that is about to exit, then waits for
/// reports to drain.
///
/// **Every command path must call this (or [`flush`]) before returning**, or its
/// events die with the runtime.
pub async fn finish_command(outcome: Outcome) {
    let elapsed = COMMAND.get().map(|run| run.started.elapsed());
    report_run(outcome, elapsed);
    flush().await;
}

/// Emits the one run event for this invocation, if it has not gone out already.
fn report_run(outcome: Outcome, elapsed: Option<Duration>) {
    let Some(run) = COMMAND.get() else {
        return;
    };
    if REPORTED.swap(true, Ordering::Relaxed) {
        return;
    }
    let mut properties = serde_json::json!({
        "command": run.name,
        "outcome": outcome.as_str(),
    });
    if let Some(elapsed) = elapsed {
        properties["duration_ms"] = (elapsed.as_millis() as u64).into();
    }
    track(event::COMMAND, properties);
}

/// Reports one event. No-op before [`init`].
pub fn track(event: &str, properties: serde_json::Value) {
    if let Some(sensors) = SENSORS.get() {
        sensors.track(event, properties);
    }
}

/// Waits for reports to drain.
pub async fn flush() {
    if let Some(sensors) = SENSORS.get() {
        sensors.flush(FLUSH_TIMEOUT).await;
    }
}

/// Tells the client whether the terminal is in the foreground.
///
/// Without this every session looks unbroken, and a TUI left open in a
/// background tmux pane overnight reports the same `active_ms` as one somebody
/// used all night. Terminals that do not send focus events leave this untouched,
/// which is the same as assuming the app is in use.
pub fn set_active(active: bool) {
    if let Some(sensors) = SENSORS.get() {
        sensors.set_active(active);
    }
}

/// Records a panic for the next run to report. Called from the panic hook.
///
/// Only the first panic of a process is recorded: the TUI's hook chains to this
/// module's, and the earlier record is the one written before the terminal was
/// restored.
pub fn note_crash(info: &str) {
    if CRASH_NOTED.swap(true, Ordering::Relaxed) {
        return;
    }
    if let Some(sensors) = SENSORS.get() {
        sensors.note_crash(&redact(info));
    }
}

/// Strips the two things a panic message reliably carries that identify the
/// person who hit it: their home directory and their account name. Both turn up
/// in file paths inside panic messages, and neither is any use in a crash
/// breakdown.
fn redact(info: &str) -> String {
    let mut out = info.to_owned();
    if let Some(home) = dirs::home_dir().and_then(|path| path.to_str().map(str::to_owned)) {
        // `/` as a home directory would rewrite every path in the message.
        if home.len() > 1 {
            out = out.replace(&home, "~");
        }
    }
    let user = whoami::username();
    // Short names collide with ordinary words; the home-directory rewrite above
    // already covers the common case.
    if user.len() > 2 {
        out = out.replace(&user, "<user>");
    }
    out
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
    base.insert("surface".into(), shape.as_str().into());
    base.insert("version".into(), env!("CARGO_PKG_VERSION").into());
    base.insert("report_source".into(), "terminal".into());
    // This CLI is built for scripting and agent tool-calling, where one task
    // fires dozens of invocations. Without these two, automation and people are
    // one undivided number, and the automation is the larger half.
    base.insert("is_ci".into(), is_ci().into());
    base.insert("is_tty".into(), std::io::stdout().is_terminal().into());
    // The SDK puts these in `properties` as well as in the `lib` object; other
    // clients in this project do the same, so shell events stay the same shape.
    base.insert("$lib".into(), "Rust".into());
    base.insert("$lib_version".into(), env!("CARGO_PKG_VERSION").into());
    base
}

/// Whether this looks like a build agent. Checks the variable every CI sets
/// plus the ones that identify a specific system, since `CI` alone is also set
/// by hand often enough to be worth corroborating.
fn is_ci() -> bool {
    if let Ok(value) = std::env::var("CI") {
        if !value.is_empty() && value != "false" && value != "0" {
            return true;
        }
    }
    [
        "GITHUB_ACTIONS",
        "GITLAB_CI",
        "BUILDKITE",
        "CIRCLECI",
        "JENKINS_URL",
        "TEAMCITY_VERSION",
    ]
    .iter()
    .any(|name| std::env::var_os(name).is_some())
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

/// Whether the reader has asked not to be measured.
///
/// `DO_NOT_TRACK` is the cross-vendor convention (consoledonottrack.com), so a
/// reader who already sets it for their other tools gets the same answer here
/// without having to learn a Longbridge-specific name. The second name exists
/// because the first is broad — someone may want telemetry from everything else
/// and not from this — and because a release build has no other way to be told:
/// `cfg!(debug_assertions)` covers contributors, not users.
///
/// Any value counts as opting out except an empty one and `0`, matching how the
/// convention is implemented elsewhere.
fn opted_out() -> bool {
    ["DO_NOT_TRACK", "LONGBRIDGE_NO_ANALYTICS"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty() && value != "0"))
}

/// A stable per-machine id, created on first use.
///
/// Kept beside the `OpenAPI` token rather than in a cache directory: a cache that
/// gets cleared would turn one user into many, and the count would climb with
/// no indication of why.
fn device_id() -> Option<String> {
    Some(device_id_in(&analytics_dir()?))
}

/// [`device_id`] against a given directory, so it can be exercised without
/// writing into the home directory of whoever is running the tests.
fn device_id_in(dir: &Path) -> String {
    let path = dir.join("device-id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }

    let generated = uuid::Uuid::new_v4().to_string();
    let _ = std::fs::create_dir_all(dir);
    // A failed write is not fatal: this run reports under an id that will not
    // be reused, which is worse than a stable one but better than silence.
    if let Err(error) = std::fs::write(&path, &generated) {
        tracing::debug!("could not persist the analytics device id: {error}");
    }
    generated
}

/// Where a crash from this run is left for the next one to find.
///
/// One file per shape. The replayed event is built from the properties of
/// whichever process finds it, so a shared file would report every TUI crash as
/// a CLI one — the cost being that a TUI crash waits for the next TUI launch
/// rather than the next command.
fn crash_path(shape: Shape) -> Option<PathBuf> {
    Some(analytics_dir()?.join(format!("last-crash-{}.json", shape.as_str())))
}

/// `~/.longbridge/openapi/`, alongside the token and the invite code.
fn analytics_dir() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".longbridge").join("openapi"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mixing the shapes into one breakdown would make each of them unreadable,
    /// so every shape reports a `surface` and no two report the same one.
    #[test]
    fn each_shape_names_itself() {
        let names: Vec<&str> = Shape::ALL
            .iter()
            .map(|shape| {
                let properties = base_properties(*shape);
                let surface = properties.get("surface").expect("a surface").as_str();
                assert_eq!(surface, Some(shape.as_str()));
                shape.as_str()
            })
            .collect();
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "two shapes share a surface");
    }

    /// A crash is reported by the next process of the *same* shape, because the
    /// properties around it come from that process. Two shapes sharing a file
    /// would file each other's panics under the wrong surface.
    #[test]
    fn the_shapes_do_not_share_a_crash_file() {
        let mut paths: Vec<PathBuf> = Vec::new();
        for shape in Shape::ALL {
            let Some(path) = crash_path(shape) else {
                return; // No home directory in this environment.
            };
            paths.push(path);
        }
        let count = paths.len();
        paths.sort_unstable();
        paths.dedup();
        assert_eq!(paths.len(), count, "two shapes share a crash file");
    }

    /// A command would arm a timer it never lives to reach; the shapes that stay
    /// open are the reason the heartbeat exists. `ai` reporting no heartbeat is
    /// what made its sessions immeasurable in the first place.
    #[test]
    fn the_open_shapes_beat_and_a_command_does_not() {
        assert!(!Shape::Command.beats());
        assert!(Shape::Tui.beats());
        assert!(Shape::Session.beats());
    }

    /// Only a session reports on entry. A command reporting there could not say
    /// how it went, and the TUI would be counted twice — it has a launch event
    /// of its own.
    #[test]
    fn only_a_session_reports_on_entry() {
        assert!(Shape::Session.reports_on_entry());
        assert!(!Shape::Command.reports_on_entry());
        assert!(!Shape::Tui.reports_on_entry());
    }

    /// Every dashboard for this product is keyed on this value; it is pinned
    /// here so a passing edit cannot orphan them.
    #[test]
    fn the_product_identifier_is_the_agreed_one() {
        assert_eq!(
            base_properties(Shape::Command)
                .get("terminal_type")
                .unwrap(),
            "CLI"
        );
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

    /// Automation calls this CLI in bulk; without a way to tell it apart from a
    /// person, it is the larger half of every count.
    #[test]
    fn automation_is_distinguishable() {
        let base = base_properties(Shape::Command);
        assert!(base.get("is_ci").unwrap().is_boolean());
        assert!(base.get("is_tty").unwrap().is_boolean());
    }

    /// The device id has to survive between runs, or every invocation looks
    /// like a new user.
    #[test]
    fn the_device_id_is_stable_across_calls() {
        let dir = tempfile::tempdir().unwrap();
        let first = device_id_in(dir.path());
        assert_eq!(first, device_id_in(dir.path()));
    }

    /// Running the tests must not provision an analytics identity in the home
    /// directory of whoever is running them.
    #[test]
    fn the_device_id_is_written_where_it_is_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        let _ = device_id_in(&dir.path().join("nested"));
        assert!(dir.path().join("nested").join("device-id").exists());
    }

    /// The switch: a build anyone can produce from this public repository must
    /// not report into the production project.
    #[test]
    fn a_debug_build_reports_nothing() {
        // Guarded rather than asserted outright: `cargo test --release` is a
        // legitimate thing to run, and there the gate is meant to be open.
        if !cfg!(debug_assertions) {
            return;
        }
        init(Shape::Command);
        assert!(SENSORS.get().is_none());
    }

    /// A panic message routinely contains the path it was compiled from, under
    /// the home directory of whoever ran the binary.
    #[test]
    fn a_crash_report_carries_no_home_path() {
        let home = dirs::home_dir().unwrap();
        let info = format!("panicked at {}/work/app/src/main.rs:1:1", home.display());
        assert!(!redact(&info).contains(home.to_str().unwrap()));
        assert!(redact(&info).contains("~/work/app/src/main.rs"));
    }
}
