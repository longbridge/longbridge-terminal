//! Full-screen chat view for `longbridge ai`.
//!
//! Modeled on grok-build's `xai-grok-pager`: a markdown scrollback, a status
//! line, and a multi-line input editor, driven by an async event loop that
//! multiplexes terminal input against the running turn's [`ChatEvent`] stream.
//! The chat view is a pure function of [`ChatState`]; all conversation mutation
//! goes through `state.apply(...)`.
//!
//! Chat is the home view; `/` commands open a Conversations list of the
//! account's saved chats and a Settings panel, and an interrupt that carries
//! options opens a structured Question view. Every view is mouse-aware (scroll
//! to browse, click to select/activate). Answers render as Markdown, and each
//! turn's source references and suggested follow-ups become clickable chips
//! above the prompt.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use crossterm::event::{
    Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use futures::StreamExt;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use rust_i18n::t;
use serde_json::Value;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::task::JoinHandle;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::editor::Editor;
use super::session_store::{self, SessionSummary};
use super::state::{ChatEvent, ChatState, Message, Role, ToolStatus};
use super::{markdown, runtime};
use crate::cli::agent::client::ConversationRequest;
use crate::cli::agent::DEFAULT_AGENT_UID;
use crate::tui::ui::assets;
use crate::tui::widgets::Terminal;
use crate::utils::text::{strip_control_chars, truncate_width};

/// Which view is on screen. `Chat` is home; the rest are reached with `/`
/// commands (or, for `Question`, by an interrupt) and left with Esc.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Chat,
    Sessions,
    Settings,
    Question,
}

/// Transcript lines a page-scroll keystroke moves.
const SCROLL_PAGE: u16 = 5;

/// Braille spinner frames for the "generating" status line.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The reader's own turns get a band of their own so a long transcript is
/// scannable. Foreground is set alongside the background: there is no theme
/// layer yet, and a background alone would be unreadable against a light
/// terminal's default dark text.
const USER_BG: Color = Color::Rgb(38, 45, 60);
const USER_FG: Color = Color::Rgb(226, 232, 240);

// History list palette: a subtle selected-row background and index badge tints.
const SEL_BG: Color = Color::Rgb(45, 50, 62);
const IDX: Color = Color::Rgb(110, 140, 190);
const IDX_SEL: Color = Color::Rgb(240, 150, 90);

/// One row of the Settings view: a preference, or something to do.
enum SettingsRow {
    Setting(&'static crate::tui::settings::SettingMeta),
    /// Sign out, then leave — the contexts this process built are bound to the
    /// credentials it started with.
    SignOut,
    /// Sign in, for a session whose token has expired or been cleared.
    SignIn,
}

/// The Settings view's rows: the chat's preferences off the shared registry,
/// then the session actions.
fn settings_rows(session: &super::account::Session) -> Vec<SettingsRow> {
    let mut rows: Vec<SettingsRow> =
        crate::tui::settings::in_scope(crate::tui::settings::Scope::Chat)
            .into_iter()
            .map(SettingsRow::Setting)
            .collect();
    // Only the action that applies: offering "sign in" to someone already signed
    // in is a trap, and offering "sign out" to someone who is not does nothing.
    rows.push(if session.signed_in() {
        SettingsRow::SignOut
    } else {
        SettingsRow::SignIn
    });
    rows
}

/// Work that has to happen on the async side of the loop, asked for by a key or
/// a click.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pending {
    SignOut,
    SignIn,
}

/// One slash command. `aliases` are dispatched and completed but never listed
/// on their own, so `/exit` and `/quit` are one entry rather than two.
struct Slash {
    /// Canonical name, with the leading slash.
    name: &'static str,
    /// Alternative names, also with leading slashes.
    aliases: &'static [&'static str],
    /// i18n key of the one-line description shown in the palette.
    desc: &'static str,
}

impl Slash {
    /// The name `exec_slash` matches on, i.e. the canonical name unslashed.
    fn key(&self) -> &'static str {
        self.name.trim_start_matches('/')
    }

    /// Whether `typed` (a `/name`, canonical or alias) addresses this command.
    fn answers_to(&self, typed: &str) -> bool {
        self.name == typed || self.aliases.contains(&typed)
    }

    /// Whether any of this command's names starts with `prefix`, so the palette
    /// surfaces `/exit` while the user is still typing `/qu`.
    fn starts_with(&self, prefix: &str) -> bool {
        self.name.starts_with(prefix) || self.aliases.iter().any(|a| a.starts_with(prefix))
    }
}

const SLASH: [Slash; 11] = [
    Slash {
        name: "/new",
        aliases: &["/clear"],
        desc: "Ai.SlashNew",
    },
    Slash {
        name: "/copy",
        aliases: &[],
        desc: "Ai.SlashCopy",
    },
    Slash {
        name: "/export",
        aliases: &[],
        desc: "Ai.SlashExport",
    },
    Slash {
        name: "/quote",
        aliases: &[],
        desc: "Ai.SlashQuote",
    },
    Slash {
        name: "/resume",
        aliases: &[],
        desc: "Ai.SlashResume",
    },
    Slash {
        name: "/settings",
        aliases: &[],
        desc: "Ai.SlashSettings",
    },
    Slash {
        name: "/agent",
        aliases: &[],
        desc: "Ai.SlashAgent",
    },
    Slash {
        name: "/login",
        aliases: &[],
        desc: "Ai.SlashLogin",
    },
    Slash {
        name: "/logout",
        aliases: &[],
        desc: "Ai.SlashLogout",
    },
    Slash {
        name: "/help",
        aliases: &[],
        desc: "Ai.SlashHelp",
    },
    Slash {
        name: "/exit",
        aliases: &["/quit"],
        desc: "Ai.SlashExit",
    },
];

/// Resolve a typed `/name` to the canonical key `exec_slash` dispatches on.
fn slash_lookup(typed: &str) -> Option<&'static str> {
    SLASH.iter().find(|c| c.answers_to(typed)).map(Slash::key)
}

/// A clickable chip in the Chat meta panel.
#[derive(Clone)]
enum Chip {
    /// Index into `state.references`.
    Reference(usize),
    /// Index into `state.further`.
    Further(usize),
    /// A security named in the transcript; opens its quote.
    Symbol(String),
    /// The title bar's ticker toggle.
    Tape,
}

/// State of the structured interrupt answering flow.
struct QuestionState {
    tool_call_id: String,
    /// `(question text, option texts)` for each question.
    questions: Vec<(String, Vec<String>)>,
    /// Which question is being answered.
    qi: usize,
    /// Collected `question -> answer` pairs.
    answers: HashMap<String, String>,
}

impl QuestionState {
    fn from_interrupt(interrupt: &Value) -> Self {
        let tool_call_id = interrupt
            .get("tool_call_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let questions = interrupt
            .get("questions")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|q| {
                        let text = q.get("question").and_then(Value::as_str)?.to_string();
                        let options = crate::cli::agent::chat::question_choices(q);
                        Some((text, options))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            tool_call_id,
            questions,
            qi: 0,
            answers: HashMap::new(),
        }
    }

    /// Every question offers at least one option, so the overlay can fully
    /// answer it (otherwise the inline free-text path is used instead).
    fn fully_selectable(&self) -> bool {
        !self.questions.is_empty() && self.questions.iter().all(|(_, o)| !o.is_empty())
    }
}

/// View/navigation state, kept separate from [`ChatState`] so the conversation
/// model stays about messages, not the UI. Hit-test rectangles are recorded
/// during each render so mouse clicks can be mapped back to tabs, rows, chips.
#[allow(clippy::struct_excessive_bools)]
struct Ui {
    view: View,
    sessions: Vec<SessionSummary>,
    question: Option<QuestionState>,
    /// Selected row within the active list view.
    sel: usize,
    /// Highlighted entry in the slash-command palette.
    slash_sel: usize,
    /// Live filter for the History list (case-insensitive title match).
    search: String,
    /// Transient one-line notice shown in the status bar.
    notice: Option<String>,
    /// Last known mouse position, used to marquee the hovered row.
    hover: Option<(u16, u16)>,
    /// Frame counter advancing the marquee scroll of truncated rows.
    tick: u64,
    /// Set during render when at least one truncated row is scrolling, so the
    /// animation timer only runs while something actually needs it.
    animating: bool,
    rows: Vec<(usize, Rect)>,
    chips: Vec<(Chip, Rect)>,
    /// Slash-palette hit rects: `(SLASH index, rect)` for the visible entries.
    slash_rows: Vec<(usize, Rect)>,
    /// The transcript area, recorded so a drag there starts a text selection.
    transcript: Rect,
    /// Active drag selection as `(anchor, cursor)` in absolute `(row, col)`
    /// screen cells; `None` when nothing is selected.
    selection: Option<((u16, u16), (u16, u16))>,
    /// The text under the current selection, extracted during render.
    selected_text: Option<String>,
    /// Cached rendered lines of the committed transcript and the signature they
    /// were built for, so Markdown is not re-parsed every frame.
    transcript_cache: Vec<Line<'static>>,
    cache_sig: u64,
    /// Max scroll-back for the current transcript (recorded during render), so
    /// input handlers can clamp and never scroll the view into a blank screen.
    max_scroll: u16,
    /// Live quotes for `x-widget` tickers, fetched after a turn, keyed by symbol.
    quotes: HashMap<String, super::quotes::QuoteCardData>,
    /// True while the server History list is being fetched.
    sessions_loading: bool,
    /// True when the last History fetch failed (vs. genuinely empty).
    sessions_error: bool,
    /// Senders for background History fetch / load, set once in `run`.
    history_tx: Option<UnboundedSender<Option<Vec<SessionSummary>>>>,
    /// Sender for background quote fetches, so a clicked symbol can ask for its
    /// own quote the same way a finished turn asks for its cards.
    cards_tx: Option<UnboundedSender<HashMap<String, super::quotes::QuoteCardData>>>,
    /// The security whose quote panel is open, if any.
    quote_panel: Option<String>,
    /// Who the chat is signed in as, for the Settings header.
    session: super::account::Session,
    /// Securities this conversation has mentioned, in the order they appeared.
    /// The title bar's ticker reads from here.
    tape: Vec<String>,
    /// Bare tickers the server confirmed, mapped to their full symbol: `SPCX` →
    /// `SPCX.US`. An answer writes tickers without a market far more often than
    /// with one, and nothing is linked until it is in here.
    aliases: HashMap<String, String>,
    /// Where the ticker has rotated to, when it is wider than the row.
    tape_at: usize,
    /// Frame counter behind the rotation: the tape steps once a second, while the
    /// frame timer ticks eight times as often.
    tape_ticks: u8,
    /// Work for the async side of the loop: signing in or out.
    pending: Option<Pending>,
    /// Sign-out is armed by one Enter and done by the next, so an arrow key and a
    /// stray Return cannot end the session.
    confirm_sign_out: bool,
    /// What to tell the reader once the full-screen view is gone.
    exit_note: Option<String>,
    load_tx: Option<UnboundedSender<Option<session_store::LoadedChat>>>,
    /// Hit rect of the running turn's `[stop]` button, recorded during render.
    stop_button: Option<Rect>,
    /// When the active turn started, for the elapsed clock. UI state rather than
    /// chat state: it exists to be displayed, not to be replayed.
    turn_started: Option<std::time::Instant>,
    /// Set to break the event loop (via `/exit` or a double Ctrl+C).
    should_quit: bool,
    /// True after one Ctrl+C on an empty prompt; a second one exits.
    ctrl_c_armed: bool,
    /// Whether the terminal window currently has focus; a turn that finishes
    /// while unfocused raises a desktop notification.
    focused: bool,
}

impl Ui {
    fn new() -> Self {
        Self {
            view: View::Chat,
            sessions: Vec::new(),
            question: None,
            sel: 0,
            slash_sel: 0,
            search: String::new(),
            notice: None,
            hover: None,
            tick: 0,
            animating: false,
            rows: Vec::new(),
            chips: Vec::new(),
            slash_rows: Vec::new(),
            transcript: Rect::default(),
            selection: None,
            selected_text: None,
            transcript_cache: Vec::new(),
            cache_sig: 0,
            max_scroll: 0,
            quotes: HashMap::new(),
            sessions_loading: false,
            sessions_error: false,
            history_tx: None,
            cards_tx: None,
            quote_panel: None,
            session: super::account::local(),
            tape: Vec::new(),
            aliases: HashMap::new(),
            tape_at: 0,
            tape_ticks: 0,
            pending: None,
            confirm_sign_out: false,
            exit_note: None,
            load_tx: None,
            stop_button: None,
            turn_started: None,
            should_quit: false,
            ctrl_c_armed: false,
            focused: true,
        }
    }

    /// History entries matching the current search, newest first.
    fn visible_sessions(&self) -> Vec<&SessionSummary> {
        let needle = self.search.to_lowercase();
        self.sessions
            .iter()
            .filter(|s| needle.is_empty() || s.title.to_lowercase().contains(&needle))
            .collect()
    }

    /// Number of selectable rows in the active list view.
    fn row_count(&self) -> usize {
        match self.view {
            View::Chat => 0,
            // Visible sessions plus the trailing "New session" action; but no
            // action row when a search yields nothing.
            View::Sessions => {
                let v = self.visible_sessions().len();
                if v == 0 && !self.search.is_empty() {
                    0
                } else {
                    v + 1
                }
            }
            View::Settings => settings_rows(&self.session).len(),
            View::Question => self
                .question
                .as_ref()
                .and_then(|q| q.questions.get(q.qi))
                .map_or(0, |(_, o)| o.len()),
        }
    }

    /// Switch to `view`, dropping any half-answered question and clamping
    /// selection. (Entering Conversations is done via `open_sessions`, which
    /// also kicks off the async fetch.)
    fn switch(&mut self, view: View) {
        self.view = view;
        self.notice = None;
        self.sel = 0;
        self.selection = None;
        self.search.clear();
        if view != View::Question {
            self.question = None;
        }
    }

    fn clamp_sel(&mut self) {
        self.sel = self.sel.min(self.row_count().saturating_sub(1));
    }

    /// Drop render state tied to the previous conversation (cached lines and
    /// fetched quotes) so a fresh chat doesn't show stale content.
    fn reset_render(&mut self) {
        self.quotes.clear();
        self.cache_sig = 0;
    }
}

/// Run the chat TUI until the user quits. The caller has already entered the
/// full-screen terminal (with mouse capture) and restores it afterwards.
/// Run the chat. Returns a note to print once the full-screen view is gone —
/// signing in or out has to be reported outside the alternate screen, or the
/// message scrolls away with it.
pub async fn run<S>(agent_uid: String, mut quotes: S) -> Result<Option<String>>
where
    S: tokio_stream::Stream<Item = longbridge::quote::PushEvent> + Send + Unpin,
{
    let mut terminal = Terminal::default();
    let mut state = ChatState::new(agent_uid, t!("Ai.Welcome").to_string());
    let mut ui = Ui::new();
    let mut editor = Editor::new();
    let mut turn: Option<JoinHandle<()>> = None;
    let (tx, mut turn_rx) = unbounded_channel::<ChatEvent>();
    let (cards_tx, mut cards_rx) =
        unbounded_channel::<HashMap<String, super::quotes::QuoteCardData>>();
    let (history_tx, mut history_rx) = unbounded_channel::<Option<Vec<SessionSummary>>>();
    let (load_tx, mut load_rx) = unbounded_channel::<Option<session_store::LoadedChat>>();
    ui.history_tx = Some(history_tx);
    ui.cards_tx = Some(cards_tx.clone());
    // The member id is the one part of the session header that needs a call, so
    // it is fetched once in the background and the header renders without it
    // until it lands.
    let (aliases_tx, mut aliases_rx) = unbounded_channel::<HashMap<String, String>>();
    let (session_tx, mut session_rx) = unbounded_channel::<Option<String>>();
    tokio::spawn(async move {
        let _ = session_tx.send(super::account::member_id().await);
    });
    ui.load_tx = Some(load_tx);
    let mut events = EventStream::new();
    // Drives the marquee of truncated rows; only consulted while `animating`.
    let mut ticker = tokio::time::interval(Duration::from_millis(120));
    // Bracketed paste makes multi-line pastes arrive as one `Event::Paste`
    // instead of a stream of keystrokes (whose embedded newlines would each
    // submit the prompt). Focus tracking lets a turn that finishes while the
    // window is in the background raise a desktop notification. Both are
    // disabled again on exit.
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::EnableBracketedPaste,
        crossterm::event::EnableFocusChange,
    );

    loop {
        terminal.draw(|f| view(f, &mut ui, &state, &editor))?;
        tokio::select! {
            _ = ticker.tick(), if ui.animating => {
                ui.tick = ui.tick.wrapping_add(1);
            }
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                        on_key(key, &mut ui, &mut state, &mut editor, &mut turn, &tx);
                    }
                    Some(Ok(Event::Mouse(m))) => {
                        on_mouse(m, &mut ui, &mut state, &mut editor, &mut turn, &tx);
                    }
                    Some(Ok(Event::Paste(text))) => editor.insert_str(&text),
                    Some(Ok(Event::FocusGained)) => ui.focused = true,
                    Some(Ok(Event::FocusLost)) => ui.focused = false,
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => break,
                }
            }
            Some(event) = turn_rx.recv() => {
                let finished = matches!(event, ChatEvent::TurnFinished { .. });
                state.apply(event);
                if finished {
                    turn = None;
                    maybe_open_question(&mut ui, &state);
                    resolve_session_tickers(&ui, &state, &aliases_tx);
                    track_session_symbols(&mut ui, &state);
                    fetch_missing_quotes(&ui, &cards_tx);
                    if !ui.focused && super::settings::notify_on_finish() {
                        notify(&t!("Ai.NotifyDone"));
                    }
                }
            }
            Some(cards) = cards_rx.recv() => {
                ui.quotes.extend(cards);
                ui.cache_sig = 0; // force a transcript rebuild so cards appear
            }
            // A streamed quote for a security already on screen: fold it in and
            // let the panel and any card redraw with it.
            Some(event) = tokio_stream::StreamExt::next(&mut quotes) => {
                if let longbridge::quote::PushEventDetail::Quote(quote) = &event.detail {
                    if let Some(card) = ui.quotes.get_mut(&event.symbol) {
                        card.apply_push(quote);
                        ui.cache_sig = 0;
                    }
                }
            }
            Some(member_id) = session_rx.recv() => {
                ui.session.member_id = member_id;
            }
            // Bare tickers the server confirmed: they join the ticker, get a
            // quote, and become links in the transcript.
            Some(resolved) = aliases_rx.recv() => {
                ui.aliases.extend(resolved);
                track_session_symbols(&mut ui, &state);
                fetch_missing_quotes(&ui, &cards_tx);
                ui.cache_sig = 0;
            }
            Some(result) = history_rx.recv() => {
                ui.sessions_loading = false;
                if let Some(list) = result {
                    ui.sessions = list;
                    ui.sessions_error = false;
                } else {
                    ui.sessions.clear();
                    ui.sessions_error = true;
                }
                ui.clamp_sel();
            }
            Some(loaded) = load_rx.recv() => {
                if let Some(loaded) = loaded {
                    session_store::restore(loaded, &mut state);
                    ui.reset_render();
                    resolve_session_tickers(&ui, &state, &aliases_tx);
                    track_session_symbols(&mut ui, &state);
                    fetch_missing_quotes(&ui, &cards_tx);
                    ui.switch(View::Chat);
                } else {
                    // Resume failed: stay on History with an error notice.
                    ui.notice = Some(t!("Ai.SessionsError").to_string());
                }
            }
        }
        if let Some(action) = ui.pending.take() {
            match run_pending(action).await {
                Ok(note) => {
                    // Both actions replace the credentials this process built its
                    // contexts from, and those are process-wide singletons, so the
                    // only honest thing after either is to leave.
                    ui.exit_note = Some(note);
                    ui.should_quit = true;
                }
                Err(e) => ui.notice = Some(e),
            }
        }
        if ui.should_quit {
            break;
        }
    }
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::DisableBracketedPaste,
        crossterm::event::DisableFocusChange,
    );
    if let Some(turn) = turn.take() {
        turn.abort();
    }
    Ok(ui.exit_note.take())
}

