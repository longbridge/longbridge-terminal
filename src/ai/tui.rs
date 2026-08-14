//! Full-screen chat view for `longbridge ai`.
//!
//! Modeled on grok-build's `xai-grok-pager`: a markdown scrollback, a status
//! line, and a multi-line input editor, driven by an async event loop that
//! multiplexes terminal input against the running turn's [`ChatEvent`] stream.
//! The chat view is a pure function of [`ChatState`]; all conversation mutation
//! goes through `state.apply(...)`.
//!
//! A clickable tab bar switches between Chat, a History list of saved sessions,
//! and a Settings panel; Settings opens an Agent picker, and an interrupt that
//! carries options opens a structured Question view. Every view is mouse-aware
//! (scroll to browse, click to select/activate). Answers render as Markdown,
//! and each turn's source references and suggested follow-ups become clickable
//! chips above the prompt.

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
use super::state::{ChatEvent, ChatState, Message, Role};
use super::{markdown, runtime};
use crate::cli::agent::client::{AgentInfo, ConversationRequest};
use crate::tui::widgets::Terminal;

/// Which view is on screen. `Chat`/`Sessions`/`Settings` have tabs; `Agents`
/// and `Question` are overlays reached from within the app.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Chat,
    Sessions,
    Settings,
    Agents,
    Question,
}

/// Braille spinner frames for the "generating" status line.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

// History list palette: a subtle selected-row background and index badge tints.
const SEL_BG: Color = Color::Rgb(45, 50, 62);
const IDX: Color = Color::Rgb(110, 140, 190);
const IDX_SEL: Color = Color::Rgb(240, 150, 90);

/// Interactive rows in the Settings view, in display order.
#[derive(Clone, Copy)]
enum Setting {
    Agent,
    NewChat,
}

const SETTINGS: [Setting; 2] = [Setting::Agent, Setting::NewChat];

/// Slash commands: `(name, i18n description key)`.
const SLASH: [(&str, &str); 8] = [
    ("/new", "Ai.SlashNew"),
    ("/copy", "Ai.SlashCopy"),
    ("/export", "Ai.SlashExport"),
    ("/history", "Ai.SlashHistory"),
    ("/settings", "Ai.SlashSettings"),
    ("/agent", "Ai.SlashAgent"),
    ("/help", "Ai.SlashHelp"),
    ("/exit", "Ai.SlashExit"),
];

/// A clickable chip in the Chat meta panel.
#[derive(Clone, Copy)]
enum Chip {
    /// Index into `state.references`.
    Reference(usize),
    /// Index into `state.further`.
    Further(usize),
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
    agents: Vec<AgentInfo>,
    agents_loading: bool,
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
    /// Live quotes for `x-widget` tickers, fetched after a turn, keyed by symbol.
    quotes: HashMap<String, crate::cli::agent::render::QuoteCardData>,
    /// True while the server History list is being fetched.
    sessions_loading: bool,
    /// Senders for background History fetch / load, set once in `run`.
    history_tx: Option<UnboundedSender<Vec<SessionSummary>>>,
    load_tx: Option<UnboundedSender<session_store::LoadedChat>>,
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
            agents: Vec::new(),
            agents_loading: false,
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
            quotes: HashMap::new(),
            sessions_loading: false,
            history_tx: None,
            load_tx: None,
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
            View::Settings => SETTINGS.len(),
            View::Agents => self.agents.len(),
            View::Question => self
                .question
                .as_ref()
                .and_then(|q| q.questions.get(q.qi))
                .map_or(0, |(_, o)| o.len()),
        }
    }

    /// Switch to `view`, dropping any half-answered question and clamping
    /// selection. (Entering History is done via `open_history`, which also
    /// kicks off the async fetch.)
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
}

