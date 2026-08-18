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
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
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
/// Rows a PageUp/PageDown moves the selection in a list view.
const LIST_PAGE: usize = 8;

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

/// A sign-in waiting on the reader's browser.
struct LoginPrompt {
    /// Where to authorize. Copied to the clipboard as well, since a browser that
    /// did not open leaves the reader with a URL they cannot click.
    url: String,
    /// The short code the page asks them to confirm; empty if the server omits it.
    code: String,
    browser_opened: bool,
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

const SLASH: [Slash; 12] = [
    Slash {
        name: "/new",
        aliases: &["/clear"],
        desc: "Ai.SlashNew",
    },
    Slash {
        name: "/retry",
        aliases: &["/regenerate"],
        desc: "Ai.SlashRetry",
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
    /// One of the welcome screen's example prompts; sends it.
    Sample(&'static str),
    /// The brand badge, which opens Longbridge AI on the web.
    Brand,
    /// The title bar's control for the account's conversations.
    Sessions,
}

/// State of the structured interrupt answering flow.
struct QuestionState {
    /// `(question text, option texts)` for each question.
    questions: Vec<(String, Vec<String>)>,
    /// Where the selected option for each displayed step belongs in the resume
    /// payload: `(interrupt_id, answer key, wire values by option index)`.
    targets: Vec<(String, String, Option<Vec<String>>, bool)>,
    /// Which question is being answered.
    qi: usize,
    /// Collected `{interrupt_id: {answer key: answer}}` resume payload.
    answers: longbridge::agent::AnswersByToolCall,
    /// Human-readable selections echoed into the transcript.
    summaries: Vec<String>,
    /// Selected option indices for multi-select questions, keyed by question index.
    multi_selected: HashMap<usize, Vec<usize>>,
}

/// In-transcript search (Ctrl+F): a query and which of its matches is focused.
///
/// The matches themselves are recomputed each render from the transcript cache
/// rather than stored, so they stay correct as the conversation grows.
#[derive(Default)]
struct FindState {
    query: String,
    /// Index of the focused match within the current match list.
    current: usize,
}

/// Content-line indices whose text contains `query`, case-insensitively, in order.
fn find_matches(lines: &[String], query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.to_lowercase().contains(&q))
        .map(|(i, _)| i)
        .collect()
}

/// The scroll-back (lines from the bottom) that brings content `line` into view,
/// a couple of rows below the top so the reader sees some context above it.
fn scroll_to_line(total: usize, height: usize, max_scroll: u16, line: usize) -> u16 {
    let start = line.saturating_sub(2);
    let bottom = (start + height).min(total);
    let scroll = total.saturating_sub(bottom);
    u16::try_from(scroll).unwrap_or(u16::MAX).min(max_scroll)
}

impl QuestionState {
    fn from_interrupt(interrupt: &Value) -> Self {
        let mut questions = Vec::new();
        let mut targets = Vec::new();
        if runtime::interrupt_interactions(interrupt)
            .iter()
            .any(|interaction| {
                matches!(
                    interaction.get("type").and_then(Value::as_str),
                    Some(
                        "trade_password"
                            | "connector_reauth"
                            | "openapi_reauth"
                            | "data_authorization"
                    )
                )
            })
        {
            return Self {
                questions,
                targets,
                qi: 0,
                answers: HashMap::new(),
                summaries: Vec::new(),
                multi_selected: HashMap::new(),
            };
        }
        for interaction in runtime::interrupt_interactions(interrupt) {
            let Some(interrupt_id) = runtime::interaction_id(interaction) else {
                continue;
            };
            let kind = interaction
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("ask_human");
            if kind == "ask_human" || !runtime::interaction_questions(interaction).is_empty() {
                for question in runtime::interaction_questions(interaction) {
                    let Some(text) = question.get("question").and_then(Value::as_str) else {
                        continue;
                    };
                    let multi_select = question
                        .get("multi_select")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let mut choices = crate::cli::agent::chat::question_choices(question);
                    if multi_select && !choices.is_empty() {
                        choices.push(t!("Ai.ConfirmSelection").to_string());
                    }
                    questions.push((text.to_string(), choices));
                    targets.push((
                        interrupt_id.to_string(),
                        text.to_string(),
                        None,
                        multi_select,
                    ));
                }
                continue;
            }
            if kind == "authorization" {
                // `tool_name` is an internal MCP function identifier. Only use
                // the server's human-readable display name; otherwise the
                // generic prompt is clearer than leaking implementation detail.
                let name = interaction
                    .get("tool_display_name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let prompt = if name.is_empty() {
                    t!("Ai.AuthorizationPrompt").to_string()
                } else {
                    t!("Ai.AuthorizationToolPrompt", name = name).to_string()
                };
                questions.push((
                    prompt,
                    vec![t!("Ai.Decline").to_string(), t!("Ai.Allow").to_string()],
                ));
                targets.push((
                    interrupt_id.to_string(),
                    "authorized".to_string(),
                    Some(vec!["false".into(), "true".into()]),
                    false,
                ));
            }
        }
        Self {
            questions,
            targets,
            qi: 0,
            answers: HashMap::new(),
            summaries: Vec::new(),
            multi_selected: HashMap::new(),
        }
    }

    /// Every question offers at least one option, so the overlay can fully
    /// answer it (otherwise the inline free-text path is used instead).
    fn fully_selectable(&self) -> bool {
        !self.questions.is_empty() && self.questions.iter().all(|(_, o)| !o.is_empty())
    }

    fn has_confirmation(&self) -> bool {
        self.targets
            .iter()
            .any(|(_, _, values, _)| values.is_some())
    }

    fn select(&mut self, option_index: usize) -> bool {
        let Some((_question, options)) = self.questions.get(self.qi) else {
            return false;
        };
        let Some(choice) = options.get(option_index).cloned() else {
            return false;
        };
        let Some((interrupt_id, key, wire_values, multi_select)) = self.targets.get(self.qi) else {
            return false;
        };
        if *multi_select {
            let confirm_index = options.len().saturating_sub(1);
            if option_index != confirm_index {
                let selected = self.multi_selected.entry(self.qi).or_default();
                if let Some(pos) = selected.iter().position(|index| *index == option_index) {
                    selected.remove(pos);
                } else {
                    selected.push(option_index);
                }
                return false;
            }
            let selected = self
                .multi_selected
                .get(&self.qi)
                .cloned()
                .unwrap_or_default();
            if selected.is_empty() {
                return false;
            }
            let choices = selected
                .into_iter()
                .filter_map(|index| options.get(index).cloned())
                .collect::<Vec<_>>();
            let value = choices.join(", ");
            self.answers
                .entry(interrupt_id.clone())
                .or_default()
                .insert(key.clone(), value.clone());
            self.summaries.push(value);
            return true;
        }
        let value = wire_values
            .as_ref()
            .and_then(|values| values.get(option_index))
            .cloned()
            .unwrap_or_else(|| choice.clone());
        self.answers
            .entry(interrupt_id.clone())
            .or_default()
            .insert(key.clone(), value);
        self.summaries.push(choice);
        true
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
    /// Active selection as `(anchor, cursor)`, each `(content-line, display-col)`
    /// in *content* coordinates — the line's index into the whole transcript, not
    /// a screen row. Content coordinates are what let a selection survive scrolling
    /// (and be dragged past the visible edge): the same line keeps the same index
    /// as the view moves under it. `None` when nothing is selected.
    selection: Option<((usize, u16), (usize, u16))>,
    /// Content index of the first visible transcript row, and how many rows are
    /// visible, recorded during render so a mouse cell can be mapped to a content
    /// line (and back) while dragging a selection.
    view_start: usize,
    view_rows: usize,
    /// Total transcript rows at the last render, for clamping a drag to content.
    view_total: usize,
    /// The text under the current selection, extracted during render.
    selected_text: Option<String>,
    /// Cached rendered lines of the committed transcript and the signature they
    /// were built for, so Markdown is not re-parsed every frame.
    transcript_cache: Vec<Line<'static>>,
    /// Plain text per cached line, rebuilt alongside `transcript_cache`, so
    /// in-transcript search does not re-flatten every span each keystroke.
    cache_text: Vec<String>,
    cache_sig: u64,
    /// A live quote arrived but the transcript was not rebuilt for it (the reader
    /// was scrolled up, or a rebuild just happened), so the inline price cards are
    /// a tick stale. Flushed when the reader returns to the bottom.
    cards_dirty: bool,
    /// When the transcript was last rebuilt for a live quote, so a burst of pushes
    /// refreshes the cards a few times a second rather than re-rendering every
    /// chart on every tick — which is what made a chart-heavy scroll stutter.
    cards_synced_at: Option<std::time::Instant>,
    /// In-transcript search state (Ctrl+F), `None` when the find bar is closed.
    find: Option<FindState>,
    /// Max scroll-back for the current transcript (recorded during render), so
    /// input handlers can clamp and never scroll the view into a blank screen.
    max_scroll: u16,
    /// Total transcript rows at the previous render, so that while the reader is
    /// scrolled up a streaming answer keeps their view anchored to the same
    /// content instead of drifting as new lines are appended below.
    prev_total: usize,
    /// Plain text of each visible transcript row, recorded during render so a
    /// double-click can find the word under the pointer.
    visible_text: Vec<String>,
    /// The previous left-click's time, cell, and consecutive-click count, for
    /// double- (word) and triple-click (line) selection.
    last_click: Option<(std::time::Instant, u16, u16, u8)>,
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
    /// Scroll offset of the help overlay, when it is open.
    help: Option<u16>,
    /// Where the panel's `WEB` button is, so it can be clicked.
    open_button: Option<Rect>,
    /// Where the panel's `‹` / `›` arrows are, when the conversation named more
    /// than one security, so a click can step to the previous / next one.
    prev_button: Option<Rect>,
    next_button: Option<Rect>,
    /// The column a clicked security sat at, so the drawer opens directly beneath
    /// it rather than in the corner. `None` for a keyboard/command open, which has
    /// no on-screen anchor and falls back to the right edge.
    quote_anchor_x: Option<u16>,
    /// Where the topmost thing's `[Close]` button is. One field rather than one per
    /// overlay: only ever one is on screen, and the click path then works the same
    /// in every view.
    close_button: Option<Rect>,
    /// The day's price path per security, for the panel's sparkline.
    paths: HashMap<String, Vec<f64>>,
    paths_tx: Option<UnboundedSender<(String, Vec<f64>)>>,
    /// The panel's richer figures per security, fetched lazily on open.
    details: HashMap<String, super::quotes::QuoteDetail>,
    details_tx: Option<UnboundedSender<(String, super::quotes::QuoteDetail)>>,
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
    /// How many entries the ticker drew last frame, so stepping the open drawer
    /// knows which securities are on screen and pages only when the next one is not.
    tape_drawn: usize,
    /// When the ticker last turned over, so the dwell is wall-clock rather than a
    /// count of frames.
    tape_shown_at: Option<std::time::Instant>,
    /// Work for the async side of the loop: signing in or out.
    pending: Option<Pending>,
    /// A device-code sign-in in progress, shown as a panel: the reader authorizes
    /// in a browser while the chat waits, rather than being thrown out to a shell.
    login: Option<LoginPrompt>,
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
    /// When the last Ctrl+C on an empty prompt was pressed; a second within
    /// [`CTRL_C_WINDOW`] exits, but the armed state expires so it never lingers.
    ctrl_c_armed: Option<std::time::Instant>,
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
            view_start: 0,
            view_rows: 0,
            view_total: 0,
            selected_text: None,
            transcript_cache: Vec::new(),
            cache_text: Vec::new(),
            cache_sig: 0,
            cards_dirty: false,
            cards_synced_at: None,
            find: None,
            max_scroll: 0,
            prev_total: 0,
            visible_text: Vec::new(),
            last_click: None,
            quotes: HashMap::new(),
            sessions_loading: false,
            sessions_error: false,
            history_tx: None,
            cards_tx: None,
            quote_panel: None,
            help: None,
            open_button: None,
            prev_button: None,
            next_button: None,
            quote_anchor_x: None,
            close_button: None,
            paths: HashMap::new(),
            paths_tx: None,
            details: HashMap::new(),
            details_tx: None,
            session: super::account::local(),
            tape: Vec::new(),
            aliases: HashMap::new(),
            tape_at: 0,
            tape_drawn: 0,
            tape_shown_at: None,
            pending: None,
            login: None,
            confirm_sign_out: false,
            exit_note: None,
            load_tx: None,
            stop_button: None,
            turn_started: None,
            should_quit: false,
            ctrl_c_armed: None,
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
        // The quote panel overlays the chat; only Esc *in Chat* closes it. Leaving
        // another view means it would float over that view with no way to dismiss
        // it there, so a switch away from Chat takes it down.
        if view != View::Chat {
            self.quote_panel = None;
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
        // The title ticker is the conversation's securities, so a fresh chat
        // starts with an empty tape rather than the last chat's tickers.
        self.tape.clear();
        self.tape_at = 0;
        // These are all keyed to the previous conversation too. Leaving the panel
        // open would strand it — its quote was just cleared, so it would render a
        // "loading" placeholder forever with nothing to refetch it — and leaving
        // the sparkline paths or the confirmed-ticker aliases would show one
        // conversation's securities in the next.
        self.quote_panel = None;
        self.paths.clear();
        self.aliases.clear();
    }
}

/// Run the chat TUI until the user quits. The caller has already entered the
/// full-screen terminal (with mouse capture) and restores it afterwards.
/// Run the chat. Returns a note to print once the full-screen view is gone —
/// signing in or out has to be reported outside the alternate screen, or the
/// message scrolls away with it.
pub async fn run(agent_uid: String, quotes: Option<QuoteStream>) -> Result<Option<String>> {
    // Boxed and swappable: opened signed out there is no stream at all, and a
    // sign-in from inside the chat produces one. `pending()` stands in for "none"
    // so the select arm below needs no special case.
    let mut quotes: QuoteStream = quotes.unwrap_or_else(|| Box::pin(tokio_stream::pending()));
    let mut terminal = Terminal::default();
    let mut state = ChatState::new(agent_uid, t!("Ai.Welcome").to_string());
    let mut ui = Ui::new();
    let mut editor = Editor::new();
    // Seed the prompt history from disk so ↑/↓ recalls prompts from previous
    // sessions, the way a shell does.
    editor.seed_history(super::history::load());
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
    let (paths_tx, mut paths_rx) = unbounded_channel::<(String, Vec<f64>)>();
    ui.paths_tx = Some(paths_tx);
    let (details_tx, mut details_rx) = unbounded_channel::<(String, super::quotes::QuoteDetail)>();
    ui.details_tx = Some(details_tx);
    let (login_tx, mut login_rx) = unbounded_channel::<Result<(), String>>();
    let mut login_task: Option<JoinHandle<()>> = None;
    let (aliases_tx, mut aliases_rx) = unbounded_channel::<HashMap<String, String>>();
    let (session_tx, mut session_rx) = unbounded_channel::<Option<String>>();
    tokio::spawn(async move {
        let _ = session_tx.send(super::account::member_id().await);
    });
    ui.load_tx = Some(load_tx);
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
    // Read terminal events on a blocking thread into a channel we own. Unlike
    // crossterm's async `EventStream`, a tokio receiver can be drained with
    // `try_recv`, so a fast wheel-scroll's burst is coalesced into one redraw
    // instead of one redraw per tick — which let the view fall behind the wheel.
    let (event_tx, mut event_rx) = unbounded_channel::<Event>();
    std::thread::spawn(move || {
        while let Ok(event) = crossterm::event::read() {
            if event_tx.send(event).is_err() {
                break;
            }
        }
    });

    loop {
        terminal.draw(|f| view(f, &mut ui, &mut state, &editor))?;
        tokio::select! {
            _ = ticker.tick(), if ui.animating => {
                ui.tick = ui.tick.wrapping_add(1);
            }
            maybe_event = event_rx.recv() => {
                let Some(ev) = maybe_event else { break };
                dispatch_event(ev, &mut ui, &mut state, &mut editor, &mut turn, &tx);
                // Drain the rest of a buffered burst before drawing, so a fast
                // wheel-scroll coalesces into a single frame and reversing direction
                // takes effect at once. Bounded so a flood still yields to a redraw.
                let mut budget = 256;
                while budget > 0 {
                    match event_rx.try_recv() {
                        Ok(ev) => {
                            dispatch_event(ev, &mut ui, &mut state, &mut editor, &mut turn, &tx);
                            budget -= 1;
                        }
                        Err(_) => break,
                    }
                }
            }
            Some(event) = turn_rx.recv() => {
                let finished = matches!(event, ChatEvent::TurnFinished { .. });
                state.apply(event);
                if finished {
                    turn = None;
                    maybe_open_question(&mut ui, &state);
                    // Whatever the reader typed while waiting goes out now. It
                    // answers a pending question too, which is what a reply typed
                    // while the agent was asking is: an answer.
                    let waiting_for_confirmation = ui
                        .question
                        .as_ref()
                        .is_some_and(QuestionState::has_confirmation);
                    if !state.queued.is_empty() && !waiting_for_confirmation {
                        let next = state.queued.remove(0);
                        // The drawer was asking what this message answers, so it
                        // goes; a reader who has wandered into Settings stays there.
                        if ui.view == View::Question {
                            ui.view = View::Chat;
                        }
                        ui.question = None;
                        start_turn(next, &mut state, &mut turn, &tx);
                    }
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
                        // A price tick changes the inline quote cards, not the
                        // charts, but the transcript is cached as one unit — so
                        // rebuilding it re-renders every chart. Only do that while
                        // the reader is at the bottom watching the price, and then
                        // only a few times a second; scrolled up through a
                        // chart-heavy history it would just make scrolling stutter.
                        let now = std::time::Instant::now();
                        let due = ui
                            .cards_synced_at
                            .is_none_or(|t| now.duration_since(t) >= CARD_REFRESH);
                        if state.scroll == 0 && due {
                            ui.cache_sig = 0;
                            ui.cards_synced_at = Some(now);
                            ui.cards_dirty = false;
                        } else {
                            ui.cards_dirty = true;
                        }
                    }
                }
            }
            Some((symbol, path)) = paths_rx.recv() => {
                ui.paths.insert(symbol, path);
            }
            Some((symbol, detail)) = details_rx.recv() => {
                ui.details.insert(symbol, detail);
            }
            Some(member_id) = session_rx.recv() => {
                ui.session.member_id = member_id;
            }
            Some(result) = login_rx.recv() => {
                login_task = None;
                ui.login = None;
                match result {
                    Ok(()) => {
                        ui.session = super::account::local();
                        // Signing in finishes here, in the chat — whether it opened
                        // signed out or the reader signed out and back in (as a
                        // different account, usually). The contexts are rebuilt from
                        // the new credentials and this session carries on with them;
                        // only a failure to build them is worth leaving for.
                        if let Ok((rx, _, _)) = crate::openapi::init_contexts().await {
                            quotes = Box::pin(rx);
                            ui.session = super::account::local();
                            ui.notice = Some(t!("Ai.SignedIn").to_string());
                            for symbol in ui.tape.clone() {
                                subscribe_quote(&symbol);
                            }
                            fetch_missing_quotes(&ui, &cards_tx);
                        } else {
                            ui.exit_note = Some(t!("Ai.SignedIn").to_string());
                            ui.should_quit = true;
                        }
                    }
                    Err(e) => ui.notice = Some(format!("{}: {e}", t!("Ai.SignInFailed"))),
                }
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
                    maybe_open_question(&mut ui, &state);
                } else {
                    // Resume failed: stay on History with an error notice.
                    ui.notice = Some(t!("Ai.SessionsError").to_string());
                }
            }
        }
        if ui.pending == Some(Pending::SignIn) {
            ui.pending = None;
            // Signing in happens here, in the chat: the flow only needs a URL on
            // screen and a poll in the background, and tearing the whole view down
            // for it was the thing that made signing in feel like leaving.
            // Named so the authorization page, and the account's list of
            // authorized clients afterwards, say which client is asking.
            match crate::auth::device_login_start(false, None).await {
                Ok(login) => {
                    ui.notice = None;
                    // A browser that did not open leaves the reader with a URL they
                    // cannot click, so it goes to the clipboard either way.
                    copy_to_clipboard(&login.verification_url);
                    ui.login = Some(LoginPrompt {
                        url: login.verification_url.clone(),
                        code: login.user_code.clone(),
                        browser_opened: login.browser_opened,
                    });
                    let tx = login_tx.clone();
                    login_task = Some(tokio::spawn(async move {
                        let result = crate::auth::device_login_wait(&login, false).await;
                        let _ = tx.send(result.map_err(|e| format!("{e:#}")));
                    }));
                }
                Err(e) => ui.notice = Some(format!("{}: {e:#}", t!("Ai.SignInFailed"))),
            }
        }
        // Signing out is the only action left to run here; signing in is handled
        // above, where the panel can stay open while the browser round trip runs.
        if ui.pending.take() == Some(Pending::SignOut) {
            // A turn in flight belongs to the credentials being revoked.
            if let Some(task) = turn.take() {
                task.abort();
                state.cancel(&t!("Ai.Cancelled"));
            }
            match crate::auth::clear_token().await {
                Ok(()) => {
                    // Signing out stays in the chat. The contexts cannot be torn
                    // down — they are process-wide singletons — so the process is
                    // marked signed out instead, and the view goes back to what an
                    // anonymous start looks like: the transcript is still readable,
                    // and asking or quoting anything says to sign in first.
                    crate::openapi::mark_signed_out();
                    // The conversation stays on screen to read, but not to add to:
                    // whoever signs in next may be a different account, and this
                    // thread is not theirs. It is still on the server for the
                    // account that owns it, one `/resume` away.
                    state.chat_uid = None;
                    state.message_id = None;
                    state.parent_message_id = None;
                    state.pending_interrupt = None;
                    ui.session = super::account::local();
                    ui.tape.clear();
                    ui.quotes.clear();
                    ui.paths.clear();
                    ui.quote_panel = None;
                    ui.cache_sig = 0;
                    ui.notice = Some(t!("Ai.SignedOut").to_string());
                }
                Err(e) => ui.notice = Some(format!("{}: {e}", t!("Ai.SignOutFailed"))),
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
    if let Some(task) = login_task.take() {
        task.abort();
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
        if matches!(message.role, Role::System | Role::Alert | Role::Tool) {
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
    // Signed out there is nothing to ask: the chat opens anyway and offers to
    // sign in, and every quote path stays quiet until it has credentials.
    if !crate::openapi::is_ready() || (!super::settings::quote_cards() && !super::settings::tape())
    {
        return;
    }
    let mut candidates: Vec<String> = Vec::new();
    for message in &state.messages {
        if matches!(message.role, Role::System | Role::Alert | Role::Tool) {
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
    // enough to fetch. Signed out, neither can be.
    if !crate::openapi::is_ready() || (!super::settings::quote_cards() && !super::settings::tape())
    {
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

/// Route one terminal event to its handler. Split out of the loop so a burst of
/// buffered events can be drained through it before a single redraw.
fn dispatch_event(
    event: Event,
    ui: &mut Ui,
    state: &mut ChatState,
    editor: &mut Editor,
    turn: &mut Option<JoinHandle<()>>,
    tx: &UnboundedSender<ChatEvent>,
) {
    match event {
        Event::Key(key) if key.kind != KeyEventKind::Release => {
            on_key(key, ui, state, editor, turn, tx);
        }
        Event::Mouse(m) => on_mouse(m, ui, state, editor, turn, tx),
        // Paste only when the prompt is on screen. In a list view the editor is
        // hidden, so a paste there would silently fill an invisible input; route it
        // to the History search instead.
        Event::Paste(text) => match ui.view {
            View::Chat => editor.paste(&text),
            View::Sessions => {
                ui.search.push_str(text.trim());
                ui.sel = 0;
            }
            _ => {}
        },
        Event::FocusGained => ui.focused = true,
        Event::FocusLost => ui.focused = false,
        _ => {}
    }
}

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
    ui.ctrl_c_armed = None;
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
        ui.ctrl_c_armed = None;
    } else if !editor.is_blank() {
        editor.clear();
        ui.ctrl_c_armed = None;
    } else if ui.ctrl_c_armed.is_some_and(|t| t.elapsed() < CTRL_C_WINDOW) {
        ui.should_quit = true;
    } else {
        ui.ctrl_c_armed = Some(std::time::Instant::now());
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
        KeyCode::PageUp => ui.sel = ui.sel.saturating_sub(LIST_PAGE),
        KeyCode::PageDown => {
            let last = ui.row_count().saturating_sub(1);
            ui.sel = (ui.sel + LIST_PAGE).min(last);
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
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let newline = key
        .modifiers
        .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT);
    // The panel floats over the chat, so Esc dismisses it before anything else
    // reads the key — otherwise the reader would clear their input instead.
    // A sign-in in progress owns Esc: it is a step the reader is in the middle of,
    // and cancelling it must not also clear their input.
    if ui.login.is_some() && key.code == KeyCode::Esc {
        ui.login = None;
        ui.notice = Some(t!("Ai.LoginCancelled").to_string());
        return;
    }
    // The panel takes Esc and nothing else: every other key belongs to the chat
    // behind it, which stays live while a quote is open.
    if ui.quote_panel.is_some() && key.code == KeyCode::Esc {
        close_quote_panel(ui);
        return;
    }
    // While the panel is open the arrows walk the conversation's securities in
    // place, so the reader flips through them without closing and reopening. Only
    // the bare arrows — a modified one still belongs to the chat behind it.
    if ui.quote_panel.is_some() && !ctrl && !shift && !alt {
        match key.code {
            KeyCode::Left => {
                step_quote_panel(ui, -1);
                return;
            }
            KeyCode::Right => {
                step_quote_panel(ui, 1);
                return;
            }
            _ => {}
        }
    }
    // Help is modal, because there is nothing to type at while reading it.
    if let Some(offset) = ui.help {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => ui.help = Some(offset.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => ui.help = Some(offset.saturating_add(1)),
            KeyCode::PageUp => ui.help = Some(offset.saturating_sub(10)),
            KeyCode::PageDown => ui.help = Some(offset.saturating_add(10)),
            KeyCode::Home => ui.help = Some(0),
            // A large offset the render clamps to the last page.
            KeyCode::End => ui.help = Some(u16::MAX),
            _ => ui.help = None,
        }
        return;
    }
    // The find bar owns the keyboard while open: it is a small query field over
    // the transcript, so a keystroke searches rather than composing a prompt.
    if let Some(mut find) = ui.find.take() {
        let count = find_matches(&ui.cache_text, &find.query).len();
        match key.code {
            KeyCode::Esc => return, // leave it closed
            // Enter / ↓ walk forward through hits, ↑ walks back, both wrapping.
            KeyCode::Enter | KeyCode::Down if count > 0 => {
                find.current = (find.current + 1) % count;
            }
            KeyCode::Up if count > 0 => {
                find.current = (find.current + count - 1) % count;
            }
            KeyCode::Backspace => {
                find.query.pop();
                find.current = 0;
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                find.query.push(c);
                find.current = 0;
            }
            _ => {}
        }
        // Scroll the focused hit into view.
        let matches = find_matches(&ui.cache_text, &find.query);
        if let Some(&line) = matches.get(find.current) {
            state.scroll = scroll_to_line(
                ui.view_total,
                ui.transcript.height as usize,
                ui.max_scroll,
                line,
            );
        }
        ui.find = Some(find);
        return;
    }
    // When the slash palette is open — a `/` prefix that still matches a command —
    // arrows/Enter/Esc drive it instead of the transcript or history. Once the
    // input no longer matches any command (e.g. "/why is …"), the palette is not
    // shown and these keys fall through, so a prompt that merely starts with `/`
    // can still be sent rather than having its Enter swallowed.
    let count = slash_matches(editor).len();
    if slash_active(editor) && count > 0 {
        // The selection wraps, so ↑ past the top lands on the last command and ↓
        // past the bottom on the first — a short list is quicker to reach either
        // way round than to walk back through.
        let cur = ui.slash_sel.min(count - 1);
        match key.code {
            KeyCode::Up => {
                ui.slash_sel = if cur == 0 { count - 1 } else { cur - 1 };
                return;
            }
            KeyCode::Down => {
                ui.slash_sel = if cur + 1 >= count { 0 } else { cur + 1 };
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
        // Mid-turn too: `submit` queues it rather than starting a second turn, which
        // is why this no longer refuses while busy.
        KeyCode::Enter => submit(ui, state, editor, turn, tx),
        KeyCode::Backspace | KeyCode::Char('w') if ctrl => editor.delete_word(),
        // Emacs-style line editing shortcuts, familiar from the shell.
        KeyCode::Char('a') if ctrl => editor.home(),
        KeyCode::Char('e') if ctrl => editor.end(),
        KeyCode::Char('u') if ctrl => editor.clear(),
        KeyCode::Char('k') if ctrl => editor.kill_to_end(),
        // Ctrl+P/Ctrl+N walk prompt history, the shell/emacs reflex — an
        // always-available alias for ↑/↓ that never collides with cursor movement.
        KeyCode::Char('p') if ctrl => editor.recall_prev(),
        KeyCode::Char('n') if ctrl => editor.recall_next(),
        // Ctrl+Z undoes the last edit to the prompt; Ctrl+Y replays it.
        KeyCode::Char('z') if ctrl => editor.undo(),
        KeyCode::Char('y') if ctrl => editor.redo(),
        // Ctrl+F opens the in-transcript find bar.
        KeyCode::Char('f') if ctrl => ui.find = Some(FindState::default()),
        // Ctrl+R opens saved conversations, the shell's reverse-history reflex.
        KeyCode::Char('r') if ctrl => open_sessions(ui),
        KeyCode::Backspace => editor.backspace(),
        // Word-wise movement with Alt/Ctrl held, char-wise otherwise.
        KeyCode::Left if ctrl || alt => editor.word_left(),
        KeyCode::Right if ctrl || alt => editor.word_right(),
        KeyCode::Left => editor.left(),
        KeyCode::Right => editor.right(),
        // Ctrl+Home/End jump the transcript to the top / bottom (latest); plain
        // Home/End stay with the input line.
        KeyCode::Home if ctrl => state.scroll = ui.max_scroll,
        KeyCode::End if ctrl => state.scroll = 0,
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
        // Alt+1..9 sends the Nth suggested follow-up without reaching for the
        // mouse — the keyboard route to what clicking a green chip does.
        KeyCode::Char(d @ '1'..='9') if alt => {
            let idx = (d as usize) - ('1' as usize);
            if idx < state.further.len().min(FURTHER_SHOWN) {
                click_chip(Chip::Further(idx), ui, state, editor, turn, tx);
            }
        }
        // An unhandled Ctrl combination is swallowed. Without this the fallback
        // below types its letter, so every unbound Ctrl+key silently inserted
        // text.
        KeyCode::Char(_) if ctrl => {}
        KeyCode::Char(c) => {
            // Starting to type the next message clears a stale one-off notice
            // ("Copied to clipboard", "Exported to …") that would otherwise hang
            // above the prompt.
            ui.notice = None;
            editor.insert_char(c);
        }
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
    if editor.is_blank() {
        return;
    }
    let typed = editor.text();
    let trimmed = typed.trim();
    // A command is the typed text alone — a folded paste is content, never a
    // command — so `exit`/`/…` are only interpreted when nothing is attached.
    if editor.attachments().is_empty() {
        // A bare `exit` / `quit` leaves, like a REPL — no leading slash needed.
        if trimmed.eq_ignore_ascii_case("exit") || trimmed.eq_ignore_ascii_case("quit") {
            ui.should_quit = true;
            return;
        }
        // A known `/command` runs locally; anything else starting with `/` is
        // just a prompt. Everything after the name is the command's argument.
        if trimmed.starts_with('/') {
            let (name, args) = split_command(trimmed);
            if let Some(key) = slash_lookup(name) {
                editor.clear();
                // Retry starts a turn, so it needs the turn/sender the other
                // commands do not — handle it here rather than in `exec_slash`.
                if key == "retry" {
                    retry_last(ui, state, turn, tx);
                } else {
                    exec_slash(key, args, ui, state);
                }
                return;
            }
        }
    }
    // The sent prompt folds the pasted blocks back in; history keeps only the
    // typed part, so ↑ recalls the question, not a wall of pasted log.
    let query = editor.submission_text();
    let remember = || {
        if !trimmed.is_empty() {
            super::history::append(trimmed);
        }
    };
    // Mid-turn, the prompt joins the queue instead of starting a second concurrent
    // turn on the same conversation. It goes out when this one is done — and a turn
    // is already running, so there is nothing to check about credentials here.
    if state.busy {
        if !trimmed.is_empty() {
            editor.push_history(trimmed);
        }
        remember();
        editor.clear();
        ui.notice = None;
        ui.selection = None;
        state.queued.push(query);
        return;
    }
    // A turn needs credentials. Signed out the chat is still useful — the reader
    // can read a resumed conversation and reach Settings — so the prompt says what
    // to do rather than the send failing somewhere deeper.
    if !crate::openapi::is_ready() {
        ui.notice = Some(t!("Ai.SignInToAsk").to_string());
        return;
    }
    if !trimmed.is_empty() {
        editor.push_history(trimmed);
    }
    remember();
    editor.clear();
    ui.notice = None;
    ui.selection = None;
    start_turn(query, state, turn, tx);
}

/// Send `query` as the next turn.
/// Re-ask the last question for a fresh answer (`/retry`).
///
/// Re-sends the most recent user prompt as a new turn — the honest reading of a
/// server that threads by message: it asks again rather than editing the answer
/// in place. Refuses while a turn is running or signed out, and says why.
fn retry_last(
    ui: &mut Ui,
    state: &mut ChatState,
    turn: &mut Option<JoinHandle<()>>,
    tx: &UnboundedSender<ChatEvent>,
) {
    if state.busy {
        ui.notice = Some(t!("Ai.RetryBusy").to_string());
        return;
    }
    if !crate::openapi::is_ready() {
        ui.notice = Some(t!("Ai.SignInToAsk").to_string());
        return;
    }
    let Some(last) = state
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .map(|m| m.text.clone())
    else {
        ui.notice = Some(t!("Ai.NothingToRetry").to_string());
        return;
    };
    ui.notice = None;
    ui.selection = None;
    start_turn(last, state, turn, tx);
}

fn start_turn(
    query: String,
    state: &mut ChatState,
    turn: &mut Option<JoinHandle<()>>,
    tx: &UnboundedSender<ChatEvent>,
) {
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
            // A keyboard open has no on-screen anchor, so the drawer falls back to
            // the right edge under the ticker.
            ui.quote_anchor_x = None;
            if args.is_empty() {
                match last_symbol(state) {
                    Some(symbol) => open_quote_panel(ui, symbol),
                    None => ui.notice = Some(t!("Ai.QuoteNoSymbol").to_string()),
                }
            } else {
                let symbol = args.trim().to_uppercase();
                // A bare ticker (no `.MARKET`) can't be looked up unambiguously,
                // so guide the reader rather than opening an empty panel.
                if super::answer::is_symbol(&symbol) {
                    open_quote_panel(ui, symbol);
                } else {
                    ui.notice = Some(t!("Ai.QuoteBadSymbol").to_string());
                }
            }
        }
        // Typed deliberately, so no second keypress to confirm — the row in
        // Settings is the one that can be hit by accident.
        "logout" => ui.pending = Some(Pending::SignOut),
        "login" => ui.pending = Some(Pending::SignIn),
        "copy" => {
            // In a chat, "copy" means the answer, not the whole log — that is what
            // `/export` is for. Copy the latest assistant answer; select-drag still
            // copies any span, and `/export` still writes the conversation.
            match last_answer(state) {
                Some(answer) => copy_with_notice(ui, Some(answer)),
                None => ui.notice = Some(t!("Ai.NothingToCopy").to_string()),
            }
        }
        "export" => {
            // Nothing said yet: don't drop an empty file in Downloads.
            if transcript_text(state).trim().is_empty() {
                ui.notice = Some(t!("Ai.NothingToExport").to_string());
                return;
            }
            ui.notice = Some(match export_conversation(state) {
                Ok(path) => t!("Ai.Exported", path = path.display().to_string()).to_string(),
                Err(_) => t!("Ai.ExportFailed").to_string(),
            });
        }
        "resume" => open_sessions(ui),
        "settings" => ui.switch(View::Settings),
        "agent" => switch_agent(args, ui, state),
        // A panel, not a message: help is something you consult and dismiss, and as
        // a transcript entry it could not be dismissed at all.
        "help" => ui.help = Some(0),
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
    ui.reset_render();
    ui.switch(View::Chat);
    // `switch` clears the status line, so the confirmation is set after it.
    ui.notice = Some(notice);
}

/// The `/help` message: the command list is derived from [`SLASH`] so it cannot
/// drift out of sync, followed by the key hints.
/// The help panel's rows: `(left, right)` pairs, with an empty left column for a
/// section heading and an empty pair for a blank row.
fn help_rows() -> Vec<(String, String)> {
    let mut rows = vec![(String::new(), t!("Ai.HelpCommands").to_string())];
    for c in &SLASH {
        let names = if c.aliases.is_empty() {
            c.name.to_string()
        } else {
            format!("{} {}", c.name, c.aliases.join(" "))
        };
        rows.push((names, t!(c.desc).to_string()));
    }
    rows.push((String::new(), String::new()));
    rows.push((String::new(), t!("Ai.HelpKeys").to_string()));
    for (keys, desc) in [
        ("Enter", "Ai.HelpSend"),
        ("Shift+Enter", "Ai.HelpNewline"),
        ("Tab", "Ai.HelpComplete"),
        ("↑ ↓  Ctrl+P N", "Ai.HelpHistory"),
        ("Ctrl+Z Y", "Ai.HelpUndo"),
        ("Shift+↑↓  PgUp PgDn", "Ai.HelpScroll"),
        ("Ctrl+Home End", "Ai.HelpJump"),
        ("Alt+← →", "Ai.HelpWordMove"),
        ("Ctrl+A E U K W", "Ai.HelpLineEdit"),
        ("Alt+1..9", "Ai.HelpFollowUp"),
        ("Ctrl+F", "Ai.HelpFind"),
        ("Ctrl+R", "Ai.HelpResume"),
        ("Esc", "Ai.HelpEscape"),
        ("drag / 2×3× click", "Ai.HelpSelect"),
        ("Ctrl+C ×2", "Ai.HelpQuit"),
    ] {
        rows.push((keys.to_string(), t!(desc).to_string()));
    }
    rows
}

/// Keyboard navigation for the Settings list view.
fn on_list_key(key: crossterm::event::KeyEvent, ui: &mut Ui, state: &mut ChatState) {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    // Moving the selection disarms a pending sign-out, so the row does not keep
    // its red "confirm" look after the reader has stepped off it.
    if matches!(
        key.code,
        KeyCode::Up | KeyCode::Down | KeyCode::Home | KeyCode::End | KeyCode::Char('j' | 'k')
    ) {
        ui.confirm_sign_out = false;
    }
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
        KeyCode::Home => ui.sel = 0,
        KeyCode::End => ui.sel = ui.row_count().saturating_sub(1),
        KeyCode::Enter => answer_selected(ui, state, turn, tx),
        KeyCode::Char('x' | 'X') => skip_question(ui, state, turn, tx),
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
        // The selection is in content coordinates, so wheel-scrolling no longer
        // moves it off its text — it stays put, as it would in any editor.
        MouseEventKind::ScrollUp => {
            scroll(ui, state, true);
        }
        MouseEventKind::ScrollDown => {
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
            // Whatever is on top, its `[Close]` closes it — in every view, which a
            // chip could not do: chips are only consulted in the chat.
            if ui.close_button.is_some_and(|r| hit(r, col, row)) {
                close_topmost(ui);
                return;
            }
            // Help is modal: a click anywhere dismisses it, like any key does.
            if ui.help.is_some() {
                ui.help = None;
                return;
            }
            // The panel floats over the transcript, and the symbol hit rects were
            // recorded from what is underneath it. A click while it is open
            // dismisses it rather than reaching through to a target the reader
            // cannot see.
            if let Some(symbol) = ui.quote_panel.clone() {
                // The arrows on the frame step to the neighbouring security; the WEB
                // hint opens the web page; anywhere else dismisses the panel rather
                // than reaching through to what it covers.
                if ui.prev_button.is_some_and(|r| hit(r, col, row)) {
                    step_quote_panel(ui, -1);
                } else if ui.next_button.is_some_and(|r| hit(r, col, row)) {
                    step_quote_panel(ui, 1);
                } else if ui.open_button.is_some_and(|r| hit(r, col, row)) {
                    open_url(&quote_web_url(&symbol));
                } else {
                    close_quote_panel(ui);
                }
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
                } else if let Some((chip, rect)) = ui
                    .chips
                    .iter()
                    .find(|(_, r)| hit(*r, col, row))
                    .map(|(c, r)| (c.clone(), *r))
                {
                    // A clicked security drops its drawer directly beneath it, so
                    // remember the column it sat at; the panel reads it when placing
                    // itself. Other chips leave the anchor alone.
                    if matches!(chip, Chip::Symbol(_)) {
                        ui.quote_anchor_x = Some(rect.x);
                    }
                    click_chip(chip, ui, state, editor, turn, tx);
                } else if hit(ui.transcript, col, row) {
                    // Count consecutive clicks on the same cell: 1 begins a drag
                    // selection, 2 selects the word, 3 the whole line.
                    let count = match ui.last_click {
                        Some((when, pcol, prow, prev))
                            if pcol == col && prow == row && when.elapsed() < DOUBLE_CLICK =>
                        {
                            prev + 1
                        }
                        _ => 1,
                    };
                    match count {
                        2 => select_word_at(ui, col, row),
                        n if n >= 3 => select_line_at(ui, row),
                        _ => {
                            let pos = content_at(ui, col, row);
                            ui.selection = Some((pos, pos));
                        }
                    }
                    ui.last_click = Some((std::time::Instant::now(), col, row, count));
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
                // Dragging past the top or bottom edge auto-scrolls the transcript,
                // so a selection can run beyond the visible page.
                let pos = drag_to_content(ui, state, m.column, m.row);
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

/// How often, at most, a burst of live quotes rebuilds the transcript to refresh
/// the inline price cards. Faster than the eye needs, and it spares a chart-heavy
/// transcript from re-rendering every chart on every tick.
const CARD_REFRESH: std::time::Duration = std::time::Duration::from_millis(400);

/// Two clicks within this window on the same cell count as a double-click.
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);

/// A second Ctrl+C within this window exits; after it, the first press is
/// forgotten so an idle armed state never turns a lone Ctrl+C into a quit.
const CTRL_C_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);

/// Select the whitespace-delimited word under `(col, row)` in the transcript, so
/// the release copies it — the terminal's usual double-click-to-select.
fn select_word_at(ui: &mut Ui, col: u16, row: u16) {
    let ri = row.saturating_sub(ui.transcript.y) as usize;
    let Some(text) = ui.visible_text.get(ri) else {
        return;
    };
    let target = col.saturating_sub(ui.transcript.x) as usize;
    let chars: Vec<char> = text.chars().collect();
    // Display start column of each char.
    let mut starts = Vec::with_capacity(chars.len());
    let mut c = 0usize;
    for &ch in &chars {
        starts.push(c);
        c += UnicodeWidthChar::width(ch).unwrap_or(0);
    }
    let Some(idx) = (0..chars.len()).find(|&k| {
        let w = UnicodeWidthChar::width(chars[k]).unwrap_or(0);
        starts[k] <= target && target < starts[k] + w.max(1)
    }) else {
        return;
    };
    if chars[idx].is_whitespace() {
        return;
    }
    let mut a = idx;
    while a > 0 && !chars[a - 1].is_whitespace() {
        a -= 1;
    }
    let mut b = idx;
    while b + 1 < chars.len() && !chars[b + 1].is_whitespace() {
        b += 1;
    }
    let ws = starts[a];
    let we = starts[b] + UnicodeWidthChar::width(chars[b]).unwrap_or(1);
    let line = ui.view_start + ri;
    ui.selection = Some((
        (line, u16::try_from(ws).unwrap_or(0)),
        (line, u16::try_from(we).unwrap_or(0)),
    ));
}

/// Select the whole visible transcript row (a triple-click).
fn select_line_at(ui: &mut Ui, row: u16) {
    let ri = row.saturating_sub(ui.transcript.y) as usize;
    let Some(text) = ui.visible_text.get(ri) else {
        return;
    };
    let w = UnicodeWidthStr::width(text.as_str());
    if w == 0 {
        ui.selection = None;
        return;
    }
    let line = ui.view_start + ri;
    ui.selection = Some(((line, 0), (line, u16::try_from(w).unwrap_or(u16::MAX))));
}

/// Map a mouse cell to a `(content-line, display-col)` position within the
/// transcript, using the view offset recorded at the last render.
fn content_at(ui: &Ui, col: u16, row: u16) -> (usize, u16) {
    let vis_row = row.saturating_sub(ui.transcript.y) as usize;
    let line = (ui.view_start + vis_row).min(ui.view_total.saturating_sub(1));
    (line, col.saturating_sub(ui.transcript.x))
}

/// Like [`content_at`], but for a drag: a pointer past the top or bottom edge
/// scrolls the transcript toward it, so the selection can extend beyond the page.
/// The view start is recomputed against the new scroll so the mapping is right
/// this frame rather than one behind.
fn drag_to_content(ui: &mut Ui, state: &mut ChatState, col: u16, row: u16) -> (usize, u16) {
    // No visible transcript to drag over (a fresh welcome screen never runs
    // `render_chat`, so `view_rows` stays 0): map without scrolling. Without this
    // the `clamp(top, top - 1)` below would panic (min > max).
    if ui.view_rows == 0 {
        return content_at(ui, col, row);
    }
    let tr = ui.transcript;
    let top = tr.y;
    let bottom = top + u16::try_from(ui.view_rows).unwrap_or(0);
    if row < top {
        state.scroll = state.scroll.saturating_add(top - row).min(ui.max_scroll);
    } else if row >= bottom {
        state.scroll = state.scroll.saturating_sub(row - bottom + 1);
    }
    let height = tr.height as usize;
    let scroll = state.scroll.min(ui.max_scroll) as usize;
    let start = ui.view_total.saturating_sub(scroll).saturating_sub(height);
    let vis_row = row.clamp(top, bottom.saturating_sub(1)).saturating_sub(top) as usize;
    let line = (start + vis_row).min(ui.view_total.saturating_sub(1));
    (line, col.saturating_sub(tr.x))
}

/// Scroll wheel: pans the transcript in Chat, moves the selection in a list.
fn scroll(ui: &mut Ui, state: &mut ChatState, up: bool) {
    // The help overlay scrolls itself when open, rather than the transcript
    // behind it.
    if let Some(offset) = ui.help {
        ui.help = Some(if up {
            offset.saturating_sub(1)
        } else {
            offset.saturating_add(1)
        });
        return;
    }
    // The quote panel and the sign-in panel are fixed overlays; scrolling should
    // not move the transcript hidden behind them.
    if ui.quote_panel.is_some() || ui.login.is_some() {
        return;
    }
    if ui.view == View::Chat {
        state.scroll = if up {
            state.scroll.saturating_add(3).min(ui.max_scroll)
        } else {
            state.scroll.saturating_sub(3)
        };
        // Back at the bottom, catch the inline cards up to any live quotes that
        // arrived while scrolled up (their rebuilds were suppressed to keep the
        // scroll smooth).
        if state.scroll == 0 && ui.cards_dirty {
            ui.cache_sig = 0;
            ui.cards_dirty = false;
            ui.cards_synced_at = Some(std::time::Instant::now());
        }
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

/// Open the floating quote panel for `symbol`, fetching its quote if the card is
/// not already cached.
///
/// The panel floats over the transcript rather than replacing it: the reader
/// clicked a symbol inside a sentence they were reading, and the answer around it
/// is the context for the number.
fn open_quote_panel(ui: &mut Ui, symbol: String) {
    if !ui.quotes.contains_key(&symbol) && crate::openapi::is_ready() {
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
    // And the session's shape. Refetched every open: the sparkline is the session
    // *so far*, so a path cached at first open would freeze the morning's shape
    // into the afternoon. The cached one still renders immediately; the fresh one
    // overwrites it when it lands.
    if crate::openapi::is_ready() {
        if let Some(tx) = ui.paths_tx.clone() {
            let wanted = symbol.clone();
            spawn_bg(async move {
                let path = super::quotes::intraday_path(&wanted, SPARK_W).await;
                if !path.is_empty() {
                    let _ = tx.send((wanted, path));
                }
            });
        }
        // The panel's richer figures (valuation, per-share, amplitude…). Refetched
        // every open like the sparkline: they move through the session, and the
        // panel is only ever showing one security's worth at a time.
        if let Some(tx) = ui.details_tx.clone() {
            let wanted = symbol.clone();
            spawn_bg(async move {
                if let Some(detail) = super::quotes::fetch_detail(&wanted).await {
                    let _ = tx.send((wanted, detail));
                }
            });
        }
    }
    ui.quote_panel = Some(symbol);
}

/// Close whatever is on top: the overlays in the order they are drawn in, and
/// failing that the list view itself, which is the only other thing with a
/// `[Close]`.
fn close_topmost(ui: &mut Ui) {
    if ui.login.is_some() {
        ui.login = None;
        ui.notice = Some(t!("Ai.LoginCancelled").to_string());
    } else if ui.help.is_some() {
        ui.help = None;
    } else if ui.quote_panel.is_some() {
        close_quote_panel(ui);
    } else {
        ui.switch(View::Chat);
    }
}

/// Close the panel. The subscription stays: the ticker and the inline prices are
/// reading the same stream, and dropping it here would freeze them.
fn close_quote_panel(ui: &mut Ui) {
    ui.quote_panel = None;
}

/// Step the open panel to the previous (`-1`) or next (`+1`) security the
/// conversation named.
///
/// The tape is that ordered list, so the reader flips through the same securities
/// the ticker rotates. The walk wraps at the ends. A no-op when the current one is
/// not on the tape — a bare `/quote` for something never mentioned — or there is
/// nothing else to move to.
fn step_quote_panel(ui: &mut Ui, delta: isize) {
    let Some(current) = ui.quote_panel.clone() else {
        return;
    };
    let Some(pos) = ui.tape.iter().position(|s| *s == current) else {
        return;
    };
    let len = ui.tape.len();
    if len < 2 {
        return;
    }
    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    let next = (pos as isize + delta).rem_euclid(len as isize) as usize;
    // Page the ticker only when the next security is off screen, so it stays put
    // while stepping between two entries already in view and slides just enough to
    // bring the next one on — where the drawer then anchors under it.
    let visible = (0..ui.tape_drawn).any(|k| (ui.tape_at + k) % len == next);
    if !visible {
        ui.tape_at = next;
    }
    let symbol = ui.tape[next].clone();
    open_quote_panel(ui, symbol);
}

fn subscribe_quote(symbol: &str) {
    if !crate::openapi::is_ready() {
        return;
    }
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
    if !crate::openapi::is_ready() {
        // The list lives behind the account, so there is nothing to open yet.
        ui.notice = Some(t!("Ai.SignInToAsk").to_string());
        return;
    }
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
    if !q.select(ui.sel) {
        return;
    }
    q.qi += 1;
    ui.sel = 0;
    if q.qi >= q.questions.len() {
        let qs = ui.question.take().expect("question present");
        // Answering resumes the paused run, which needs credentials. Signed out
        // (sign-out clears the ids the resume addresses) the send would drop
        // silently and the conversation would stall with no explanation, so say
        // what to do — as the free-text prompt path does.
        if !submit_answers(&qs, state, turn, tx) {
            ui.notice = Some(t!("Ai.SignInToAsk").to_string());
        }
        ui.view = View::Chat;
    }
}

/// Skip the current ask-human interaction using the same sentinel as the web
/// and GPUI clients, then continue with the next interaction (if any).
fn skip_question(
    ui: &mut Ui,
    state: &mut ChatState,
    turn: &mut Option<JoinHandle<()>>,
    tx: &UnboundedSender<ChatEvent>,
) {
    let Some(q) = ui.question.as_mut() else {
        return;
    };
    let Some((interrupt_id, _, _, _)) = q.targets.get(q.qi).cloned() else {
        return;
    };
    // Authorization decisions must be explicit; skip applies only to ask_human.
    if q.targets
        .get(q.qi)
        .is_some_and(|(_, _, wire, _)| wire.is_some())
    {
        return;
    }
    q.answers.insert(
        interrupt_id.clone(),
        HashMap::from([
            ("_skipped".into(), "true".into()),
            (
                "_note".into(),
                "Human ignored this question and may not want to answer".into(),
            ),
        ]),
    );
    q.summaries.push(t!("Ai.SkipQuestion").to_string());
    while q.qi < q.targets.len() && q.targets[q.qi].0 == interrupt_id {
        q.qi += 1;
    }
    ui.sel = 0;
    if q.qi >= q.questions.len() {
        let qs = ui.question.take().expect("question present");
        if !submit_answers(&qs, state, turn, tx) {
            ui.notice = Some(t!("Ai.SignInToAsk").to_string());
        }
        ui.view = View::Chat;
    }
}

/// Returns whether the answers were dispatched; `false` means the paused run
/// could not be addressed (no credentials / conversation id), so the caller can
/// tell the reader rather than dropping the answers on the floor.
fn submit_answers(
    qs: &QuestionState,
    state: &mut ChatState,
    turn: &mut Option<JoinHandle<()>>,
    tx: &UnboundedSender<ChatEvent>,
) -> bool {
    let (Some(chat_uid), Some(message_id)) = (state.chat_uid.clone(), state.message_id.clone())
    else {
        return false;
    };
    let answers = qs.answers.clone();
    let summary = qs.summaries.join(", ");
    let req = ConversationRequest::Continue {
        agent_uid: state.agent_uid.clone(),
        chat_uid,
        message_id,
        answers,
    };
    state.apply(ChatEvent::UserPrompt(summary));
    state.pending_interrupt = None;
    *turn = Some(runtime::spawn_turn(req, tx.clone()));
    true
}

/// Handle a click on a Chat meta chip: open a reference URL, or send a
/// suggested follow-up as the next prompt.
fn click_chip(
    chip: Chip,
    ui: &mut Ui,
    state: &mut ChatState,
    editor: &mut Editor,
    turn: &mut Option<JoinHandle<()>>,
    tx: &UnboundedSender<ChatEvent>,
) {
    match chip {
        // The example is the quickest way in: clicking it sends it, rather than
        // leaving the reader to retype what is already on screen.
        Chip::Sample(key) => {
            editor.set_text(&t!(key));
            submit(ui, state, editor, turn, tx);
        }
        Chip::Brand => open_url(AI_WEB_URL),
        Chip::Sessions => open_sessions(ui),
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
            if !crate::openapi::is_ready() {
                ui.notice = Some(t!("Ai.SignInToAsk").to_string());
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
/// The most recent assistant answer, for `/copy`. `None` before the first one.
fn last_answer(state: &ChatState) -> Option<String> {
    state
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant)
        .map(|m| m.text.clone())
}

fn transcript_text(state: &ChatState) -> String {
    state
        .messages
        .iter()
        // Only the conversation itself: system notices and tool-trace lines are
        // UI, not something to copy (a tool line is a name, not a message, and
        // would otherwise be labelled as if the assistant had said it).
        .filter(|m| matches!(m.role, Role::User | Role::Assistant))
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
    // A title slug makes the file findable in Downloads; the timestamp keeps two
    // exports of the same conversation from colliding.
    let slug = state
        .title
        .as_deref()
        .map(export_slug)
        .filter(|s| !s.is_empty());
    let name = match &slug {
        Some(slug) => format!("longbridge-ai-{slug}-{}.md", now_secs()),
        None => format!("longbridge-ai-{}.md", now_secs()),
    };
    let path = dir.join(name);
    let heading = state
        .title
        .clone()
        .unwrap_or_else(|| t!("Ai.Title").to_string());
    let mut body = format!("# {heading}\n\n");
    for m in &state.messages {
        let label = match m.role {
            Role::User => t!("Ai.You"),
            Role::Assistant => t!("Ai.Assistant"),
            // Tool lines are UI trace, not conversation; an export is the
            // conversation.
            Role::System | Role::Alert | Role::Tool => continue,
        };
        let _ = write!(body, "**{label}:**\n\n{}\n\n", m.text);
    }
    std::fs::write(&path, body)?;
    Ok(path)
}

/// A filesystem-safe slug from a conversation title: lowercase ASCII words
/// joined by `-`, capped so the filename stays reasonable.
fn export_slug(title: &str) -> String {
    let mut out = String::new();
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.extend(ch.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

/// Copy `text` and set a status notice reflecting the outcome.
fn copy_with_notice(ui: &mut Ui, text: Option<String>) {
    ui.notice = Some(
        match text {
            // Distinguish "nothing selected" from a clipboard write that failed —
            // the second is not the reader's doing and a different message.
            Some(t) if !t.trim().is_empty() => {
                if copy_to_clipboard(&t) {
                    t!("Ai.Copied")
                } else {
                    t!("Ai.CopyFailed")
                }
            }
            _ => t!("Ai.NothingToCopy"),
        }
        .to_string(),
    );
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

fn view(f: &mut ratatui::Frame, ui: &mut Ui, state: &mut ChatState, editor: &Editor) {
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
    // A running turn gets a row of its own so its spinner, timer and cancel
    // button cannot be hidden by a notice — and the notice cannot be hidden by
    // it. Only while busy, so idle chrome stays one row on a short terminal.
    let has_turn = is_chat && state.busy;
    // Idle, a blank row sits above the boxed prompt to lift it off the
    // transcript's last line. While a turn runs, the status row already separates
    // them, so the box drops that blank and hugs the status — the extra gap read
    // as too much empty space between the spinner and the prompt.
    let footer_h = if is_chat {
        let extra = if has_turn { 2 } else { 3 };
        (editor.lines().len() as u16 + extra).clamp(if has_turn { 3 } else { 4 }, 9)
    } else {
        4
    };
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
    // A list view's hint gets a blank row above it, so it does not read as one more
    // row of the list.
    if !is_chat {
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
    if !is_chat {
        idx += 1; // the spacer
    }
    let status = chunks[idx];
    idx += 1;
    let footer = chunks[idx];

    // Chips are recorded by both the transcript (references) and the meta panel
    // (follow-ups), so the list is cleared once per frame rather than by each.
    ui.chips.clear();
    // Re-recorded by whatever draws a `[Close]` this frame; a stale rect would
    // otherwise swallow clicks where nothing is any more.
    ui.close_button = None;
    // The Chat view is chrome-free and keeps the title bar. The others carry their
    // own name, so they take the title bar's rows too rather than sitting under a
    // second title.
    let full = Rect {
        height: title.height + body.height,
        ..title
    };
    match ui.view {
        // The Question drawer is an overlay, not a view: the transcript it is
        // asking about stays behind it.
        View::Chat | View::Question => {
            render_title(f, title, ui, state);
            render_chat(f, body, ui, state);
        }
        View::Sessions => {
            let inner = render_view_header(f, full, ui, "Ai.TabSessions");
            render_sessions(f, inner, ui);
        }
        View::Settings => {
            let inner = render_view_header(f, full, ui, "Ai.TabSettings");
            render_settings(f, inner, ui);
        }
    }
    if ui.view == View::Question {
        render_question(f, body, ui);
    }
    if let Some(meta) = meta {
        render_chips(f, meta, ui, state);
    }
    if let Some(row) = turn_row {
        render_turn_status(f, row, ui, state);
    } else {
        ui.stop_button = None;
    }
    render_status(f, status, ui, state, editor);
    render_footer(f, footer, ui, editor, has_turn);
    // The command palette hangs directly off the prompt box, drawn last so it
    // floats over the status row and any chrome between the transcript and the
    // prompt rather than being anchored to the transcript's foot with a gap.
    if ui.view == View::Chat {
        render_slash_dropdown(f, body, footer, ui, editor);
    } else {
        ui.slash_rows.clear();
    }
    // Last, so they float over whatever is underneath. The sign-in panel outranks
    // the quote panel: it is a step the reader is in the middle of.
    render_quote_panel(f, body, ui);
    render_help(f, body, ui);
    if ui.login.is_some() {
        ui.animating = true;
        let spin = ui.tick as usize;
        render_login_panel(f, body, ui, spin);
    }
}

/// The live-quote stream, boxed so it can be replaced when the reader signs in
/// from inside the chat.
pub type QuoteStream =
    std::pin::Pin<Box<dyn tokio_stream::Stream<Item = longbridge::quote::PushEvent> + Send>>;

/// Draw the sign-in panel while a device authorization is outstanding.
///
/// The reader authorizes in a browser and comes back to a chat that is still
/// here: no torn-down screen, no shell prompt, no relaunch.
fn render_login_panel(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui, spin: usize) {
    use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding};

    let Some(login) = &ui.login else {
        return;
    };
    let dim = Style::default().fg(Color::DarkGray);
    let inner_w = 64usize.min(area.width.saturating_sub(8) as usize).max(20);
    let mut body = vec![Line::from(Span::styled(
        if login.browser_opened {
            t!("Ai.LoginBrowserOpened").to_string()
        } else {
            t!("Ai.LoginOpenUrl").to_string()
        },
        Style::default().fg(Color::Gray),
    ))];
    body.push(Line::from(""));
    // The URL is long and the reader may need to type it, so it wraps rather than
    // being truncated — a clipped URL is useless.
    for row in wrap(&login.url, inner_w) {
        body.push(Line::from(Span::styled(
            row,
            Style::default().fg(Color::Blue),
        )));
    }
    if !login.code.is_empty() {
        body.push(Line::from(""));
        body.push(Line::from(vec![
            Span::styled(format!("{}  ", t!("Ai.LoginCode")), dim),
            Span::styled(
                login.code.clone(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    body.push(Line::from(""));
    body.push(Line::from(vec![
        Span::styled(format!("{} ", SPINNER[spin % SPINNER.len()]), dim),
        Span::styled(t!("Ai.LoginWaiting").to_string(), dim),
    ]));
    let width = (inner_w as u16 + 6).min(area.width);
    let height = (body.len() as u16 + 2).min(area.height);
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    f.render_widget(Clear, rect);
    blank_straddling_glyphs(f.buffer_mut(), rect, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .padding(Padding::horizontal(2))
        .title(Line::from(Span::styled(
            format!(" {} ", t!("Ai.SignIn")),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )))
        .title_bottom(Line::from(Span::styled(close_label(), dim)).right_aligned());
    f.render_widget(Paragraph::new(Text::from(body)).block(block), rect);
    ui.close_button = Some(close_rect(rect));
}

/// Render the help overlay: the commands and the keys, in two columns.
///
/// A panel rather than a message in the transcript, which is what it used to be —
/// help you cannot dismiss is help that has taken your conversation hostage. It
/// scrolls when it does not fit, because it is longer than a short terminal.
fn render_help(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui) {
    let Some(offset) = ui.help else {
        return;
    };
    let rows = help_rows();
    let left_w = rows.iter().map(|(l, _)| l.width()).max().unwrap_or(0);
    let right_w = rows.iter().map(|(_, r)| r.width()).max().unwrap_or(0);
    let inner_w = left_w + 2 + right_w;
    let width = (inner_w + 6).min(area.width as usize) as u16;
    let height = (rows.len() as u16 + 2).min(area.height);
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    f.render_widget(Clear, rect);
    blank_straddling_glyphs(f.buffer_mut(), rect, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(Color::DarkGray))
        .padding(Padding::horizontal(2))
        .title(Span::styled(
            format!(" {} ", t!("Ai.Help")),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(
            Line::from(Span::styled(
                // `↕` only when there is more than fits, so the reader knows to look
                // rather than assuming they have seen it all.
                if rows.len() + 2 > height as usize {
                    format!(" ↕ {}", close_label())
                } else {
                    close_label()
                },
                Style::default().fg(Color::DarkGray),
            ))
            .right_aligned(),
        );
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    // Clamp here rather than on the keypress: the height is only known now.
    let fit = inner.height as usize;
    let max_offset = rows.len().saturating_sub(fit) as u16;
    let offset = offset.min(max_offset);
    ui.help = Some(offset);
    ui.close_button = Some(close_rect(rect));
    let lines: Vec<Line> = rows
        .into_iter()
        .skip(offset as usize)
        .take(fit)
        .map(|(left, right)| {
            if left.is_empty() {
                // A heading, or a blank row.
                Line::from(Span::styled(
                    right,
                    Style::default()
                        .fg(Color::Gray)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                let pad = " ".repeat(left_w + 2 - left.width());
                Line::from(vec![
                    Span::styled(left, Style::default().fg(Color::Cyan)),
                    Span::raw(pad),
                    Span::styled(right, Style::default().fg(Color::Gray)),
                ])
            }
        })
        .collect();
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

/// Draw the quote panel, if one is open.
///
/// A drawer, not a centred popup: it hangs from the top of the transcript, under
/// the ticker the securities live in, and drops down only as far as its content —
/// the reader clicked a stock up there and its detail slides down from the same
/// place, leaving the sentence they were reading in view below. Right-aligned to
/// sit under the ticker. Square corners and a title inset in the top border, after
/// grok-build's overlays; the chrome is a `WEB` button and nothing else, so every
/// row inside carries data.
fn render_quote_panel(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui) {
    use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding};

    // The tallest the drawer's content gets — name, price, sparkline, the six core
    // figures and the detail block — so it opens at full height and fills in rather
    // than growing (and flashing) as the quote and its detail arrive.
    const DRAWER_INNER: usize = 13;

    let Some(symbol) = ui.quote_panel.clone() else {
        ui.open_button = None;
        ui.prev_button = None;
        ui.next_button = None;
        return;
    };
    let path = ui.paths.get(&symbol).map_or(&[][..], Vec::as_slice);
    let detail = ui.details.get(&symbol);
    let mut body = match ui.quotes.get(&symbol) {
        Some(card) => card_lines(card, path, detail),
        None => vec![Line::from(Span::styled(
            t!("Ai.QuoteLoading").to_string(),
            Style::default().fg(Color::DarkGray),
        ))],
    };
    let title = format!(" ${symbol} ");
    let web = format!(" {WEB_ICON} ");
    let content_w = body.iter().map(line_width).max().unwrap_or(0);
    let chrome = UnicodeWidthStr::width(title.as_str()) + UnicodeWidthStr::width(web.as_str()) + 4;
    let width = (content_w + 6).max(chrome).min(area.width as usize) as u16;
    // Open at the reserved height and pad to it, rather than growing from a short
    // "loading" box to a tall one as the quote and then its detail land — the
    // resize flashed; a fixed frame that fills in does not.
    let inner_h = DRAWER_INNER.min((area.height as usize).saturating_sub(2));
    while body.len() < inner_h {
        body.push(Line::from(""));
    }
    let height = (body.len() as u16 + 2).min(area.height);
    // A drawer: it drops from the top of the transcript rather than floating in the
    // middle over the answer. It anchors under the security's own ticker entry —
    // found among this frame's chips, topmost occurrence so the ticker wins over an
    // inline mention — which keeps the drawer and its highlighted tab lined up as
    // the reader steps through. Failing that (rotated out, or a keyboard open), the
    // remembered click column, then the right edge. Clamped so it never spills off.
    let right_edge = area.x + area.width.saturating_sub(width);
    let anchor = ui
        .chips
        .iter()
        .filter_map(|(c, r)| match c {
            Chip::Symbol(s) if s == &symbol => Some(*r),
            _ => None,
        })
        .min_by_key(|r| r.y)
        .map(|r| r.x)
        .or(ui.quote_anchor_x);
    let x = anchor.map_or(right_edge, |ax| ax.min(right_edge));
    let rect = Rect {
        x,
        y: area.y,
        width,
        height,
    };
    f.render_widget(Clear, rect);
    // A wide glyph in the transcript can straddle the panel's edge: its first cell
    // outside, its second under the border. Overwriting only the half inside left
    // the terminal drawing the whole glyph across the frame, which is what made
    // the border look broken. The cell outside has to go too.
    blank_straddling_glyphs(f.buffer_mut(), rect, area);
    // The button's geometry comes first so the live dot cannot collide with it, and
    // so the icon can brighten under the pointer.
    let web_w = UnicodeWidthStr::width(web.as_str()) as u16;
    let open_rect = Rect {
        x: rect.x + rect.width.saturating_sub(web_w + 1),
        y: rect.y,
        width: web_w,
        height: 1,
    };
    let web_fg = if hovering(ui, open_rect) {
        Color::White
    } else {
        Color::Blue
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(Color::DarkGray))
        .padding(Padding::horizontal(2))
        .title(Line::from(vec![
            Span::styled(
                title,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            // The dot says the number is streaming, so a price that sits still is a
            // quiet market rather than a stale panel.
            Span::styled("●", Style::default().fg(Color::Green)),
        ]))
        .title_top(
            Line::from(Span::styled(
                web,
                Style::default().fg(web_fg).add_modifier(Modifier::BOLD),
            ))
            .right_aligned(),
        );
    f.render_widget(Paragraph::new(Text::from(body)).block(block), rect);
    // No `[Close]` label: a click anywhere outside the panel (or Esc) dismisses it,
    // so the button was clutter. The whole-view close handler reads `quote_panel`
    // directly, not this rect.
    ui.close_button = None;
    // The web page has the chart and the filings this panel cannot hold, so the way
    // out to it is a button on the frame rather than a line of instructions.
    ui.open_button = Some(open_rect);
    // When the conversation named more than one security, `‹`/`›` on the side
    // borders step through them without leaving the panel — the click twin of the
    // arrow keys. Drawn only then, and only when this security is on the tape, so
    // the arrows never promise a move that goes nowhere.
    let steppable = ui.tape.len() > 1 && ui.tape.contains(&symbol);
    if steppable && rect.height >= 3 {
        let mid_y = rect.y + rect.height / 2;
        let arrow = |buf: &mut ratatui::buffer::Buffer, x: u16, glyph: &str, hovered: bool| {
            if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, mid_y)) {
                cell.set_symbol(glyph);
                let fg = if hovered { Color::White } else { Color::Cyan };
                cell.set_style(Style::default().fg(fg).add_modifier(Modifier::BOLD));
            }
        };
        let right_x = rect.x + rect.width.saturating_sub(1);
        let prev_rect = Rect {
            x: rect.x,
            y: mid_y,
            width: 1,
            height: 1,
        };
        let next_rect = Rect {
            x: right_x,
            y: mid_y,
            width: 1,
            height: 1,
        };
        arrow(f.buffer_mut(), rect.x, "‹", hovering(ui, prev_rect));
        arrow(f.buffer_mut(), right_x, "›", hovering(ui, next_rect));
        ui.prev_button = Some(prev_rect);
        ui.next_button = Some(next_rect);
    } else {
        ui.prev_button = None;
        ui.next_button = None;
    }
}

/// Blank any double-width glyph that straddles `rect`'s left or right edge.
///
/// Ratatui writes the cells inside the rect, but a wide glyph occupies two: if its
/// first cell is outside and its second inside, the terminal still draws the whole
/// glyph — across the frame just drawn.
fn blank_straddling_glyphs(buf: &mut ratatui::buffer::Buffer, rect: Rect, area: Rect) {
    use ratatui::layout::Position;

    for y in rect.y..rect.y.saturating_add(rect.height) {
        if rect.x > area.x {
            let left = Position::new(rect.x - 1, y);
            if buf
                .cell(left)
                .is_some_and(|c| UnicodeWidthStr::width(c.symbol()) > 1)
            {
                if let Some(cell) = buf.cell_mut(left) {
                    cell.set_symbol(" ");
                }
            }
        }
        // And on the right: a glyph whose first cell was the panel's last column
        // leaves an orphaned continuation cell just outside it.
        let right = Position::new(rect.x.saturating_add(rect.width), y);
        if right.x < area.x.saturating_add(area.width)
            && buf.cell(right).is_some_and(|c| c.symbol().is_empty())
        {
            if let Some(cell) = buf.cell_mut(right) {
                cell.set_symbol(" ");
            }
        }
    }
}

/// Display width of a rendered line.
fn line_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

/// Longbridge AI on the web. The badge in the title bar opens it — the same
/// conversations, with the widgets a terminal can only describe.
const AI_WEB_URL: &str = "https://longbridge.com/ai";

/// The security's page on the web, where the chart and the filings are.
fn quote_web_url(symbol: &str) -> String {
    format!("https://longbridge.com/quote/{symbol}")
}

/// The "open on the web" affordance on the quote drawer's frame. An outward arrow
/// rather than the word `WEB`: it reads as "this takes you out" in any locale and
/// costs one column instead of five on a drawer whose every row is data.
const WEB_ICON: &str = "↗";

/// The `[Close]` a panel or a list view carries. Bracketed like the `[stop]`
/// button, because that is what reads as "click me" in a terminal.
fn close_label() -> String {
    format!(" [{}] ", t!("Ai.CloseButton"))
}

/// Where that button lands on a panel's bottom border, right-aligned.
fn close_rect(panel: Rect) -> Rect {
    let w = close_label().width() as u16;
    Rect {
        x: panel.x + panel.width.saturating_sub(w + 1),
        y: panel.y + panel.height.saturating_sub(1),
        width: w,
        height: 1,
    }
}

fn render_title(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui, state: &ChatState) {
    // The brand badge, and nothing else by default. The server-generated
    // conversation title lived here, but it is a label for picking a chat out of
    // a list — which is where it is shown — not something worth a permanent row
    // in the chat you are already reading.
    let badge = format!(" {} ", t!("Ai.Title"));
    let badge_rect = Rect {
        x: area.x,
        y: area.y,
        width: badge.width() as u16,
        height: 1,
    };
    let mut badge_style = Style::default()
        .fg(Color::Black)
        .bg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    // The badge is a link out to the web; underline it under the pointer, the one
    // hover cue a cell already filled with a background can still show.
    if hovering(ui, badge_rect) {
        badge_style = badge_style.add_modifier(Modifier::UNDERLINED);
    }
    let mut left = vec![Span::styled(badge, badge_style)];
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
    // Reaching an earlier conversation was `/resume` and nothing else, which only
    // helps a reader who already knows it exists. A glyph, not the word: the view it
    // opens is titled, and the badge beside it is already carrying text.
    let sessions = format!("   {} ", t!("Ai.ChatsButton"));
    let sessions_x = area.x + left.iter().map(|s| s.content.width()).sum::<usize>() as u16;
    let sessions_rect = Rect {
        x: sessions_x,
        y: area.y,
        width: sessions.width() as u16,
        height: 1,
    };
    left.push(Span::styled(
        sessions.clone(),
        if hovering(ui, sessions_rect) {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::DarkGray)
        },
    ));
    ui.chips.push((Chip::Sessions, sessions_rect));
    let left_w: usize = left.iter().map(|s| s.content.width()).sum();
    // The badge is a link: the same conversations are on the web, with the widgets
    // a terminal can only describe.
    let badge_w = left.first().map_or(0, |s| s.content.width()) as u16;
    ui.chips.push((
        Chip::Brand,
        Rect {
            x: area.x,
            y: area.y,
            width: badge_w,
            height: 1,
        },
    ));

    // The rest of the row is the ticker: the securities this conversation has
    // mentioned, with their quotes. It is the one row of chrome that was carrying
    // nothing, and a trader reading about a stock wants its price in view.
    // Open, the quotes speak for themselves and the control is just a way out —
    // `✕` pushes them off. Collapsed, it is the only thing saying they exist, so it
    // says so: `QUOTES +`. No brackets either way; the row is too valuable.
    let toggle = if ui.tape.is_empty() {
        String::new()
    } else if super::settings::tape() {
        " ✕ ".to_string()
    } else {
        format!(" {} + ", t!("Ai.TapeToggle"))
    };
    let toggle_w = toggle.width();
    let room = (area.width as usize).saturating_sub(left_w + toggle_w);
    // The ticker is right-aligned against the control, so its first column depends
    // on how wide it turns out to be: measured first, then placed. Its span is
    // capped to about three-fifths of the row, though — a long watchlist otherwise
    // sprawled left across most of the title bar. The cap only bites when there is
    // width to spare: its floor holds a page of a couple of entries, so a narrow
    // terminal keeps near-full room and only a wide one is trimmed. Beyond the cap
    // the ticker rotates; the spacer below still uses the full room, so the toggle
    // stays pinned to the right.
    let cap = room.min((area.width as usize * 3 / 5).max(48));
    let tape = if super::settings::tape() && !ui.tape.is_empty() {
        let measured = tape_spans(ui, area, None, cap);
        let w: usize = measured.iter().map(|s| s.content.width()).sum();
        let x = area.x + (left_w + room.saturating_sub(w)) as u16;
        tape_spans(ui, area, Some(x), cap)
    } else {
        Vec::new()
    };
    let tape_w: usize = tape.iter().map(|s| s.content.width()).sum();
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(room.saturating_sub(tape_w))));
    spans.extend(tape);
    // The toggle sits at the end of the row, where it is out of the ticker's way.
    let toggle_x = area.x + area.width.saturating_sub(toggle_w as u16);
    let toggle_rect = Rect {
        x: toggle_x,
        y: area.y,
        width: toggle_w as u16,
        height: 1,
    };
    let toggle_style = if !ui.tape.is_empty() && hovering(ui, toggle_rect) {
        Style::default().fg(Color::White)
    } else if super::settings::tape() {
        Style::default().fg(Color::DarkGray)
    } else {
        // Collapsed, it is the only thing saying the ticker is there at all, so it
        // is not dim.
        Style::default().fg(Color::Gray)
    };
    spans.push(Span::styled(toggle, toggle_style));
    if !ui.tape.is_empty() {
        ui.chips.push((Chip::Tape, toggle_rect));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The ticker's spans, rotated to fit `room` columns.
///
/// Rotation is by whole securities rather than by column: a price sliding through
/// half a symbol is unreadable, and a trader glancing up needs to take a whole
/// entry in at once. The rotation advances only when there is more to show than
/// fits, so a short list sits still.
/// How long a page of the ticker stays put. Long enough to read a handful of
/// symbols with their prices, which a step a second was not.
const TAPE_DWELL: std::time::Duration = std::time::Duration::from_secs(6);

fn tape_spans(ui: &mut Ui, area: Rect, start_x: Option<u16>, room: usize) -> Vec<Span<'static>> {
    // A dim middle dot separates entries: it reads as a deliberate ticker rather
    // than a sparse run of blanks, and packs the row tighter than the old wide gap.
    const GAP: &str = " · ";
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
    let mut hits: Vec<(Chip, Rect)> = Vec::new();
    let mut used = 0usize;
    let mut drawn = 0usize;
    for i in 0..entries.len() {
        let (symbol, price, color) = &entries[(ui.tape_at + i) % entries.len()];
        let w = symbol.width() + price.width() + if used == 0 { 0 } else { GAP.width() };
        if used + w > room {
            break;
        }
        if used > 0 {
            spans.push(Span::styled(GAP, Style::default().fg(Color::DarkGray)));
        }
        // Every entry is a button. `None` is the measuring pass: the ticker is
        // right-aligned, so its first column is not known until its width is.
        let rect = start_x.map(|x| {
            let gap = if used == 0 { 0 } else { GAP.width() };
            Rect {
                x: x.saturating_add((used + gap) as u16),
                y: area.y,
                width: (symbol.width() + price.width()) as u16,
                height: 1,
            }
        });
        if let Some(rect) = rect {
            hits.push((Chip::Symbol(symbol.clone()), rect));
        }
        // The entry the drawer is showing reads as a selected tab; a hovered one
        // gets the lighter cue every clickable does. Selected wins over hover.
        let selected = ui.quote_panel.as_deref() == Some(symbol.as_str());
        let bg = if selected {
            Some(TAB_BG)
        } else if rect.is_some_and(|r| hovering(ui, r)) {
            Some(HOVER_BG)
        } else {
            None
        };
        let mut symbol_style =
            Style::default().fg(if selected { Color::White } else { Color::Gray });
        if selected {
            symbol_style = symbol_style.add_modifier(Modifier::BOLD);
        }
        if let Some(bg) = bg {
            symbol_style = symbol_style.bg(bg);
        }
        spans.push(Span::styled(symbol.clone(), symbol_style));
        if !price.is_empty() {
            let mut price_style = Style::default().fg(*color);
            if let Some(bg) = bg {
                price_style = price_style.bg(bg);
            }
            spans.push(Span::styled(price.clone(), price_style));
        }
        used += w;
        drawn += 1;
    }
    ui.chips.extend(hits);
    if start_x.is_some() {
        ui.tape_drawn = drawn;
    }
    // The frame timer has to keep running for the ticker to advance. Only the
    // placing pass may advance it — the measuring pass runs on the same frame. And
    // not while the drawer is open: it is anchored to one entry and the reader is
    // stepping through them by hand, so a rotation underneath would fight that.
    if rotating && start_x.is_some() && ui.quote_panel.is_none() {
        ui.animating = true;
        let now = std::time::Instant::now();
        // A first frame starts the clock rather than advancing: the reader has not
        // seen this page yet.
        let due = ui
            .tape_shown_at
            .is_some_and(|shown| now.duration_since(shown) >= TAPE_DWELL);
        ui.tape_shown_at.get_or_insert(now);
        if due {
            ui.tape_shown_at = Some(now);
            // A page at a time, not an entry: a step a second shuffled every symbol
            // along the row just as the reader started on one. A whole new set, held
            // long enough to read, is something you can follow.
            let page = drawn.max(1);
            ui.tape_at = if ui.tape_at + page >= entries.len() {
                0
            } else {
                ui.tape_at + page
            };
        }
    }
    spans
}

/// Header for a `/`-opened view: a bold name badge and an "Esc to go back"
/// hint. Returns the remaining area below it for the view body.
fn render_view_header(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui, label_key: &str) -> Rect {
    let [top, rest] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    // The way out is a button as well as a key: the view is opened by clicking, so
    // it should be closable the same way.
    let close = format!("[{}]", t!("Ai.CloseButton"));
    let close_w = close.width() as u16;
    let [title_rect, close_rect] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(close_w)]).areas(top);
    f.render_widget(
        Paragraph::new(Span::styled(
            format!(" {} ", t!(label_key)),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        title_rect,
    );
    f.render_widget(
        Paragraph::new(Span::styled(
            close,
            if hovering(ui, close_rect) {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        )),
        close_rect,
    );
    ui.close_button = Some(close_rect);
    rest
}

fn render_chat(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui, state: &mut ChatState) {
    ui.transcript = area;
    // Before the first exchange, show a centered welcome instead of the lone
    // system line. Only when that lone line is the welcome, though: a resumed
    // conversation of one message has a real message there, not a welcome, and
    // must render it rather than the empty state.
    let only_welcome = match state.messages.as_slice() {
        [] => true,
        [m] => m.role == Role::System,
        _ => false,
    };
    if only_welcome && state.streaming.is_none() && !state.busy {
        ui.selected_text = None;
        render_empty_state(f, area, ui);
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
        ui.cache_text = cache
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
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
    // Prompts waiting their turn, dim and unbanded: on screen so the reader can see
    // what they have lined up, but visibly not sent yet.
    for query in &state.queued {
        let indent = usize::from(MARKER_W);
        for (i, wrapped) in wrap(query, width.saturating_sub(indent).max(1))
            .into_iter()
            .enumerate()
        {
            let lead = if i == 0 {
                USER_MARKER.to_string()
            } else {
                " ".repeat(indent)
            };
            tail.push(Line::from(vec![
                Span::styled(lead, Style::default().fg(Color::DarkGray)),
                Span::styled(wrapped, Style::default().fg(Color::DarkGray)),
            ]));
        }
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
    // While the reader is scrolled up, keep their view anchored to the same
    // content as a streaming answer appends lines below — bump scroll by the
    // growth so the visible window doesn't drift downward under them.
    if state.scroll > 0 && total > ui.prev_total {
        // Keep the scrolled-up view anchored as a streaming answer appends lines.
        // The selection is in content coordinates, so it needs no adjustment — the
        // lines it names keep their indices however the view moves.
        let grew = u16::try_from(total - ui.prev_total).unwrap_or(u16::MAX);
        state.scroll = state.scroll.saturating_add(grew).min(ui.max_scroll);
    }
    ui.prev_total = total;
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
    // Plain text per visible row, for double-click word selection.
    ui.visible_text = window
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    // Record where the view sits in the transcript so the mouse handlers can map a
    // cell to a content line (and back) while selecting.
    ui.view_start = start;
    ui.view_rows = bottom - start;
    ui.view_total = total;

    // Build the lines to render: with the selection highlighted where there is
    // one, plain otherwise.
    let mut rendered: Vec<Line> = if let Some((anchor, cursor)) = ui.selection {
        // Selection is in content coordinates: order the endpoints by (line, col).
        let (sel_top, sel_end) = if anchor <= cursor {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        };
        let mut out = Vec::with_capacity(window.len());
        for (i, line) in window.into_iter().enumerate() {
            let ci = start + i;
            if ci < sel_top.0 || ci > sel_end.0 {
                out.push(line);
                continue;
            }
            let from = if ci == sel_top.0 {
                sel_top.1 as usize
            } else {
                0
            };
            let to = if ci == sel_end.0 {
                sel_end.1 as usize
            } else {
                usize::MAX
            };
            let (highlighted, _) = select_line(&line, from, to);
            out.push(highlighted);
        }
        // Gather the selected text across the whole range — including lines
        // scrolled off screen — so a drag spanning several pages copies
        // everything, not just what happens to be visible at release.
        let mut picked: Vec<String> = Vec::new();
        for ci in sel_top.0..=sel_end.0.min(total.saturating_sub(1)) {
            let line = if ci < cache_len {
                &ui.transcript_cache[ci]
            } else {
                &tail[ci - cache_len]
            };
            let from = if ci == sel_top.0 {
                sel_top.1 as usize
            } else {
                0
            };
            let to = if ci == sel_end.0 {
                sel_end.1 as usize
            } else {
                usize::MAX
            };
            let (_, text) = select_line(line, from, to);
            if !text.is_empty() {
                picked.push(text);
            }
        }
        ui.selected_text = (!picked.is_empty()).then(|| picked.join("\n"));
        out
    } else {
        ui.selected_text = None;
        window
    };
    // Mark the focused search hit so the eye lands on it after a scroll-to-match.
    if let Some(find) = &ui.find {
        let matches = find_matches(&ui.cache_text, &find.query);
        if let Some(&line) = matches.get(find.current) {
            if (start..bottom).contains(&line) {
                if let Some(row) = rendered.get_mut(line - start) {
                    *row = highlight_find_row(row);
                }
            }
        }
    }
    f.render_widget(Paragraph::new(Text::from(rendered)), area);
}

/// Re-style a whole transcript row as the focused search hit: a warm background
/// and bold, so it stands out after the view scrolls to it.
fn highlight_find_row(line: &Line<'static>) -> Line<'static> {
    let bg = Color::Rgb(64, 56, 20);
    Line::from(
        line.spans
            .iter()
            .map(|s| {
                Span::styled(
                    s.content.to_string(),
                    s.style.bg(bg).add_modifier(Modifier::BOLD),
                )
            })
            .collect::<Vec<_>>(),
    )
}

/// A centered welcome shown for a fresh, empty session.
fn render_empty_state(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui) {
    const SAMPLES: [&str; 3] = ["Ai.Sample1", "Ai.Sample2", "Ai.Sample3"];

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
    // The examples set the tone, and they are also the quickest way in: each is a
    // button, so a reader who likes one does not have to retype it.
    let mut samples: Vec<(&'static str, usize)> = Vec::new();
    for key in SAMPLES {
        samples.push((key, content.len()));
        // A cyan chevron (the same sigil a sent prompt carries) marks each sample
        // as something to click or type, and the brighter text reads as an
        // invitation rather than fine print.
        content.push(Line::from(vec![
            Span::styled("❯ ", Style::default().fg(Color::Cyan)),
            Span::styled(t!(key).to_string(), Style::default().fg(Color::Gray)),
        ]));
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
    let mut offset = 0usize;
    if area.height as usize >= mark_h + content.len() + 2 && area.width >= assets::mark_width() {
        let mut with_logo = assets::logo_mark();
        with_logo.push(Line::from(""));
        offset = with_logo.len();
        with_logo.extend(content);
        content = with_logo;
    }
    let top = (area.height as usize).saturating_sub(content.len()) / 2;
    // Hit rects follow the same centring the paragraph applies: a click has to land
    // on the row the reader sees.
    for (key, row) in samples {
        let at = offset + row;
        let y = area.y + (top + at) as u16;
        if y >= area.y.saturating_add(area.height) {
            continue;
        }
        let w = content.get(at).map_or(0, |l| line_width(l) as u16);
        let rect = Rect {
            x: area.x + area.width.saturating_sub(w) / 2,
            y,
            width: w,
            height: 1,
        };
        if hovering(ui, rect) {
            if let Some(line) = content.get_mut(at) {
                for span in &mut line.spans {
                    span.style = span.style.add_modifier(Modifier::UNDERLINED);
                }
            }
        }
        ui.chips.push((Chip::Sample(key), rect));
    }
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
/// The ground under the ticker entry whose drawer is open — a selected-tab tint,
/// lifted toward the accent so it reads as chosen rather than merely hovered.
const TAB_BG: Color = Color::Rgb(30, 58, 66);

/// The slash-command palette: a rounded, bordered menu of matching commands
/// floating above the prompt. ↑/↓ move the highlight, Enter/click runs it, and
/// the command names are column-aligned with dimmed descriptions.
fn render_slash_dropdown(
    f: &mut ratatui::Frame,
    area: Rect,
    prompt: Rect,
    ui: &mut Ui,
    editor: &Editor,
) {
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
    // The prompt box's top border sits one row into the footer (a blank row above
    // it). Hang the palette's bottom edge off that border so the two are flush,
    // instead of anchoring to the transcript's foot with the status row between.
    let box_area = Rect {
        x: area.x,
        y: (prompt.y + 1).saturating_sub(box_h),
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

/// Render the History list as one-line entries — a numbered badge, the title,
/// and a dimmed relative age — with a trailing "New session" action, an
/// optional search line, and a hit rectangle per entry.
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
        .map(|s| (s.title.clone(), session_time_label(s.updated_at, now)))
        .collect();
    let n = entries.len();
    if ui.sessions_error && n == 0 {
        // Signed out is not a failure to distrust: there is simply nobody to ask.
        // Say to sign in rather than showing a connection error.
        let (msg, color) = if crate::openapi::is_ready() {
            (t!("Ai.SessionsError"), Color::Red)
        } else {
            (t!("Ai.SessionsSignedOut"), Color::DarkGray)
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                msg.to_string(),
                Style::default().fg(color),
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

/// Push a one-line History entry: `NN  Title` with a right-aligned dimmed age,
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
    } else if secs < 7 * 86_400 {
        format!("{}d", secs / 86_400)
    } else if secs < 365 * 86_400 {
        // Beyond a week, days stop being useful; weeks and years read cleaner
        // than an ever-growing day count.
        format!("{}w", secs / (7 * 86_400))
    } else {
        format!("{}y", secs / (365 * 86_400))
    }
}

/// Relative age for recent conversations, then a local-calendar date once a
/// relative day count stops being useful.
fn session_time_label(updated: u64, now: u64) -> String {
    let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    session_time_label_at(updated, now, offset)
}

fn session_time_label_at(updated: u64, now: u64, offset: time::UtcOffset) -> String {
    const THREE_DAYS: u64 = 3 * 86_400;
    if now.saturating_sub(updated) < THREE_DAYS {
        return relative_time(updated, now);
    }
    let Some(updated_dt) = i64::try_from(updated)
        .ok()
        .and_then(|ts| time::OffsetDateTime::from_unix_timestamp(ts).ok())
    else {
        return relative_time(updated, now);
    };
    let Some(now_dt) = i64::try_from(now)
        .ok()
        .and_then(|ts| time::OffsetDateTime::from_unix_timestamp(ts).ok())
    else {
        return relative_time(updated, now);
    };
    let updated_dt = updated_dt.to_offset(offset);
    let now_dt = now_dt.to_offset(offset);
    let formatted = if updated_dt.year() == now_dt.year() {
        updated_dt.format(time::macros::format_description!(
            "[month repr:short] [day padding:none]"
        ))
    } else {
        updated_dt.format(time::macros::format_description!(
            "[month repr:short] [day padding:none], [year]"
        ))
    };
    formatted.unwrap_or_else(|_| relative_time(updated, now))
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
        // Time *until* expiry: relative_time(now, exp) counts forward, where the
        // arguments the other way round always read "just now" for a token that
        // has not expired yet.
        second.push(Span::styled(
            format!(
                "  ·  {} {}",
                t!("Ai.SessionExpires"),
                relative_time(now_secs(), exp)
            ),
            dim,
        ));
    }
    vec![Line::from(first), Line::from(second), Line::from("")]
}

/// Render the Question drawer: the pending interrupt's current question and its
/// options, as a bordered panel rising from the input.
///
/// It is deliberately not a view of its own filling the screen: the reader is
/// answering a question *about the conversation*, and covering the conversation to
/// ask it took away the thing they need to answer it. So the transcript stays
/// visible behind and the drawer takes only the rows it needs.
fn render_question(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui) {
    let Some((question, mut options, wire_values, selected, multi_select, qi, total)) =
        ui.question.as_ref().and_then(|q| {
            q.questions.get(q.qi).map(|(t, o)| {
                (
                    t.clone(),
                    o.clone(),
                    q.targets
                        .get(q.qi)
                        .and_then(|(_, _, values, _)| values.clone()),
                    q.multi_selected.get(&q.qi).cloned().unwrap_or_default(),
                    q.targets.get(q.qi).is_some_and(|(_, _, _, multi)| *multi),
                    q.qi,
                    q.questions.len(),
                )
            })
        })
    else {
        ui.rows.clear();
        return;
    };
    if multi_select {
        let confirm_index = options.len().saturating_sub(1);
        for (index, option) in options.iter_mut().enumerate().take(confirm_index) {
            let mark = if selected.contains(&index) {
                "☑"
            } else {
                "☐"
            };
            *option = format!("{mark} {option}");
        }
    }
    // The question wraps inside the frame, and the options are one row each.
    let inner_w = area.width.saturating_sub(4).max(1) as usize;
    let qlines = wrap(&question, inner_w);
    let want = qlines.len() + 1 + options.len();
    // At most half the body: an interrupt after a long answer must not push the
    // answer it is about out of view.
    let inner_h = want.min((area.height as usize / 2).max(3)) as u16;
    let height = inner_h + 2;
    let drawer = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(height),
        width: area.width,
        height: height.min(area.height),
    };
    f.render_widget(Clear, drawer);
    blank_straddling_glyphs(f.buffer_mut(), drawer, area);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(Color::DarkGray))
        .padding(Padding::horizontal(1));
    if total > 1 {
        // Only worth a counter when there is more than one to get through.
        block = block.title_top(
            Line::from(Span::styled(
                format!(" {}/{total} ", qi + 1),
                Style::default().fg(Color::DarkGray),
            ))
            .right_aligned(),
        );
    }
    let inner = block.inner(drawer);
    f.render_widget(block, drawer);
    if inner.height == 0 {
        ui.rows.clear();
        return;
    }
    let head_h = (qlines.len() as u16 + 1).min(inner.height.saturating_sub(1));
    let [head, rest] =
        Layout::vertical([Constraint::Length(head_h), Constraint::Min(0)]).areas(inner);
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
    let rows: Vec<(usize, String, Option<Color>)> = options
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            let color = option_color(wire_values.as_deref(), index);
            (index, label, color)
        })
        .collect();
    render_rows(f, rest, ui, &rows);
}

fn option_color(wire_values: Option<&[String]>, index: usize) -> Option<Color> {
    match wire_values.and_then(|values| values.get(index).map(String::as_str)) {
        Some("true") => Some(Color::Green),
        Some("false") => Some(Color::Red),
        _ => None,
    }
}

/// The drawer's option list: one row each, a subtle tinted background on the
/// selected or hovered one and an accent marker, a hit rectangle per visible row,
/// windowed around the selection.
fn render_rows(
    f: &mut ratatui::Frame,
    area: Rect,
    ui: &mut Ui,
    rows: &[(usize, String, Option<Color>)],
) {
    ui.rows.clear();
    ui.clamp_sel();
    let width = area.width as usize;
    let avail = width.saturating_sub(2);
    let fit = (area.height as usize).max(1);
    let start = if ui.sel < fit {
        0
    } else {
        (ui.sel + 1 - fit).min(rows.len().saturating_sub(fit))
    };
    let mut lines = Vec::new();
    for (idx, label, semantic_color) in rows.iter().skip(start).take(fit) {
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
        let marker_color =
            semantic_color.unwrap_or(if selected { IDX_SEL } else { Color::DarkGray });
        let mut text_style = Style::default().fg(semantic_color.unwrap_or(if selected {
            Color::White
        } else {
            Color::Gray
        }));
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
/// How many follow-ups the panel offers. Three is a nudge; a full list of six is a
/// menu standing between the reader and their own next question.
const FURTHER_SHOWN: usize = 3;

fn meta_height(state: &ChatState) -> u16 {
    (state.further.len() as u16).min(FURTHER_SHOWN as u16)
}

/// Render clickable reference / follow-up chips and record their hit rects.
fn render_chips(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui, state: &ChatState) {
    let mut lines: Vec<Line> = Vec::new();
    let mut y = area.y;
    let bottom = area.y + area.height;
    if !state.further.is_empty() && y < bottom {
        // No header: the `›` and the colour say what these are, and a label would
        // cost a row of the reader's screen to say it in words.
        for (i, q) in state.further.iter().enumerate().take(FURTHER_SHOWN) {
            if y >= bottom {
                break;
            }
            let rect = row_rect(area, y);
            // A dim leading index doubles as the Alt+N shortcut label.
            let full = format!("  {} › {q}", i + 1);
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

fn render_status(f: &mut ratatui::Frame, area: Rect, ui: &Ui, state: &ChatState, editor: &Editor) {
    // Folded pastes are announced here, above the input, so a big paste is a
    // compact chip instead of a wall of text filling the box.
    let attachments = editor.attachments();
    if ui.view == View::Chat && ui.find.is_none() && !attachments.is_empty() {
        let total_lines: usize = attachments.iter().map(|a| a.split('\n').count()).sum();
        let label = if attachments.len() == 1 {
            t!("Ai.PastedOne", lines = total_lines).to_string()
        } else {
            t!(
                "Ai.PastedMany",
                count = attachments.len(),
                lines = total_lines
            )
            .to_string()
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("⎘ {label}"),
                Style::default().fg(Color::Cyan),
            ))),
            area,
        );
        return;
    }
    // The find bar owns this row while open: the query, and where the reader is
    // among its matches.
    if let Some(find) = &ui.find {
        let count = find_matches(&ui.cache_text, &find.query).len();
        let counter = if find.query.trim().is_empty() {
            String::new()
        } else if count == 0 {
            format!("  {}", t!("Ai.FindNone"))
        } else {
            // Clamp the index: matches are recomputed each render, so if the
            // transcript shrank while streaming, a stale `current` must not show a
            // nonsensical `5/2`.
            format!("  {}/{count}", find.current.min(count - 1) + 1)
        };
        let text = format!("{}: {}{counter}", t!("Ai.FindLabel"), find.query);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                text,
                Style::default().fg(Color::Yellow),
            ))),
            area,
        );
        return;
    }
    let (text, style) = if ui.view == View::Chat && state.scroll > 0 {
        // While scrolled up, tell the user how to get back to the latest.
        (
            t!("Ai.ScrolledHint").to_string(),
            Style::default().fg(Color::Yellow),
        )
    } else if let Some(notice) = &ui.notice {
        (notice.clone(), Style::default().fg(Color::Green))
    } else if ui.view == View::Chat && !state.messages.is_empty() {
        // Once the reader has sent something they know how to send things. The row
        // stays (notices and the scrolled-up hint use it) but it stays quiet: a
        // permanent list of shortcuts a hand's width above the cursor is noise.
        (String::new(), Style::default())
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

fn render_footer(f: &mut ratatui::Frame, area: Rect, ui: &Ui, editor: &Editor, hug: bool) {
    let focused = ui.view == View::Chat;
    // The box stays — it is what separates the prompt from the transcript — but dim,
    // where a rounded cyan frame was the loudest thing on the screen. A blank row
    // above it keeps it off the transcript's last line; while a turn runs the status
    // row already does that, so the box hugs it instead of adding a second gap.
    let blank_above = u16::from(!hug);
    let boxed = Rect {
        y: area.y + blank_above,
        height: area.height.saturating_sub(blank_above),
        ..area
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(boxed);
    f.render_widget(block, boxed);
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
    // A tall prompt (a long paste) can exceed the box, so scroll it to keep the
    // cursor line visible instead of clipping the bottom the reader is typing.
    let visible = body.height.max(1) as usize;
    let (cy, col) = editor.cursor();
    let top = if cy >= visible { cy + 1 - visible } else { 0 };
    // The marker leads the first line; when scrolled past it, a `⋯` says there is
    // more above.
    f.render_widget(
        Paragraph::new(Text::from(
            (0..visible)
                .map(|i| {
                    if i == 0 && top == 0 {
                        Line::from(Span::styled(USER_MARKER, marker_style))
                    } else if i == 0 {
                        Line::from(Span::styled(" ⋯ ", Style::default().fg(Color::DarkGray)))
                    } else {
                        Line::from("")
                    }
                })
                .collect::<Vec<_>>(),
        )),
        marker,
    );
    let shown: Vec<Line> = lines.into_iter().skip(top).take(visible).collect();
    // Follow the caret horizontally: a long line with no newlines would otherwise
    // be clipped at the right border and the cursor would freeze there, hiding
    // what is being typed. Scroll the body so the caret stays one column inside
    // the right edge.
    let left = (col as u16).saturating_sub(body.width.saturating_sub(1));
    f.render_widget(Paragraph::new(Text::from(shown)).scroll((0, left)), body);
    let cy = (cy - top) as u16;
    let col = (col as u16)
        .saturating_sub(left)
        .min(body.width.saturating_sub(1));
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
/// The markers a tool row leads with, so a row can be recognised as one later.
const TOOL_MARKERS: [&str; 3] = ["  ◌ ", "  ⏺ ", "  ⚠ "];

/// Whether `line` is one of the tool rows.
fn is_tool_line(line: &Line<'_>) -> bool {
    line.spans
        .first()
        .is_some_and(|s| TOOL_MARKERS.contains(&s.content.as_ref()))
}

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
    // A run of tool rows is a block, and a block gets air on both sides: the rows
    // themselves are tight, and the answer that follows them starts after a blank.
    if message.role != Role::Tool && lines.last().is_some_and(is_tool_line) {
        lines.push(Line::from(""));
    }
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
        Role::Alert => {
            for logical in message.text.split('\n') {
                for wrapped in wrap(logical, width) {
                    lines.push(Line::from(Span::styled(
                        wrapped,
                        Style::default().fg(Color::Red),
                    )));
                }
            }
        }
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

/// The `$` before a security, and the security itself. The sigil takes the accent
/// — it is one column, so it can afford to — and the symbol stays muted: it is a
/// word in a sentence, not a heading.
const CASHTAG: Color = Color::Cyan;
const SYMBOL_FG: Color = Color::Rgb(148, 163, 184);

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
fn link_symbols(lines: &mut Vec<Line<'static>>, aliases: &HashMap<String, String>) {
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
            for (range, _) in ranges {
                let mut lead = &text[at..range.start];
                // The `$` was inserted before wrapping; give it its own span so it
                // can take the accent while the symbol stays muted.
                if lead.ends_with('$') {
                    lead = &lead[..lead.len() - 1];
                    if !lead.is_empty() {
                        out.push(Span::styled(lead.to_string(), span.style));
                    }
                    out.push(Span::styled("$", span.style.fg(CASHTAG)));
                } else if !lead.is_empty() {
                    out.push(Span::styled(lead.to_string(), span.style));
                }
                out.push(Span::styled(
                    text[range.clone()].to_string(),
                    span.style.fg(SYMBOL_FG),
                ));
                at = range.end;
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

/// Mark each security in the answer text with a `$`, before it is wrapped.
///
/// Before, because the sigil takes a column: added after wrapping it would push
/// the last column of a full line off the screen. Wrapping sees it and accounts
/// for it.
///
/// This replaced writing the price inline. The price is on the title-bar ticker,
/// and putting it in the sentence too meant the prose moved every time a quote
/// arrived — text that shifts under the reader is worse than text that says less.
fn cashtag_annotated(text: &str, aliases: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut fenced = false;
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            out.push_str(line);
            continue;
        }
        // Code is quoted verbatim. Tables are fine: their columns are measured
        // after this, so the sigil is accounted for.
        if fenced {
            out.push_str(line);
            continue;
        }
        // A ticker inside an inline `code` span is already set apart and must stay
        // verbatim — a `$` in the middle of it is markup the reader did not write.
        let code = inline_code_ranges(line);
        let mut at = 0usize;
        for (range, symbol) in super::answer::security_spans(line, aliases) {
            if range.start < at {
                continue;
            }
            if code.iter().any(|r| r.contains(&range.start)) {
                out.push_str(&line[at..range.end]);
                at = range.end;
                continue;
            }
            out.push_str(&line[at..range.start]);
            // Not twice, if the author already wrote the cashtag.
            if !out.ends_with('$') {
                out.push('$');
            }
            // The full symbol, market and all: an answer writes `TSLA` where it
            // means `TSLA.US`, and half a symbol is ambiguous — the same four
            // letters list in more than one market. Widening it here rather than at
            // render time keeps the wrapping honest.
            out.push_str(&symbol);
            at = range.end;
        }
        out.push_str(&line[at..]);
    }
    out
}

/// Byte ranges of single-backtick inline-code spans on one line.
fn inline_code_ranges(line: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut open: Option<usize> = None;
    for (i, b) in line.bytes().enumerate() {
        if b == b'`' {
            match open {
                None => open = Some(i),
                Some(start) => {
                    ranges.push(start..i + 1);
                    open = None;
                }
            }
        }
    }
    ranges
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
            // prose never arrives as a lone ticker.
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
        // Cards top with either a sharp or a rounded corner; recognise both so the
        // whole box stays one click target whichever style it uses.
        if text.contains('┌') || text.contains('╭') {
            card = Some((String::new(), y));
        } else if text.contains('└') || text.contains('╰') {
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
                let text = cashtag_annotated(&text, aliases);
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
    link_symbols(&mut out, aliases);
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
            comparison_card(&header, symbols, width, quotes)
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
            let mut rest = vec![ticket.quantity.clone()];
            rest.push(ticket.symbol.clone());
            rest.push(ticket.order_type.clone());
            if !ticket.price.is_empty() {
                rest.push(format!("@ {}", ticket.price));
            }
            let rest = rest
                .into_iter()
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            // The side is the ticket's most consequential field, so it takes the
            // buy/sell colour (respecting red-up/green-up) rather than reading like
            // the rest of the line. `order_side` produces exactly these localized
            // strings, so the comparison is reliable.
            let mut spans = vec![Span::styled(
                format!("  {}  ", t!("Ai.WidgetOrderTicket")),
                Style::default().fg(Color::DarkGray),
            )];
            if !ticket.side.is_empty() {
                let ordering = if ticket.side == t!("Trade.Buy") {
                    std::cmp::Ordering::Greater
                } else if ticket.side == t!("Trade.Sell") {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                };
                let color = change_color(match ordering {
                    std::cmp::Ordering::Greater => 1,
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                });
                spans.push(Span::styled(
                    format!("{} ", ticket.side),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ));
            }
            spans.push(Span::styled(
                rest,
                Style::default().add_modifier(Modifier::BOLD),
            ));
            vec![Line::from(spans)]
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
/// A security's cell: the accented `$`, then the symbol on its own.
///
/// Its own span for two reasons: the sigil takes a different colour, and the hit
/// test recognises a span whose content is *exactly* a symbol. Bundling the name
/// in with it — as the card used to — quietly cost the card its click target
/// whenever the name was non-empty.
fn symbol_cell(symbol: &str) -> Vec<Span<'static>> {
    vec![
        Span::styled("$", Style::default().fg(CASHTAG)),
        Span::styled(
            symbol.to_string(),
            Style::default().fg(SYMBOL_FG).add_modifier(Modifier::BOLD),
        ),
    ]
}

/// Frame `rows` in a box, padding each to the same inner width.
///
/// One helper for both card kinds so a comparison cannot drift from a single
/// quote: same border, same padding, same alignment. Each row arrives as its
/// spans plus the display width they occupy — measuring inside would mean
/// re-measuring every span, and a wrong count shows up as a ragged border.
fn framed(rows: Vec<(Vec<Span<'static>>, usize)>, inner: usize) -> Vec<Line<'static>> {
    let dim = Style::default().fg(Color::DarkGray);
    let border = |left: &str, right: &str| {
        Line::from(Span::styled(
            format!("{left}{}{right}", "─".repeat(inner + 2)),
            dim,
        ))
    };
    let mut out = vec![border("╭", "╮")];
    for (spans, used) in rows {
        let mut all = vec![Span::styled("│", dim), Span::raw(" ")];
        all.extend(spans);
        all.push(Span::raw(" ".repeat(inner.saturating_sub(used) + 1)));
        all.push(Span::styled("│", dim));
        out.push(Line::from(all));
    }
    out.push(border("╰", "╯"));
    out
}

/// The panel's body: one field per row, unboxed — the panel's own frame is the
/// border, and a dedicated panel has the room the inline card does not.
fn card_lines(
    card: &super::quotes::QuoteCardData,
    path: &[f64],
    detail: Option<&super::quotes::QuoteDetail>,
) -> Vec<Line<'static>> {
    let dim = Style::default().fg(Color::DarkGray);
    let dir = change_color(card.direction);
    let mut out = Vec::new();
    if !card.name.is_empty() {
        out.push(Line::from(Span::styled(
            card.name.clone(),
            Style::default().fg(Color::Gray),
        )));
    }
    // The price is what the panel is for, so it leads: last, then the change in the
    // direction's colour.
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
    // The session's shape, which no single number can show.
    if let Some(spark) = sparkline(path, SPARK_W) {
        out.push(Line::from(Span::styled(spark, Style::default().fg(dir))));
    }
    out.push(Line::from(""));
    // Two labelled columns, measured from the values so they line up whatever the
    // numbers are. Only the six a glance actually uses: the rest was chrome.
    let col = 6
        + [&card.open, &card.low, &card.turnover]
            .into_iter()
            .map(|v| UnicodeWidthStr::width(v.as_str()))
            .max()
            .unwrap_or(0)
        + 3;
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
    // The richer figures, once they have arrived: valuation, per-share and the
    // session's shape numbers the six above cannot carry. Their own aligned block,
    // because the labels and values are a different width from the core prices, and
    // every field is optional — a market that returns none simply adds no rows.
    if let Some(d) = detail {
        let fields: [(std::borrow::Cow<'_, str>, &Option<String>); 9] = [
            (t!("Ai.QuoteAvg"), &d.avg),
            (t!("Ai.QuoteAmplitude"), &d.amplitude),
            (t!("Ai.QuoteTurnoverRate"), &d.turnover_rate),
            (t!("Ai.QuoteVolumeRatio"), &d.volume_ratio),
            (t!("Ai.QuotePeTtm"), &d.pe_ttm),
            (t!("Ai.QuotePb"), &d.pb),
            (t!("Ai.QuoteMarketCap"), &d.market_cap),
            (t!("Ai.QuoteEps"), &d.eps_ttm),
            (t!("Ai.QuoteBps"), &d.bps),
        ];
        let present: Vec<(String, String)> = fields
            .iter()
            .filter_map(|(label, value)| value.as_ref().map(|v| (label.to_string(), v.clone())))
            .collect();
        if !present.is_empty() {
            out.push(Line::from(""));
            let label_w = present
                .iter()
                .map(|(l, _)| UnicodeWidthStr::width(l.as_str()))
                .max()
                .unwrap_or(0);
            let val_w = present
                .iter()
                .map(|(_, v)| UnicodeWidthStr::width(v.as_str()))
                .max()
                .unwrap_or(0);
            let cell = |spans: &mut Vec<Span<'static>>, label: &str, value: &str, last: bool| {
                let lpad = " ".repeat(label_w.saturating_sub(UnicodeWidthStr::width(label)) + 1);
                spans.push(Span::styled(format!("{label}{lpad}"), dim));
                if last {
                    spans.push(Span::raw(value.to_string()));
                } else {
                    let vpad = " ".repeat(val_w.saturating_sub(UnicodeWidthStr::width(value)) + 3);
                    spans.push(Span::raw(format!("{value}{vpad}")));
                }
            };
            for pair in present.chunks(2) {
                let mut spans = Vec::new();
                cell(&mut spans, &pair[0].0, &pair[0].1, pair.len() == 1);
                if let Some(second) = pair.get(1) {
                    cell(&mut spans, &second.0, &second.1, true);
                }
                out.push(Line::from(spans));
            }
        }
    }
    out
}

/// Columns the panel's sparkline occupies.
const SPARK_W: usize = 32;

/// The day's price path as one row of eighth blocks.
///
/// One row, because the panel is a glance and a full chart is a click away on the
/// web. `None` when there is nothing to draw — a flat or missing path would be a
/// row of noise.
/// The tallest block's index in [`BLOCKS`], as a float for the scaling below.
const TOP_LEVEL: f64 = 7.0;

fn sparkline(path: &[f64], width: usize) -> Option<String> {
    const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

    if path.len() < 2 || width == 0 {
        return None;
    }
    let (lo, hi) = path
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    let span = hi - lo;
    if span <= f64::EPSILON {
        return None;
    }
    let take = path.len().min(width);
    Some(
        (0..take)
            .map(|i| {
                let v = path[i * (path.len() - 1) / (take - 1).max(1)];
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let level = (((v - lo) / span) * TOP_LEVEL).round() as usize;
                BLOCKS[level.min(BLOCKS.len() - 1)]
            })
            .collect(),
    )
}

/// The previous close, formatted like the other prices.
fn prev_close(card: &super::quotes::QuoteCardData) -> String {
    card.prev_close.round_dp(3).normalize().to_string()
}

fn quote_card(card: &super::quotes::QuoteCardData, width: usize) -> Vec<Line<'static>> {
    let dir = change_color(card.direction);
    // Measured with the `$` and the two-space gap the header renders with.
    let head = if card.name.is_empty() {
        format!("${}", card.symbol)
    } else {
        format!("${}  {}", card.symbol, card.name)
    };
    // The change carries a direction arrow, so the percent drops its sign rather
    // than stating it twice. Computed once so the box-width measurement and the
    // rendered row agree.
    let (arrow, pct) = match card.direction {
        1 => ("▲ ", card.change_pct.trim_start_matches(['+', '-'])),
        -1 => ("▼ ", card.change_pct.trim_start_matches(['+', '-'])),
        _ => ("", card.change_pct.as_str()),
    };
    let price = format!("{}  {arrow}{}  {pct}", card.last, card.change);
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
        border("╭", "╮"),
        row(
            {
                let mut spans = symbol_cell(&card.symbol);
                if !card.name.is_empty() {
                    let room =
                        inner.saturating_sub(UnicodeWidthStr::width(card.symbol.as_str()) + 3);
                    spans.push(Span::styled(
                        format!("  {}", truncate_width(&card.name, room)),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                spans
            },
            UnicodeWidthStr::width(head.as_str()),
        ),
        // The price leads in its direction's colour, with the arrow carrying the
        // direction so the change reads at a glance — the same treatment the
        // floating panel gives it.
        row(
            vec![
                Span::styled(
                    format!("{}  ", card.last),
                    Style::default().fg(dir).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{arrow}{}  {pct}", card.change),
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
        border("╰", "╯"),
    ]
}

/// One security as a single aligned row, for a comparison or a list.
fn comparison_card(
    header: &str,
    symbols: &[String],
    width: usize,
    quotes: &HashMap<String, super::quotes::QuoteCardData>,
) -> Vec<Line<'static>> {
    // Columns are measured across the whole set, which is the point of the
    // widget: the prices have to line up or they cannot be compared down the page.
    // The old rendering left them to `{:>10}` and put the header at a different
    // indent from the rows, so nothing lined up with anything.
    let w = |s: &str| UnicodeWidthStr::width(s);
    let cell = |symbol: &String| quotes.get(symbol);
    // One column wider than the longest symbol: the `$` rides in front of it.
    let sym_w = symbols.iter().map(|s| w(s)).max().unwrap_or(0) + 1;
    let last_w = symbols
        .iter()
        .filter_map(cell)
        .map(|c| w(&c.last))
        .max()
        .unwrap_or(0);
    let pct_w = symbols
        .iter()
        .filter_map(cell)
        .map(|c| w(&c.change_pct))
        .max()
        .unwrap_or(0);
    // Frame plus gaps: bar, space, columns, space, bar.
    let fixed = sym_w + 2 + last_w + 2 + pct_w;
    let budget = width.saturating_sub(4);
    let name_w = budget.saturating_sub(fixed + 2);
    let name_w = symbols
        .iter()
        .filter_map(cell)
        .map(|c| w(&c.name))
        .max()
        .unwrap_or(0)
        .min(name_w);
    let inner = (fixed + if name_w > 0 { name_w + 2 } else { 0 }).min(budget);

    let mut rows: Vec<(Vec<Span<'static>>, usize)> = vec![(
        vec![Span::styled(
            header.to_string(),
            Style::default().fg(Color::DarkGray),
        )],
        w(header),
    )];
    for symbol in symbols {
        let mut spans = symbol_cell(symbol);
        spans.push(Span::raw(" ".repeat(sym_w.saturating_sub(w(symbol) + 1))));
        let mut used = sym_w;
        if let Some(card) = cell(symbol) {
            spans.push(Span::styled(
                format!("  {:>last_w$}", card.last),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            // The arrow replaces the percent's sign — same width, more legible.
            let (arrow, pct) = match card.direction {
                1 => ("▲", card.change_pct.trim_start_matches(['+', '-'])),
                -1 => ("▼", card.change_pct.trim_start_matches(['+', '-'])),
                _ => ("", card.change_pct.as_str()),
            };
            spans.push(Span::styled(
                format!("  {:>pct_w$}", format!("{arrow}{pct}")),
                Style::default().fg(change_color(card.direction)),
            ));
            used += 2 + last_w + 2 + pct_w;
            if name_w > 0 && !card.name.is_empty() {
                let name = truncate_width(&card.name, name_w);
                used += 2 + w(&name);
                spans.push(Span::styled(
                    format!("  {name}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
        } else {
            // No quote yet: the row keeps its place in the column so the card does
            // not jump when the quote lands.
            spans.push(Span::styled(
                format!("  {:>last_w$}", "…"),
                Style::default().fg(Color::DarkGray),
            ));
            used += 2 + last_w;
        }
        rows.push((spans, used));
    }
    framed(rows, inner)
}

/// Wrap `s` to `width` display columns, honoring wide (CJK) glyphs.
fn wrap(s: &str, width: usize) -> Vec<String> {
    if width == 0 || s.is_empty() {
        return vec![s.to_string()];
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut line_start = 0usize;
    let mut i = 0usize;
    let mut w = 0usize;
    // The most recent space on the current line, so a word never splits mid-token
    // unless it is itself longer than the width (or is CJK, which has no spaces).
    let mut last_space: Option<usize> = None;
    while i < chars.len() {
        let cw = chars[i].width().unwrap_or(0);
        if w + cw > width && i > line_start {
            let brk = match last_space {
                Some(sp) if sp > line_start => sp + 1,
                _ => i,
            };
            out.push(
                chars[line_start..brk]
                    .iter()
                    .collect::<String>()
                    .trim_end()
                    .to_string(),
            );
            line_start = brk;
            i = brk;
            w = 0;
            last_space = None;
            continue;
        }
        if chars[i] == ' ' {
            last_space = Some(i);
        }
        w += cw;
        i += 1;
    }
    out.push(chars[line_start..].iter().collect());
    out
}

#[cfg(test)]
mod tests {
    use super::marquee;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn a_question_with_options_is_fully_selectable() {
        let interrupt = serde_json::json!({
            "tool_call_id": "call_1",
            "questions": [
                {"question": "Which market?", "choices": ["HK", "US"]},
                {"question": "Which period?", "choices": ["1d", "1w"]},
            ],
        });
        let qs = super::QuestionState::from_interrupt(&interrupt);
        assert_eq!(qs.questions.len(), 2);
        assert_eq!(qs.targets[0].0, "call_1");
        assert!(qs.fully_selectable());
    }

    #[test]
    fn a_free_text_question_is_not_fully_selectable() {
        // One question has no options, so the option overlay cannot answer it.
        let interrupt = serde_json::json!({
            "tool_call_id": "call_2",
            "questions": [
                {"question": "Which market?", "choices": ["HK", "US"]},
                {"question": "Anything else?"},
            ],
        });
        let qs = super::QuestionState::from_interrupt(&interrupt);
        assert!(!qs.fully_selectable());
    }

    #[test]
    fn an_authorization_interaction_offers_decline_and_allow() {
        let interrupt = serde_json::json!({
            "interactions": [{
                "tool_call_id": "call_watchlist",
                "interrupt_id": "authorize_watchlist",
                "type": "authorization",
                "tool_display_name": "Read watchlist",
                "questions": []
            }]
        });
        let qs = super::QuestionState::from_interrupt(&interrupt);
        assert_eq!(qs.targets[0].0, "authorize_watchlist");
        assert!(qs.fully_selectable());
        assert_eq!(qs.questions.len(), 1);
        assert_eq!(qs.questions[0].1.len(), 2);
    }

    #[test]
    fn a_trade_password_interaction_does_not_offer_a_false_continue_path() {
        let interrupt = serde_json::json!({
            "interactions": [{
                "interrupt_id": "trade_password",
                "type": "trade_password",
                "tool_display_name": "Account details"
            }]
        });
        let qs = super::QuestionState::from_interrupt(&interrupt);
        assert!(qs.questions.is_empty());
        assert!(!qs.fully_selectable());
    }

    #[test]
    fn external_authorization_interactions_do_not_offer_fake_confirmation() {
        for kind in ["connector_reauth", "openapi_reauth", "data_authorization"] {
            let interrupt = serde_json::json!({
                "interactions": [{
                    "interrupt_id": "external_auth",
                    "type": kind
                }]
            });
            let qs = super::QuestionState::from_interrupt(&interrupt);
            assert!(qs.questions.is_empty(), "{kind} must not show Allow");
        }
    }

    #[test]
    fn multi_select_toggles_choices_then_submits_the_joined_answer() {
        let interrupt = serde_json::json!({
            "interactions": [{
                "interrupt_id": "pick_markets",
                "type": "ask_human",
                "questions": [{
                    "question": "Which markets?",
                    "options": [
                        {"description": "HK"},
                        {"description": "US"}
                    ],
                    "multi_select": true
                }]
            }]
        });
        let mut qs = super::QuestionState::from_interrupt(&interrupt);
        assert!(!qs.select(0));
        assert!(!qs.select(1));
        assert!(qs.select(2)); // Confirm selection.
        assert_eq!(qs.answers["pick_markets"]["Which markets?"], "HK, US");
    }

    #[test]
    fn skipping_uses_the_shared_hitl_sentinel_and_advances_the_interaction() {
        let interrupt = serde_json::json!({
            "interactions": [
                {
                    "interrupt_id": "first",
                    "type": "ask_human",
                    "questions": [{"question": "Private detail?", "choices": ["Yes"]}]
                },
                {
                    "interrupt_id": "second",
                    "type": "ask_human",
                    "questions": [{"question": "Market?", "choices": ["US"]}]
                }
            ]
        });
        let mut ui = super::Ui::new();
        ui.view = super::View::Question;
        ui.question = Some(super::QuestionState::from_interrupt(&interrupt));
        let mut state = super::ChatState::default();
        let mut turn = None;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        super::skip_question(&mut ui, &mut state, &mut turn, &tx);
        let question = ui.question.as_ref().expect("second question remains");
        assert_eq!(question.qi, 1);
        assert_eq!(question.answers["first"]["_skipped"], "true");
    }

    #[test]
    fn multiple_interactions_collect_one_combined_resume_payload() {
        let interrupt = serde_json::json!({
            "interactions": [
                {
                    "interrupt_id": "authorize_watchlist",
                    "type": "authorization",
                    "tool_display_name": "Read watchlist"
                },
                {
                    "interrupt_id": "ask_market",
                    "type": "ask_human",
                    "questions": [{"question": "Which market?", "choices": ["HK", "US"]}]
                },
                {
                    "interrupt_id": "authorize_orders",
                    "type": "authorization",
                    "tool_display_name": "Read orders"
                }
            ]
        });
        let mut qs = super::QuestionState::from_interrupt(&interrupt);
        assert!(qs.select(1)); // Allow watchlist.
        qs.qi += 1;
        assert!(qs.select(0)); // HK.
        qs.qi += 1;
        assert!(qs.select(0)); // Decline orders authorization.

        assert_eq!(qs.answers["authorize_watchlist"]["authorized"], "true");
        assert_eq!(qs.answers["ask_market"]["Which market?"], "HK");
        assert_eq!(qs.answers["authorize_orders"]["authorized"], "false");
    }

    #[test]
    fn authorization_choices_use_semantic_colors() {
        let values = vec!["false".to_string(), "true".to_string()];
        assert_eq!(
            super::option_color(Some(&values), 0),
            Some(ratatui::style::Color::Red)
        );
        assert_eq!(
            super::option_color(Some(&values), 1),
            Some(ratatui::style::Color::Green)
        );
    }

    #[test]
    fn copy_excludes_tool_and_system_lines() {
        use super::transcript_text;
        use crate::ai::state::{ChatState, Message, Role, ToolStatus};
        let mut state = ChatState::new("chatbot".into(), "welcome".into());
        state.messages.push(Message::new(Role::User, "hi".into()));
        state
            .messages
            .push(Message::tool("get_quote".into(), ToolStatus::Ok));
        state
            .messages
            .push(Message::new(Role::Assistant, "hello".into()));
        let copied = transcript_text(&state);
        assert!(copied.contains("hi") && copied.contains("hello"));
        assert!(
            !copied.contains("get_quote") && !copied.contains("welcome"),
            "tool and system lines must not be copied: {copied}"
        );
    }

    /// `/copy` grabs the latest answer (what "copy" means in a chat), and says so
    /// when there is not one yet.
    #[test]
    fn copy_takes_the_last_answer_or_reports_none() {
        use crate::ai::state::{ChatState, Message, Role};
        let mut ui = super::Ui::new();
        let mut state = ChatState::new("chatbot".into(), "welcome".into());
        super::exec_slash("copy", "", &mut ui, &mut state);
        assert_eq!(ui.notice.as_deref(), Some(t!("Ai.NothingToCopy").as_ref()));
        state.messages.push(Message::new(Role::User, "hi".into()));
        state
            .messages
            .push(Message::new(Role::Assistant, "the answer".into()));
        assert_eq!(super::last_answer(&state).as_deref(), Some("the answer"));
    }

    /// The command palette hangs directly off the prompt box: its bottom border
    /// sits on the row immediately above the prompt box's top border, rather than
    /// being anchored to the transcript's foot with the status row in between.
    #[test]
    fn the_command_palette_sits_on_the_prompt_box() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        let mut editor = super::Editor::new();
        editor.set_text("/");
        let backend = ratatui::backend::TestBackend::new(60, 20);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal
            .draw(|f| super::view(f, &mut ui, &mut state, &editor))
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect();
        // A bottom border (the palette's) with a top border (the prompt box's) on
        // the very next row proves the two are flush.
        let flush = rows
            .iter()
            .zip(rows.iter().skip(1))
            .any(|(a, b)| a.contains('╰') && b.contains('╭'));
        assert!(
            flush,
            "the palette should rest on the prompt box:\n{}",
            rows.join("\n")
        );
    }

    #[test]
    fn a_non_command_slash_prompt_has_no_palette_matches() {
        // A prompt that merely starts with `/` (not a command) must not match any
        // command, so the palette does not capture its Enter — the fix that lets
        // it be sent to the agent rather than swallowed.
        let mut e = super::Editor::new();
        e.set_text("/why does TSLA trade at a premium");
        assert!(super::slash_matches(&e).is_empty());
        // A real command prefix still matches.
        e.set_text("/ne");
        assert!(!super::slash_matches(&e).is_empty());
    }

    #[test]
    fn export_slug_is_filesystem_safe() {
        use super::export_slug;
        assert_eq!(
            export_slug("Tesla stock performance"),
            "tesla-stock-performance"
        );
        assert_eq!(export_slug("特斯拉 TSLA?"), "tsla");
        assert!(export_slug("  ").is_empty());
        // Capped and trimmed of trailing separators so the filename stays sane.
        let long = export_slug(&"word ".repeat(20));
        assert!(long.len() <= 40, "slug too long: {long}");
        assert!(
            !long.ends_with('-'),
            "slug has a trailing separator: {long}"
        );
    }

    #[test]
    fn double_click_selects_the_word_under_it() {
        let mut ui = super::Ui::new();
        ui.transcript = super::Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 5,
        };
        ui.visible_text = vec!["hello world foo".to_string()];
        // Click within "world" (columns 6..11).
        super::select_word_at(&mut ui, 8, 0);
        let (anchor, cursor) = ui.selection.expect("a word was selected");
        assert_eq!(anchor, (0, 6));
        assert_eq!(cursor, (0, 11));
    }

    #[test]
    fn sparkline_handles_degenerate_inputs() {
        use super::sparkline;
        // Fewer than two points, or a flat series, has no shape to draw.
        assert_eq!(sparkline(&[], 8), None);
        assert_eq!(sparkline(&[1.0], 8), None);
        assert_eq!(sparkline(&[5.0, 5.0, 5.0], 8), None);
        // A real series renders one block per sampled point, low to high.
        let s = sparkline(&[1.0, 2.0, 3.0, 4.0], 4).expect("a sparkline");
        assert_eq!(s.chars().count(), 4);
        assert!(s.starts_with('▁') && s.ends_with('█'));
    }

    #[test]
    fn reference_url_prefers_top_level_then_content() {
        use super::reference_url;
        use longbridge::agent::Reference;
        let make = |v: serde_json::Value| -> Reference { serde_json::from_value(v).unwrap() };
        // Top-level url wins over content's source_url.
        let r = make(serde_json::json!({
            "url": "https://a.example",
            "content": {"source_url": "https://b.example"},
        }));
        assert_eq!(reference_url(&r).as_deref(), Some("https://a.example"));
        // Falls back to content's url when the top level is empty.
        let r2 = make(serde_json::json!({"content": {"url": "https://c.example"}}));
        assert_eq!(reference_url(&r2).as_deref(), Some("https://c.example"));
        // Nothing to open.
        assert_eq!(reference_url(&make(serde_json::json!({}))), None);
    }

    #[test]
    fn triple_click_selects_the_whole_line() {
        let mut ui = super::Ui::new();
        ui.transcript = super::Rect {
            x: 2,
            y: 0,
            width: 40,
            height: 5,
        };
        ui.visible_text = vec!["hello world".to_string()];
        super::select_line_at(&mut ui, 0);
        let (anchor, cursor) = ui.selection.expect("the line was selected");
        // Content coordinates: line 0, columns within the line (0..width), not
        // offset by the transcript's screen x.
        assert_eq!(anchor, (0, 0));
        assert_eq!(cursor, (0, 11));
    }

    /// A fresh conversation must not inherit the previous one's floating quote
    /// panel (its quote is cleared, so it would show "loading" forever), nor its
    /// sparkline paths or confirmed-ticker aliases.
    #[test]
    fn reset_render_drops_the_previous_conversations_overlay_state() {
        let mut ui = super::Ui::new();
        ui.quote_panel = Some("TSLA.US".into());
        ui.paths.insert("TSLA.US".into(), vec![1.0, 2.0]);
        ui.aliases.insert("SPCX".into(), "SPCX.US".into());
        ui.tape.push("TSLA.US".into());
        ui.reset_render();
        assert!(ui.quote_panel.is_none());
        assert!(ui.paths.is_empty());
        assert!(ui.aliases.is_empty());
        assert!(ui.tape.is_empty());
    }

    /// The quote panel overlays the chat; switching to another view (which cannot
    /// dismiss it) must take it down rather than strand it there.
    #[test]
    fn switching_away_from_chat_closes_the_quote_panel() {
        let mut ui = super::Ui::new();
        ui.quote_panel = Some("TSLA.US".into());
        ui.switch(super::View::Sessions);
        assert!(ui.quote_panel.is_none());
    }

    /// A transcript cell maps to a content line through the recorded view offset,
    /// with the column measured from the text start, not the screen edge.
    #[test]
    fn a_transcript_cell_maps_to_its_content_line() {
        let mut ui = super::Ui::new();
        ui.transcript = super::Rect {
            x: 3,
            y: 2,
            width: 40,
            height: 5,
        };
        ui.view_start = 7;
        ui.view_total = 30;
        // Third visible row (y+2), five columns past the text start (x+5).
        assert_eq!(super::content_at(&ui, 8, 4), (9, 5));
    }

    /// Dragging past an edge scrolls the transcript so the selection can extend
    /// beyond the visible page — up past the top, down past the bottom.
    #[test]
    fn dragging_past_an_edge_scrolls_to_extend_the_selection() {
        let mut ui = super::Ui::new();
        ui.transcript = super::Rect {
            x: 0,
            y: 1,
            width: 40,
            height: 5,
        };
        // Scrolled up: view_start = total - scroll - height = 40 - 25 - 5 = 10.
        ui.view_start = 10;
        ui.view_rows = 5;
        ui.view_total = 40;
        ui.max_scroll = 35;
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        state.scroll = 25;
        // Below the bottom edge (rows are y=1..6): scroll down, cursor advances.
        let (line, _) = super::drag_to_content(&mut ui, &mut state, 0, 9);
        assert!(state.scroll < 25, "dragging below the bottom scrolls down");
        assert!(
            line > 10,
            "and the cursor reaches newly-revealed lower content"
        );
        // Above the top edge: scroll back up, cursor retreats.
        state.scroll = 25;
        let (line, _) = super::drag_to_content(&mut ui, &mut state, 0, 0);
        assert!(state.scroll > 25, "dragging above the top scrolls up");
        assert!(
            line < 10,
            "and the cursor reaches newly-revealed upper content"
        );
    }

    /// A drag on a fresh session — where no transcript has been laid out, so
    /// `view_rows` is 0 while the transcript rect starts below row 0 — must not
    /// panic on the `clamp(top, top - 1)` the edge math would otherwise reach.
    #[test]
    fn dragging_on_an_unrendered_transcript_does_not_panic() {
        let mut ui = super::Ui::new();
        ui.transcript = super::Rect {
            x: 0,
            y: 2,
            width: 40,
            height: 6,
        };
        // view_rows / view_total left at their defaults of 0.
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        let (line, _) = super::drag_to_content(&mut ui, &mut state, 5, 4);
        assert_eq!(
            line, 0,
            "with nothing laid out, the drag maps to the first line"
        );
    }

    #[test]
    fn double_click_on_whitespace_selects_nothing() {
        let mut ui = super::Ui::new();
        ui.transcript = super::Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 5,
        };
        ui.visible_text = vec!["hi   there".to_string()];
        super::select_word_at(&mut ui, 3, 0); // a space
        assert!(ui.selection.is_none());
    }

    #[test]
    fn wrap_breaks_at_word_boundaries() {
        // "hello world" at width 8 breaks between words, not mid-word.
        assert_eq!(super::wrap("hello world", 8), vec!["hello", "world"]);
    }

    #[test]
    fn wrap_hard_breaks_an_overlong_word() {
        // A single token longer than the width still has to be split.
        assert_eq!(super::wrap("abcdefgh", 4), vec!["abcd", "efgh"]);
    }

    #[test]
    fn wrap_packs_cjk_to_width() {
        // Two full-width glyphs fill width 4, so they share a line.
        assert_eq!(super::wrap("你好世界", 4), vec!["你好", "世界"]);
    }

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
        assert_eq!(relative_time(0, 14 * 86_400), "2w");
        assert_eq!(relative_time(0, 800 * 86_400), "2y");
    }

    #[test]
    fn session_time_switches_to_dates_after_72_hours() {
        let now = time::macros::datetime!(2026-08-16 12:00 UTC)
            .unix_timestamp()
            .cast_unsigned();
        assert_eq!(
            super::session_time_label_at(now - (3 * 86_400 - 1), now, time::UtcOffset::UTC),
            "2d"
        );
        assert_eq!(
            super::session_time_label_at(now - 3 * 86_400, now, time::UtcOffset::UTC),
            "Aug 13"
        );
    }

    #[test]
    fn session_time_in_another_year_includes_the_year() {
        let now = time::macros::datetime!(2026-01-02 12:00 UTC)
            .unix_timestamp()
            .cast_unsigned();
        let updated = time::macros::datetime!(2025-12-29 12:00 UTC)
            .unix_timestamp()
            .cast_unsigned();
        assert_eq!(
            super::session_time_label_at(updated, now, time::UtcOffset::UTC),
            "Dec 29, 2025"
        );
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

    /// Serialises the tests that touch the ticker setting.
    ///
    /// It is a process-wide atomic — one setting for the whole terminal — so two
    /// tests toggling it in parallel see each other's value. The lock is the test
    /// harness's problem, not the setting's.
    static TAPE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Render the whole view into an in-memory backend and return its text rows.
    fn frame(ui: &mut super::Ui, state: &mut super::ChatState, w: u16, h: u16) -> Vec<String> {
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

    #[test]
    fn turn_errors_render_as_plain_red_text() {
        let message = super::Message::new(super::Role::Alert, "Cannot continue".into());
        let mut lines = Vec::new();
        super::push_message(
            &mut lines,
            &message,
            80,
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
        );
        assert_eq!(lines[0].spans[0].content, "Cannot continue");
        assert_eq!(lines[0].spans[0].style.fg, Some(ratatui::style::Color::Red));
    }

    /// A selection that runs off the top of the view still copies the lines that
    /// scrolled out of sight, not only what happens to be on screen — the whole
    /// point of dragging past the edge.
    #[test]
    fn a_selection_copies_lines_that_scrolled_off_screen() {
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        for i in 0..20 {
            state.apply(super::ChatEvent::UserPrompt(format!("Q{i}")));
            state.apply(super::ChatEvent::Delta(format!("ANSWER{i}")));
            state.apply(super::ChatEvent::TurnFinished { error: None });
        }
        let mut ui = super::Ui::new();
        // First render populates the transcript cache and the view metrics.
        frame(&mut ui, &mut state, 40, 8);
        assert!(
            ui.view_total > ui.view_rows,
            "the content must overflow the view for the top to be off screen"
        );
        // Select the whole transcript, top (off screen) to bottom.
        ui.selection = Some(((0, 0), (ui.view_total - 1, 200)));
        frame(&mut ui, &mut state, 40, 8);
        let copied = ui.selected_text.clone().unwrap_or_default();
        assert!(
            copied.contains("ANSWER0") && copied.contains("Q0"),
            "the earliest, off-screen lines are copied: {copied:?}"
        );
        assert!(
            copied.contains("ANSWER19"),
            "and so is the visible bottom: {copied:?}"
        );
    }

    #[test]
    fn find_matches_are_case_insensitive_and_ordered() {
        let lines: Vec<String> = ["Apple pie", "banana", "APPLE crumble"]
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(super::find_matches(&lines, "apple"), vec![0, 2]);
        assert!(super::find_matches(&lines, "").is_empty());
        assert!(super::find_matches(&lines, "   ").is_empty());
        assert!(super::find_matches(&lines, "xyz").is_empty());
    }

    #[test]
    fn scroll_to_line_brings_a_line_into_view_and_respects_the_cap() {
        // A mid-transcript line: a couple of rows of context above it.
        assert_eq!(super::scroll_to_line(100, 10, 90, 50), 42);
        // A line near the bottom pins to the latest.
        assert_eq!(super::scroll_to_line(100, 10, 90, 99), 0);
        // Never scrolls past the top.
        assert!(super::scroll_to_line(100, 10, 5, 0) <= 5);
    }

    /// Ctrl+F opens the find bar, typing filters, and the view scrolls up to a
    /// match buried early in a long transcript; Esc closes it.
    #[test]
    fn ctrl_f_scrolls_to_a_match_deep_in_the_transcript() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        for i in 0..20 {
            state.apply(super::ChatEvent::UserPrompt(format!("Q{i}")));
            state.apply(super::ChatEvent::Delta(format!("ANSWER about topic {i}")));
            state.apply(super::ChatEvent::TurnFinished { error: None });
        }
        let mut ui = super::Ui::new();
        let mut editor = super::Editor::new();
        let mut turn = None;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        // Render once so the text cache and view metrics are populated.
        frame(&mut ui, &mut state, 48, 10);
        let send = |ui: &mut super::Ui,
                    state: &mut super::ChatState,
                    editor: &mut super::Editor,
                    turn: &mut Option<tokio::task::JoinHandle<()>>,
                    code,
                    mods| {
            super::on_chat_key(KeyEvent::new(code, mods), ui, state, editor, turn, &tx);
        };
        send(
            &mut ui,
            &mut state,
            &mut editor,
            &mut turn,
            KeyCode::Char('f'),
            KeyModifiers::CONTROL,
        );
        assert!(ui.find.is_some(), "Ctrl+F opens the find bar");
        for c in "topic 3".chars() {
            send(
                &mut ui,
                &mut state,
                &mut editor,
                &mut turn,
                KeyCode::Char(c),
                KeyModifiers::NONE,
            );
        }
        let query = ui.find.as_ref().unwrap().query.clone();
        assert_eq!(query, "topic 3");
        assert_eq!(
            super::find_matches(&ui.cache_text, &query).len(),
            1,
            "only one line matches 'topic 3'"
        );
        assert!(state.scroll > 0, "the view scrolled up to the buried match");
        send(
            &mut ui,
            &mut state,
            &mut editor,
            &mut turn,
            KeyCode::Esc,
            KeyModifiers::NONE,
        );
        assert!(ui.find.is_none(), "Esc closes the find bar");
    }

    /// A conversation waiting on a structured question.
    fn asking() -> (super::Ui, super::ChatState) {
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        state.apply(super::ChatEvent::UserPrompt("compare NVDA and AMD".into()));
        state.apply(super::ChatEvent::Delta(
            "I can compare them on a few axes.".into(),
        ));
        state.apply(super::ChatEvent::TurnFinished { error: None });
        let mut ui = super::Ui::new();
        ui.view = super::View::Question;
        ui.question = Some(super::QuestionState {
            questions: vec![(
                "Which timeframe should the comparison use?".into(),
                vec![
                    "Past month".into(),
                    "Past year".into(),
                    "Year to date".into(),
                ],
            )],
            targets: vec![(
                "call_1".into(),
                "Which timeframe should the comparison use?".into(),
                None,
                false,
            )],
            qi: 0,
            answers: std::collections::HashMap::new(),
            summaries: Vec::new(),
            multi_selected: std::collections::HashMap::new(),
        });
        (ui, state)
    }

    /// The question is a drawer over the transcript, not a view replacing it: the
    /// answer it is asking about has to stay readable while the reader answers.
    #[test]
    fn a_question_rises_from_the_input_without_covering_the_chat() {
        let (mut ui, mut state) = asking();
        let rows = frame(&mut ui, &mut state, 72, 22);
        let text = rows.join("\n");
        assert!(
            text.contains("compare NVDA and AMD"),
            "the transcript is still there: {text}"
        );
        assert!(
            text.contains("Which timeframe"),
            "and so is the question: {text}"
        );
        // Bordered, and only the lower part of the screen.
        let top = rows
            .iter()
            .position(|r| r.contains('┌'))
            .expect("a framed drawer");
        assert!(
            top > 3,
            "the drawer stays low, opened at row {top} of {}",
            rows.len()
        );
        // Directly above the input, which is the last block on screen.
        let bottom = rows
            .iter()
            .rposition(|r| r.contains('└'))
            .expect("a closed frame");
        assert!(
            bottom < rows.len() - 3,
            "the drawer sits on the input, not over it: {bottom}"
        );
        assert!(
            bottom - top < rows.len() / 2,
            "and takes at most half the screen: rows {top}..={bottom}"
        );
        // Compact: every option on its own row, no blank rows between them.
        let first = rows
            .iter()
            .position(|r| r.contains("Past month"))
            .expect("first option");
        assert!(
            rows[first + 1].contains("Past year") && rows[first + 2].contains("Year to date"),
            "options are one per row: {text}"
        );
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
        let rows = frame(&mut ui, &mut busy_state(), 70, 16);
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
        let busy = frame(&mut ui, &mut busy_state(), 70, 16);
        assert!(
            busy.iter().any(|l| l.contains("[stop]")),
            "a running turn should offer [stop]:\n{}",
            busy.join("\n")
        );
        assert!(ui.stop_button.is_some(), "the button needs a hit rect");

        let mut idle = super::ChatState::new("chatbot".into(), "welcome".into());
        let mut ui = super::Ui::new();
        let rows = frame(&mut ui, &mut idle, 70, 16);
        assert!(
            !rows.iter().any(|l| l.contains("[stop]")),
            "no turn, no stop button:\n{}",
            rows.join("\n")
        );
        assert!(ui.stop_button.is_none());
    }

    /// While a turn runs the prompt box hugs the status row rather than leaving a
    /// second blank row between the spinner and the input.
    #[test]
    fn the_prompt_hugs_the_status_while_a_turn_runs() {
        let mut ui = super::Ui::new();
        let rows = frame(&mut ui, &mut busy_state(), 70, 16);
        let turn = rows
            .iter()
            .position(|l| l.contains("[stop]"))
            .expect("the turn row");
        let box_top = rows
            .iter()
            .position(|l| l.contains('╭'))
            .expect("the prompt box");
        assert!(box_top > turn, "the box sits below the turn row");
        assert!(
            box_top - turn <= 2,
            "at most one blank row between the spinner and the prompt, got {}:\n{}",
            box_top - turn,
            rows.join("\n")
        );
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

    /// The `/` command palette's selection wraps: ↑ past the top lands on the last
    /// command and ↓ past the bottom on the first.
    #[test]
    fn the_command_palette_selection_wraps() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let plain = |code| KeyEvent::new(code, KeyModifiers::empty());
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        let mut editor = super::Editor::new();
        editor.set_text("/");
        let mut turn = None;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let count = super::slash_matches(&editor).len();
        assert!(count > 1, "several commands match a bare slash");
        ui.slash_sel = 0;
        super::on_chat_key(
            plain(KeyCode::Up),
            &mut ui,
            &mut state,
            &mut editor,
            &mut turn,
            &tx,
        );
        assert_eq!(ui.slash_sel, count - 1, "Up past the top wraps to the last");
        super::on_chat_key(
            plain(KeyCode::Down),
            &mut ui,
            &mut state,
            &mut editor,
            &mut turn,
            &tx,
        );
        assert_eq!(ui.slash_sel, 0, "Down past the bottom wraps to the first");
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
        let rows = frame(&mut ui, &mut state, 70, 20);
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
        let rows = frame(&mut ui, &mut state, 70, 14);
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
    /// Scrolled up mid-stream, the visible window must stay anchored to the same
    /// content as new answer lines are appended below — not drift downward.
    #[test]
    fn a_scrolled_view_stays_anchored_while_streaming() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        for i in 0..40 {
            state
                .messages
                .push(super::Message::new(super::Role::User, format!("line {i}")));
        }
        state.scroll = 10;
        let before = frame(&mut ui, &mut state, 40, 12);
        // A streaming answer grows the transcript beneath the scrolled window.
        state.streaming = Some("new answer line".into());
        state.busy = true;
        let after = frame(&mut ui, &mut state, 40, 12);
        assert_eq!(
            before[1], after[1],
            "the top visible row should not move as lines are appended below"
        );
    }

    #[test]
    fn the_title_bar_is_separated_from_the_transcript() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        let rows = frame(&mut ui, &mut state, 70, 24);
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
        // The percent drops its sign; a ▲/▼ arrow carries the direction instead.
        assert!(text.contains("182.4") && text.contains("▲") && text.contains("2.10%"));
        assert!(
            text.contains("179.2"),
            "the day's range belongs on the card"
        );
        assert!(
            (text.contains('┌') || text.contains('╭'))
                && (text.contains('└') || text.contains('╰')),
            "it is a box"
        );
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
        // The direction rides an arrow, so the percent shows without its sign.
        assert!(text.contains("▼1.59%") && text.contains("▲2.10%"));
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
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());

        let mut ui = super::Ui::new();
        let tall = frame(&mut ui, &mut state, 80, (logo_h + 20) as u16);
        assert!(
            tall.iter().any(|l| l.contains('█')),
            "a tall terminal should show the mark:\n{}",
            tall.join("\n")
        );

        let mut ui = super::Ui::new();
        let short = frame(&mut ui, &mut state, 80, 14);
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
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        for width in [40u16, 60, 80, 120] {
            let mut ui = super::Ui::new();
            for line in frame(&mut ui, &mut state, width, 40) {
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
        let _ = frame(&mut ui, &mut state, 64, 20);
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
        let rows = frame(&mut ui, &mut state, 64, 20);
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
        let _ = frame(&mut ui, &mut state, 70, 20);
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
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        let rows = frame(&mut ui, &mut state, 74, 24);
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
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        let text = frame(&mut ui, &mut state, 74, 24).join("\n");
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
        let _guard = TAPE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::ai::settings::set_tape(true);
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
        let first = frame(&mut ui, &mut state, 78, 10)[0].clone();
        assert!(first.contains("700.HK 512.5 ▲1.28%"), "{first}");
        // More than fits, so the ticker rotates rather than truncating.
        assert!(
            !first.contains("9988.HK"),
            "the row cannot hold them all: {first}"
        );
        // Pretend the page has been up for its full dwell. The frame that finds it
        // due draws the page it is replacing, so the new one lands on the next.
        ui.tape_shown_at = std::time::Instant::now().checked_sub(super::TAPE_DWELL);
        let _ = frame(&mut ui, &mut state, 78, 10);
        let rotated = frame(&mut ui, &mut state, 78, 10)[0].clone();
        assert_ne!(first, rotated, "the ticker should have advanced");
        assert!(
            !rotated.contains("700.HK") && !rotated.contains("AAPL.US"),
            "and by a page, not a symbol, so the whole set turns over: {rotated}"
        );
    }

    /// A ticker that fits sits still, and the toggle turns it off — the row is
    /// chrome, and chrome that moves for no reason is a distraction.
    #[test]
    fn a_short_ticker_does_not_rotate_and_can_be_turned_off() {
        let _guard = TAPE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::ai::settings::set_tape(true);
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        state.apply(super::ChatEvent::UserPrompt("700.HK".into()));
        state.apply(super::ChatEvent::TurnFinished { error: None });
        super::track_session_symbols(&mut ui, &state);
        ui.quotes
            .insert("700.HK".into(), card("700.HK", "512.5", "+1.28%", 1));
        let before = frame(&mut ui, &mut state, 78, 10)[0].clone();
        ui.tape_shown_at = std::time::Instant::now().checked_sub(super::TAPE_DWELL);
        let _ = frame(&mut ui, &mut state, 78, 10);
        assert_eq!(
            before,
            frame(&mut ui, &mut state, 78, 10)[0],
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
        let off = frame(&mut ui, &mut state, 78, 10)[0].clone();
        assert!(!off.contains("512.5"), "collapsed: {off}");
        crate::ai::settings::set_tape(true);
    }

    /// A bare ticker the server confirmed is a link, priced like a dotted one,
    /// and clicking it opens the full symbol.
    #[test]
    fn a_confirmed_bare_ticker_behaves_like_a_symbol() {
        // Its price is read off the ticker, which another test toggles.
        let _guard = TAPE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::ai::settings::set_tape(true);
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
        let rows = frame(&mut ui, &mut state, 70, 16);
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
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        let rows = frame(&mut ui, &mut state, 60, 16);
        let prompt = rows
            .iter()
            .rev()
            .find(|r| r.contains(t!("Ai.Placeholder").as_ref()))
            .expect("the placeholder should be on screen");
        assert!(prompt.contains("❯ "), "marked: {prompt:?}");
        // A rule above and below, no sides: they cost the prompt the column that
        // lines it up with the same words once they are in the transcript.
        // The box stays, dim, with a blank row between it and the transcript.
        let top = rows
            .iter()
            .position(|r| r.contains('╭'))
            .expect("the input keeps its frame");
        assert!(
            rows[top - 1].trim().is_empty(),
            "a blank row above it: {rows:?}"
        );
    }

    /// A long single line of input follows the caret: the end being typed is on
    /// screen even though it sits far past the box width, rather than clipped with
    /// the cursor frozen at the border.
    #[test]
    fn a_long_prompt_line_scrolls_to_follow_the_caret() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        let mut editor = super::Editor::new();
        // A line far wider than the 40-column frame, with a distinct tail so its
        // visibility can only come from horizontal scrolling.
        editor.insert_str(&format!("{}END", "a".repeat(80)));
        let backend = ratatui::backend::TestBackend::new(40, 16);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal
            .draw(|f| super::view(f, &mut ui, &mut state, &editor))
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        let joined: String = (0..buf.area.height)
            .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
            .map(|(x, y)| buf[(x, y)].symbol().to_string())
            .collect();
        assert!(
            joined.contains("END"),
            "the caret end of a long line should be visible: {joined:?}"
        );
    }

    /// A security reads as a cashtag: an accented `$` and a muted symbol. No price
    /// in the sentence — the ticker carries that, and a quote arriving used to
    /// move the prose under the reader.
    #[test]
    fn a_security_reads_as_a_cashtag() {
        let mut quotes = std::collections::HashMap::new();
        quotes.insert("700.HK".to_string(), card("700.HK", "512.5", "+1.28%", 1));
        let lines = super::render_answer_lines(
            "700.HK 走强。",
            60,
            &quotes,
            &std::collections::HashMap::new(),
        );
        let spans: Vec<&ratatui::text::Span> = lines.iter().flat_map(|l| &l.spans).collect();
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("$700.HK"), "the cashtag: {text}");
        assert!(
            !text.contains("512.5"),
            "the price belongs on the ticker, not in the sentence: {text}"
        );
        let sigil = spans
            .iter()
            .find(|s| s.content.as_ref() == "$")
            .expect("the sigil should be its own span");
        assert_eq!(sigil.style.fg, Some(super::CASHTAG));
        let symbol = spans
            .iter()
            .find(|s| s.content.as_ref() == "700.HK")
            .expect("the symbol should be its own span");
        assert_eq!(symbol.style.fg, Some(super::SYMBOL_FG));
    }

    /// The `$` costs a column, so it goes in before the wrap: added afterwards it
    /// would push the end of a full line off the screen.
    #[test]
    fn the_cashtag_is_inside_the_width() {
        let width = 40;
        let answer = "本周 700.HK 与 AAPL.US 同步走强，而 NVDA.US 回落，建议关注成交量变化。";
        let lines = super::render_answer_lines(
            answer,
            width,
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
        );
        for line in &lines {
            let w: usize = line
                .spans
                .iter()
                .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            assert!(w <= width, "line of {w} columns: {line:?}");
        }
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert_eq!(text.matches('$').count(), 3, "one per security: {text}");
    }

    /// A cashtag the author already wrote does not get a second `$`.
    #[test]
    fn an_authored_cashtag_is_left_alone() {
        let out = super::cashtag_annotated("$AAPL.US 与 700.HK", &std::collections::HashMap::new());
        assert_eq!(out, "$AAPL.US 与 $700.HK");
    }

    #[test]
    fn a_ticker_in_inline_code_keeps_verbatim() {
        // A ticker inside `code` is already set apart; it must not gain a `$`.
        let out =
            super::cashtag_annotated("see `700.HK` and 700.HK", &std::collections::HashMap::new());
        assert_eq!(out, "see `700.HK` and $700.HK");
    }

    /// A comparison is a card, and its columns line up: reading a set of prices
    /// down the page is the whole point of the widget. It used to be a header at
    /// one indent and rows at another, with the prices left to a fixed `{:>10}`.
    #[test]
    fn a_comparison_is_a_framed_card_with_aligned_columns() {
        let mut quotes = std::collections::HashMap::new();
        for (symbol, name, last, pct) in [
            ("QQQ.US", "纳指 100 ETF - Invesco", "729.615", "-0.33%"),
            ("SPY.US", "标普 500 ETF - SPDR", "776.085", "-0.23%"),
        ] {
            let mut c = card(symbol, last, pct, -1);
            c.name = name.to_string();
            quotes.insert(symbol.to_string(), c);
        }
        let lines = super::comparison_card(
            "Comparison",
            &["QQQ.US".to_string(), "SPY.US".into(), "IWM.US".into()],
            72,
            &quotes,
        );
        let widths: Vec<usize> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                    .sum()
            })
            .collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "every row of the frame is the same width: {widths:?}"
        );
        assert!(widths[0] <= 72, "and inside the pane: {widths:?}");
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(
            text[0].starts_with('┌') || text[0].starts_with('╭'),
            "framed: {text:?}"
        );
        // The prices start at the same column on every row.
        let at = |row: &str, needle: &str| row.find(needle).map(|i| row[..i].chars().count());
        assert_eq!(
            at(&text[2], "729.615"),
            at(&text[3], "776.085"),
            "prices share a column: {text:?}"
        );
        // A security with no quote yet keeps its place rather than moving the card
        // when the quote lands.
        assert!(text[4].contains("$IWM.US"), "{text:?}");
    }

    /// The card's symbol is its own span, so it is a click target even when the
    /// security has a name — bundling the two cost the card its target.
    #[test]
    fn a_named_card_is_still_a_click_target() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        state.apply(super::ChatEvent::UserPrompt("700.HK?".into()));
        state.apply(super::ChatEvent::Delta(
            "<x-widget src=\"widget://quote/security/detail?symbol=700.HK\"></x-widget>".into(),
        ));
        state.apply(super::ChatEvent::TurnFinished { error: None });
        let mut c = card("700.HK", "512.5", "+1.28%", 1);
        c.name = "腾讯控股".into();
        ui.quotes.insert("700.HK".into(), c);
        let _ = frame(&mut ui, &mut state, 70, 20);
        let tall = ui
            .chips
            .iter()
            .filter(|(chip, _)| matches!(chip, super::Chip::Symbol(s) if s == "700.HK"))
            .map(|(_, rect)| rect.height)
            .max()
            .unwrap_or(0);
        assert!(
            tall > 1,
            "the whole card should be clickable, got {tall} rows"
        );
    }

    /// The prose carries the full symbol, market and all: the same four letters
    /// list in more than one market, so half a symbol is ambiguous.
    #[test]
    fn the_prose_widens_a_bare_ticker_to_its_full_symbol() {
        let mut aliases = std::collections::HashMap::new();
        aliases.insert("TSLA".to_string(), "TSLA.US".to_string());
        let out = super::cashtag_annotated("看 TSLA 和 700.HK", &aliases);
        assert_eq!(out, "看 $TSLA.US 和 $700.HK");
    }

    /// Collapsed, the control is the only thing saying the ticker exists, so it is
    /// labelled; open, the quotes speak for themselves and it is only a way out.
    #[test]
    fn the_ticker_control_is_labelled_only_when_collapsed() {
        let _guard = TAPE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::ai::settings::set_tape(true);
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        // Nothing to collapse: no control at all.
        let bare = frame(&mut ui, &mut state, 72, 12)[0].clone();
        assert!(
            !bare.contains(t!("Ai.TapeToggle").as_ref()),
            "no ticker, no control: {bare:?}"
        );
        ui.tape.push("700.HK".into());
        let open = frame(&mut ui, &mut state, 72, 12)[0].clone();
        assert!(
            !open.contains(t!("Ai.TapeToggle").as_ref()),
            "open, the label is dead weight: {open:?}"
        );
        assert!(!open.contains('['), "and no brackets: {open:?}");
        crate::ai::settings::set_tape(false);
        let shut = frame(&mut ui, &mut state, 72, 12)[0].clone();
        assert!(
            shut.contains(t!("Ai.TapeToggle").as_ref()),
            "collapsed, the label is what says the ticker is there: {shut:?}"
        );
        assert!(
            !shut.contains("700.HK"),
            "and the quotes are gone: {shut:?}"
        );
        crate::ai::settings::set_tape(true);
    }

    /// Every ticker entry is a button: the row is how a reader reaches a security
    /// they are not currently reading about.
    #[test]
    fn ticker_entries_open_their_security() {
        let _guard = TAPE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::ai::settings::set_tape(true);
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        for symbol in ["700.HK", "TSLA.US"] {
            ui.tape.push(symbol.into());
        }
        let rows = frame(&mut ui, &mut state, 72, 12);
        for symbol in ["700.HK", "TSLA.US"] {
            let (_, rect) = ui
                .chips
                .iter()
                .find(|(chip, _)| matches!(chip, super::Chip::Symbol(s) if s == symbol))
                .unwrap_or_else(|| panic!("no click target for {symbol}"));
            assert_eq!(rect.y, 0, "the ticker lives on the title row");
            // The rect covers the symbol where it was actually drawn.
            let at = rows[0].find(symbol).expect("drawn");
            assert!(
                (rect.x as usize) <= at && at < (rect.x + rect.width) as usize,
                "{symbol} drawn at {at} but targeted at {rect:?}"
            );
        }
    }

    /// The dialog is sized to its content, with the title, the live dot and both
    /// hints in the border, so every row inside carries data.
    #[test]
    fn the_quote_dialog_is_sized_to_its_content() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        let mut c = card("SPCX.US", "139.239", "-1.45%", -1);
        c.name = "SpaceX".into();
        ui.quotes.insert("SPCX.US".into(), c);
        ui.quote_panel = Some("SPCX.US".into());
        let rows = frame(&mut ui, &mut state, 100, 20);
        let framed: Vec<&String> = rows.iter().filter(|r| r.contains('│')).collect();
        assert!(!framed.is_empty(), "the dialog should be drawn: {rows:?}");
        let text = rows.join("\n");
        assert!(text.contains("$SPCX.US"), "the title: {text}");
        // No `[Close]` button: the panel is dismissed by a click outside it or Esc,
        // so it registers no close rect.
        assert!(
            ui.close_button.is_none(),
            "the panel carries no close button"
        );
        // The way out to the web is an icon on the frame, not a line of prose.
        assert!(text.contains(super::WEB_ICON), "{text}");
        // Content-sized: nowhere near the 100 columns available. Measured by
        // trimming the drawer's own top border out of its row — it is right-aligned,
        // so both the leading gap and any trailing space have to go.
        let top = rows
            .iter()
            .find(|r| r.contains(super::WEB_ICON))
            .expect("the top border carries the icon");
        let widest = unicode_width::UnicodeWidthStr::width(top.trim());
        assert!(widest < 80, "sized to content, got {widest} of 100");
        // A drawer: it hangs from the top of the transcript rather than floating in
        // the middle, so its top border is one of the first rows, not a centred one.
        let top_row = rows
            .iter()
            .position(|r| r.contains(super::WEB_ICON))
            .expect("the drawer is drawn");
        assert!(
            top_row <= 2,
            "the drawer drops from the top, at row {top_row}"
        );
        // The web link is a button, positioned on the top border.
        let button = ui.open_button.expect("the link should be clickable");
        assert!(button.height == 1 && button.width > 0);
    }

    /// The drawer opens at a fixed height and anchors under the security that was
    /// clicked: it must not grow as the detail lands (the flash), and it drops from
    /// the clicked column, not the corner.
    #[test]
    fn the_drawer_is_stable_and_anchors_under_the_click() {
        let render = |with_detail: bool, anchor: Option<u16>| -> Vec<String> {
            let mut ui = super::Ui::new();
            ui.quote_anchor_x = anchor;
            let mut state = super::ChatState::new("c".into(), "w".into());
            ui.quotes.insert(
                "NVDA.US".into(),
                sample_card("NVDA.US", "英伟达", "182.4", "+3.75", "+2.10%", 1),
            );
            if with_detail {
                ui.details.insert(
                    "NVDA.US".into(),
                    crate::ai::quotes::QuoteDetail {
                        avg: Some("181.2".into()),
                        pe_ttm: Some("48.3".into()),
                        ..Default::default()
                    },
                );
            }
            ui.quote_panel = Some("NVDA.US".into());
            frame(&mut ui, &mut state, 100, 24)
        };
        let bars = |rows: &[String]| rows.iter().filter(|r| r.contains('│')).count();
        assert_eq!(
            bars(&render(false, None)),
            bars(&render(true, None)),
            "the drawer must not grow when detail lands"
        );
        // Anchored under the clicked column: its left border corner sits there.
        let rows = render(true, Some(30));
        let top = rows.iter().find(|r| r.contains('┌')).expect("a top border");
        assert_eq!(
            top.find('┌'),
            Some(30),
            "the drawer opens under the clicked column: {top}"
        );
    }

    /// The richer figures render once they arrive, and only the ones a market
    /// returned: a `None` field adds no row rather than an empty one.
    #[test]
    fn the_panel_shows_the_richer_figures_when_present() {
        let card = sample_card("NVDA.US", "英伟达", "182.4", "+3.75", "+2.10%", 1);
        let detail = crate::ai::quotes::QuoteDetail {
            avg: Some("181.2".into()),
            amplitude: Some("2.51%".into()),
            pe_ttm: Some("48.3".into()),
            market_cap: Some("4.47B".into()),
            eps_ttm: Some("3.78".into()),
            ..Default::default()
        };
        let text: String = super::card_lines(&card, &[], Some(&detail))
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        for value in ["181.2", "2.51%", "48.3", "4.47B", "3.78"] {
            assert!(text.contains(value), "missing {value} in:\n{text}");
        }
        // Nothing came back for PB, so its label never appears.
        assert!(
            !text.contains(t!("Ai.QuotePb").as_ref()),
            "an absent field must not draw a row:\n{text}"
        );
        // And with no detail at all, only the core six rows are drawn.
        let plain: String = super::card_lines(&card, &[], None)
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!plain.contains("181.2"), "no detail rows without a detail");
    }

    /// Left/Right walk the conversation's securities in place and wrap at the ends;
    /// a security the tape never named leaves the panel where it is.
    #[test]
    fn the_panel_steps_through_the_sessions_securities() {
        let mut ui = super::Ui::new();
        ui.tape = vec!["AAPL.US".into(), "NVDA.US".into(), "TSLA.US".into()];
        ui.quote_panel = Some("NVDA.US".into());

        super::step_quote_panel(&mut ui, 1);
        assert_eq!(ui.quote_panel.as_deref(), Some("TSLA.US"), "forward");
        super::step_quote_panel(&mut ui, 1);
        assert_eq!(
            ui.quote_panel.as_deref(),
            Some("AAPL.US"),
            "wraps past the end"
        );
        super::step_quote_panel(&mut ui, -1);
        assert_eq!(ui.quote_panel.as_deref(), Some("TSLA.US"), "wraps back");

        // A symbol not on the tape (a bare /quote) has no neighbours to step to.
        ui.quote_panel = Some("SPCX.US".into());
        super::step_quote_panel(&mut ui, 1);
        assert_eq!(
            ui.quote_panel.as_deref(),
            Some("SPCX.US"),
            "off-tape is a no-op"
        );

        // And a lone security stays put rather than stepping onto itself.
        ui.tape = vec!["AAPL.US".into()];
        ui.quote_panel = Some("AAPL.US".into());
        super::step_quote_panel(&mut ui, 1);
        assert_eq!(ui.quote_panel.as_deref(), Some("AAPL.US"), "nowhere to go");
    }

    /// Stepping pages the ticker only when the next security is off the visible
    /// window, so the drawer's tab and the ticker stay lined up: a step within the
    /// page holds the ticker still, a step past its edge slides it just enough.
    #[test]
    fn stepping_pages_the_ticker_only_when_the_next_stock_is_off_screen() {
        let mut ui = super::Ui::new();
        for i in 0..8 {
            ui.tape.push(format!("SYM{i}.US"));
        }
        // Three of the eight are on screen, starting at the first.
        ui.tape_drawn = 3;
        ui.tape_at = 0;
        ui.quote_panel = Some("SYM1.US".into());
        // SYM2 is already visible, so the ticker does not move.
        super::step_quote_panel(&mut ui, 1);
        assert_eq!(ui.quote_panel.as_deref(), Some("SYM2.US"));
        assert_eq!(
            ui.tape_at, 0,
            "an on-screen neighbour holds the ticker still"
        );
        // Stepping back from the first entry wraps to the last, which is off screen,
        // so the ticker pages to bring it on.
        ui.tape_at = 0;
        ui.tape_drawn = 3;
        ui.quote_panel = Some("SYM0.US".into());
        super::step_quote_panel(&mut ui, -1);
        assert_eq!(ui.quote_panel.as_deref(), Some("SYM7.US"));
        assert_eq!(ui.tape_at, 7, "an off-screen entry pages the ticker to it");
    }

    /// The web page has the chart and the filings the panel cannot hold.
    #[test]
    fn the_dialog_links_to_the_web_page() {
        assert_eq!(
            super::quote_web_url("700.HK"),
            "https://longbridge.com/quote/700.HK"
        );
    }

    /// The chat opens signed out — signing in is the one thing you would come here
    /// to do without a token — so a prompt says what to do instead of the send
    /// failing somewhere deeper.
    #[test]
    fn a_signed_out_chat_asks_you_to_sign_in_instead_of_sending() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        let mut editor = super::Editor::new();
        editor.set_text("700.HK 怎么样");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut turn = None;
        super::submit(&mut ui, &mut state, &mut editor, &mut turn, &tx);
        // No credentials in a test process, so the turn must not have been spawned.
        assert!(turn.is_none(), "nothing was sent");
        assert_eq!(ui.notice.as_deref(), Some(t!("Ai.SignInToAsk").as_ref()));
        assert!(
            state.messages.iter().all(|m| m.role != super::Role::User),
            "and the prompt was not added to the transcript"
        );
    }

    /// Signing in happens in the chat: a panel with the URL and the code, and the
    /// chat still there behind it. Being thrown out to a shell was the thing that
    /// made signing in feel like leaving.
    #[test]
    fn signing_in_is_shown_in_a_panel() {
        let mut ui = super::Ui::new();
        ui.login = Some(super::LoginPrompt {
            url: "https://longbridge.com/oauth/device?user_code=WDJB-MJHT".into(),
            code: "WDJB-MJHT".into(),
            browser_opened: true,
        });
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        let rows = frame(&mut ui, &mut state, 84, 20);
        let text = rows.join("\n");
        assert!(
            text.contains("longbridge.com/oauth/device"),
            "the URL: {text}"
        );
        assert!(text.contains("WDJB-MJHT"), "the code: {text}");
        assert!(text.contains(t!("Ai.LoginWaiting").as_ref()), "{text}");
        assert!(
            text.contains(t!("Ai.CloseButton").as_ref()),
            "a way out: {text}"
        );
        // Esc cancels the sign-in without touching the input.
        let mut editor = super::Editor::new();
        editor.set_text("half a question");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut turn = None;
        let mut state = state;
        super::on_chat_key(
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Esc,
                crossterm::event::KeyModifiers::NONE,
            ),
            &mut ui,
            &mut state,
            &mut editor,
            &mut turn,
            &tx,
        );
        assert!(ui.login.is_none(), "cancelled");
        assert_eq!(editor.text(), "half a question", "the input is untouched");
    }

    /// The welcome screen's examples are the quickest way in, so they are buttons.
    #[test]
    fn a_welcome_example_sends_itself() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        let _ = frame(&mut ui, &mut state, 80, 24);
        let samples: Vec<&'static str> = ui
            .chips
            .iter()
            .filter_map(|(chip, _)| match chip {
                super::Chip::Sample(key) => Some(*key),
                _ => None,
            })
            .collect();
        assert_eq!(samples.len(), 3, "every example is clickable: {samples:?}");
        // And the badge opens Longbridge AI on the web.
        assert!(
            ui.chips
                .iter()
                .any(|(chip, _)| matches!(chip, super::Chip::Brand)),
            "the badge should be a link"
        );
    }

    /// Typing while a turn runs used to start a second one on the same conversation.
    #[test]
    fn a_prompt_typed_mid_turn_joins_a_queue() {
        let mut ui = super::Ui::new();
        let mut state = busy_state();
        let mut editor = super::Editor::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut turn = None;
        editor.set_text("那 NVDA 呢？");
        // Through the key handler: the guard that dropped Enter while busy lived
        // there, so a test calling `submit` directly would not have seen it.
        super::on_key(
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
            ),
            &mut ui,
            &mut state,
            &mut editor,
            &mut turn,
            &tx,
        );
        assert_eq!(state.queued, vec!["那 NVDA 呢？".to_string()]);
        assert!(turn.is_none(), "nothing was sent yet");
        assert!(editor.is_blank(), "and the input is clear for the next one");
        // On screen, dim, so the reader can see what they have lined up.
        let text = frame(&mut ui, &mut state, 60, 16).join("\n");
        assert!(text.contains("NVDA 呢"), "{text}");
        // Cancelling means stop, not "stop this one and start the next".
        state.cancel("(cancelled)");
        assert!(state.queued.is_empty());
    }

    /// A folded paste is sent together with the typed prompt, and the input is
    /// cleared afterwards — the whole paste→submit path end to end.
    #[test]
    fn a_folded_paste_is_sent_with_the_typed_prompt() {
        let mut ui = super::Ui::new();
        // The busy path queues without needing credentials, so it exercises the
        // submit logic offline.
        let mut state = busy_state();
        let mut editor = super::Editor::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut turn = None;
        let big = (0..12)
            .map(|i| format!("row {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        editor.paste(&big);
        for c in "explain this".chars() {
            editor.insert_char(c);
        }
        // The box shows only the typed text; the paste is a chip.
        assert_eq!(editor.text(), "explain this");
        assert_eq!(editor.attachments().len(), 1);
        super::on_key(
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
            ),
            &mut ui,
            &mut state,
            &mut editor,
            &mut turn,
            &tx,
        );
        assert_eq!(state.queued.len(), 1);
        let sent = &state.queued[0];
        assert!(
            sent.contains("explain this") && sent.contains("row 11"),
            "the paste is folded back into the sent prompt: {sent}"
        );
        assert!(editor.is_blank(), "the input clears after send");
    }

    /// `/retry` will not start a second turn while one is already running; it says
    /// to wait instead.
    #[test]
    fn retry_refuses_while_a_turn_is_running() {
        let mut ui = super::Ui::new();
        let mut state = busy_state();
        let mut turn = None;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        super::retry_last(&mut ui, &mut state, &mut turn, &tx);
        assert!(turn.is_none(), "no turn was started");
        assert_eq!(ui.notice.as_deref(), Some(t!("Ai.RetryBusy").as_ref()));
    }

    /// A run of tool rows is a block, and a block needs air on both sides — the
    /// answer used to begin on the line straight after the last tool.
    #[test]
    fn a_tool_run_is_separated_from_the_answer_below_it() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        state.apply(super::ChatEvent::UserPrompt("看看 SPCX".into()));
        for name in ["Get Real Time Quote", "List Ticker News"] {
            state.apply(super::ChatEvent::ToolStarted(name.into()));
            state.apply(super::ChatEvent::ToolFinished {
                name: name.into(),
                ok: true,
            });
        }
        state.apply(super::ChatEvent::Delta("Answer.".into()));
        state.apply(super::ChatEvent::TurnFinished { error: None });
        let rows = frame(&mut ui, &mut state, 60, 16);
        let last_tool = rows
            .iter()
            .rposition(|r| r.contains("List Ticker News"))
            .expect("the tools are listed");
        assert!(
            rows[last_tool + 1].trim().is_empty(),
            "a blank row under the block: {rows:?}"
        );
        assert!(
            rows[last_tool - 1].contains("Get Real Time Quote"),
            "but the rows themselves stay tight: {rows:?}"
        );
    }

    /// A view opened by clicking should close by clicking too.
    #[test]
    fn a_list_view_closes_from_its_header() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        ui.view = super::View::Sessions;
        let rows = frame(&mut ui, &mut state, 64, 12);
        let rect = ui.close_button.expect("a way out that is not only a key");
        assert_eq!(rect.y, 0, "it sits on the header row");
        let label = format!("[{}]", t!("Ai.CloseButton"));
        let at = rows[0].find(&label).expect("drawn as a button");
        assert!(
            (rect.x as usize) <= at && at < (rect.x + rect.width) as usize,
            "drawn at {at} but targeted at {rect:?}"
        );
        ui.switch(super::View::Chat);
        assert!(ui.view == super::View::Chat, "and it goes back to the chat");
    }

    #[test]
    fn a_list_views_close_button_is_flush_right() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        ui.view = super::View::Sessions;
        let width = 64usize;
        let rows = frame(&mut ui, &mut state, width as u16, 12);
        assert_eq!(rows[0].chars().nth(width - 1), Some(']'), "{:?}", rows[0]);
        let rect = ui.close_button.expect("the visible close button");
        assert_eq!(rect.x + rect.width, width as u16);
    }

    /// The hint under a list is not one more row of the list.
    #[test]
    fn a_list_views_hint_stands_off_the_rows() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        ui.view = super::View::Sessions;
        ui.sessions = (0..9)
            .map(|i| super::SessionSummary {
                id: format!("s{i}"),
                updated_at: 1_786_700_000,
                title: format!("chat {i}"),
                agent: String::new(),
            })
            .collect();
        let rows = frame(&mut ui, &mut state, 64, 12);
        let hint = rows
            .iter()
            .position(|r| r.contains(t!("Ai.SessionsHint").split(' ').next().unwrap()))
            .expect("the hint is on screen");
        assert!(
            rows[hint - 1].trim().is_empty(),
            "a blank row above it: {rows:?}"
        );
        assert!(
            rows[hint - 2].contains("chat"),
            "and the list right above that: {rows:?}"
        );
    }

    #[test]
    fn a_session_row_shows_age_without_the_agent_name() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        ui.view = super::View::Sessions;
        ui.sessions = vec![super::SessionSummary {
            id: "chat-1".into(),
            updated_at: super::now_secs().saturating_sub(60),
            title: "Market outlook".into(),
            agent: "LongbridgeAI".into(),
        }];

        let text = frame(&mut ui, &mut state, 72, 12).join("\n");
        assert!(text.contains("Market outlook"), "title: {text}");
        assert!(text.contains("1m"), "relative update time: {text}");
        assert!(!text.contains("LongbridgeAI"), "redundant agent: {text}");
    }

    /// A view that carries its own name takes the title bar's rows: two titles, one
    /// above the other, read as a nesting that is not there.
    #[test]
    fn a_list_view_replaces_the_title_bar_rather_than_stacking_under_it() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        ui.view = super::View::Sessions;
        let rows = frame(&mut ui, &mut state, 72, 12);
        assert!(
            !rows.join("\n").contains(t!("Ai.Title").as_ref()),
            "the brand title bar is covered: {rows:?}"
        );
        assert!(
            rows[0]
                .trim_start()
                .starts_with(t!("Ai.TabSessions").as_ref()),
            "and the view's own name is the top row: {:?}",
            rows[0]
        );
    }

    /// Earlier conversations were reachable only by knowing to type `/resume`.
    #[test]
    fn the_title_bar_offers_the_conversations() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        let rows = frame(&mut ui, &mut state, 76, 12);
        let (_, rect) = ui
            .chips
            .iter()
            .find(|(chip, _)| matches!(chip, super::Chip::Sessions))
            .expect("a control for the conversations");
        assert_eq!(rect.y, 0, "it lives on the title row");
        let at = rows[0].find(t!("Ai.ChatsButton").as_ref()).expect("drawn");
        assert!(
            (rect.x as usize) <= at && at < (rect.x + rect.width) as usize,
            "drawn at {at} but targeted at {rect:?}"
        );
    }

    /// `/help` used to append itself to the conversation, where it could not be
    /// dismissed. It is a panel now, and the transcript is left alone.
    #[test]
    fn help_opens_as_a_panel_and_leaves_the_transcript_alone() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        let before = state.messages.len();
        super::exec_slash("help", "", &mut ui, &mut state);
        assert_eq!(ui.help, Some(0), "the panel is open");
        assert_eq!(state.messages.len(), before, "and nothing was written");
        let text = frame(&mut ui, &mut state, 80, 30).join("\n");
        assert!(text.contains(t!("Ai.Help").as_ref()), "{text}");
        assert!(text.contains("/resume"), "the commands are listed: {text}");
        assert!(text.contains("Shift+Enter"), "and the keys: {text}");
    }

    /// Anything that is not scrolling dismisses it — including Esc, which is what a
    /// reader will reach for.
    #[test]
    fn help_scrolls_and_any_other_key_dismisses_it() {
        let mut ui = super::Ui::new();
        let mut state = super::ChatState::new("chatbot".into(), "welcome".into());
        let mut editor = super::Editor::new();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut turn = None;
        ui.help = Some(0);
        macro_rules! press {
            ($code:expr) => {
                super::on_key(
                    crossterm::event::KeyEvent::new($code, crossterm::event::KeyModifiers::NONE),
                    &mut ui,
                    &mut state,
                    &mut editor,
                    &mut turn,
                    &tx,
                )
            };
        }
        press!(crossterm::event::KeyCode::Down);
        assert_eq!(ui.help, Some(1), "scrolled");
        press!(crossterm::event::KeyCode::Up);
        assert_eq!(ui.help, Some(0));
        press!(crossterm::event::KeyCode::Esc);
        assert_eq!(ui.help, None, "dismissed");
        assert!(
            editor.text().is_empty(),
            "and nothing was typed into the input"
        );
    }
}