/// Add every security the conversation has mentioned to the ticker, and keep its
/// quote streaming.
///
/// Session-scoped and append-only: a security stays on the ticker once mentioned,
/// because the reader is following the whole conversation, not just its last turn.
/// The subscription is never dropped either — it is what keeps the ticker, the
/// inline prices and any card current, and a handful of securities is nothing
/// against the subscription limit.
fn track_session_symbols(ui: &mut Ui, state: &ChatState) {
    let mut fresh = Vec::new();
    for message in &state.messages {
        if matches!(message.role, Role::System | Role::Tool) {
            continue;
        }
        for (_, symbol) in super::answer::security_spans(&message.text, &ui.aliases) {
            if !ui.tape.contains(&symbol) && !fresh.contains(&symbol) {
                fresh.push(symbol);
            }
        }
    }
    for symbol in fresh {
        subscribe_quote(&symbol);
        ui.tape.push(symbol);
    }
}

/// Ask the server which of the conversation's bare tickers are real securities.
///
/// An answer writes `SPCX`, not `SPCX.US`, and a chat about options is full of
/// words shaped like tickers — `ITM`, `MACD`, `BOLL`. Guessing from the shape
/// would litter the transcript with links that answer nothing, so the candidates
/// go to the server and only what it recognises becomes a link.
fn resolve_session_tickers(
    ui: &Ui,
    state: &ChatState,
    tx: &UnboundedSender<HashMap<String, String>>,
) {
    if !super::settings::quote_cards() && !super::settings::tape() {
        return;
    }
    let mut candidates: Vec<String> = Vec::new();
    for message in &state.messages {
        if matches!(message.role, Role::System | Role::Tool) {
            continue;
        }
        for candidate in super::answer::ticker_candidates(&message.text) {
            // Asked once per session: a token the server did not recognise is not
            // going to start existing, and one it did is already resolved.
            if !ui.aliases.contains_key(&candidate) && !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }
    if candidates.is_empty() {
        return;
    }
    let tx = tx.clone();
    spawn_bg(async move {
        let resolved = super::quotes::resolve_symbols(&candidates).await;
        if !resolved.is_empty() {
            let _ = tx.send(resolved);
        }
    });
}

/// Fetch a quote for every security on the ticker that has none yet.
///
/// Driven by the ticker rather than by the last answer, because a security the
/// reader named in their own question — "how is SPCX doing tonight?" — is on the
/// ticker too, and used to sit there without a price forever. It also covers the
/// securities an answer only mentions in prose, which the old widget-only scan
/// skipped entirely.
fn fetch_missing_quotes(
    ui: &Ui,
    cards_tx: &UnboundedSender<HashMap<String, super::quotes::QuoteCardData>>,
) {
    // The cards and the ticker read the same quotes; either being on is reason
    // enough to fetch.
    if !super::settings::quote_cards() && !super::settings::tape() {
        return;
    }
    let wanted: Vec<String> = ui
        .tape
        .iter()
        .filter(|symbol| !ui.quotes.contains_key(*symbol))
        .cloned()
        .collect();
    if wanted.is_empty() {
        return;
    }
    let cards_tx = cards_tx.clone();
    spawn_bg(async move {
        let cards = super::quotes::fetch_cards_for(&wanted).await;
        if !cards.is_empty() {
            let _ = cards_tx.send(cards);
        }
    });
}

/// Raise a desktop notification via the OSC 9 terminal escape (`iTerm2`,
/// `WezTerm`, kitty, …), plus a bell so terminals without OSC 9 still flag the
/// tab. Safe to emit while ratatui owns the screen — it never moves the cursor.
fn notify(message: &str) {
    use std::io::Write;
    let seq = format!("\x1b]9;{message}\x07\x07");
    let mut out = std::io::stdout();
    let _ = out.write_all(seq.as_bytes());
    let _ = out.flush();
}

/// After a turn, open the structured Question view if the interrupt is fully
/// answerable by picking options; otherwise leave the inline free-text path.
fn maybe_open_question(ui: &mut Ui, state: &ChatState) {
    if let Some(interrupt) = &state.pending_interrupt {
        let qs = QuestionState::from_interrupt(interrupt);
        if qs.fully_selectable() {
            ui.question = Some(qs);
            ui.view = View::Question;
            ui.sel = 0;
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

// ── input ─────────────────────────────────────────────────────────────────────

/// Handle one keypress. Quitting is signalled via `ui.should_quit`.
fn on_key(
    key: crossterm::event::KeyEvent,
    ui: &mut Ui,
    state: &mut ChatState,
    editor: &mut Editor,
    turn: &mut Option<JoinHandle<()>>,
    tx: &UnboundedSender<ChatEvent>,
) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && key.code == KeyCode::Char('c') {
        on_ctrl_c(ui, state, editor, turn);
        return;
    }
    // Any other key disarms the "press Ctrl+C again to exit" prompt.
    ui.ctrl_c_armed = false;
    // Tab completes the highlighted slash command; it has no other use now that
    // views are reached via `/` commands rather than a tab bar.
    if key.code == KeyCode::Tab {
        if ui.view == View::Chat && slash_active(editor) {
            complete_slash(ui, editor);
        }
        return;
    }
    match ui.view {
        View::Chat => on_chat_key(key, ui, state, editor, turn, tx),
        View::Question => on_question_key(key, ui, state, turn, tx),
        View::Sessions => on_sessions_key(key, ui, state),
        View::Settings => on_list_key(key, ui, state),
    }
}

/// Ctrl+C follows Claude's convention: cancel a running turn, else clear the
/// prompt, else require a second press on an empty prompt to actually exit.
fn on_ctrl_c(
    ui: &mut Ui,
    state: &mut ChatState,
    editor: &mut Editor,
    turn: &mut Option<JoinHandle<()>>,
) {
    if state.busy {
        cancel_turn(state, turn);
        ui.ctrl_c_armed = false;
    } else if !editor.is_blank() {
        editor.clear();
        ui.ctrl_c_armed = false;
    } else if ui.ctrl_c_armed {
        ui.should_quit = true;
    } else {
        ui.ctrl_c_armed = true;
        ui.notice = Some(t!("Ai.PressCtrlCAgain").to_string());
    }
}

/// Abort the active turn and fold any partial answer into the transcript.
fn cancel_turn(state: &mut ChatState, turn: &mut Option<JoinHandle<()>>) {
    if let Some(turn) = turn.take() {
        turn.abort();
    }
    state.cancel(&t!("Ai.Cancelled"));
}

/// Conversations view: arrows/Enter select and open, Del removes, typing filters.
fn on_sessions_key(key: crossterm::event::KeyEvent, ui: &mut Ui, state: &mut ChatState) {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Esc => {
            if ui.search.is_empty() {
                ui.switch(View::Chat);
            } else {
                ui.search.clear();
                ui.clamp_sel();
            }
        }
        // Jump to the ends. A MacBook's built-in keyboard reaches Home/End only
        // through Fn, so Shift+arrows carry the same action.
        KeyCode::Home => ui.sel = 0,
        KeyCode::End => ui.sel = ui.row_count().saturating_sub(1),
        KeyCode::Up if shift => ui.sel = 0,
        KeyCode::Down if shift => ui.sel = ui.row_count().saturating_sub(1),
        KeyCode::Up => ui.sel = ui.sel.saturating_sub(1),
        KeyCode::Down => {
            let last = ui.row_count().saturating_sub(1);
            ui.sel = (ui.sel + 1).min(last);
        }
        KeyCode::Enter => activate(ui, state),
        KeyCode::Backspace => {
            ui.search.pop();
            ui.clamp_sel();
        }
        KeyCode::Char(c) => {
            ui.search.push(c);
            ui.sel = 0;
        }
        _ => {}
    }
}

fn on_chat_key(
    key: crossterm::event::KeyEvent,
    ui: &mut Ui,
    state: &mut ChatState,
    editor: &mut Editor,
    turn: &mut Option<JoinHandle<()>>,
    tx: &UnboundedSender<ChatEvent>,
) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let newline = key
        .modifiers
        .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT);
    // The panel floats over the chat, so Esc dismisses it before anything else
    // reads the key — otherwise the reader would clear their input instead.
    if ui.quote_panel.is_some() && matches!(key.code, KeyCode::Esc) {
        close_quote_panel(ui);
        return;
    }
    // When the slash palette is open, arrows/Enter/Esc drive it instead of the
    // transcript or history, matching the grok-style command menu.
    if slash_active(editor) {
        let count = slash_matches(editor).len();
        match key.code {
            KeyCode::Up => {
                ui.slash_sel = ui.slash_sel.saturating_sub(1);
                return;
            }
            KeyCode::Down => {
                ui.slash_sel = (ui.slash_sel + 1).min(count.saturating_sub(1));
                return;
            }
            KeyCode::Enter if !newline => {
                run_slash_selected(ui, state, editor);
                return;
            }
            KeyCode::Esc => {
                editor.clear();
                return;
            }
            _ => {}
        }
    }
    match key.code {
        // Esc cancels a running turn or clears the prompt — it never quits
        // (use /exit or double Ctrl+C for that), matching Claude's convention.
        KeyCode::Esc => {
            if state.busy {
                cancel_turn(state, turn);
            } else if !editor.is_blank() {
                editor.clear();
            }
        }
        KeyCode::Enter if newline => editor.insert_newline(),
        // Most terminals cannot distinguish Shift+Enter from Enter and send a
        // bare LF instead. crossterm decodes control bytes 0x01..=0x1A as
        // Ctrl+letter, so LF (0x0A) arrives as Ctrl+J — which is the newline the
        // user asked for, not the letter `j` the fallback used to insert.
        KeyCode::Char('j') if ctrl => editor.insert_newline(),
        KeyCode::Enter if !state.busy => submit(ui, state, editor, turn, tx),
        KeyCode::Backspace | KeyCode::Char('w') if ctrl => editor.delete_word(),
        // Emacs-style line editing shortcuts, familiar from the shell.
        KeyCode::Char('a') if ctrl => editor.home(),
        KeyCode::Char('e') if ctrl => editor.end(),
        KeyCode::Char('u') if ctrl => editor.clear(),
        KeyCode::Char('k') if ctrl => editor.kill_to_end(),
        KeyCode::Backspace => editor.backspace(),
        KeyCode::Left => editor.left(),
        KeyCode::Right => editor.right(),
        KeyCode::Home => editor.home(),
        KeyCode::End => editor.end(),
        // Scrolling the transcript. A MacBook's built-in keyboard has no
        // PageUp/PageDown key — they need Fn — so Shift+arrows carry the same
        // action, and the dedicated keys stay as aliases for full keyboards.
        KeyCode::PageUp => {
            state.scroll = state.scroll.saturating_add(SCROLL_PAGE).min(ui.max_scroll);
        }
        KeyCode::PageDown => state.scroll = state.scroll.saturating_sub(SCROLL_PAGE),
        KeyCode::Up if shift => {
            state.scroll = state.scroll.saturating_add(SCROLL_PAGE).min(ui.max_scroll);
        }
        KeyCode::Down if shift => state.scroll = state.scroll.saturating_sub(SCROLL_PAGE),
        KeyCode::Up => {
            if !editor.up() {
                editor.recall_prev();
            }
        }
        KeyCode::Down => {
            if !editor.down() {
                editor.recall_next();
            }
        }
        // An unhandled Ctrl combination is swallowed. Without this the fallback
        // below types its letter, so every unbound Ctrl+key silently inserted
        // text.
        KeyCode::Char(_) if ctrl => {}
        KeyCode::Char(c) => editor.insert_char(c),
        _ => {}
    }
    // Editing the query re-filters the palette; keep the highlight in range.
    if slash_active(editor) {
        ui.slash_sel = ui
            .slash_sel
            .min(slash_matches(editor).len().saturating_sub(1));
    }
}

/// Submit the prompt: run a slash command, or start a conversation turn.
fn submit(
    ui: &mut Ui,
    state: &mut ChatState,
    editor: &mut Editor,
    turn: &mut Option<JoinHandle<()>>,
    tx: &UnboundedSender<ChatEvent>,
) {
    let text = editor.text();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }
    // A bare `exit` / `quit` also leaves, like a REPL — no leading slash needed.
    if trimmed.eq_ignore_ascii_case("exit") || trimmed.eq_ignore_ascii_case("quit") {
        ui.should_quit = true;
        return;
    }
    // A known `/command` runs locally; anything else starting with `/` is just
    // a prompt. Everything after the name is the command's argument.
    if trimmed.starts_with('/') {
        let (name, args) = split_command(trimmed);
        if let Some(key) = slash_lookup(name) {
            editor.clear();
            exec_slash(key, args, ui, state);
            return;
        }
    }
    let query = trimmed.to_string();
    editor.push_history(&query);
    editor.clear();
    ui.notice = None;
    ui.selection = None;
    let req = runtime::build_request(state, query.clone());
    state.apply(ChatEvent::UserPrompt(query));
    state.pending_interrupt = None;
    *turn = Some(runtime::spawn_turn(req, tx.clone()));
}

/// Split `/name rest` into the `/name` and its trimmed argument string.
fn split_command(input: &str) -> (&str, &str) {
    match input.find(char::is_whitespace) {
        Some(i) => (&input[..i], input[i..].trim()),
        None => (input, ""),
    }
}

/// Run the command named by its canonical [`Slash::key`], with `args` as the
/// (possibly empty) rest of the line.
fn exec_slash(name: &str, args: &str, ui: &mut Ui, state: &mut ChatState) {
    match name {
        "new" => {
            state.reset(t!("Ai.Welcome").to_string());
            ui.reset_render();
            ui.switch(View::Chat);
        }
        // Keyboard route to what a click on a symbol does. With no argument it
        // opens the security the answer mentioned last, which is usually the one
        // the reader is looking at.
        "quote" => {
            let symbol = if args.is_empty() {
                last_symbol(state)
            } else {
                Some(args.trim().to_uppercase())
            };
            match symbol {
                Some(symbol) => open_quote_panel(ui, symbol),
                None => ui.notice = Some(t!("Ai.QuoteNoSymbol").to_string()),
            }
        }
        // Typed deliberately, so no second keypress to confirm — the row in
        // Settings is the one that can be hit by accident.
        "logout" => ui.pending = Some(Pending::SignOut),
        "login" => ui.pending = Some(Pending::SignIn),
        "copy" => {
            let text = transcript_text(state);
            copy_with_notice(ui, Some(text));
        }
        "export" => {
            ui.notice = Some(match export_conversation(state) {
                Ok(path) => t!("Ai.Exported", path = path.display().to_string()).to_string(),
                Err(_) => t!("Ai.ExportFailed").to_string(),
            });
        }
        "resume" => open_sessions(ui),
        "settings" => ui.switch(View::Settings),
        "agent" => switch_agent(args, ui, state),
        "help" => state.messages.push(Message::new(Role::System, help_text())),
        "exit" => ui.should_quit = true,
        _ => {}
    }
}

/// `/agent <agent-id>` switches agent; `/agent reset` returns to Longbridge
/// AI's own assistant. There is deliberately no roster to browse — an agent uid
/// is addressed, never listed — so a bare `/agent` only restates the usage.
///
/// The uid is not validated here: only the server knows which agents the
/// account may drive, so a bad one surfaces as that agent's first-turn error.
fn switch_agent(args: &str, ui: &mut Ui, state: &mut ChatState) {
    if args.is_empty() {
        ui.notice = Some(t!("Ai.AgentUsage").to_string());
        return;
    }
    let reset = args.eq_ignore_ascii_case("reset");
    let uid = if reset {
        DEFAULT_AGENT_UID.to_string()
    } else {
        args.to_string()
    };
    // Already there: say so rather than claiming a switch, and leave the
    // conversation alone — re-running the command must not discard it.
    if uid == state.agent_uid {
        ui.notice = Some(t!("Ai.AgentUnchanged").to_string());
        return;
    }
    let notice = if reset {
        t!("Ai.AgentReset")
    } else {
        t!("Ai.AgentSwitched")
    }
    .to_string();
    // A conversation belongs to its agent server-side, so switching starts a
    // fresh one rather than continuing this thread under a different agent.
    state.reset(t!("Ai.Welcome").to_string());
    state.agent_uid = uid;
    ui.switch(View::Chat);
    // `switch` clears the status line, so the confirmation is set after it.
    ui.notice = Some(notice);
}

/// The `/help` message: the command list is derived from [`SLASH`] so it cannot
/// drift out of sync, followed by the key hints.
fn help_text() -> String {
    let commands = SLASH
        .iter()
        .map(|c| {
            if c.aliases.is_empty() {
                c.name.to_string()
            } else {
                format!("{} ({})", c.name, c.aliases.join(" "))
            }
        })
        .collect::<Vec<_>>()
        .join(" · ");
    format!(
        "{} · {}",
        t!("Ai.HelpCommands", commands = commands),
        t!("Ai.HelpKeys")
    )
}

/// Keyboard navigation for the Settings list view.
fn on_list_key(key: crossterm::event::KeyEvent, ui: &mut Ui, state: &mut ChatState) {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Esc => ui.switch(View::Chat),
        // Jump to the ends; Shift+arrows because Home/End need Fn on a MacBook.
        KeyCode::Home => ui.sel = 0,
        KeyCode::End => ui.sel = ui.row_count().saturating_sub(1),
        KeyCode::Up if shift => ui.sel = 0,
        KeyCode::Down if shift => ui.sel = ui.row_count().saturating_sub(1),
        KeyCode::Up | KeyCode::Char('k') => ui.sel = ui.sel.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => {
            let last = ui.row_count().saturating_sub(1);
            ui.sel = (ui.sel + 1).min(last);
        }
        // Space changes a setting as well as Enter, matching the market view's
        // modal — the same table, the same keys.
        KeyCode::Enter => activate(ui, state),
        KeyCode::Char(' ') if ui.view == View::Settings => activate(ui, state),
        _ => {}
    }
}

/// Keyboard navigation for the structured Question view.
fn on_question_key(
    key: crossterm::event::KeyEvent,
    ui: &mut Ui,
    state: &mut ChatState,
    turn: &mut Option<JoinHandle<()>>,
    tx: &UnboundedSender<ChatEvent>,
) {
    match key.code {
        KeyCode::Esc => ui.switch(View::Chat),
        KeyCode::Up | KeyCode::Char('k') => ui.sel = ui.sel.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => {
            let last = ui.row_count().saturating_sub(1);
            ui.sel = (ui.sel + 1).min(last);
        }
        KeyCode::Enter => answer_selected(ui, state, turn, tx),
        _ => {}
    }
}