/// Run the chat TUI until the user quits. The caller has already entered the
/// full-screen terminal (with mouse capture) and restores it afterwards.
pub async fn run(agent_uid: String) -> Result<()> {
    let mut terminal = Terminal::default();
    let mut state = ChatState::new(agent_uid, t!("Ai.Welcome").to_string());
    let mut ui = Ui::new();
    let mut editor = Editor::new();
    let mut turn: Option<JoinHandle<()>> = None;
    let (tx, mut turn_rx) = unbounded_channel::<ChatEvent>();
    let (agents_tx, mut agents_rx) = unbounded_channel::<Vec<AgentInfo>>();
    let (cards_tx, mut cards_rx) =
        unbounded_channel::<HashMap<String, crate::cli::agent::render::QuoteCardData>>();
    let (history_tx, mut history_rx) = unbounded_channel::<Vec<SessionSummary>>();
    let (load_tx, mut load_rx) = unbounded_channel::<session_store::LoadedChat>();
    ui.history_tx = Some(history_tx);
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
                        on_key(key, &mut ui, &mut state, &mut editor, &mut turn, &tx, &agents_tx);
                    }
                    Some(Ok(Event::Mouse(m))) => {
                        on_mouse(m, &mut ui, &mut state, &mut editor, &mut turn, &tx, &agents_tx);
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
                    fetch_quote_cards_for(&state, &cards_tx);
                    if !ui.focused {
                        notify(&t!("Ai.NotifyDone"));
                    }
                }
            }
            Some(list) = agents_rx.recv() => {
                ui.agents = list;
                ui.agents_loading = false;
                ui.clamp_sel();
            }
            Some(cards) = cards_rx.recv() => {
                ui.quotes.extend(cards);
                ui.cache_sig = 0; // force a transcript rebuild so cards appear
            }
            Some(list) = history_rx.recv() => {
                ui.sessions = list;
                ui.sessions_loading = false;
                ui.clamp_sel();
            }
            Some(loaded) = load_rx.recv() => {
                session_store::restore(loaded, &mut state);
                ui.quotes.clear();
                ui.cache_sig = 0;
                ui.switch(View::Chat);
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
    Ok(())
}