fn on_mouse(
    m: crossterm::event::MouseEvent,
    ui: &mut Ui,
    state: &mut ChatState,
    editor: &mut Editor,
    turn: &mut Option<JoinHandle<()>>,
    tx: &UnboundedSender<ChatEvent>,
) {
    if let MouseEventKind::Moved = m.kind {
        ui.hover = Some((m.column, m.row));
        return;
    }
    match m.kind {
        MouseEventKind::ScrollUp => {
            ui.selection = None;
            scroll(ui, state, true);
        }
        MouseEventKind::ScrollDown => {
            ui.selection = None;
            scroll(ui, state, false);
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let (col, row) = (m.column, m.row);
            ui.selection = None;
            // The running turn's stop button, wherever the current view is.
            if let Some(rect) = ui.stop_button {
                if hit(rect, col, row) {
                    cancel_turn(state, turn);
                    ui.stop_button = None;
                    return;
                }
            }
            // The panel floats over the transcript, and the symbol hit rects were
            // recorded from what is underneath it. A click while it is open
            // dismisses it rather than reaching through to a target the reader
            // cannot see.
            if ui.quote_panel.is_some() {
                close_quote_panel(ui);
                return;
            }
            if ui.view == View::Chat {
                if let Some(idx) = ui
                    .slash_rows
                    .iter()
                    .find(|(_, r)| hit(*r, col, row))
                    .map(|(i, _)| *i)
                {
                    run_slash(idx, ui, state, editor);
                } else if let Some(chip) = ui
                    .chips
                    .iter()
                    .find(|(_, r)| hit(*r, col, row))
                    .map(|(c, _)| c.clone())
                {
                    click_chip(chip, ui, state, turn, tx);
                } else if hit(ui.transcript, col, row) {
                    // Begin a text selection in the transcript.
                    ui.selection = Some(((row, col), (row, col)));
                }
            } else if let Some(idx) = ui
                .rows
                .iter()
                .find(|(_, r)| hit(*r, col, row))
                .map(|(i, _)| *i)
            {
                ui.sel = idx;
                if ui.view == View::Question {
                    answer_selected(ui, state, turn, tx);
                } else {
                    activate(ui, state);
                }
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some((anchor, _)) = ui.selection {
                let pos = clamp_to(ui.transcript, m.column, m.row);
                ui.selection = Some((anchor, pos));
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if let Some((anchor, cursor)) = ui.selection {
                if anchor == cursor {
                    // A click with no drag is not a selection.
                    ui.selection = None;
                } else if let Some(text) = ui.selected_text.clone() {
                    copy_with_notice(ui, Some(text));
                }
            }
        }
        _ => {}
    }
}

/// Clamp a screen cell to lie within `rect` (used while dragging a selection).
fn clamp_to(rect: Rect, col: u16, row: u16) -> (u16, u16) {
    let r = row.clamp(rect.y, rect.y + rect.height.saturating_sub(1));
    let c = col.clamp(rect.x, rect.x + rect.width.saturating_sub(1));
    (r, c)
}

/// Scroll wheel: pans the transcript in Chat, moves the selection in a list.
fn scroll(ui: &mut Ui, state: &mut ChatState, up: bool) {
    if ui.view == View::Chat {
        state.scroll = if up {
            state.scroll.saturating_add(3).min(ui.max_scroll)
        } else {
            state.scroll.saturating_sub(3)
        };
    } else if up {
        ui.sel = ui.sel.saturating_sub(1);
    } else {
        let last = ui.row_count().saturating_sub(1);
        ui.sel = (ui.sel + 1).min(last);
    }
}

/// Run the selected row's action in the active list view.
fn activate(ui: &mut Ui, state: &mut ChatState) {
    match ui.view {
        View::Sessions => {
            // The row past the last session is the "New session" action.
            if let Some(id) = ui.visible_sessions().get(ui.sel).map(|s| s.id.clone()) {
                // Fetch the full conversation in the background, then restore.
                if let Some(tx) = ui.load_tx.clone() {
                    ui.notice = Some(t!("Ai.SessionLoading").to_string());
                    tokio::spawn(async move {
                        let _ = tx.send(session_store::load_detail(&id).await);
                    });
                }
            } else {
                state.reset(t!("Ai.Welcome").to_string());
                ui.reset_render();
                ui.switch(View::Chat);
            }
        }
        // A preference changes in place and stays on the list: the reader is here
        // to set several, and a row that navigated away was a command wearing a
        // setting's clothes.
        View::Settings => match settings_rows(&ui.session).get(ui.sel) {
            Some(SettingsRow::Setting(meta)) => {
                crate::tui::settings::cycle(meta);
                // A colour or card change alters every rendered answer.
                ui.cache_sig = 0;
                ui.confirm_sign_out = false;
            }
            Some(SettingsRow::SignOut) => {
                if std::mem::replace(&mut ui.confirm_sign_out, true) {
                    ui.confirm_sign_out = false;
                    ui.pending = Some(Pending::SignOut);
                }
            }
            Some(SettingsRow::SignIn) => ui.pending = Some(Pending::SignIn),
            None => {}
        },
        View::Chat | View::Question => {}
    }
}

/// The security mentioned most recently in the transcript.
fn last_symbol(state: &ChatState) -> Option<String> {
    state.messages.iter().rev().find_map(|m| {
        super::answer::symbol_spans(&m.text)
            .last()
            .map(|r| m.text[r.clone()].to_string())
    })
}

/// Perform a session action, returning the note to print after the screen is
/// handed back.
///
/// Signing in needs the terminal: the flow prints a URL and waits on a browser
/// round trip, so the full-screen view is torn down for the duration and restored
/// afterwards — even on failure, or the reader would be left staring at a raw
/// shell.
async fn run_pending(action: Pending) -> Result<String, String> {
    match action {
        Pending::SignOut => match crate::auth::clear_token().await {
            Ok(()) => Ok(t!("Ai.SignedOut").to_string()),
            Err(e) => Err(format!("{}: {e}", t!("Ai.SignOutFailed"))),
        },
        Pending::SignIn => {
            Terminal::exit_full_screen();
            let result = crate::auth::device_login(false, None).await;
            Terminal::enter_full_screen();
            match result {
                Ok(()) => Ok(t!("Ai.SignedIn").to_string()),
                Err(e) => Err(format!("{}: {e}", t!("Ai.SignInFailed"))),
            }
        }
    }
}

/// Open the floating quote panel for `symbol`, fetching its quote if the card is
/// not already cached.
///
/// The panel floats over the transcript rather than replacing it: the reader
/// clicked a symbol inside a sentence they were reading, and the answer around it
/// is the context for the number.
fn open_quote_panel(ui: &mut Ui, symbol: String) {
    if !ui.quotes.contains_key(&symbol) {
        if let Some(tx) = ui.cards_tx.clone() {
            let wanted = symbol.clone();
            spawn_bg(async move {
                let cards = super::quotes::fetch_cards_for(&[wanted]).await;
                if !cards.is_empty() {
                    let _ = tx.send(cards);
                }
            });
        }
    }
    // The panel shows a price, so the price has to be live. Subscribing here as
    // well as on the ticker covers a security the reader reached before the
    // conversation had mentioned it — the SDK folds a repeat subscription into the
    // existing one.
    subscribe_quote(&symbol);
    ui.quote_panel = Some(symbol);
}

/// Close the panel. The subscription stays: the ticker and the inline prices are
/// reading the same stream, and dropping it here would freeze them.
fn close_quote_panel(ui: &mut Ui) {
    ui.quote_panel = None;
}

fn subscribe_quote(symbol: &str) {
    let symbol = symbol.to_string();
    spawn_bg(async move {
        let _ = crate::openapi::quote()
            .subscribe([symbol], longbridge::quote::SubFlags::QUOTE)
            .await;
    });
}

/// Spawn background work, doing nothing when there is no runtime.
///
/// Opening the panel is a UI action and has to stay callable from a render test,
/// where `tokio::spawn` would panic for want of a reactor.
fn spawn_bg<F>(task: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(task);
    }
}

/// Open the Conversations view and fetch the account's chats in the background.
fn open_sessions(ui: &mut Ui) {
    ui.view = View::Sessions;
    ui.sel = 0;
    ui.search.clear();
    ui.question = None;
    ui.sessions_loading = true;
    ui.sessions_error = false;
    if let Some(tx) = ui.history_tx.clone() {
        tokio::spawn(async move {
            let _ = tx.send(session_store::list_summaries().await);
        });
    }
}

/// Record the current question's chosen option and advance, submitting the
/// continuation once every question is answered.
fn answer_selected(
    ui: &mut Ui,
    state: &mut ChatState,
    turn: &mut Option<JoinHandle<()>>,
    tx: &UnboundedSender<ChatEvent>,
) {
    let Some(q) = ui.question.as_mut() else {
        return;
    };
    let Some((question, options)) = q.questions.get(q.qi) else {
        return;
    };
    if let Some(choice) = options.get(ui.sel) {
        q.answers.insert(question.clone(), choice.clone());
    }
    q.qi += 1;
    ui.sel = 0;
    if q.qi >= q.questions.len() {
        let qs = ui.question.take().expect("question present");
        submit_answers(&qs, state, turn, tx);
        ui.view = View::Chat;
    }
}

fn submit_answers(
    qs: &QuestionState,
    state: &mut ChatState,
    turn: &mut Option<JoinHandle<()>>,
    tx: &UnboundedSender<ChatEvent>,
) {
    let (Some(chat_uid), Some(message_id)) = (state.chat_uid.clone(), state.message_id.clone())
    else {
        return;
    };
    let answers = runtime::answers_by_tool_call(&qs.tool_call_id, &qs.answers);
    let summary = qs
        .questions
        .iter()
        .filter_map(|(q, _)| qs.answers.get(q).cloned())
        .collect::<Vec<_>>()
        .join(", ");
    let req = ConversationRequest::Continue {
        agent_uid: state.agent_uid.clone(),
        chat_uid,
        message_id,
        answers,
    };
    state.apply(ChatEvent::UserPrompt(summary));
    state.pending_interrupt = None;
    *turn = Some(runtime::spawn_turn(req, tx.clone()));
}

/// Handle a click on a Chat meta chip: open a reference URL, or send a
/// suggested follow-up as the next prompt.
fn click_chip(
    chip: Chip,
    ui: &mut Ui,
    state: &mut ChatState,
    turn: &mut Option<JoinHandle<()>>,
    tx: &UnboundedSender<ChatEvent>,
) {
    match chip {
        Chip::Reference(i) => {
            if let Some(url) = state.references.get(i).and_then(reference_url) {
                open_url(&url);
            }
        }
        Chip::Symbol(symbol) => open_quote_panel(ui, symbol),
        Chip::Tape => {
            let meta = crate::tui::settings::all()
                .iter()
                .find(|m| m.id == crate::tui::settings::SettingId::Tape);
            if let Some(meta) = meta {
                crate::tui::settings::cycle(meta);
            }
        }
        Chip::Further(i) => {
            if state.busy {
                return;
            }
            if let Some(query) = state.further.get(i).cloned() {
                ui.notice = None;
                let req = runtime::build_request(state, query.clone());
                state.apply(ChatEvent::UserPrompt(query));
                state.pending_interrupt = None;
                *turn = Some(runtime::spawn_turn(req, tx.clone()));
            }
        }
    }
}

/// Copy `text` to the system clipboard via the OSC 52 terminal escape, which
/// works over SSH and inside tmux (with clipboard passthrough) without a
/// native clipboard dependency. Writing an OSC sequence does not disturb the
/// ratatui screen, so it is safe to emit mid-render.
fn copy_to_clipboard(text: &str) -> bool {
    use base64::Engine;
    use std::io::Write;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let seq = format!("\x1b]52;c;{encoded}\x07");
    let mut out = std::io::stdout();
    out.write_all(seq.as_bytes())
        .and_then(|()| out.flush())
        .is_ok()
}

/// The whole conversation as `Role: text` blocks, for `/copy`.
fn transcript_text(state: &ChatState) -> String {
    state
        .messages
        .iter()
        .filter(|m| m.role != Role::System)
        .map(|m| {
            let who = if m.role == Role::User {
                t!("Ai.You")
            } else {
                t!("Ai.Assistant")
            };
            format!("{who}: {}", m.text)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Write the conversation to a timestamped Markdown file (in the user's
/// Downloads dir, falling back to home then the temp dir) and return its path.
/// Assistant text is already Markdown, so it is written through verbatim.
fn export_conversation(state: &ChatState) -> std::io::Result<std::path::PathBuf> {
    use std::fmt::Write;
    let dir = dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(std::env::temp_dir);
    let path = dir.join(format!("longbridge-ai-{}.md", now_secs()));
    let mut body = format!("# {}\n\n", t!("Ai.Title"));
    for m in &state.messages {
        let label = match m.role {
            Role::User => t!("Ai.You"),
            Role::Assistant => t!("Ai.Assistant"),
            // Tool lines are UI trace, not conversation; an export is the
            // conversation.
            Role::System | Role::Tool => continue,
        };
        let _ = write!(body, "**{label}:**\n\n{}\n\n", m.text);
    }
    std::fs::write(&path, body)?;
    Ok(path)
}

/// Copy `text` and set a status notice reflecting the outcome.
fn copy_with_notice(ui: &mut Ui, text: Option<String>) {
    ui.notice = Some(match text {
        Some(t) if !t.trim().is_empty() && copy_to_clipboard(&t) => t!("Ai.Copied").to_string(),
        _ => t!("Ai.NothingToCopy").to_string(),
    });
}

/// Open a URL with the platform's default handler (best-effort, non-blocking).
fn open_url(url: &str) {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(program).arg(url).spawn();
}

fn hit(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
}

/// Render `full` for a single-line row that may be wider than `width`. When the
/// row is truncated *and* focused (hovered or keyboard-selected), it scrolls as
/// a marquee and the frame is marked `animating`; otherwise the text is returned
/// as-is and the `Paragraph` clips it.
fn row_text(ui: &mut Ui, full: &str, width: usize, rect: Rect, selected: bool) -> String {
    let focused = selected || ui.hover.is_some_and(|(c, r)| hit(rect, c, r));
    if focused && full.width() > width {
        ui.animating = true;
        marquee(full, width, ui.tick)
    } else {
        full.to_string()
    }
}

/// A window of `width` display columns into `text`, scrolled by `tick` and
/// wrapping around through a small gap so the text loops continuously.
fn marquee(text: &str, width: usize, tick: u64) -> String {
    if width == 0 || text.width() <= width {
        return text.to_string();
    }
    let full: Vec<char> = text.chars().chain("   ".chars()).collect();
    let n = full.len();
    let start = (tick as usize) % n;
    let mut out = String::new();
    let mut w = 0;
    let mut i = 0;
    while w < width && i < n {
        let ch = full[(start + i) % n];
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > width {
            break;
        }
        out.push(ch);
        w += cw;
        i += 1;
    }
    out
}

/// Whether the command palette owns the input, i.e. the user is still typing a
/// bare command name. Once an argument is started the palette closes, so Enter
/// submits `/agent <id>` through [`submit`] instead of re-running the command
/// with no argument.
fn slash_active(editor: &Editor) -> bool {
    let text = editor.text();
    editor.is_single_line()
        && text.starts_with('/')
        && !text.trim_end().contains(char::is_whitespace)
}

/// SLASH indices whose name or an alias starts with the current input.
fn slash_matches(editor: &Editor) -> Vec<usize> {
    let prefix = editor.text();
    let prefix = prefix.trim_end();
    SLASH
        .iter()
        .enumerate()
        .filter(|(_, cmd)| cmd.starts_with(prefix))
        .map(|(i, _)| i)
        .collect()
}

/// Complete the input to the highlighted command's canonical name.
fn complete_slash(ui: &Ui, editor: &mut Editor) {
    let matches = slash_matches(editor);
    if let Some(&idx) = matches.get(ui.slash_sel.min(matches.len().saturating_sub(1))) {
        editor.set_text(&format!("{} ", SLASH[idx].name));
    }
}

/// Run the highlighted palette command.
fn run_slash_selected(ui: &mut Ui, state: &mut ChatState, editor: &mut Editor) {
    let matches = slash_matches(editor);
    if let Some(&idx) = matches.get(ui.slash_sel) {
        run_slash(idx, ui, state, editor);
    }
}

/// Clear the input and execute the command at `SLASH[idx]`, with no argument —
/// the palette only ever holds a bare command name.
fn run_slash(idx: usize, ui: &mut Ui, state: &mut ChatState, editor: &mut Editor) {
    let key = SLASH[idx].key();
    editor.clear();
    exec_slash(key, "", ui, state);
}

// ── rendering ────────────────────────────────────────────────────────────────

fn view(f: &mut ratatui::Frame, ui: &mut Ui, state: &ChatState, editor: &Editor) {
    // Recomputed each frame: set true if any truncated row is scrolling.
    ui.animating = false;
    let area = f.area();
    // Below this the layout can't fit the header/status/prompt legibly.
    if area.width < 24 || area.height < 6 {
        f.render_widget(
            Paragraph::new(t!("Ai.WindowTooSmall").to_string())
                .style(Style::default().fg(Color::DarkGray))
                .alignment(ratatui::layout::Alignment::Center),
            area,
        );
        return;
    }
    let is_chat = ui.view == View::Chat;
    // Keep the frame timer running while a turn streams so the status spinner
    // animates even between deltas (e.g. during a long tool call).
    if is_chat && state.busy {
        ui.animating = true;
    }
    // The elapsed clock is derived from `busy` rather than set by each of the
    // several places that start or cancel a turn, so it cannot fall out of sync.
    if state.busy {
        ui.turn_started.get_or_insert_with(std::time::Instant::now);
    } else {
        ui.turn_started = None;
    }
    let has_meta = is_chat && !state.busy && !state.further.is_empty();
    let meta_h = if has_meta { meta_height(state) } else { 0 };
    let footer_h = if is_chat {
        (editor.lines().len() as u16 + 2).clamp(3, 8)
    } else {
        3
    };

    // A running turn gets a row of its own so its spinner, timer and cancel
    // button cannot be hidden by a notice — and the notice cannot be hidden by
    // it. Only while busy, so idle chrome stays one row on a short terminal.
    let has_turn = is_chat && state.busy;
    // The title gets a blank row under it so it reads as chrome rather than as
    // the transcript's first line. Skipped on a short terminal, where a spare
    // row is worth more than the separation.
    let title_h = if area.height >= 12 { 2 } else { 1 };
    let mut constraints = vec![Constraint::Length(title_h), Constraint::Min(1)];
    if has_meta {
        constraints.push(Constraint::Length(meta_h));
    }
    if has_turn {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Length(footer_h));
    let chunks = Layout::vertical(constraints).split(area);

    let (title, body) = (chunks[0], chunks[1]);
    let mut idx = 2;
    let meta = has_meta.then(|| {
        let m = chunks[idx];
        idx += 1;
        m
    });
    let turn_row = has_turn.then(|| {
        let r = chunks[idx];
        idx += 1;
        r
    });
    let status = chunks[idx];
    idx += 1;
    let footer = chunks[idx];

    // Chips are recorded by both the transcript (references) and the meta panel
    // (follow-ups), so the list is cleared once per frame rather than by each.
    ui.chips.clear();
    render_title(f, title, ui, state);
    // The Chat view is chrome-free; other views get a header with the view name
    // (they are opened via `/` commands and left with Esc).
    match ui.view {
        View::Chat => render_chat(f, body, ui, state),
        View::Question => render_question(f, body, ui),
        View::Sessions => {
            let inner = render_view_header(f, body, "Ai.TabSessions");
            render_sessions(f, inner, ui);
        }
        View::Settings => {
            let inner = render_view_header(f, body, "Ai.TabSettings");
            render_settings(f, inner, ui);
        }
    }
    if ui.view == View::Chat {
        render_slash_dropdown(f, body, ui, editor);
    } else {
        ui.slash_rows.clear();
    }
    if let Some(meta) = meta {
        render_chips(f, meta, ui, state);
    }
    if let Some(row) = turn_row {
        render_turn_status(f, row, ui, state);
    } else {
        ui.stop_button = None;
    }
    render_status(f, status, ui, state);
    render_footer(f, footer, ui, editor);
    // Last, so it floats over whatever is underneath.
    render_quote_panel(f, body, ui);
}

/// Draw the floating quote panel, if one is open.
///
/// Centred over the transcript and no wider than the card it holds, so the answer
/// the symbol was read in stays visible around it.
fn render_quote_panel(f: &mut ratatui::Frame, area: Rect, ui: &Ui) {
    use ratatui::widgets::{Block, BorderType, Borders, Clear};

    let Some(symbol) = &ui.quote_panel else {
        return;
    };
    // Wide enough for two labelled columns with room to breathe: this is the
    // panel the reader opened to look at a price, not a chip.
    let inner_w = 52usize.min(area.width.saturating_sub(4) as usize);
    let body = match ui.quotes.get(symbol) {
        Some(card) => card_lines(card, inner_w),
        None => vec![Line::from(Span::styled(
            t!("Ai.QuoteLoading").to_string(),
            Style::default().fg(Color::DarkGray),
        ))],
    };
    // Everything the panel shows is assembled before it is measured — sizing off
    // the body alone clipped the hint that says how to close it.
    let mut lines = body;
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        t!("Ai.QuotePanelHint").to_string(),
        Style::default().fg(Color::DarkGray),
    )));
    let height = (lines.len() as u16 + 2).min(area.height);
    let width = (inner_w as u16 + 4).min(area.width);
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Line::from(vec![
            Span::styled(
                format!(" {symbol} "),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            // The dot says the number is streaming, so a price that sits still is
            // a quiet market rather than a stale panel.
            Span::styled("● ", Style::default().fg(Color::Green)),
        ]));
    f.render_widget(Clear, rect);
    f.render_widget(Paragraph::new(Text::from(lines)).block(block), rect);
}

fn render_title(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui, state: &ChatState) {
    // The brand badge, and nothing else by default. The server-generated
    // conversation title lived here, but it is a label for picking a chat out of
    // a list — which is where it is shown — not something worth a permanent row
    // in the chat you are already reading.
    let mut left = vec![Span::styled(
        format!(" {} ", t!("Ai.Title")),
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )];
    // A marker only when the conversation is not with Longbridge AI's own
    // assistant. An agent uid is an internal handle and never goes on screen (see
    // `cli::agent::DEFAULT_AGENT_UID`), so this says *that* a custom agent is in
    // use without naming it — the badge on the left already names the default.
    if state.agent_uid != DEFAULT_AGENT_UID {
        left.push(Span::styled(
            format!("  {}", t!("Ai.CustomAgent")),
            Style::default().fg(Color::DarkGray),
        ));
    }
    let left_w: usize = left.iter().map(|s| s.content.width()).sum();

    // The rest of the row is the ticker: the securities this conversation has
    // mentioned, with their quotes. It is the one row of chrome that was carrying
    // nothing, and a trader reading about a stock wants its price in view.
    let toggle = if super::settings::tape() {
        " ▾ "
    } else {
        " ▸ "
    };
    let toggle_w = toggle.width();
    let room = (area.width as usize).saturating_sub(left_w + toggle_w);
    let tape = if super::settings::tape() && !ui.tape.is_empty() {
        tape_spans(ui, room)
    } else {
        Vec::new()
    };
    let tape_w: usize = tape.iter().map(|s| s.content.width()).sum();
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(room.saturating_sub(tape_w))));
    spans.extend(tape);
    // The toggle sits at the end of the row, where it is out of the ticker's way.
    let toggle_x = area.x + area.width.saturating_sub(toggle_w as u16);
    spans.push(Span::styled(toggle, Style::default().fg(Color::DarkGray)));
    if !ui.tape.is_empty() {
        ui.chips.push((
            Chip::Tape,
            Rect {
                x: toggle_x,
                y: area.y,
                width: toggle_w as u16,
                height: 1,
            },
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The ticker's spans, rotated to fit `room` columns.
///
/// Rotation is by whole securities rather than by column: a price sliding through
/// half a symbol is unreadable, and a trader glancing up needs to take a whole
/// entry in at once. The rotation advances only when there is more to show than
/// fits, so a short list sits still.
fn tape_spans(ui: &mut Ui, room: usize) -> Vec<Span<'static>> {
    const GAP: &str = "   ";
    // The name stays neutral and only the price carries the direction: colouring
    // the symbol too made the whole row swing red and green with no added meaning.
    let entries: Vec<(String, String, Color)> = ui
        .tape
        .iter()
        .map(|symbol| match ui.quotes.get(symbol) {
            Some(card) => (
                symbol.clone(),
                price_chip(card),
                change_color(card.direction),
            ),
            None => (symbol.clone(), String::new(), Color::DarkGray),
        })
        .collect();
    if entries.is_empty() || room == 0 {
        return Vec::new();
    }
    let total: usize = entries
        .iter()
        .map(|(symbol, price, _)| symbol.width() + price.width() + GAP.width())
        .sum();
    let rotating = total > room;
    if !rotating {
        ui.tape_at = 0;
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for i in 0..entries.len() {
        let (symbol, price, color) = &entries[(ui.tape_at + i) % entries.len()];
        let w = symbol.width() + price.width() + if used == 0 { 0 } else { GAP.width() };
        if used + w > room {
            break;
        }
        if used > 0 {
            spans.push(Span::raw(GAP));
        }
        spans.push(Span::styled(
            symbol.clone(),
            Style::default().fg(Color::Gray),
        ));
        if !price.is_empty() {
            spans.push(Span::styled(price.clone(), Style::default().fg(*color)));
        }
        used += w;
    }
    if rotating {
        // The frame timer has to keep running for the ticker to advance, and the
        // step is one entry a second — fast enough to get through a long list,
        // slow enough to read.
        ui.animating = true;
        ui.tape_ticks = ui.tape_ticks.wrapping_add(1);
        if ui.tape_ticks >= 8 {
            ui.tape_ticks = 0;
            ui.tape_at = (ui.tape_at + 1) % entries.len();
        }
    }
    spans
}

/// Header for a `/`-opened view: a bold name badge and an "Esc to go back"
/// hint. Returns the remaining area below it for the view body.
fn render_view_header(f: &mut ratatui::Frame, area: Rect, label_key: &str) -> Rect {
    let [top, rest] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", t!(label_key)),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", t!("Ai.BackHint")),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(line), top);
    rest
}

fn render_chat(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui, state: &ChatState) {
    ui.transcript = area;
    // Before the first exchange, show a centered welcome instead of the lone
    // system line, so an empty session doesn't look bare.
    if state.messages.len() <= 1 && state.streaming.is_none() && !state.busy {
        ui.selected_text = None;
        render_empty_state(f, area);
        return;
    }
    let width = area.width.max(1) as usize;
    // Committed messages are parsed once and cached; only the streaming tail is
    // re-rendered each frame, and only the visible window is cloned. This keeps
    // the 120ms spinner ticks from re-parsing the whole transcript's Markdown.
    let sig = transcript_sig(state, width);
    if ui.cache_sig != sig {
        let mut cache = Vec::new();
        for m in &state.messages {
            push_message(&mut cache, m, width, &ui.quotes, &ui.aliases);
        }
        ui.transcript_cache = cache;
        ui.cache_sig = sig;
    }
    // The tail is whatever is not worth caching: the answer still streaming, and
    // the finished turn's references, which change with every turn.
    let mut tail = Vec::new();
    if let Some(text) = &state.streaming {
        push_message(
            &mut tail,
            &Message::new(Role::Assistant, text.clone()),
            width,
            &ui.quotes,
            &ui.aliases,
        );
    }
    let cache_len = ui.transcript_cache.len();
    // Where each reference row lands in the transcript, so a visible one can be
    // clicked even though it now scrolls with the content.
    let mut ref_rows: Vec<(usize, usize)> = Vec::new();
    if !state.busy && !state.references.is_empty() {
        tail.push(Line::from(Span::styled(
            format!("{}:", t!("Agent.References")),
            Style::default().fg(Color::DarkGray),
        )));
        for (i, r) in state.references.iter().enumerate() {
            ref_rows.push((cache_len + tail.len(), i));
            tail.push(Line::from(Span::styled(
                truncate_width(&format!("  [{}] {}", r.index, reference_label(r)), width),
                Style::default().fg(Color::Blue),
            )));
        }
        tail.push(Line::from(""));
    }
    let total = cache_len + tail.len();
    let height = area.height as usize;
    // Clamp scroll-back so the view can never be scrolled past the top into a
    // blank screen; the top is reached when `start` hits 0.
    ui.max_scroll = u16::try_from(total.saturating_sub(height)).unwrap_or(u16::MAX);
    let scroll = (state.scroll).min(ui.max_scroll) as usize;
    let bottom = total.saturating_sub(scroll);
    let start = bottom.saturating_sub(height);
    let mut window: Vec<Line> = (start..bottom)
        .map(|i| {
            if i < cache_len {
                ui.transcript_cache[i].clone()
            } else {
                tail[i - cache_len].clone()
            }
        })
        .collect();

    for (row, i) in ref_rows {
        if (start..bottom).contains(&row) {
            let rect = row_rect(area, area.y + (row - start) as u16);
            ui.chips.push((Chip::Reference(i), rect));
        }
    }
    link_visible_symbols(&mut window, area, ui);

    let Some((anchor, cursor)) = ui.selection else {
        ui.selected_text = None;
        f.render_widget(Paragraph::new(Text::from(window)), area);
        return;
    };
    // Highlight the selected span on each row and gather its text.
    let (top, end) = if anchor <= cursor {
        (anchor, cursor)
    } else {
        (cursor, anchor)
    };
    let mut out = Vec::with_capacity(window.len());
    let mut picked: Vec<String> = Vec::new();
    for (i, line) in window.into_iter().enumerate() {
        let row = area.y + i as u16;
        if row < top.0 || row > end.0 {
            out.push(line);
            continue;
        }
        let from = if row == top.0 {
            top.1.saturating_sub(area.x) as usize
        } else {
            0
        };
        let to = if row == end.0 {
            end.1.saturating_sub(area.x) as usize
        } else {
            usize::MAX
        };
        let (highlighted, text) = select_line(&line, from, to);
        if !text.is_empty() {
            picked.push(text);
        }
        out.push(highlighted);
    }
    ui.selected_text = (!picked.is_empty()).then(|| picked.join("\n"));
    f.render_widget(Paragraph::new(Text::from(out)), area);
}

/// A centered welcome shown for a fresh, empty session.
fn render_empty_state(f: &mut ratatui::Frame, area: Rect) {
    let mut content = vec![
        Line::from(Span::styled(
            t!("Ai.Title").to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            t!("Ai.Welcome").to_string(),
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
    ];
    // A few example prompts to set the tone (inspiration, not clickable).
    for key in ["Ai.Sample1", "Ai.Sample2", "Ai.Sample3"] {
        content.push(Line::from(Span::styled(
            format!("“{}”", t!(key)),
            Style::default().fg(Color::DarkGray),
        )));
    }
    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        t!("Ai.EmptyHint").to_string(),
        Style::default().fg(Color::DarkGray),
    )));
    // The brand mark goes above the copy, but only when the area can hold both:
    // the welcome text and the example prompts are what make the empty state
    // useful, so the mark yields to them rather than pushing them off screen. The
    // whole block is centre-aligned, so the mark needs no indent of its own —
    // adding one would offset it twice.
    let mark_h = usize::from(assets::mark_height());
    if area.height as usize >= mark_h + content.len() + 2 && area.width >= assets::mark_width() {
        let mut with_logo = assets::logo_mark();
        with_logo.push(Line::from(""));
        with_logo.extend(content);
        content = with_logo;
    }
    let top = (area.height as usize).saturating_sub(content.len()) / 2;
    let mut lines = vec![Line::from(""); top];
    lines.extend(content);
    f.render_widget(
        Paragraph::new(Text::from(lines)).alignment(ratatui::layout::Alignment::Center),
        area,
    );
}

/// Reverse-video the display columns `[from, to)` of `line`, returning the
/// restyled line and the plain text of the selected span.
fn select_line(line: &Line, from: usize, to: usize) -> (Line<'static>, String) {
    let mut cells: Vec<(char, Style)> = Vec::new();
    let mut picked = String::new();
    let mut col = 0usize;
    for span in &line.spans {
        for ch in span.content.chars() {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
            let selected = cw > 0 && col >= from && col < to;
            let style = if selected {
                picked.push(ch);
                span.style.add_modifier(Modifier::REVERSED)
            } else {
                span.style
            };
            cells.push((ch, style));
            col += cw;
        }
    }
    (coalesce_cells(&cells), picked)
}

/// Merge a run of styled chars into a [`Line`], grouping equal styles.
fn coalesce_cells(cells: &[(char, Style)]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut current: Option<Style> = None;
    for (ch, style) in cells {
        if current != Some(*style) {
            if let Some(prev) = current {
                spans.push(Span::styled(std::mem::take(&mut buf), prev));
            }
            current = Some(*style);
        }
        buf.push(*ch);
    }
    if let Some(prev) = current {
        spans.push(Span::styled(buf, prev));
    }
    Line::from(spans)
}

const HOVER_BG: Color = Color::Rgb(48, 48, 48);

/// The slash-command palette: a rounded, bordered menu of matching commands
/// floating above the prompt. ↑/↓ move the highlight, Enter/click runs it, and
/// the command names are column-aligned with dimmed descriptions.
fn render_slash_dropdown(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui, editor: &Editor) {
    ui.slash_rows.clear();
    if !slash_active(editor) {
        return;
    }
    let matches = slash_matches(editor);
    if matches.is_empty() {
        return;
    }
    ui.slash_sel = ui.slash_sel.min(matches.len() - 1);
    let name_w = matches
        .iter()
        .map(|&i| SLASH[i].name.len())
        .max()
        .unwrap_or(0);
    let box_h = matches.len() as u16 + 2;
    let box_w = area.width.clamp(24, 56);
    let box_area = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(box_h),
        width: box_w,
        height: box_h,
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            format!(" {} ", t!("Ai.CommandsTitle")),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(box_area);
    f.render_widget(Clear, box_area);
    f.render_widget(block, box_area);

    let iw = inner.width as usize;
    let mut lines = Vec::new();
    for (row, &idx) in matches.iter().enumerate() {
        let name = SLASH[idx].name;
        let desc = t!(SLASH[idx].desc).to_string();
        let selected = row == ui.slash_sel;
        let rect = Rect {
            x: inner.x,
            y: inner.y + row as u16,
            width: inner.width,
            height: 1,
        };
        let content = format!(" {name:<name_w$}   {desc}");
        let line = if selected {
            Line::from(Span::styled(
                pad_to(&content, iw),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))
        } else {
            let bg = hovering(ui, rect).then_some(HOVER_BG);
            let mut name_style = Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD);
            let mut desc_style = Style::default().fg(Color::DarkGray);
            if let Some(bg) = bg {
                name_style = name_style.bg(bg);
                desc_style = desc_style.bg(bg);
            }
            let mut spans = vec![
                Span::styled(format!(" {name:<name_w$}   "), name_style),
                Span::styled(desc.clone(), desc_style),
            ];
            let used = 1 + name_w + 3 + desc.width();
            if let Some(bg) = bg {
                if used < iw {
                    spans.push(Span::styled(" ".repeat(iw - used), Style::default().bg(bg)));
                }
            }
            Line::from(spans)
        };
        lines.push(line);
        ui.slash_rows.push((idx, rect));
    }
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

/// Pad `s` with spaces to `width` display columns (no truncation).
fn pad_to(s: &str, width: usize) -> String {
    let w = s.width();
    if w >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - w))
    }
}

/// Render the History list as spaced, two-line entries — a numbered badge, the
/// title, and a dimmed `agent · age` subtitle — with a trailing "New session"
/// action, an optional search line, and a hit rectangle per entry.
// The loop index spans past `entries` into the trailing New-session row, so it
// cannot be a plain iterator over `entries`.
#[allow(clippy::needless_range_loop)]
fn render_sessions(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui) {
    ui.rows.clear();
    if ui.sessions_loading && ui.sessions.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                t!("Ai.SessionsLoading").to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            area,
        );
        return;
    }
    ui.clamp_sel();
    // A search line appears above the list only while filtering.
    let list_area = if ui.search.is_empty() {
        area
    } else {
        let [top, rest] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("/ ", Style::default().fg(Color::Cyan)),
                Span::raw(ui.search.clone()),
                Span::styled("▏", Style::default().fg(Color::Cyan)),
            ])),
            top,
        );
        rest
    };

    let now = now_secs();
    let entries: Vec<(String, String)> = ui
        .visible_sessions()
        .iter()
        .map(|s| {
            let when = relative_time(s.updated_at, now);
            // An unnamed agent contributes nothing, so no dangling separator.
            let sub = if s.agent.is_empty() {
                when
            } else {
                format!("{}  ·  {when}", s.agent)
            };
            (s.title.clone(), sub)
        })
        .collect();
    let n = entries.len();
    if ui.sessions_error && n == 0 {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                t!("Ai.SessionsError").to_string(),
                Style::default().fg(Color::Red),
            ))),
            list_area,
        );
        return;
    }
    if n == 0 && !ui.search.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                t!("Ai.SessionsNoMatch").to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            list_area,
        );
        return;
    }
    // An account with no conversations still gets the "New session" row below,
    // so the empty state is a note above the list rather than a replacement.
    let list_area = if n == 0 {
        let [note, rest] =
            Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(list_area);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                t!("Ai.SessionsEmpty").to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            note,
        );
        rest
    } else {
        list_area
    };

    // One row per entry: a conversation is an index, a title and when it was
    // last touched, all of which fit on a line. Spreading each over three rows
    // showed four conversations where a screen can hold twenty.
    let total = n + 1;
    let fit = (list_area.height as usize).max(1);
    let start = if ui.sel < fit {
        0
    } else {
        (ui.sel + 1 - fit).min(total.saturating_sub(fit))
    };
    let width = list_area.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    for i in start..total {
        if lines.len() >= list_area.height as usize {
            break;
        }
        let rect = Rect {
            x: list_area.x,
            y: list_area.y + lines.len() as u16,
            width: list_area.width,
            height: 1,
        };
        let selected = i == ui.sel;
        let bg = if selected {
            Some(SEL_BG)
        } else if hovering(ui, rect) {
            Some(HOVER_BG)
        } else {
            None
        };
        if i < n {
            let (title, subtitle) = &entries[i];
            push_session_entry(&mut lines, i + 1, title, subtitle, width, selected, bg);
        } else {
            lines.push(bg_pad(
                vec![
                    Span::styled(" +  ", with_bg(Style::default().fg(IDX), bg)),
                    Span::styled(
                        t!("Ai.NewSessionAction").to_string(),
                        with_bg(Style::default().fg(Color::Gray), bg),
                    ),
                ],
                width,
                bg,
            ));
        }
        ui.rows.push((i, rect));
    }
    f.render_widget(Paragraph::new(Text::from(lines)), list_area);
}

/// Push a two-line History entry: `NN  Title` then an indented dimmed subtitle,
/// both tinted with the row background when selected/hovered.
fn push_session_entry(
    lines: &mut Vec<Line<'static>>,
    number: usize,
    title: &str,
    subtitle: &str,
    width: usize,
    selected: bool,
    bg: Option<Color>,
) {
    let idx_color = if selected { IDX_SEL } else { IDX };
    let mut title_style = Style::default().fg(if selected { Color::White } else { Color::Gray });
    if selected {
        title_style = title_style.add_modifier(Modifier::BOLD);
    }
    // The subtitle is trailing detail, so it keeps its columns and the title
    // gives way — a truncated "…" beats a title that pushes the timestamp off
    // the row.
    let lead = format!("{number:>2}  ");
    let lead_w = UnicodeWidthStr::width(lead.as_str());
    let sub_w = UnicodeWidthStr::width(subtitle);
    let title_w = width.saturating_sub(lead_w + sub_w + 2);
    let title = truncate_width(title, title_w);
    let gap = width.saturating_sub(lead_w + UnicodeWidthStr::width(title.as_str()) + sub_w);
    lines.push(bg_pad(
        vec![
            Span::styled(lead, with_bg(Style::default().fg(idx_color), bg)),
            Span::styled(title, with_bg(title_style, bg)),
            Span::styled(
                format!("{}{subtitle}", " ".repeat(gap)),
                with_bg(Style::default().fg(Color::DarkGray), bg),
            ),
        ],
        width,
        bg,
    ));
}