/// If the just-finished answer embeds `x-widget` quote tickers, fetch their
/// live quotes in the background and deliver them on `cards_tx`.
fn fetch_quote_cards_for(
    state: &ChatState,
    cards_tx: &UnboundedSender<HashMap<String, crate::cli::agent::render::QuoteCardData>>,
) {
    let Some(answer) = state
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant)
        .map(|m| m.text.clone())
    else {
        return;
    };
    let widgets = crate::cli::agent::events::extract_widgets(&answer);
    if !widgets
        .iter()
        .any(|w| matches!(w, crate::cli::agent::events::Widget::XWidget { .. }))
    {
        return;
    }
    let cards_tx = cards_tx.clone();
    tokio::spawn(async move {
        let cards = crate::cli::agent::chat::fetch_quote_cards(&widgets).await;
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

/// Discover chat-capable agents across every workspace for the picker.
async fn fetch_agents() -> Vec<AgentInfo> {
    let api = crate::cli::agent::client::LbAgentApi { verbose: false };
    crate::cli::agent::collect_agents(&api, None, None, true, false, 1, 100)
        .await
        .map(|listing| listing.agents)
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
    agents_tx: &UnboundedSender<Vec<AgentInfo>>,
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
        View::Chat => on_chat_key(key, ui, state, editor, turn, tx, agents_tx),
        View::Question => on_question_key(key, ui, state, turn, tx),
        View::Sessions => on_sessions_key(key, ui, state, agents_tx),
        _ => on_list_key(key, ui, state, agents_tx),
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

/// History view: arrows/Enter select and open, Del removes, typing filters.
fn on_sessions_key(
    key: crossterm::event::KeyEvent,
    ui: &mut Ui,
    state: &mut ChatState,
    agents_tx: &UnboundedSender<Vec<AgentInfo>>,
) {
    match key.code {
        KeyCode::Esc => {
            if ui.search.is_empty() {
                ui.switch(View::Chat);
            } else {
                ui.search.clear();
                ui.clamp_sel();
            }
        }
        KeyCode::Up => ui.sel = ui.sel.saturating_sub(1),
        KeyCode::Down => {
            let last = ui.row_count().saturating_sub(1);
            ui.sel = (ui.sel + 1).min(last);
        }
        KeyCode::Enter => activate(ui, state, agents_tx),
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
    agents_tx: &UnboundedSender<Vec<AgentInfo>>,
) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let newline = key
        .modifiers
        .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT);
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
                run_slash_selected(ui, state, editor, agents_tx);
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
        KeyCode::Enter if !state.busy => submit(ui, state, editor, turn, tx, agents_tx),
        KeyCode::Backspace | KeyCode::Char('w') if ctrl => editor.delete_word(),
        KeyCode::Backspace => editor.backspace(),
        KeyCode::Left => editor.left(),
        KeyCode::Right => editor.right(),
        KeyCode::Home => editor.home(),
        KeyCode::End => editor.end(),
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
        KeyCode::PageUp => state.scroll = state.scroll.saturating_add(5),
        KeyCode::PageDown => state.scroll = state.scroll.saturating_sub(5),
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
    agents_tx: &UnboundedSender<Vec<AgentInfo>>,
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
    if let Some(cmd) = trimmed.strip_prefix('/') {
        let name = cmd.split_whitespace().next().unwrap_or("");
        if SLASH.iter().any(|(n, _)| *n == format!("/{name}")) {
            editor.clear();
            exec_slash(name, ui, state, agents_tx);
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

fn exec_slash(
    name: &str,
    ui: &mut Ui,
    state: &mut ChatState,
    agents_tx: &UnboundedSender<Vec<AgentInfo>>,
) {
    match name {
        "new" => {
            state.reset(t!("Ai.Welcome").to_string());
            ui.switch(View::Chat);
        }
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
        "history" => open_history(ui),
        "settings" => ui.switch(View::Settings),
        "agent" => open_agents(ui, agents_tx),
        "help" => state.messages.push(Message {
            role: Role::System,
            text: t!("Ai.HelpText").to_string(),
        }),
        "exit" | "quit" => ui.should_quit = true,
        _ => {}
    }
}

/// Keyboard navigation for the History / Settings / Agents list views.
fn on_list_key(
    key: crossterm::event::KeyEvent,
    ui: &mut Ui,
    state: &mut ChatState,
    agents_tx: &UnboundedSender<Vec<AgentInfo>>,
) {
    match key.code {
        KeyCode::Esc => ui.switch(View::Chat),
        KeyCode::Up | KeyCode::Char('k') => ui.sel = ui.sel.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => {
            let last = ui.row_count().saturating_sub(1);
            ui.sel = (ui.sel + 1).min(last);
        }
        KeyCode::Enter => activate(ui, state, agents_tx),
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
    agents_tx: &UnboundedSender<Vec<AgentInfo>>,
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
            if ui.view == View::Chat {
                if let Some(idx) = ui
                    .slash_rows
                    .iter()
                    .find(|(_, r)| hit(*r, col, row))
                    .map(|(i, _)| *i)
                {
                    run_slash(idx, ui, state, editor, agents_tx);
                } else if let Some(chip) = ui
                    .chips
                    .iter()
                    .find(|(_, r)| hit(*r, col, row))
                    .map(|(c, _)| *c)
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
                    activate(ui, state, agents_tx);
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
            state.scroll.saturating_add(3)
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
fn activate(ui: &mut Ui, state: &mut ChatState, agents_tx: &UnboundedSender<Vec<AgentInfo>>) {
    match ui.view {
        View::Sessions => {
            // The row past the last session is the "New session" action.
            if let Some(id) = ui.visible_sessions().get(ui.sel).map(|s| s.id.clone()) {
                // Fetch the full conversation in the background, then restore.
                if let Some(tx) = ui.load_tx.clone() {
                    ui.notice = Some(t!("Ai.SessionLoading").to_string());
                    tokio::spawn(async move {
                        if let Some(loaded) = session_store::load_detail(&id).await {
                            let _ = tx.send(loaded);
                        }
                    });
                }
            } else {
                state.reset(t!("Ai.Welcome").to_string());
                ui.switch(View::Chat);
            }
        }
        View::Settings => match SETTINGS.get(ui.sel) {
            Some(Setting::Agent) => open_agents(ui, agents_tx),
            Some(Setting::NewChat) => {
                state.reset(t!("Ai.Welcome").to_string());
                ui.switch(View::Chat);
            }
            None => {}
        },
        View::Agents => {
            if let Some(agent) = ui.agents.get(ui.sel) {
                let uid = agent.uid.clone();
                state.reset(t!("Ai.Welcome").to_string());
                state.agent_uid = uid;
                ui.switch(View::Chat);
            }
        }
        View::Chat | View::Question => {}
    }
}

/// Open the History view and fetch the account's chats in the background.
fn open_history(ui: &mut Ui) {
    ui.view = View::Sessions;
    ui.sel = 0;
    ui.search.clear();
    ui.question = None;
    ui.sessions_loading = true;
    if let Some(tx) = ui.history_tx.clone() {
        tokio::spawn(async move {
            let _ = tx.send(session_store::list_summaries().await);
        });
    }
}

/// Open the Agent picker, fetching the list once and caching it.
fn open_agents(ui: &mut Ui, agents_tx: &UnboundedSender<Vec<AgentInfo>>) {
    ui.view = View::Agents;
    ui.sel = 0;
    ui.question = None;
    if ui.agents.is_empty() && !ui.agents_loading {
        ui.agents_loading = true;
        let tx = agents_tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(fetch_agents().await);
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
            Role::System => continue,
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

fn slash_active(editor: &Editor) -> bool {
    editor.is_single_line() && editor.text().starts_with('/')
}

/// SLASH indices whose names start with the current input.
fn slash_matches(editor: &Editor) -> Vec<usize> {
    let prefix = editor.text();
    SLASH
        .iter()
        .enumerate()
        .filter(|(_, (name, _))| name.starts_with(&prefix))
        .map(|(i, _)| i)
        .collect()
}

/// Complete the input to the highlighted command's name.
fn complete_slash(ui: &Ui, editor: &mut Editor) {
    let matches = slash_matches(editor);
    if let Some(&idx) = matches.get(ui.slash_sel.min(matches.len().saturating_sub(1))) {
        editor.set_text(&format!("{} ", SLASH[idx].0));
    }
}

/// Run the highlighted palette command.
fn run_slash_selected(
    ui: &mut Ui,
    state: &mut ChatState,
    editor: &mut Editor,
    agents_tx: &UnboundedSender<Vec<AgentInfo>>,
) {
    let matches = slash_matches(editor);
    if let Some(&idx) = matches.get(ui.slash_sel) {
        run_slash(idx, ui, state, editor, agents_tx);
    }
}

/// Clear the input and execute the command at `SLASH[idx]`.
fn run_slash(
    idx: usize,
    ui: &mut Ui,
    state: &mut ChatState,
    editor: &mut Editor,
    agents_tx: &UnboundedSender<Vec<AgentInfo>>,
) {
    let name = SLASH[idx].0.trim_start_matches('/');
    editor.clear();
    exec_slash(name, ui, state, agents_tx);
}

// ── rendering ────────────────────────────────────────────────────────────────

fn view(f: &mut ratatui::Frame, ui: &mut Ui, state: &ChatState, editor: &Editor) {
    // Recomputed each frame: set true if any truncated row is scrolling.
    ui.animating = false;
    let area = f.area();
    let is_chat = ui.view == View::Chat;
    // Keep the frame timer running while a turn streams so the status spinner
    // animates even between deltas (e.g. during a long tool call).
    if is_chat && state.busy {
        ui.animating = true;
    }
    let has_meta =
        is_chat && !state.busy && (!state.references.is_empty() || !state.further.is_empty());
    let meta_h = if has_meta { meta_height(state) } else { 0 };
    let footer_h = if is_chat {
        (editor.lines().len() as u16 + 2).clamp(3, 8)
    } else {
        3
    };

    let mut constraints = vec![Constraint::Length(1), Constraint::Min(1)];
    if has_meta {
        constraints.push(Constraint::Length(meta_h));
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
    let status = chunks[idx];
    idx += 1;
    let footer = chunks[idx];

    render_title(f, title, state);
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
            render_settings(f, inner, ui, state);
        }
        View::Agents => {
            let inner = render_view_header(f, body, "Ai.SettingAgent");
            render_agents(f, inner, ui);
        }
    }
    if ui.view == View::Chat {
        render_slash_dropdown(f, body, ui, editor);
    } else {
        ui.slash_rows.clear();
    }
    if let Some(meta) = meta {
        render_chips(f, meta, ui, state);
    } else {
        ui.chips.clear();
    }
    render_status(f, status, ui, state);
    render_footer(f, footer, ui, editor);
}

fn render_title(f: &mut ratatui::Frame, area: Rect, state: &ChatState) {
    let mut spans = vec![
        Span::styled(
            format!(" {} ", t!("Ai.Title")),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", state.agent_uid),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    // Show the server-generated conversation title once one arrives.
    if let Some(title) = &state.title {
        spans.push(Span::styled(
            format!("  ·  {title}"),
            Style::default().fg(Color::Gray),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
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
            push_message(&mut cache, m, width, &ui.quotes);
        }
        ui.transcript_cache = cache;
        ui.cache_sig = sig;
    }
    let mut streaming = Vec::new();
    if let Some(text) = &state.streaming {
        push_message(
            &mut streaming,
            &Message {
                role: Role::Assistant,
                text: text.clone(),
            },
            width,
            &ui.quotes,
        );
    }
    let cache_len = ui.transcript_cache.len();
    let total = cache_len + streaming.len();
    let height = area.height as usize;
    let bottom = total.saturating_sub(state.scroll as usize);
    let start = bottom.saturating_sub(height);
    let window: Vec<Line> = (start..bottom)
        .map(|i| {
            if i < cache_len {
                ui.transcript_cache[i].clone()
            } else {
                streaming[i - cache_len].clone()
            }
        })
        .collect();

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
    let content = [
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
        Line::from(Span::styled(
            t!("Ai.EmptyHint").to_string(),
            Style::default().fg(Color::DarkGray),
        )),
    ];
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
    let name_w = matches.iter().map(|&i| SLASH[i].0.len()).max().unwrap_or(0);
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
        let (name, desc) = SLASH[idx];
        let desc = t!(desc).to_string();
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
            (
                s.title.clone(),
                format!("{}  ·  {}", s.agent, relative_time(s.updated_at, now)),
            )
        })
        .collect();
    let n = entries.len();
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

    // Each entry is 3 rows (title, subtitle, gap); the New action is 2. Window
    // in entry units so the selection stays visible.
    let total = n + 1;
    let fit = (list_area.height as usize / 3).max(1);
    let start = if ui.sel < fit {
        0
    } else {
        (ui.sel + 1 - fit).min(total.saturating_sub(fit))
    };
    let width = list_area.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    for i in start..total {
        if lines.len() + 2 > list_area.height as usize {
            break;
        }
        let rect_h = if i < n { 2 } else { 1 };
        let rect = Rect {
            x: list_area.x,
            y: list_area.y + lines.len() as u16,
            width: list_area.width,
            height: rect_h,
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
        lines.push(Line::from("")); // spacing between entries
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
    lines.push(bg_pad(
        vec![
            Span::styled(
                format!("{number:>2}  "),
                with_bg(Style::default().fg(idx_color), bg),
            ),
            Span::styled(title.to_string(), with_bg(title_style, bg)),
        ],
        width,
        bg,
    ));
    lines.push(bg_pad(
        vec![Span::styled(
            format!("    {subtitle}"),
            with_bg(Style::default().fg(Color::DarkGray), bg),
        )],
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

/// Compact "3m / 2h / 5d" age of an entry, from Unix seconds.
fn relative_time(updated: u64, now: u64) -> String {
    let secs = now.saturating_sub(updated);
    if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// Render the Settings panel and record a hit rectangle per row.
fn render_settings(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui, state: &ChatState) {
    let rows: Vec<(usize, String)> = SETTINGS
        .iter()
        .enumerate()
        .map(|(i, setting)| {
            let label = match setting {
                Setting::Agent => format!("{}: {}", t!("Ai.SettingAgent"), state.agent_uid),
                Setting::NewChat => t!("Ai.NewChat").to_string(),
            };
            (i, label)
        })
        .collect();
    render_rows(f, area, ui, &rows);
}

/// Render the Agent picker.
fn render_agents(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui) {
    if ui.agents_loading {
        ui.rows.clear();
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                t!("Ai.AgentsLoading").to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            area,
        );
        return;
    }
    let rows: Vec<(usize, String)> = ui
        .agents
        .iter()
        .enumerate()
        .map(|(i, a)| (i, format!("{}  ({})", a.name, a.uid)))
        .collect();
    render_rows(f, area, ui, &rows);
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
    ui.rows.clear();
    let mut lines = vec![
        Line::from(Span::styled(
            question,
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    let base = area.y + lines.len() as u16;
    let avail = (area.width as usize).saturating_sub(2);
    for (i, option) in options.iter().enumerate() {
        if area.y + lines.len() as u16 >= area.y + area.height {
            break;
        }
        let selected = i == ui.sel;
        let rect = Rect {
            x: area.x,
            y: base + i as u16,
            width: area.width,
            height: 1,
        };
        let hovered = hovering(ui, rect);
        let marker = if selected { "› " } else { "  " };
        let text = row_text(ui, option, avail, rect, selected);
        lines.push(Line::from(Span::styled(
            format!("{marker}{text}"),
            row_style_state(selected, hovered),
        )));
        ui.rows.push((i, rect));
    }
    f.render_widget(Paragraph::new(Text::from(lines)), area);
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

fn row_style(selected: bool) -> Style {
    if selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

/// Row style reflecting both keyboard selection and mouse hover, so pointing at
/// a clickable row gives immediate visual feedback.
fn row_style_state(selected: bool, hovered: bool) -> Style {
    if selected {
        row_style(true)
    } else if hovered {
        Style::default().bg(HOVER_BG)
    } else {
        Style::default()
    }
}

/// Whether the mouse currently rests on `rect`.
fn hovering(ui: &Ui, rect: Rect) -> bool {
    ui.hover.is_some_and(|(c, r)| hit(rect, c, r))
}

/// Number of rows the Chat meta panel needs (references + follow-ups + headers).
fn meta_height(state: &ChatState) -> u16 {
    let mut n = 0u16;
    if !state.references.is_empty() {
        n += 1 + state.references.len() as u16;
    }
    if !state.further.is_empty() {
        n += 1 + state.further.len() as u16;
    }
    n.clamp(1, 8)
}

/// Render clickable reference / follow-up chips and record their hit rects.
fn render_chips(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui, state: &ChatState) {
    ui.chips.clear();
    let mut lines: Vec<Line> = Vec::new();
    let mut y = area.y;
    let bottom = area.y + area.height;
    if !state.references.is_empty() && y < bottom {
        lines.push(Line::from(Span::styled(
            format!("{}:", t!("Agent.References")),
            Style::default().fg(Color::DarkGray),
        )));
        y += 1;
        for (i, r) in state.references.iter().enumerate() {
            if y >= bottom {
                break;
            }
            let rect = row_rect(area, y);
            let full = format!("  [{}] {}", r.index, reference_label(r));
            let hovered = hovering(ui, rect);
            let text = row_text(ui, &full, area.width as usize, rect, false);
            lines.push(Line::from(Span::styled(
                text,
                chip_style(Color::Blue, hovered),
            )));
            ui.chips.push((Chip::Reference(i), rect));
            y += 1;
        }
    }
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

fn render_status(f: &mut ratatui::Frame, area: Rect, ui: &Ui, state: &ChatState) {
    let (text, style) = if let Some(notice) = &ui.notice {
        (notice.clone(), Style::default().fg(Color::Green))
    } else if state.busy && ui.view == View::Chat {
        let frame = SPINNER[(ui.tick as usize) % SPINNER.len()];
        (
            format!("{frame} {}", state.status),
            Style::default().fg(Color::Yellow),
        )
    } else {
        let hint = match ui.view {
            View::Chat => t!("Ai.InputHint"),
            View::Sessions => t!("Ai.SessionsHint"),
            View::Settings => t!("Ai.SettingsHint"),
            View::Agents => t!("Ai.AgentsHint"),
            View::Question => t!("Ai.QuestionHint"),
        };
        (hint.to_string(), Style::default().fg(Color::DarkGray))
    };
    f.render_widget(Paragraph::new(Line::from(Span::styled(text, style))), area);
}

fn render_footer(f: &mut ratatui::Frame, area: Rect, ui: &Ui, editor: &Editor) {
    let focused = ui.view == View::Chat;
    let border = if focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if !focused {
        return;
    }
    if editor.is_blank() {
        // Dim placeholder when nothing has been typed yet.
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                t!("Ai.Placeholder").to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            inner,
        );
    } else {
        let lines: Vec<Line> = editor
            .lines()
            .iter()
            .map(|l| Line::from(l.clone()))
            .collect();
        f.render_widget(Paragraph::new(Text::from(lines)), inner);
    }
    let (cy, col) = editor.cursor();
    let cy = (cy as u16).min(inner.height.saturating_sub(1));
    let col = (col as u16).min(inner.width.saturating_sub(1));
    f.set_cursor_position((inner.x + col, inner.y + cy));
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

fn push_message(
    lines: &mut Vec<Line<'static>>,
    message: &Message,
    width: usize,
    quotes: &HashMap<String, crate::cli::agent::render::QuoteCardData>,
) {
    let (label, accent) = match message.role {
        Role::User => (t!("Ai.You").to_string(), Color::Cyan),
        Role::Assistant => (t!("Ai.Assistant").to_string(), Color::Green),
        Role::System => (String::new(), Color::DarkGray),
    };
    if !label.is_empty() {
        // A colored accent bar precedes each speaker label for scannability.
        lines.push(Line::from(vec![
            Span::styled("▌ ", Style::default().fg(accent)),
            Span::styled(
                label,
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    if message.role == Role::Assistant {
        lines.extend(render_answer_lines(&message.text, width, quotes));
    } else {
        let body_style = if message.role == Role::System {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        };
        for logical in message.text.split('\n') {
            for wrapped in wrap(logical, width) {
                lines.push(Line::from(Span::styled(wrapped, body_style)));
            }
        }
    }
    lines.push(Line::from(""));
}

/// Render an assistant answer the way `agent chat` does: split into text /
/// chart / widget segments, then render Markdown text, draw `vis-chart` blocks
/// as charts, and reduce `x-widget` tags to a compact reference instead of
/// dumping raw JSON/markup into the transcript.
fn render_answer_lines(
    answer: &str,
    width: usize,
    quotes: &HashMap<String, crate::cli::agent::render::QuoteCardData>,
) -> Vec<Line<'static>> {
    use crate::cli::agent::render::{
        parse_quote_widget_symbol, render_vis_chart, replace_inline_markers, segment_answer,
        strip_control_chars, Segment,
    };
    let mut out = Vec::new();
    for segment in segment_answer(answer) {
        match segment {
            Segment::Text(text) => {
                let text = replace_inline_markers(&text, false);
                out.extend(markdown::render(&text, width));
            }
            Segment::VisChart(spec) => {
                let chart = render_vis_chart(&spec, width, false);
                for line in chart.split('\n') {
                    out.push(Line::from(Span::styled(
                        strip_control_chars(line),
                        Style::default().fg(Color::Cyan),
                    )));
                }
            }
            Segment::XWidget(src) => {
                let sym = parse_quote_widget_symbol(&src);
                if let Some(card) = sym.as_ref().and_then(|s| quotes.get(s)) {
                    // Live quote fetched: render an inline quote chip.
                    out.push(quote_chip(card));
                } else {
                    // Pending / non-quote widget: a compact reference.
                    let label = sym.map_or_else(|| strip_control_chars(&src), |s| format!("→ {s}"));
                    out.push(Line::from(Span::styled(
                        label,
                        Style::default()
                            .fg(Color::Blue)
                            .add_modifier(Modifier::UNDERLINED),
                    )));
                }
            }
        }
    }
    out
}

/// A one-line quote chip: `symbol  last  ±change%`, the change tinted by
/// direction, mirroring the web quote card in a terminal-friendly form.
fn quote_chip(card: &crate::cli::agent::render::QuoteCardData) -> Line<'static> {
    let dir = match card.direction {
        1 => Color::Green,
        -1 => Color::Red,
        _ => Color::Gray,
    };
    let mut spans = vec![
        Span::styled(
            format!("  {} ", card.symbol),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {} ", card.last),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(card.change_pct.clone(), Style::default().fg(dir)),
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
}