/// Apply `bg` to `style` when present.
fn with_bg(style: Style, bg: Option<Color>) -> Style {
    match bg {
        Some(c) => style.bg(c),
        None => style,
    }
}

/// Extend a row's background to the full width by padding with a trailing span.
fn bg_pad(mut spans: Vec<Span<'static>>, width: usize, bg: Option<Color>) -> Line<'static> {
    if let Some(bg) = bg {
        let used: usize = spans.iter().map(|s| s.content.width()).sum();
        if used < width {
            spans.push(Span::styled(
                " ".repeat(width - used),
                Style::default().bg(bg),
            ));
        }
    }
    Line::from(spans)
}

/// Compact "just now / 3m / 2h / 5d" age of an entry, from Unix seconds.
fn relative_time(updated: u64, now: u64) -> String {
    let secs = now.saturating_sub(updated);
    if secs < 60 {
        t!("Ai.JustNow").to_string()
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// Render the Settings panel and record a hit rectangle per row.
fn render_settings(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui) {
    use ratatui::widgets::{List, ListItem, ListState};

    ui.rows.clear();
    ui.clamp_sel();
    // The session goes above the preferences: "which account am I asking about my
    // portfolio?" has no other answer from inside the chat.
    let header = session_lines(&ui.session);
    let header_h = (header.len() as u16).min(area.height);
    let [head, list_area] =
        Layout::vertical([Constraint::Length(header_h), Constraint::Min(0)]).areas(area);
    f.render_widget(Paragraph::new(Text::from(header)), head);

    let rows = settings_rows(&ui.session);
    let width = list_area.width as usize;
    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| match row {
            SettingsRow::Setting(meta) => {
                let label = t!(meta.label).to_string();
                let value = t!(meta.value_label()).to_string();
                // The value is right-aligned so a column of them reads down the
                // page, and the label gives way when the row is tight — a setting
                // you cannot read the value of is not set.
                let gap = width
                    .saturating_sub(2 + UnicodeWidthStr::width(label.as_str()))
                    .saturating_sub(UnicodeWidthStr::width(value.as_str()) + 2);
                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(format!("  {label}"), Style::default().fg(Color::Gray)),
                        Span::raw(" ".repeat(gap)),
                        Span::styled(
                            value,
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(Span::styled(
                        truncate_width(&format!("  {}", t!(meta.description)), width),
                        Style::default().fg(Color::DarkGray),
                    )),
                ])
            }
            SettingsRow::SignOut => action_item(
                &t!("Ai.SignOut"),
                &if ui.confirm_sign_out {
                    t!("Ai.SignOutConfirm").to_string()
                } else {
                    t!("Ai.SignOutHint").to_string()
                },
                if ui.confirm_sign_out {
                    Color::Red
                } else {
                    Color::Gray
                },
                width,
            ),
            SettingsRow::SignIn => {
                action_item(&t!("Ai.SignIn"), &t!("Ai.SignInHint"), Color::Green, width)
            }
        })
        .collect();
    let mut list_state = ListState::default().with_selected(Some(ui.sel));
    f.render_stateful_widget(
        List::new(items).highlight_style(Style::default().bg(SEL_BG)),
        list_area,
        &mut list_state,
    );
    // Hit rects come after the render, because the widget is what decides where
    // the list scrolled to — computing them from the selection alone would map a
    // click to the wrong row once the list is longer than the pane. Two rows per
    // item: the setting and what it does. A blank row between them cost a fifth of
    // the pane, and the highlight already marks the selection.
    let offset = list_state.offset();
    for i in offset..rows.len() {
        let y = list_area.y + ((i - offset) as u16) * 2;
        let Some(height) = (list_area.y + list_area.height)
            .checked_sub(y)
            .filter(|h| *h > 0)
        else {
            break;
        };
        ui.rows.push((
            i,
            Rect {
                x: list_area.x,
                y,
                width: list_area.width,
                height: height.min(2),
            },
        ));
    }
}

/// One action row, shaped like a setting row so the list reads evenly.
fn action_item<'a>(
    label: &str,
    hint: &str,
    color: Color,
    width: usize,
) -> ratatui::widgets::ListItem<'a> {
    ratatui::widgets::ListItem::new(vec![
        Line::from(Span::styled(
            format!("  {label}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            truncate_width(&format!("  {hint}"), width),
            Style::default().fg(Color::DarkGray),
        )),
    ])
}

/// The session header: who, where, and how long the credentials last.
fn session_lines(session: &super::account::Session) -> Vec<Line<'static>> {
    let dim = Style::default().fg(Color::DarkGray);
    let (status, color) = match session.status {
        "valid" | "present" => (t!("Ai.SessionValid").to_string(), Color::Green),
        "refresh_pending" => (t!("Ai.SessionRefreshing").to_string(), Color::Yellow),
        "expired" => (t!("Ai.SessionExpired").to_string(), Color::Red),
        "decrypt_failed" => (t!("Ai.SessionUnreadable").to_string(), Color::Red),
        _ => (t!("Ai.SessionNone").to_string(), Color::Red),
    };
    let mut first = vec![
        Span::styled(format!("  {}  ", t!("Ai.Account")), dim),
        Span::styled(status, Style::default().fg(color)),
    ];
    if let Some(id) = &session.member_id {
        first.push(Span::styled(
            format!("   #{id}"),
            Style::default().fg(Color::Gray),
        ));
    }
    let mut second = vec![Span::styled(
        format!("  {}", session.access_point.trim_start_matches("https://")),
        dim,
    )];
    if let Some(dc) = session.dc_region {
        second.push(Span::styled(format!("  ·  {}", dc.to_uppercase()), dim));
    }
    if let Some(exp) = session.expires_at {
        second.push(Span::styled(
            format!(
                "  ·  {} {}",
                t!("Ai.SessionExpires"),
                relative_time(exp, now_secs())
            ),
            dim,
        ));
    }
    vec![Line::from(first), Line::from(second), Line::from("")]
}

/// Render the structured Question view: the current question and its options.
fn render_question(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui) {
    let Some((question, options)) = ui
        .question
        .as_ref()
        .and_then(|q| q.questions.get(q.qi))
        .map(|(t, o)| (t.clone(), o.clone()))
    else {
        ui.rows.clear();
        return;
    };
    // The question text sits above the option list (which reuses the shared
    // muted list style), separated by a blank line.
    let width = area.width.max(1) as usize;
    let qlines = wrap(&question, width);
    let header_h = (qlines.len() as u16 + 1).min(area.height.saturating_sub(1));
    let [head, rest] =
        Layout::vertical([Constraint::Length(header_h), Constraint::Min(0)]).areas(area);
    let head_lines: Vec<Line> = qlines
        .into_iter()
        .map(|l| {
            Line::from(Span::styled(
                l,
                Style::default().add_modifier(Modifier::BOLD),
            ))
        })
        .collect();
    f.render_widget(Paragraph::new(Text::from(head_lines)), head);
    let rows: Vec<(usize, String)> = options
        .iter()
        .enumerate()
        .map(|(i, o)| (i, o.clone()))
        .collect();
    render_rows(f, rest, ui, &rows);
}

/// Shared list renderer (Settings / Agents): spaced rows with a subtle tinted
/// background on the selected/hovered one and an accent marker, records a hit
/// rectangle per visible row, and windows around the selection.
fn render_rows(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui, rows: &[(usize, String)]) {
    ui.rows.clear();
    ui.clamp_sel();
    let width = area.width as usize;
    let avail = width.saturating_sub(2);
    let fit = (area.height as usize / 2).max(1);
    let start = if ui.sel < fit {
        0
    } else {
        (ui.sel + 1 - fit).min(rows.len().saturating_sub(fit))
    };
    let mut lines = Vec::new();
    for (idx, label) in rows.iter().skip(start).take(fit) {
        if lines.len() >= area.height as usize {
            break;
        }
        let rect = Rect {
            x: area.x,
            y: area.y + lines.len() as u16,
            width: area.width,
            height: 1,
        };
        let selected = *idx == ui.sel;
        let hovered = hovering(ui, rect);
        let bg = if selected {
            Some(SEL_BG)
        } else if hovered {
            Some(HOVER_BG)
        } else {
            None
        };
        let marker_color = if selected { IDX_SEL } else { Color::DarkGray };
        let mut text_style = Style::default().fg(if selected { Color::White } else { Color::Gray });
        if selected {
            text_style = text_style.add_modifier(Modifier::BOLD);
        }
        let text = row_text(ui, label, avail, rect, selected);
        lines.push(bg_pad(
            vec![
                Span::styled(
                    if selected { "▸ " } else { "  " },
                    with_bg(Style::default().fg(marker_color), bg),
                ),
                Span::styled(text, with_bg(text_style, bg)),
            ],
            width,
            bg,
        ));
        ui.rows.push((*idx, rect));
        lines.push(Line::from("")); // spacing between rows
    }
    f.render_widget(Paragraph::new(Text::from(lines)), area);
}

/// Whether the mouse currently rests on `rect`.
fn hovering(ui: &Ui, rect: Rect) -> bool {
    ui.hover.is_some_and(|(c, r)| hit(rect, c, r))
}

/// Number of rows the Chat meta panel needs: the follow-up chips and a header.
///
/// References are not here — they belong to the answer above them and scroll
/// with it. Pinning them spent up to eight rows of the transcript on the least
/// urgent thing in the turn.
fn meta_height(state: &ChatState) -> u16 {
    if state.further.is_empty() {
        0
    } else {
        (1 + state.further.len() as u16).clamp(1, 6)
    }
}

/// Render clickable reference / follow-up chips and record their hit rects.
fn render_chips(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui, state: &ChatState) {
    let mut lines: Vec<Line> = Vec::new();
    let mut y = area.y;
    let bottom = area.y + area.height;
    if !state.further.is_empty() && y < bottom {
        lines.push(Line::from(Span::styled(
            format!("{}:", t!("Agent.FurtherQuestions")),
            Style::default().fg(Color::DarkGray),
        )));
        y += 1;
        for (i, q) in state.further.iter().enumerate() {
            if y >= bottom {
                break;
            }
            let rect = row_rect(area, y);
            let full = format!("  › {q}");
            let hovered = hovering(ui, rect);
            let text = row_text(ui, &full, area.width as usize, rect, false);
            lines.push(Line::from(Span::styled(
                text,
                chip_style(Color::Green, hovered),
            )));
            ui.chips.push((Chip::Further(i), rect));
            y += 1;
        }
    }
    f.render_widget(Paragraph::new(Text::from(lines)), area);
}

/// A chip's style: colored, underlined when hovered to signal it is clickable.
fn chip_style(color: Color, hovered: bool) -> Style {
    let base = Style::default().fg(color);
    if hovered {
        base.add_modifier(Modifier::UNDERLINED | Modifier::BOLD)
    } else {
        base
    }
}

fn row_rect(area: Rect, y: u16) -> Rect {
    Rect {
        x: area.x,
        y,
        width: area.width,
        height: 1,
    }
}

/// A one-line label for a reference, mirroring the CLI footer: news source and
/// description when present, otherwise the reference type/id the server sent.
fn reference_label(r: &longbridge::agent::Reference) -> String {
    let content = r.content.clone().unwrap_or(Value::Null);
    let source = content
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let desc = content
        .get("description")
        .and_then(Value::as_str)
        .or_else(|| content.get("title").and_then(Value::as_str))
        .unwrap_or_default();
    if source.is_empty() && desc.is_empty() {
        [r.ref_type.as_str(), r.id.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    } else {
        format!("{source} · {desc}")
            .trim_matches([' ', '·'])
            .to_string()
    }
}

/// The best URL for a reference, from the top-level field or its `content`.
fn reference_url(r: &longbridge::agent::Reference) -> Option<String> {
    if !r.url.is_empty() {
        return Some(r.url.clone());
    }
    r.content
        .as_ref()
        .and_then(|c| c.get("source_url").or_else(|| c.get("url")))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// The running turn's own row: what it is doing, for how long, over how many
/// tools, and how to stop it.
///
/// Cancelling used to require already knowing Esc or Ctrl+C; `[stop]` is the
/// same action with a visible affordance. The elapsed clock is the difference
/// between "this is slow" and "this is stuck".
fn render_turn_status(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui, state: &ChatState) {
    let frame = SPINNER[(ui.tick as usize) % SPINNER.len()];
    let mut spans = vec![
        Span::styled(format!("{frame} "), Style::default().fg(Color::Yellow)),
        Span::styled(state.status.clone(), Style::default().fg(Color::Yellow)),
    ];
    let tools = tools_this_turn(state);
    if tools > 0 {
        spans.push(Span::styled(
            format!("   · {}", t!("Ai.ToolCount", count = tools)),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if let Some(started) = ui.turn_started {
        let secs = started.elapsed().as_secs();
        spans.push(Span::styled(
            format!("   {}:{:02}", secs / 60, secs % 60),
            Style::default().fg(Color::DarkGray),
        ));
    }
    // The button is right-aligned, so its rect is derived from the label width.
    let label = format!("[{}]", t!("Ai.Stop"));
    let label_w = UnicodeWidthStr::width(label.as_str()) as u16;
    let used: u16 = spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()) as u16)
        .sum();
    let rect = (area.width > used + label_w + 1).then(|| Rect {
        x: area.x + area.width - label_w,
        y: area.y,
        width: label_w,
        height: 1,
    });
    ui.stop_button = rect;
    if let Some(rect) = rect {
        let gap = area.width - used - label_w;
        spans.push(Span::raw(" ".repeat(gap as usize)));
        let hot = hovering(ui, rect);
        spans.push(Span::styled(
            label,
            Style::default().fg(if hot { Color::Red } else { Color::DarkGray }),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Tool calls made since the user's last prompt.
fn tools_this_turn(state: &ChatState) -> usize {
    state
        .messages
        .iter()
        .rev()
        .take_while(|m| m.role != Role::User)
        .filter(|m| m.role == Role::Tool)
        .count()
}

fn render_status(f: &mut ratatui::Frame, area: Rect, ui: &Ui, state: &ChatState) {
    let (text, style) = if ui.view == View::Chat && state.scroll > 0 {
        // While scrolled up, tell the user how to get back to the latest.
        (
            t!("Ai.ScrolledHint").to_string(),
            Style::default().fg(Color::Yellow),
        )
    } else if let Some(notice) = &ui.notice {
        (notice.clone(), Style::default().fg(Color::Green))
    } else {
        let hint = match ui.view {
            View::Chat => t!("Ai.InputHint"),
            View::Sessions => t!("Ai.SessionsHint"),
            View::Settings => t!("Ai.SettingsHint"),
            View::Question => t!("Ai.QuestionHint"),
        };
        (hint.to_string(), Style::default().fg(Color::DarkGray))
    };
    f.render_widget(Paragraph::new(Line::from(Span::styled(text, style))), area);
}

fn render_footer(f: &mut ratatui::Frame, area: Rect, ui: &Ui, editor: &Editor) {
    let focused = ui.view == View::Chat;
    // The box stays — it is what separates the prompt from the transcript — but
    // dim. A rounded cyan frame was the loudest thing on the screen.
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let marker_style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    // Inside the box, the `❯` the reader's own turns carry in the transcript, so
    // the line being typed looks like the line it will become.
    let [marker, body] =
        Layout::horizontal([Constraint::Length(MARKER_W), Constraint::Min(0)]).areas(inner);
    if !focused {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(USER_MARKER, marker_style))),
            marker,
        );
        return;
    }
    let mut lines: Vec<Line> = Vec::new();
    if editor.is_blank() {
        // Dim placeholder when nothing has been typed yet.
        lines.push(Line::from(Span::styled(
            t!("Ai.Placeholder").to_string(),
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.extend(editor.lines().iter().map(|l| Line::from(l.clone())));
    }
    // The marker leads the first row; the rest are blank, so a multi-line prompt
    // reads as one block indented under it.
    f.render_widget(
        Paragraph::new(Text::from(
            (0..lines.len().max(1))
                .map(|i| {
                    if i == 0 {
                        Line::from(Span::styled(USER_MARKER, marker_style))
                    } else {
                        Line::from("")
                    }
                })
                .collect::<Vec<_>>(),
        )),
        marker,
    );
    f.render_widget(Paragraph::new(Text::from(lines)), body);
    let (cy, col) = editor.cursor();
    let cy = (cy as u16).min(body.height.saturating_sub(1));
    let col = (col as u16).min(body.width.saturating_sub(1));
    f.set_cursor_position((body.x + col, body.y + cy));
}

/// A cheap signature of the committed transcript: message count, width, and a
/// hash of the last message's text. It changes on any append, reset, or
/// restore (even one that keeps the count), invalidating the render cache
/// without hashing the whole transcript each frame.
fn transcript_sig(state: &ChatState, width: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    state.messages.len().hash(&mut h);
    width.hash(&mut h);
    if let Some(last) = state.messages.last() {
        last.text.hash(&mut h);
    }
    h.finish()
}

/// One row naming a tool the agent called and how the call went.
///
/// The point is traceability: an answer about a stock should show that it
/// actually read a quote. A pending call is dim, a finished one takes the
/// accent, and a failure is red and says so — a silently failed tool used to be
/// invisible unless the whole turn came back empty.
fn tool_line(name: &str, status: ToolStatus, width: usize) -> Line<'static> {
    let (marker, color) = match status {
        ToolStatus::Running => ("◌", Color::DarkGray),
        ToolStatus::Ok => ("⏺", Color::Green),
        ToolStatus::Failed => ("⚠", Color::Red),
    };
    let mut spans = vec![
        Span::styled(format!("  {marker} "), Style::default().fg(color)),
        Span::styled(
            truncate_width(name, width.saturating_sub(6)),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    if status == ToolStatus::Failed {
        spans.push(Span::styled(
            format!("  {}", t!("Ai.ToolFailed")),
            Style::default().fg(Color::Red),
        ));
    }
    Line::from(spans)
}

fn push_message(
    lines: &mut Vec<Line<'static>>,
    message: &Message,
    width: usize,
    quotes: &HashMap<String, super::quotes::QuoteCardData>,
    aliases: &HashMap<String, String>,
) {
    // A tool line is one compact row, not a speaker turn: it belongs to the
    // answer around it, so it gets no accent bar and no trailing blank.
    if message.role == Role::Tool {
        if let Some(status) = message.tool {
            // A failure changes how much to trust the answer above it; a success
            // rarely does, so the reader can keep only the failures — or nothing.
            let show = match super::settings::tool_calls() {
                super::settings::ToolCalls::All => true,
                super::settings::ToolCalls::Failures => status == ToolStatus::Failed,
                super::settings::ToolCalls::Off => false,
            };
            if show {
                lines.push(tool_line(&message.text, status, width));
            }
        }
        return;
    }
    match message.role {
        // The answer is the page. Labelling it says nothing the reader does not
        // already know and costs a row per turn, so it goes in unannounced —
        // only the reader's own turns are marked, which is what makes a long
        // transcript scannable.
        Role::Assistant => {
            lines.extend(render_answer_lines(&message.text, width, quotes, aliases));
        }
        Role::User => lines.extend(user_lines(&message.text, width)),
        Role::System | Role::Tool => {
            for logical in message.text.split('\n') {
                for wrapped in wrap(logical, width) {
                    lines.push(Line::from(Span::styled(
                        wrapped,
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
        }
    }
    lines.push(Line::from(""));
}

/// The reader's own message: a quote marker and a band across the transcript.
///
/// Wrapped rows are indented under the marker so the block reads as one
/// quotation, and the band runs the full width — stopping at the last glyph
/// makes it read as a highlight on the text rather than as the reader's turn.
fn user_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    let indent = usize::from(MARKER_W);
    let body_w = width.saturating_sub(indent).max(1);
    let band = Style::default().fg(USER_FG).bg(USER_BG);
    let mut out = Vec::new();
    for logical in text.split('\n') {
        for wrapped in wrap(logical, body_w) {
            let lead = if out.is_empty() {
                USER_MARKER.to_string()
            } else {
                " ".repeat(indent)
            };
            let pad = width.saturating_sub(indent + UnicodeWidthStr::width(wrapped.as_str()));
            out.push(Line::from(vec![
                Span::styled(lead, band.fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{wrapped}{}", " ".repeat(pad)), band),
            ]));
        }
    }
    out
}

/// The mark on the reader's own lines, in the transcript and at the prompt: the
/// line being typed should look like the line it will become.
const USER_MARKER: &str = "❯ ";
const MARKER_W: u16 = 2;

/// Split out the securities named in `lines` so each is its own span, and colour
/// the price chip that follows one.
///
/// A symbol has to be a span of its own before it can be given a colour, a hover
/// or a hit rectangle, and Markdown gives us one span per style run — `看 700.HK
/// 的走势` arrives whole. The original style is kept and only the colour changes,
/// so a symbol inside a heading stays bold.
///
/// The chip was inserted as plain text before wrapping (see [`price_annotated`]),
/// so it is recognised here by matching what that would have produced. When the
/// wrap happened to break between a symbol and its chip the chip stays plain —
/// the information is still right, it just loses its colour.
fn link_symbols(
    lines: &mut Vec<Line<'static>>,
    quotes: &HashMap<String, super::quotes::QuoteCardData>,
    aliases: &HashMap<String, String>,
) {
    for line in lines.iter_mut() {
        if !line
            .spans
            .iter()
            .any(|s| !super::answer::security_spans(&s.content, aliases).is_empty())
        {
            continue;
        }
        let mut out: Vec<Span<'static>> = Vec::new();
        for span in line.spans.drain(..) {
            let ranges = super::answer::security_spans(&span.content, aliases);
            if ranges.is_empty() {
                out.push(span);
                continue;
            }
            let text = span.content.to_string();
            let mut at = 0usize;
            for (range, symbol) in ranges {
                if range.start > at {
                    out.push(Span::styled(text[at..range.start].to_string(), span.style));
                }
                // Left in the prose's own style. A colour on the symbol fought
                // with the price beside it — two tints on one short run reads as
                // noise — so the hover underline is the affordance instead.
                out.push(Span::styled(text[range.clone()].to_string(), span.style));
                at = range.end;
                if let Some(card) = quotes.get(&symbol) {
                    let chip = price_chip(card);
                    if text[at..].starts_with(&chip) {
                        out.push(Span::styled(
                            chip.clone(),
                            span.style.fg(change_color(card.direction)),
                        ));
                        at += chip.len();
                    }
                }
            }
            if at < text.len() {
                out.push(Span::styled(text[at..].to_string(), span.style));
            }
        }
        line.spans = out;
    }
}

/// The price chip that follows a security in the prose.
///
/// Built in one place because it is inserted into the text before wrapping and
/// recognised again after it, and the two have to agree exactly.
fn price_chip(card: &super::quotes::QuoteCardData) -> String {
    // The arrow carries the direction, so the percent drops its sign rather than
    // saying it twice.
    // Any sign the server sent is dropped rather than only the matching one: a
    // percent whose sign disagreed with the direction rendered as `▼+1.28%`.
    let (arrow, pct) = match card.direction {
        1 => ("▲", card.change_pct.trim_start_matches(['+', '-'])),
        -1 => ("▼", card.change_pct.trim_start_matches(['+', '-'])),
        _ => ("", card.change_pct.as_str()),
    };
    format!(" {} {arrow}{pct}", card.last)
}

/// Insert each security's price into the answer text, right after the symbol.
///
/// Done before the Markdown is wrapped, not after: a chip appended to a finished
/// line either overruns the width or gets dropped, and prose wraps to nearly the
/// full width, so in practice it was always dropped. Inserted here it takes part
/// in the wrapping and always fits.
///
/// Fenced code and table rows are left alone — code is quoted verbatim, and a
/// table's columns are measured, so a chip would break its alignment.
fn price_annotated(
    text: &str,
    quotes: &HashMap<String, super::quotes::QuoteCardData>,
    aliases: &HashMap<String, String>,
) -> String {
    if quotes.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut fenced = false;
    // A ticker is mentioned repeatedly in one answer; the price belongs beside the
    // first mention. Repeating it on every one reads as noise, and every later
    // mention is still clickable.
    let mut priced: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            out.push_str(line);
            continue;
        }
        if fenced || line.trim_start().starts_with('|') {
            out.push_str(line);
            continue;
        }
        let mut at = 0usize;
        for (range, symbol) in super::answer::security_spans(line, aliases) {
            out.push_str(&line[at..range.end]);
            at = range.end;
            if let Some(card) = quotes.get(&symbol) {
                if priced.insert(symbol) {
                    out.push_str(&price_chip(card));
                }
            }
        }
        out.push_str(&line[at..]);
    }
    out
}

/// Record a hit rectangle for every security visible in `window`, underlining the
/// one under the pointer.
///
/// Resolved per frame against what is actually on screen, so scrolling cannot
/// leave a stale target behind — the same reason the reference rows are recorded
/// during the render rather than cached.
fn link_visible_symbols(window: &mut [Line<'static>], area: Rect, ui: &mut Ui) {
    // A bare ticker's click target is the symbol it resolved to, not the four
    // letters on screen. Copied out first because the chips are pushed onto `ui`
    // in the same pass; the map holds a handful of short strings.
    let aliases = ui.aliases.clone();
    let resolve = |text: &str| -> Option<String> {
        if super::answer::is_symbol(text) {
            Some(text.to_string())
        } else {
            aliases.get(text).cloned()
        }
    };
    // A card is a block, and the reader aims at the block rather than at the six
    // columns of its ticker. Its rows are recognised by the box this module drew,
    // so anywhere on the card opens the panel.
    let mut card: Option<(String, u16)> = None;
    for (i, line) in window.iter_mut().enumerate() {
        let y = area.y + i as u16;
        let mut x = area.x;
        let mut symbol_here: Option<String> = None;
        for span in &mut line.spans {
            let w = UnicodeWidthStr::width(span.content.as_ref()) as u16;
            // A span that is exactly a security is one `link_symbols` split out:
            // prose never arrives as a lone ticker. Identified by its text rather
            // than by a marker colour, now that the symbol carries none.
            let target = resolve(&span.content);
            if let Some(symbol) = target {
                let rect = Rect {
                    x,
                    y,
                    width: w,
                    height: 1,
                };
                if hovering(ui, rect) {
                    span.style = span.style.add_modifier(Modifier::UNDERLINED);
                }
                ui.chips.push((Chip::Symbol(symbol.clone()), rect));
                symbol_here = Some(symbol);
            }
            x = x.saturating_add(w);
        }
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        if text.contains('┌') {
            card = Some((String::new(), y));
        } else if text.contains('└') {
            if let Some((symbol, top)) = card.take() {
                if !symbol.is_empty() {
                    ui.chips.push((
                        Chip::Symbol(symbol),
                        Rect {
                            x: area.x,
                            y: top,
                            width: area.width,
                            height: y + 1 - top,
                        },
                    ));
                }
            }
        } else if let (Some(entry), Some(symbol)) = (card.as_mut(), symbol_here) {
            if entry.0.is_empty() {
                entry.0 = symbol;
            }
        }
    }
}

/// Render an assistant answer the way `agent chat` does: split into text /
/// chart / widget segments, then render Markdown text, draw `vis-chart` blocks
/// as charts, and reduce `x-widget` tags to a compact reference instead of
/// dumping raw JSON/markup into the transcript.
fn render_answer_lines(
    answer: &str,
    width: usize,
    quotes: &HashMap<String, super::quotes::QuoteCardData>,
    aliases: &HashMap<String, String>,
) -> Vec<Line<'static>> {
    use super::answer::{replace_inline_markers, segment_answer, Segment};
    let mut out: Vec<Line<'static>> = Vec::new();
    // A drawn block — a chart, a card, a reference — needs air around it or it
    // reads as another paragraph of the prose it interrupts. Only one blank row
    // either side: the block already carries its own shape.
    let breathe = |out: &mut Vec<Line<'static>>| {
        let blank = |l: &Line| l.spans.iter().all(|s| s.content.trim().is_empty());
        if !out.is_empty() && !out.last().is_some_and(blank) {
            out.push(Line::from(""));
        }
    };
    for segment in segment_answer(answer) {
        match segment {
            Segment::Text(text) => {
                let text = replace_inline_markers(&text, false);
                let text = price_annotated(&text, quotes, aliases);
                out.extend(markdown::render(&text, width));
            }
            // Straight from the chart renderer, which already styles each part
            // (line, volume, axes) and sizes itself to `width`. Going through
            // the CLI's ANSI form only to repaint every row one flat color threw
            // that away.
            Segment::VisChart(spec) => {
                breathe(&mut out);
                out.extend(super::chart::render(&spec, width));
                out.push(Line::from(""));
            }
            Segment::XWidget(src) => {
                breathe(&mut out);
                out.extend(render_widget(&src, width, quotes));
                out.push(Line::from(""));
            }
        }
    }
    link_symbols(&mut out, quotes, aliases);
    // A block at the very end leaves a trailing blank the turn separator would
    // double.
    while out
        .last()
        .is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty()) && out.len() > 1)
    {
        out.pop();
    }
    out
}

/// Render a widget reference as what it is about.
///
/// The web client draws a chart here. A terminal cannot, but the CLI already
/// speaks to the quote API, so the honest terminal equivalent is the live quote
/// behind the reference: a card for one security, an aligned row per security for
/// a comparison or a list. Anything else is named, never shown as a URL.
fn render_widget(
    src: &str,
    width: usize,
    quotes: &HashMap<String, super::quotes::QuoteCardData>,
) -> Vec<Line<'static>> {
    use super::answer::{parse_widget, WidgetRef};

    let Some(widget) = parse_widget(src) else {
        // Not a widget URL at all; show the text rather than dropping it.
        return vec![Line::from(Span::styled(
            strip_control_chars(src),
            Style::default().fg(Color::DarkGray),
        ))];
    };
    match &widget {
        WidgetRef::Quote { symbol } => match quotes.get(symbol) {
            Some(card) => quote_card(card, width),
            None => vec![pending_ref(symbol)],
        },
        WidgetRef::Comparison { symbols } | WidgetRef::StockList { symbols } => {
            let header = if matches!(widget, WidgetRef::Comparison { .. }) {
                t!("Ai.WidgetComparison")
            } else {
                t!("Ai.WidgetStockList")
            };
            let mut out = vec![Line::from(Span::styled(
                format!("  {header}"),
                Style::default().fg(Color::DarkGray),
            ))];
            // One row per security, columns aligned so the set can be compared
            // down the page — which is the whole point of the widget.
            let name_w = symbols
                .iter()
                .map(|s| UnicodeWidthStr::width(s.as_str()))
                .max()
                .unwrap_or(0);
            for symbol in symbols {
                out.push(match quotes.get(symbol) {
                    Some(card) => quote_row(card, name_w),
                    None => pending_ref(symbol),
                });
            }
            out
        }
        // The CTA kinds are a closed set, so each gets a real label; an
        // unrecognized one names itself rather than showing an i18n key.
        WidgetRef::Cta { action } => {
            let label = match action.as_str() {
                "open_account" => t!("Ai.WidgetCta.open_account").to_string(),
                "fund_account" => t!("Ai.WidgetCta.fund_account").to_string(),
                "complete_profile" => t!("Ai.WidgetCta.complete_profile").to_string(),
                other => other.replace('_', " "),
            };
            vec![Line::from(Span::styled(
                format!("  → {label}"),
                Style::default().fg(Color::Blue),
            ))]
        }
        // An order ticket is actionable in the app and only readable here, so it
        // is read back rather than named: what the agent proposed is the most
        // consequential thing it can put in an answer.
        WidgetRef::OrderTicket(ticket) => {
            let mut parts = vec![ticket.side.clone(), ticket.quantity.clone()];
            parts.push(ticket.symbol.clone());
            parts.push(ticket.order_type.clone());
            if !ticket.price.is_empty() {
                parts.push(format!("@ {}", ticket.price));
            }
            let summary = parts
                .into_iter()
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            vec![Line::from(vec![
                Span::styled(
                    format!("  {}  ", t!("Ai.WidgetOrderTicket")),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(summary, Style::default().add_modifier(Modifier::BOLD)),
            ])]
        }
        WidgetRef::OrderDetail { order_id } => vec![Line::from(vec![
            Span::styled(
                format!("  {}  ", t!("Ai.WidgetOrderDetail")),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw(order_id.clone()),
        ])],
        WidgetRef::Other { path } => vec![Line::from(Span::styled(
            format!("  → {path}"),
            Style::default().fg(Color::DarkGray),
        ))],
    }
}

/// A reference whose quote has not arrived (or will not): name the security.
fn pending_ref(symbol: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("  → {symbol}"),
        Style::default().fg(Color::Blue),
    ))
}

/// The color a change should be tinted, honoring the up/down color preference.
/// The colour of a price change, honouring the terminal's up/down convention.
///
/// A reader who set red-up in the market view was seeing the opposite here,
/// which for a price is not a cosmetic difference.
fn change_color(direction: i8) -> Color {
    match crate::tui::ui::styles::up_color(direction.cmp(&0)) {
        // `up_color` returns Reset for no change; the transcript wants it dim.
        Color::Reset => Color::Gray,
        c => c,
    }
}

/// A boxed card for one security: price and change, then the day's range and
/// activity — what a trader reads before deciding the reference was worth
/// following.
/// The panel's body: one field per row, unboxed — the panel's own frame is the
/// border, and a dedicated panel has the room the inline card does not.
fn card_lines(card: &super::quotes::QuoteCardData, width: usize) -> Vec<Line<'static>> {
    let dim = Style::default().fg(Color::DarkGray);
    let dir = change_color(card.direction);
    let mut out = Vec::new();
    if !card.name.is_empty() {
        out.push(Line::from(Span::styled(
            truncate_width(&card.name, width),
            Style::default().fg(Color::Gray),
        )));
        out.push(Line::from(""));
    }
    // The price is what the panel is for, so it gets the room: last on its own,
    // the change beside it in the direction's colour.
    out.push(Line::from(vec![
        Span::styled(
            card.last.clone(),
            Style::default().fg(dir).add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled(
            format!("{}  {}", card.change, card.change_pct),
            Style::default().fg(dir),
        ),
    ]));
    out.push(Line::from(""));
    // Two labelled columns per row: a panel is read down, not across.
    let col = width / 2;
    let pair = |a: (&str, &str), b: (&str, &str)| {
        let left = format!("{:<6}{}", a.0, a.1);
        Line::from(vec![
            Span::styled(format!("{:<6}", a.0), dim),
            Span::raw(format!(
                "{}{}",
                a.1,
                " ".repeat(col.saturating_sub(UnicodeWidthStr::width(left.as_str())))
            )),
            Span::styled(format!("{:<6}", b.0), dim),
            Span::raw(b.1.to_string()),
        ])
    };
    out.push(pair(
        (&t!("Ai.QuoteOpen"), &card.open),
        (&t!("Ai.QuoteHigh"), &card.high),
    ));
    out.push(pair(
        (&t!("Ai.QuoteLow"), &card.low),
        (&t!("Ai.QuotePrevClose"), &prev_close(card)),
    ));
    out.push(pair(
        (&t!("Ai.QuoteVolume"), &card.volume),
        (&t!("Ai.QuoteTurnover"), &card.turnover),
    ));
    out.push(Line::from(""));
    out.push(Line::from(Span::styled(
        format!("{} {}", t!("Ai.QuoteAt"), card.at),
        dim,
    )));
    out
}

/// The previous close, formatted like the other prices.
fn prev_close(card: &super::quotes::QuoteCardData) -> String {
    card.prev_close.round_dp(3).normalize().to_string()
}

fn quote_card(card: &super::quotes::QuoteCardData, width: usize) -> Vec<Line<'static>> {
    let dir = change_color(card.direction);
    let head = if card.name.is_empty() {
        card.symbol.clone()
    } else {
        format!("{}  {}", card.symbol, card.name)
    };
    let price = format!("{}  {}  {}", card.last, card.change, card.change_pct);
    let range = format!(
        "{} {}   {} {}   {} {}",
        t!("Ai.QuoteOpen"),
        card.open,
        t!("Ai.QuoteHigh"),
        card.high,
        t!("Ai.QuoteLow"),
        card.low
    );
    let flow = format!(
        "{} {}   {} {}   {}",
        t!("Ai.QuoteVolume"),
        card.volume,
        t!("Ai.QuoteTurnover"),
        card.turnover,
        card.at
    );
    // The frame costs 4 columns: a bar and a space on each side. No indent — the
    // card is a block like a table, and the two lined up against nothing.
    let budget = width.saturating_sub(4);
    let inner = [&head, &price, &range, &flow]
        .into_iter()
        .map(|s| UnicodeWidthStr::width(s.as_str()))
        .max()
        .unwrap_or(0)
        .min(budget);
    // Content is clipped to the box, not just measured against it — a long name
    // or a wide CJK label would otherwise push the right border off the line.
    let head = truncate_width(&head, inner);
    let price = truncate_width(&price, inner);
    let range = truncate_width(&range, inner);
    let flow = truncate_width(&flow, inner);
    let border = |left: &str, right: &str| {
        Line::from(Span::styled(
            format!("{left}{}{right}", "─".repeat(inner + 2)),
            Style::default().fg(Color::DarkGray),
        ))
    };
    let bar = || Span::styled("│", Style::default().fg(Color::DarkGray));
    let row = |spans: Vec<Span<'static>>, used: usize| {
        let mut all = vec![bar(), Span::raw(" ")];
        all.extend(spans);
        all.push(Span::raw(" ".repeat(inner.saturating_sub(used) + 1)));
        all.push(bar());
        Line::from(all)
    };
    vec![
        border("┌", "┐"),
        row(
            vec![Span::styled(
                head.clone(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )],
            UnicodeWidthStr::width(head.as_str()),
        ),
        row(
            vec![
                Span::styled(
                    format!("{}  ", card.last),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{}  {}", card.change, card.change_pct),
                    Style::default().fg(dir),
                ),
            ],
            UnicodeWidthStr::width(price.as_str()),
        ),
        row(
            vec![Span::styled(
                range.clone(),
                Style::default().fg(Color::DarkGray),
            )],
            UnicodeWidthStr::width(range.as_str()),
        ),
        row(
            vec![Span::styled(
                flow.clone(),
                Style::default().fg(Color::DarkGray),
            )],
            UnicodeWidthStr::width(flow.as_str()),
        ),
        border("└", "┘"),
    ]
}

/// One security as a single aligned row, for a comparison or a list.
fn quote_row(card: &super::quotes::QuoteCardData, symbol_w: usize) -> Line<'static> {
    let pad = symbol_w.saturating_sub(UnicodeWidthStr::width(card.symbol.as_str()));
    let mut spans = vec![
        Span::styled(
            format!("{}{}", card.symbol, " ".repeat(pad)),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {:>10}", card.last),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {:>9}", card.change_pct),
            Style::default().fg(change_color(card.direction)),
        ),
    ];
    if !card.name.is_empty() {
        spans.push(Span::styled(
            format!("  {}", card.name),
            Style::default().fg(Color::DarkGray),
        ));
    }
    Line::from(spans)
}

/// Wrap `s` to `width` display columns, honoring wide (CJK) glyphs.
fn wrap(s: &str, width: usize) -> Vec<String> {
    if width == 0 || s.is_empty() {
        return vec![s.to_string()];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if w + cw > width && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            w = 0;
        }
        cur.push(ch);
        w += cw;
    }
    out.push(cur);
    out
}

#[cfg(test)]
mod tests {
    use super::marquee;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn fits_within_width_is_unchanged() {
        assert_eq!(marquee("abc", 10, 7), "abc");
    }

    #[test]
    fn scrolls_by_one_step_and_stays_within_width() {
        let (text, width) = ("abcdefgh", 4);
        let a = marquee(text, width, 0);
        let b = marquee(text, width, 1);
        assert_eq!(a, "abcd");
        assert_ne!(a, b, "advancing the tick should shift the window");
        assert!(b.width() <= width);
    }

    #[test]
    fn window_wraps_around_through_the_gap() {
        // A tick past the end wraps back to the start, so scrolling loops.
        let text = "abcd";
        let n = text.chars().count() + 3; // trailing gap
        assert_eq!(marquee(text, 2, 0), marquee(text, 2, n as u64));
    }

    #[test]
    fn relative_time_buckets_by_unit() {
        use super::relative_time;
        assert_eq!(relative_time(0, 120), "2m");
        assert_eq!(relative_time(0, 7200), "2h");
        assert_eq!(relative_time(0, 172_800), "2d");
    }

    #[test]
    fn select_line_extracts_column_range() {
        use ratatui::text::Line;
        let line = Line::from("hello world");
        let (_, text) = super::select_line(&line, 6, 11);
        assert_eq!(text, "world");
    }

    #[test]
    fn select_line_counts_wide_glyphs_by_column() {
        use ratatui::text::Line;
        // "你好" spans columns 0..4; selecting [2,4) yields the second glyph.
        let line = Line::from("你好");
        let (_, text) = super::select_line(&line, 2, 4);
        assert_eq!(text, "好");
    }

    /// Every canonical name and alias must resolve, or the command silently
    /// becomes a prompt sent to the model (which is how `/quit` once behaved).
    #[test]
    fn every_name_and_alias_resolves() {
        use super::{slash_lookup, SLASH};
        for cmd in &SLASH {
            assert_eq!(slash_lookup(cmd.name), Some(cmd.key()), "{}", cmd.name);
            for alias in cmd.aliases {
                assert_eq!(
                    slash_lookup(alias),
                    Some(cmd.key()),
                    "alias {alias} must dispatch to {}",
                    cmd.name
                );
            }
        }
        assert_eq!(slash_lookup("/nope"), None);
        // Both are advertised in `/help`, so both have to reach a command.
        assert_eq!(slash_lookup("/quit"), Some("exit"));
        assert_eq!(slash_lookup("/clear"), Some("new"));
    }

    #[test]
    fn split_command_separates_name_from_argument() {
        use super::split_command;
        assert_eq!(split_command("/agent"), ("/agent", ""));
        assert_eq!(split_command("/agent  my-bot "), ("/agent", "my-bot"));
        assert_eq!(split_command("/agent reset"), ("/agent", "reset"));
    }

    /// The palette must let go of the input once an argument is being typed,
    /// otherwise Enter re-runs the bare command and the argument is dropped.
    #[test]
    fn palette_closes_once_an_argument_is_typed() {
        use super::{slash_active, Editor};
        let mut e = Editor::new();
        e.set_text("/agent");
        assert!(slash_active(&e));
        // Tab-completion leaves a trailing space; the name is still bare.
        e.set_text("/agent ");
        assert!(slash_active(&e));
        e.set_text("/agent my-bot");
        assert!(!slash_active(&e));
    }

    /// A prefix that only matches an alias still surfaces its command, so the
    /// dropdown is never empty while a valid command is being typed. `/qu` is
    /// ambiguous — `/quote` by name and `/exit` by its `/quit` alias — and both
    /// have to be offered; typing either in full still dispatches exactly.
    #[test]
    fn palette_matches_aliases() {
        use super::{slash_lookup, slash_matches, Editor, SLASH};
        let mut e = Editor::new();
        e.set_text("/qu");
        let names: Vec<&str> = slash_matches(&e)
            .into_iter()
            .map(|i| SLASH[i].name)
            .collect();
        assert!(names.contains(&"/exit"), "the alias surfaces: {names:?}");
        assert!(names.contains(&"/quote"), "and the name: {names:?}");
        assert_eq!(slash_lookup("/quit"), Some("exit"));
        assert_eq!(slash_lookup("/quote"), Some("quote"));
    }

    /// Render the whole view into an in-memory backend and return its text rows.
    fn frame(ui: &mut super::Ui, state: &super::ChatState, w: u16, h: u16) -> Vec<String> {
        let backend = ratatui::backend::TestBackend::new(w, h);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        let editor = super::Editor::new();
        terminal
            .draw(|f| super::view(f, ui, state, &editor))
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    fn busy_state() -> super::ChatState {
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        state.apply(super::ChatEvent::UserPrompt("what is NVDA doing?".into()));
        state.apply(super::ChatEvent::ToolStarted("Get Quote".into()));
        state.apply(super::ChatEvent::Status("Calling Get Quote".into()));
        state
    }

    /// A `/copy` confirmation used to take over the one status row and hide the
    /// spinner, so a running turn looked finished.
    #[test]
    fn a_notice_does_not_hide_the_running_turn() {
        let mut ui = super::Ui::new();
        ui.notice = Some("Copied to clipboard.".into());
        let rows = frame(&mut ui, &busy_state(), 70, 16);
        let screen = rows.join("\n");
        assert!(
            screen.contains("Copied to clipboard."),
            "the notice should still show:\n{screen}"
        );
        assert!(
            super::SPINNER.iter().any(|f| screen.contains(f)),
            "the spinner must survive alongside it:\n{screen}"
        );
        assert!(
            screen.contains("Calling Get Quote"),
            "the turn's status text should show:\n{screen}"
        );
    }

    /// The turn row carries a cancel affordance, and it is only there while a
    /// turn is running — idle chrome stays one row.
    #[test]
    fn the_turn_row_exists_only_while_busy() {
        let mut ui = super::Ui::new();
        let busy = frame(&mut ui, &busy_state(), 70, 16);
        assert!(
            busy.iter().any(|l| l.contains("[stop]")),
            "a running turn should offer [stop]:\n{}",
            busy.join("\n")
        );
        assert!(ui.stop_button.is_some(), "the button needs a hit rect");

        let idle = super::ChatState::new("chatbot".into(), "welcome".into());
        let mut ui = super::Ui::new();
        let rows = frame(&mut ui, &idle, 70, 16);
        assert!(
            !rows.iter().any(|l| l.contains("[stop]")),
            "no turn, no stop button:\n{}",
            rows.join("\n")
        );
        assert!(ui.stop_button.is_none());
    }

    /// The count is per turn, not per conversation.
    #[test]
    fn tool_count_covers_only_the_current_turn() {
        use super::{ChatEvent, ChatState};
        let mut state = ChatState::new("chatbot".into(), "welcome".into());
        state.apply(ChatEvent::UserPrompt("first".into()));
        state.apply(ChatEvent::ToolStarted("A".into()));
        state.apply(ChatEvent::TurnFinished { error: None });
        assert_eq!(super::tools_this_turn(&state), 1);

        state.apply(ChatEvent::UserPrompt("second".into()));
        assert_eq!(
            super::tools_this_turn(&state),
            0,
            "a new turn starts at zero"
        );
        state.apply(ChatEvent::ToolStarted("B".into()));
        state.apply(ChatEvent::ToolStarted("C".into()));
        assert_eq!(super::tools_this_turn(&state), 2);
    }

    /// A `MacBook`'s built-in keyboard has no `PageUp`/`PageDown`/`Home`/`End` —
    /// they need Fn — so every action bound to one must also be reachable
    /// another way.
    #[test]
    fn scrolling_and_jumping_work_without_fn_keys() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let shift = |code| KeyEvent::new(code, KeyModifiers::SHIFT);

        // Chat: Shift+arrows scroll the transcript, like PageUp/PageDown.
        let mut ui = super::Ui::new();
        ui.max_scroll = 40;
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        let mut editor = super::Editor::new();
        let mut turn = None;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        super::on_chat_key(
            shift(KeyCode::Up),
            &mut ui,
            &mut state,
            &mut editor,
            &mut turn,
            &tx,
        );
        assert!(state.scroll > 0, "Shift+Up should scroll back");
        let scrolled = state.scroll;
        super::on_chat_key(
            shift(KeyCode::Down),
            &mut ui,
            &mut state,
            &mut editor,
            &mut turn,
            &tx,
        );
        assert!(state.scroll < scrolled, "Shift+Down should scroll forward");

        // Lists: Shift+arrows jump to the ends, like Home/End.
        for handler in [
            super::on_list_key as fn(KeyEvent, &mut super::Ui, &mut super::ChatState),
            super::on_sessions_key as fn(KeyEvent, &mut super::Ui, &mut super::ChatState),
        ] {
            let mut ui = super::Ui::new();
            ui.view = super::View::Settings;
            let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
            handler(shift(KeyCode::Down), &mut ui, &mut state);
            assert_eq!(
                ui.sel,
                ui.row_count().saturating_sub(1),
                "Shift+Down should reach the last row"
            );
            handler(shift(KeyCode::Up), &mut ui, &mut state);
            assert_eq!(ui.sel, 0, "Shift+Up should reach the first row");
        }
    }

    /// The reader's own turns get a band, and it reaches the full width — a
    /// background that stops at the last glyph reads as a highlight, not a block.
    #[test]
    fn user_messages_get_a_full_width_band() {
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        state.apply(super::ChatEvent::UserPrompt("short".into()));
        let mut lines = Vec::new();
        let msg = state
            .messages
            .iter()
            .find(|m| m.role == super::Role::User)
            .expect("a user message");
        super::push_message(
            &mut lines,
            msg,
            40,
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
        );
        let banded: Vec<&ratatui::text::Line> = lines
            .iter()
            .filter(|l| l.spans.iter().any(|s| s.style.bg == Some(super::USER_BG)))
            .collect();
        assert!(!banded.is_empty(), "the user's text should be banded");
        for line in banded {
            let w: usize = line
                .spans
                .iter()
                .filter(|s| s.style.bg == Some(super::USER_BG))
                .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            assert_eq!(w, 40, "the band should span the width");
        }
    }

    /// The answer is the page: labelling it costs a row per turn and says nothing
    /// the reader does not know. Only the reader's own turns are marked, with a
    /// quote marker and the band.
    #[test]
    fn only_the_readers_turns_are_marked() {
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        state.apply(super::ChatEvent::UserPrompt("查一下我的关注列表".into()));
        state.apply(super::ChatEvent::Delta(
            "Your watchlist has 12 names.".into(),
        ));
        state.apply(super::ChatEvent::TurnFinished { error: None });
        let mut lines = Vec::new();
        for m in &state.messages {
            super::push_message(
                &mut lines,
                m,
                40,
                &std::collections::HashMap::new(),
                &std::collections::HashMap::new(),
            );
        }
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        for label in [t!("Ai.You"), t!("Ai.Assistant")] {
            assert!(
                !text.iter().any(|l| l.trim() == label.as_ref()),
                "speaker label {label:?} leaked into the transcript: {text:?}"
            );
        }
        assert!(
            !text.iter().any(|l| l.contains('▌')),
            "the old speaker accent bar is gone: {text:?}"
        );
        assert!(
            text.iter().any(|l| l.starts_with("❯ 查一下")),
            "the reader's turn should carry the quote marker: {text:?}"
        );
        assert!(
            text.iter().any(|l| l.contains("watchlist")),
            "the answer should still be there: {text:?}"
        );
    }

    /// A reference belongs to the answer above it, so it scrolls with the
    /// transcript rather than pinning rows to the bottom.
    #[test]
    fn references_scroll_with_the_transcript() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        state.apply(super::ChatEvent::UserPrompt("news?".into()));
        state.apply(super::ChatEvent::Delta("Here is the news.".into()));
        state.apply(super::ChatEvent::TurnFinished { error: None });
        state.references = vec![longbridge::agent::Reference {
            index: 1,
            original_index: 1,
            ref_type: "NewsArticle".into(),
            id: "n1".into(),
            title: "A market note".into(),
            url: "https://example.com/n1".into(),
            content: None,
        }];
        let rows = frame(&mut ui, &state, 70, 20);
        let refs_at = rows
            .iter()
            .position(|r| r.contains(t!("Agent.References").as_ref()))
            .expect("the references header should be in the transcript");
        let answer_at = rows
            .iter()
            .position(|r| r.contains("Here is the news"))
            .expect("the answer should be there");
        assert!(
            refs_at > answer_at,
            "references follow their answer instead of being pinned: {rows:?}"
        );
        // Not pinned to the bottom: the footer and status rows are below them.
        assert!(
            refs_at < rows.len() - 3,
            "references should not sit against the footer: {rows:?}"
        );
    }

    /// The conversation title belongs to the list you pick a chat from, not to a
    /// permanent row above the chat you are reading.
    #[test]
    fn the_header_does_not_carry_the_conversation_title() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        state.apply(super::ChatEvent::Title("A very specific chat title".into()));
        let rows = frame(&mut ui, &state, 70, 14);
        assert!(
            !rows
                .iter()
                .any(|l| l.contains("A very specific chat title")),
            "the title should not be in the header:\n{}",
            rows.join("\n")
        );
    }

    /// The title bar needs air under it, or it reads as the transcript's first
    /// line rather than as chrome.
    #[test]
    fn the_title_bar_is_separated_from_the_transcript() {
        let mut ui = super::Ui::new();
        let state = super::ChatState::new("chatbot".into(), "welcome".into());
        let rows = frame(&mut ui, &state, 70, 24);
        assert!(
            rows[0].contains("Longbridge AI"),
            "row 0 is the badge: {:?}",
            rows[0]
        );
        assert!(
            rows[1].trim().is_empty(),
            "row 1 should be blank, got {:?}",
            rows[1]
        );
    }

    /// Terminals that cannot distinguish Shift+Enter from Enter send a bare LF,
    /// which crossterm reports as Ctrl+J. That used to fall through to the
    /// insert-a-character arm and type a literal `j` into the prompt.
    #[test]
    fn shift_enter_as_ctrl_j_inserts_a_newline() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        let mut editor = super::Editor::new();
        let mut turn = None;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        editor.insert_str("first");
        super::on_chat_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
            &mut ui,
            &mut state,
            &mut editor,
            &mut turn,
            &tx,
        );
        editor.insert_str("second");
        assert_eq!(
            editor.text(),
            "first\nsecond",
            "expected a newline, not a `j`"
        );
    }

    /// A Ctrl combination we do not bind must do nothing, rather than typing its
    /// letter into the prompt.
    #[test]
    fn an_unbound_ctrl_key_types_nothing() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        let mut editor = super::Editor::new();
        let mut turn = None;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        for c in ['g', 'o', 'p', 'z'] {
            super::on_chat_key(
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL),
                &mut ui,
                &mut state,
                &mut editor,
                &mut turn,
                &tx,
            );
        }
        assert_eq!(
            editor.text(),
            "",
            "unbound Ctrl keys leaked into the prompt"
        );
    }

    fn sample_card(
        symbol: &str,
        name: &str,
        last: &str,
        chg: &str,
        pct: &str,
        dir: i8,
    ) -> crate::ai::quotes::QuoteCardData {
        crate::ai::quotes::QuoteCardData {
            prev_close: rust_decimal_macros::dec!(180.0),
            symbol: symbol.into(),
            name: name.into(),
            last: last.into(),
            change: chg.into(),
            change_pct: pct.into(),
            direction: dir,
            open: "179.2".into(),
            high: "183.5".into(),
            low: "178.9".into(),
            volume: "4212万".into(),
            turnover: "58.3亿".into(),
            at: "15:09".into(),
        }
    }

    fn cards() -> std::collections::HashMap<String, crate::ai::quotes::QuoteCardData> {
        let mut q = std::collections::HashMap::new();
        q.insert(
            "NVDA.US".to_string(),
            sample_card("NVDA.US", "英伟达", "182.4", "+3.75", "+2.10%", 1),
        );
        q.insert(
            "TSLA.US".to_string(),
            sample_card("TSLA.US", "特斯拉", "327.29", "-5.29", "-1.59%", -1),
        );
        q
    }

    fn text_of(lines: &[ratatui::text::Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A single-quote reference becomes a card carrying the live quote — the
    /// terminal's honest stand-in for the chart the web client draws.
    #[test]
    fn a_quote_widget_becomes_a_card() {
        let out = super::render_widget(
            "widget://quote/security/detail?symbol=NVDA.US&time_range=1",
            72,
            &cards(),
        );
        let text = text_of(&out);
        assert!(text.contains("NVDA.US") && text.contains("英伟达"));
        assert!(text.contains("182.4") && text.contains("+2.10%"));
        assert!(
            text.contains("179.2"),
            "the day's range belongs on the card"
        );
        assert!(text.contains('┌') && text.contains('└'), "it is a box");
        assert!(!text.contains("widget://"), "never show the URL");
    }

    /// A comparison names every security, one aligned row each, so the set can
    /// be read down the page.
    #[test]
    fn a_comparison_lists_every_security() {
        let out = super::render_widget(
            "widget://quote/security/comparison?symbols=TSLA.US&symbols=NVDA.US&time_range=3",
            72,
            &cards(),
        );
        let text = text_of(&out);
        assert!(text.contains("TSLA.US") && text.contains("NVDA.US"));
        assert!(text.contains("-1.59%") && text.contains("+2.10%"));
        assert!(!text.contains("widget://"));
    }

    /// A security with no quote yet is still named, not dropped and not shown as
    /// a URL.
    #[test]
    fn a_security_without_a_quote_is_still_named() {
        let out = super::render_widget(
            "widget://stock/list?symbols=NVDA.US&symbols=AAPL.US",
            72,
            &cards(),
        );
        let text = text_of(&out);
        assert!(text.contains("AAPL.US"), "the pending symbol should show");
        assert!(!text.contains("widget://"));
    }

    /// Widgets that name no security get a label, and an unknown kind names its
    /// path — in no case does a URL reach the transcript.
    #[test]
    fn non_security_widgets_are_labelled_not_urled() {
        for src in [
            "widget://cta/open_account",
            "widget://cta/fund_account",
            "widget://ipo/list",
            "widget://quant/backtest_result?backtest_uuid=abc123",
        ] {
            let text = text_of(&super::render_widget(src, 72, &cards()));
            assert!(!text.contains("widget://"), "{src} leaked its URL: {text}");
            assert!(!text.trim().is_empty(), "{src} rendered nothing");
        }
    }

    /// The card is a block inside a scrolling transcript, so it must respect the
    /// width it is given.
    #[test]
    fn a_quote_card_fits_the_width() {
        for width in [30usize, 50, 72] {
            let out = super::render_widget(
                "widget://quote/security/detail?symbol=NVDA.US",
                width,
                &cards(),
            );
            for line in out {
                let w: usize = line
                    .spans
                    .iter()
                    .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                    .sum();
                assert!(w <= width, "card line of {w} cols exceeds width {width}");
            }
        }
    }

    /// The mark is a nice-to-have; the welcome copy and the example prompts are
    /// the point, so a short terminal keeps the text and drops the logo.
    #[test]
    fn the_logo_yields_to_the_welcome_copy_on_a_short_terminal() {
        let logo_h = usize::from(crate::tui::ui::assets::mark_height());
        let state = super::ChatState::new("chatbot".into(), "welcome".into());

        let mut ui = super::Ui::new();
        let tall = frame(&mut ui, &state, 80, (logo_h + 20) as u16);
        assert!(
            tall.iter().any(|l| l.contains('█')),
            "a tall terminal should show the mark:\n{}",
            tall.join("\n")
        );

        let mut ui = super::Ui::new();
        let short = frame(&mut ui, &state, 80, 14);
        assert!(
            !short.iter().any(|l| l.contains('█')),
            "a short terminal should drop it:\n{}",
            short.join("\n")
        );
        // The copy is the localized welcome, not the message text passed in.
        let welcome = rust_i18n::t!("Ai.Welcome").to_string();
        let head: String = welcome.chars().take(12).collect();
        assert!(
            short.iter().any(|l| l.contains(&head)),
            "and keep the copy:\n{}",
            short.join("\n")
        );
    }

    /// Nothing in the welcome state may exceed the width, logo included.
    #[test]
    fn the_welcome_state_fits_the_width() {
        let state = super::ChatState::new("chatbot".into(), "welcome".into());
        for width in [40u16, 60, 80, 120] {
            let mut ui = super::Ui::new();
            for line in frame(&mut ui, &state, width, 40) {
                assert!(
                    unicode_width::UnicodeWidthStr::width(line.trim_end()) <= width as usize,
                    "welcome line exceeds {width}: {line:?}"
                );
            }
        }
    }

    /// The mark stays small enough to sit above the wordmark, and every row is
    /// the same width or the bars would not line up.
    #[test]
    fn the_mark_is_wordmark_sized() {
        let mark = crate::tui::ui::assets::logo_mark();
        assert_eq!(
            u16::try_from(mark.len()).unwrap(),
            crate::tui::ui::assets::mark_height(),
            "one line per mark row"
        );
        for line in &mark {
            let w: u16 = line
                .spans
                .iter()
                .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()) as u16)
                .sum();
            assert_eq!(
                w,
                crate::tui::ui::assets::mark_width(),
                "ragged mark row: {line:?}"
            );
        }
        let width = crate::tui::ui::assets::mark_width();
        assert!(
            width <= 24,
            "the mark should stay small next to the wordmark, got {width} columns"
        );
    }

    /// The mark is a bar chart, so it must keep its bars *and* the gaps between
    /// them: the tall bars reach the top row, the short ones do not, and the
    /// bottom row is not one solid run.
    #[test]
    fn the_mark_keeps_the_original_proportions() {
        let mark = crate::tui::ui::assets::logo_mark();
        let row = |i: usize| -> String {
            mark[i]
                .spans
                .iter()
                .map(|s| s.content.to_string())
                .collect()
        };
        let (top, bottom) = (row(0), row(mark.len() - 1));
        assert!(
            top.starts_with("▄ ▄▄"),
            "the two tall bars end half way into the top row, got {top:?}"
        );
        assert!(
            top.trim_end().len() < bottom.trim_end().len(),
            "the short bars must leave the top row empty"
        );
        assert!(
            bottom.contains(' '),
            "the bottom row keeps the gaps between bars, got {bottom:?}"
        );
        // Rows covered per column, counting a half block as a covered row: the
        // tall bars reach every row and the gaps reach none.
        let heights: Vec<usize> =
            (0..mark.len())
                .map(row)
                .fold(vec![0; bottom.chars().count()], |mut acc, line| {
                    for (i, c) in line.chars().enumerate() {
                        if c == '█' || c == '▄' {
                            acc[i] += 1;
                        }
                    }
                    acc
                });
        assert!(heights.contains(&mark.len()), "a bar spans every row");
        assert!(heights.contains(&0), "the gaps stay empty top to bottom");
    }

    /// A drawn block sits in its own air, like a table does: prose, blank, block,
    /// blank, prose. Without it a chart read as another paragraph.
    #[test]
    fn a_drawn_block_gets_a_blank_row_either_side() {
        let answer = "before\n\n```vis-chart\n{\"type\":\"line\",\"data\":[{\"time\":\"1/2\",\"value\":1.0},{\"time\":\"1/9\",\"value\":2.0}]}\n```\n\nafter";
        let lines = super::render_answer_lines(
            answer,
            50,
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
        );
        let text: Vec<String> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        let before = text
            .iter()
            .position(|l| l.contains("before"))
            .expect("prose");
        let after = text
            .iter()
            .position(|l| l.contains("after"))
            .expect("prose");
        let chart = text
            .iter()
            .position(|l| l.chars().any(|c| ('\u{2801}'..='\u{28FF}').contains(&c)))
            .expect("a drawn chart");
        assert!(before < chart && chart < after, "{text:?}");
        assert!(
            text[chart - 1].is_empty() || text[before + 1].is_empty(),
            "a blank row above the block: {text:?}"
        );
        assert!(
            text[after - 1].is_empty(),
            "a blank row below the block: {text:?}"
        );
        // And never two blanks where one will do.
        assert!(
            !text.windows(3).any(|w| w.iter().all(String::is_empty)),
            "no run of three blank rows: {text:?}"
        );
    }

    /// A symbol in an answer is something to open, so it has to be its own span,
    /// in the link colour, with a hit rectangle over exactly its columns.
    #[test]
    fn symbols_in_an_answer_are_clickable() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        state.apply(super::ChatEvent::UserPrompt("compare".into()));
        state.apply(super::ChatEvent::Delta(
            "700.HK and AAPL.US both rose.".into(),
        ));
        state.apply(super::ChatEvent::TurnFinished { error: None });
        let _ = frame(&mut ui, &state, 64, 20);
        let symbols: Vec<(String, u16)> = ui
            .chips
            .iter()
            .filter_map(|(chip, rect)| match chip {
                super::Chip::Symbol(s) => Some((s.clone(), rect.width)),
                _ => None,
            })
            .collect();
        assert!(
            symbols.iter().any(|(s, w)| s == "700.HK" && *w == 6),
            "700.HK should be clickable over its own six columns: {symbols:?}"
        );
        assert!(
            symbols.iter().any(|(s, w)| s == "AAPL.US" && *w == 7),
            "AAPL.US should be clickable: {symbols:?}"
        );
    }

    /// The panel opens over the transcript and closes on Esc, leaving the reader
    /// where they were.
    #[test]
    fn the_quote_panel_opens_and_closes() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        state.apply(super::ChatEvent::UserPrompt("700.HK?".into()));
        state.apply(super::ChatEvent::Delta("700.HK rose.".into()));
        state.apply(super::ChatEvent::TurnFinished { error: None });
        // `/quote` with no argument opens the security the answer mentioned last.
        super::exec_slash("quote", "", &mut ui, &mut state);
        assert_eq!(ui.quote_panel.as_deref(), Some("700.HK"));
        let rows = frame(&mut ui, &state, 64, 20);
        assert!(
            rows.iter().any(|r| r.contains("700.HK")),
            "the panel names the security: {rows:?}"
        );
        let mut editor = super::Editor::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut turn = None;
        super::on_chat_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut ui,
            &mut state,
            &mut editor,
            &mut turn,
            &tx,
        );
        assert!(ui.quote_panel.is_none(), "Esc closes the panel");
        assert_eq!(state.scroll, 0, "and the transcript stays where it was");
    }

    /// An inline security shows its own price: direction, last and change. The
    /// chip is inserted before the text is wrapped, so it always fits — appended
    /// afterwards it would overrun the width and be dropped, which is what
    /// happens to prose that wraps to nearly the full width.
    #[test]
    fn an_inline_security_carries_its_price() {
        let mut quotes = std::collections::HashMap::new();
        quotes.insert("700.HK".to_string(), card("700.HK", "512.5", "+1.28%", 1));
        quotes.insert(
            "AAPL.US".to_string(),
            card("AAPL.US", "182.4", "-0.62%", -1),
        );
        let answer = "本周 700.HK 走强，而 AAPL.US 回落，关注 700.HK 的成交量。";
        let lines =
            super::render_answer_lines(answer, 72, &quotes, &std::collections::HashMap::new());
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(text.contains("512.5 ▲1.28%"), "up chip: {text}");
        assert!(text.contains("182.4 ▼0.62%"), "down chip: {text}");
        // The price belongs beside the first mention; repeating it on every one
        // reads as noise.
        assert_eq!(text.matches("512.5").count(), 1, "priced once: {text}");
        // Every line still fits, and no line splits a symbol.
        for line in &lines {
            let w: usize = line
                .spans
                .iter()
                .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            assert!(w <= 72, "line of {w} columns: {line:?}");
        }
        assert!(
            !text.contains("70\n0.HK"),
            "a symbol must not be split by the wrap"
        );
    }

    /// The chip's colour is the direction, so it has to be its own span.
    #[test]
    fn the_price_chip_is_coloured_by_direction() {
        let mut quotes = std::collections::HashMap::new();
        quotes.insert(
            "AAPL.US".to_string(),
            card("AAPL.US", "182.4", "-0.62%", -1),
        );
        let lines = super::render_answer_lines(
            "AAPL.US fell today.",
            60,
            &quotes,
            &std::collections::HashMap::new(),
        );
        let chip = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content.contains("182.4"))
            .expect("the chip should be rendered");
        assert_eq!(chip.style.fg, Some(super::change_color(-1)));
    }

    /// A card is a block the reader aims at, so the whole box opens the panel —
    /// not only the six columns of its ticker.
    #[test]
    fn the_whole_quote_card_is_a_click_target() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        state.apply(super::ChatEvent::UserPrompt("700.HK?".into()));
        state.apply(super::ChatEvent::Delta(
            "<x-widget src=\"widget://quote/security/detail?symbol=700.HK\"></x-widget>".into(),
        ));
        state.apply(super::ChatEvent::TurnFinished { error: None });
        ui.quotes
            .insert("700.HK".to_string(), card("700.HK", "512.5", "+1.28%", 1));
        let _ = frame(&mut ui, &state, 70, 20);
        let tall = ui
            .chips
            .iter()
            .filter(|(chip, _)| matches!(chip, super::Chip::Symbol(s) if s == "700.HK"))
            .map(|(_, rect)| rect.height)
            .max()
            .unwrap_or(0);
        assert!(
            tall > 1,
            "the card's rows should all be clickable, got a {tall}-row target"
        );
    }

    /// A quote card fixture with the fields a test cares about.
    fn card(
        symbol: &str,
        last: &str,
        change_pct: &str,
        direction: i8,
    ) -> crate::ai::quotes::QuoteCardData {
        crate::ai::quotes::QuoteCardData {
            symbol: symbol.into(),
            name: String::new(),
            prev_close: rust_decimal_macros::dec!(500),
            last: last.into(),
            change: "+6.5".into(),
            change_pct: change_pct.into(),
            direction,
            open: "508".into(),
            high: "515".into(),
            low: "506".into(),
            volume: "18.2M".into(),
            turnover: "9.3B".into(),
            at: "16:08".into(),
        }
    }

    /// The Settings view answers "which account is this?" — there is no other way
    /// to check from inside the chat — and offers the action that applies.
    #[test]
    fn settings_shows_the_session_and_one_action() {
        let mut ui = super::Ui::new();
        ui.view = super::View::Settings;
        ui.session = crate::ai::account::Session {
            status: "valid",
            dc_region: Some("ap"),
            access_point: "https://openapi.longbridge.com".into(),
            logged_in_at: None,
            expires_at: None,
            member_id: Some("123456".into()),
        };
        let state = super::ChatState::new("chatbot".into(), "welcome".into());
        let rows = frame(&mut ui, &state, 74, 24);
        let text = rows.join("\n");
        assert!(text.contains(t!("Ai.Account").as_ref()), "{text}");
        assert!(text.contains("#123456"), "the member id: {text}");
        assert!(
            text.contains("openapi.longbridge.com"),
            "the access point: {text}"
        );
        assert!(text.contains("AP"), "the data centre: {text}");
        assert!(
            text.contains(t!("Ai.SignOut").as_ref()),
            "sign out is offered"
        );
        assert!(
            !text.contains(t!("Ai.SignIn").as_ref()),
            "and signing in is not, to someone already signed in: {text}"
        );
    }

    /// Someone whose token has expired needs the other action.
    #[test]
    fn a_signed_out_session_is_offered_sign_in() {
        let mut ui = super::Ui::new();
        ui.view = super::View::Settings;
        ui.session = crate::ai::account::Session {
            status: "expired",
            ..Default::default()
        };
        let state = super::ChatState::new("chatbot".into(), "welcome".into());
        let text = frame(&mut ui, &state, 74, 24).join("\n");
        assert!(text.contains(t!("Ai.SessionExpired").as_ref()), "{text}");
        assert!(text.contains(t!("Ai.SignIn").as_ref()), "{text}");
    }

    /// Ending the session takes two keypresses from a list row: an arrow key and a
    /// stray Return must not sign the reader out.
    #[test]
    fn signing_out_from_the_row_takes_a_confirmation() {
        let mut ui = super::Ui::new();
        ui.view = super::View::Settings;
        ui.session = crate::ai::account::Session {
            status: "valid",
            ..Default::default()
        };
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        ui.sel = super::settings_rows(&ui.session).len() - 1;
        super::activate(&mut ui, &mut state);
        assert!(ui.pending.is_none(), "the first Enter only arms it");
        assert!(ui.confirm_sign_out);
        super::activate(&mut ui, &mut state);
        assert_eq!(ui.pending, Some(super::Pending::SignOut));
        // Typed deliberately, `/logout` needs no confirmation.
        let mut ui = super::Ui::new();
        super::exec_slash("logout", "", &mut ui, &mut state);
        assert_eq!(ui.pending, Some(super::Pending::SignOut));
    }

    /// The title bar was carrying nothing. It now carries the securities the
    /// conversation has mentioned, with their quotes, and rotates through them
    /// when there are more than fit.
    #[test]
    fn the_title_bar_carries_the_sessions_securities() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        state.apply(super::ChatEvent::UserPrompt("对比".into()));
        state.apply(super::ChatEvent::Delta(
            "700.HK、AAPL.US、NVDA.US、TSLA.US 与 9988.HK 分化。".into(),
        ));
        state.apply(super::ChatEvent::TurnFinished { error: None });
        super::track_session_symbols(&mut ui, &state);
        assert_eq!(
            ui.tape,
            ["700.HK", "AAPL.US", "NVDA.US", "TSLA.US", "9988.HK"],
            "every security mentioned, in the order it appeared"
        );
        for symbol in ui.tape.clone() {
            ui.quotes
                .insert(symbol.clone(), card(&symbol, "512.5", "+1.28%", 1));
        }
        let first = frame(&mut ui, &state, 78, 10)[0].clone();
        assert!(first.contains("700.HK 512.5 ▲1.28%"), "{first}");
        // More than fits, so the ticker rotates rather than truncating.
        assert!(
            !first.contains("9988.HK"),
            "the row cannot hold them all: {first}"
        );
        ui.tape_ticks = 7;
        let _ = frame(&mut ui, &state, 78, 10);
        let rotated = frame(&mut ui, &state, 78, 10)[0].clone();
        assert_ne!(first, rotated, "the ticker should have advanced");
    }

    /// A ticker that fits sits still, and the toggle turns it off — the row is
    /// chrome, and chrome that moves for no reason is a distraction.
    #[test]
    fn a_short_ticker_does_not_rotate_and_can_be_turned_off() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        state.apply(super::ChatEvent::UserPrompt("700.HK".into()));
        state.apply(super::ChatEvent::TurnFinished { error: None });
        super::track_session_symbols(&mut ui, &state);
        ui.quotes
            .insert("700.HK".into(), card("700.HK", "512.5", "+1.28%", 1));
        let before = frame(&mut ui, &state, 78, 10)[0].clone();
        ui.tape_ticks = 7;
        let _ = frame(&mut ui, &state, 78, 10);
        assert_eq!(
            before,
            frame(&mut ui, &state, 78, 10)[0],
            "one security fits, so nothing moves"
        );
        // The toggle is a chip in the row, and clicking it hides the ticker.
        assert!(
            ui.chips
                .iter()
                .any(|(chip, _)| matches!(chip, super::Chip::Tape)),
            "the toggle should be clickable"
        );
        crate::ai::settings::set_tape(false);
        let off = frame(&mut ui, &state, 78, 10)[0].clone();
        assert!(!off.contains("512.5"), "collapsed: {off}");
        crate::ai::settings::set_tape(true);
    }

    /// A bare ticker the server confirmed is a link, priced like a dotted one,
    /// and clicking it opens the full symbol.
    #[test]
    fn a_confirmed_bare_ticker_behaves_like_a_symbol() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        state.apply(super::ChatEvent::UserPrompt("查询 SPCX".into()));
        state.apply(super::ChatEvent::Delta(
            "SPCX 当前盘中走弱，卖 Put 已 ITM。".into(),
        ));
        state.apply(super::ChatEvent::TurnFinished { error: None });
        ui.aliases.insert("SPCX".into(), "SPCX.US".into());
        ui.quotes
            .insert("SPCX.US".into(), card("SPCX.US", "135.995", "-3.75%", -1));
        super::track_session_symbols(&mut ui, &state);
        assert_eq!(ui.tape, ["SPCX.US"], "the ticker is tracked by its symbol");
        let rows = frame(&mut ui, &state, 70, 16);
        let text = rows.join("\n");
        assert!(text.contains("135.995 ▼3.75%"), "priced inline: {text}");
        // Clicking the four letters opens the security they resolve to.
        let target = ui.chips.iter().find_map(|(chip, _)| match chip {
            super::Chip::Symbol(s) => Some(s.clone()),
            _ => None,
        });
        assert_eq!(target.as_deref(), Some("SPCX.US"));
        // And the jargon beside it is left alone.
        assert!(
            !ui.chips
                .iter()
                .any(|(chip, _)| matches!(chip, super::Chip::Symbol(s) if s.starts_with("ITM"))),
            "ITM must not become a link"
        );
    }

    /// The prompt carries the same mark as the reader's turns, and no box: a
    /// rounded frame around the input was the loudest thing on screen.
    #[test]
    fn the_prompt_is_marked_not_boxed() {
        let mut ui = super::Ui::new();
        let state = super::ChatState::new("chatbot".into(), "welcome".into());
        let rows = frame(&mut ui, &state, 60, 16);
        let prompt = rows
            .iter()
            .rev()
            .find(|r| r.contains(t!("Ai.Placeholder").as_ref()))
            .expect("the placeholder should be on screen");
        assert!(prompt.contains("❯ "), "marked: {prompt:?}");
        // The box stays, dim: it is what separates the prompt from the transcript.
        assert!(
            rows.iter().any(|r| r.contains('╭')),
            "the input keeps its frame: {rows:?}"
        );
    }

    /// The symbol carries no colour of its own: with a coloured price beside it,
    /// two tints on one short run read as noise. The hover underline is the
    /// affordance, and clicking still resolves the security.
    #[test]
    fn a_security_is_not_tinted() {
        let mut quotes = std::collections::HashMap::new();
        quotes.insert("700.HK".to_string(), card("700.HK", "512.5", "+1.28%", 1));
        let lines = super::render_answer_lines(
            "700.HK 走强。",
            60,
            &quotes,
            &std::collections::HashMap::new(),
        );
        let spans: Vec<&ratatui::text::Span> = lines.iter().flat_map(|l| &l.spans).collect();
        let symbol = spans
            .iter()
            .find(|s| s.content.as_ref() == "700.HK")
            .expect("the symbol should be its own span");
        assert_eq!(symbol.style.fg, None, "no colour of its own");
        let price = spans
            .iter()
            .find(|s| s.content.contains("512.5"))
            .expect("the price should be there");
        assert_eq!(
            price.style.fg,
            Some(super::change_color(1)),
            "only the price is tinted"
        );
    }
}
