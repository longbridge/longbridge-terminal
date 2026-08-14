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
use ratatui::widgets::{Block, Clear, Paragraph};
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

const TABS: [View; 3] = [View::Chat, View::Sessions, View::Settings];

/// Braille spinner frames for the "generating" status line.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Interactive rows in the Settings view, in display order.
#[derive(Clone, Copy)]
enum Setting {
    Agent,
    NewChat,
    ClearHistory,
}

const SETTINGS: [Setting; 3] = [Setting::Agent, Setting::NewChat, Setting::ClearHistory];

/// Slash commands: `(name, i18n description key)`.
const SLASH: [(&str, &str); 6] = [
    ("/new", "Ai.SlashNew"),
    ("/clear", "Ai.SlashClear"),
    ("/history", "Ai.SlashHistory"),
    ("/settings", "Ai.SlashSettings"),
    ("/agent", "Ai.SlashAgent"),
    ("/help", "Ai.SlashHelp"),
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
    /// Transient one-line notice shown in the status bar.
    notice: Option<String>,
    /// Last known mouse position, used to marquee the hovered row.
    hover: Option<(u16, u16)>,
    /// Frame counter advancing the marquee scroll of truncated rows.
    tick: u64,
    /// Set during render when at least one truncated row is scrolling, so the
    /// animation timer only runs while something actually needs it.
    animating: bool,
    tabs: Vec<(View, Rect)>,
    rows: Vec<(usize, Rect)>,
    chips: Vec<(Chip, Rect)>,
    /// Slash-palette hit rects: `(SLASH index, rect)` for the visible entries.
    slash_rows: Vec<(usize, Rect)>,
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
            notice: None,
            hover: None,
            tick: 0,
            animating: false,
            tabs: Vec::new(),
            rows: Vec::new(),
            chips: Vec::new(),
            slash_rows: Vec::new(),
        }
    }

    /// Number of selectable rows in the active list view.
    fn row_count(&self) -> usize {
        match self.view {
            View::Chat => 0,
            View::Sessions => self.sessions.len(),
            View::Settings => SETTINGS.len(),
            View::Agents => self.agents.len(),
            View::Question => self
                .question
                .as_ref()
                .and_then(|q| q.questions.get(q.qi))
                .map_or(0, |(_, o)| o.len()),
        }
    }

    /// Switch to `view`, refreshing the History list, dropping any half-answered
    /// question, and clamping selection.
    fn switch(&mut self, view: View) {
        self.view = view;
        self.notice = None;
        self.sel = 0;
        if view != View::Question {
            self.question = None;
        }
        if view == View::Sessions {
            self.sessions = session_store::list();
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
    let mut events = EventStream::new();
    // Drives the marquee of truncated rows; only consulted while `animating`.
    let mut ticker = tokio::time::interval(Duration::from_millis(120));

    loop {
        terminal.draw(|f| view(f, &mut ui, &state, &editor))?;
        tokio::select! {
            _ = ticker.tick(), if ui.animating => {
                ui.tick = ui.tick.wrapping_add(1);
            }
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                        if on_key(key, &mut ui, &mut state, &mut editor, &mut turn, &tx, &agents_tx) {
                            break;
                        }
                    }
                    Some(Ok(Event::Mouse(m))) => {
                        on_mouse(m, &mut ui, &mut state, &mut editor, &mut turn, &tx, &agents_tx);
                    }
                    Some(Ok(Event::Paste(text))) => editor.insert_str(&text),
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => break,
                }
            }
            Some(event) = turn_rx.recv() => {
                let finished = matches!(event, ChatEvent::TurnFinished { .. });
                state.apply(event);
                if finished {
                    turn = None;
                    persist(&state);
                    maybe_open_question(&mut ui, &state);
                }
            }
            Some(list) = agents_rx.recv() => {
                ui.agents = list;
                ui.agents_loading = false;
                ui.clamp_sel();
            }
        }
    }
    if let Some(turn) = turn.take() {
        turn.abort();
    }
    Ok(())
}

/// Persist the current conversation under its server chat id, so History can
/// list and resume it. No id yet means the turn never started; nothing to save.
fn persist(state: &ChatState) {
    if let Some(id) = state.chat_uid.clone() {
        session_store::save(&id, now_secs(), state);
    }
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

/// Handle one keypress. Returns `true` to quit.
fn on_key(
    key: crossterm::event::KeyEvent,
    ui: &mut Ui,
    state: &mut ChatState,
    editor: &mut Editor,
    turn: &mut Option<JoinHandle<()>>,
    tx: &UnboundedSender<ChatEvent>,
    agents_tx: &UnboundedSender<Vec<AgentInfo>>,
) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && key.code == KeyCode::Char('c') {
        return true;
    }
    if key.code == KeyCode::Tab {
        if ui.view == View::Chat && slash_active(editor) {
            complete_slash(ui, editor);
        } else {
            cycle_view(ui);
        }
        return false;
    }
    match ui.view {
        View::Chat => on_chat_key(key, ui, state, editor, turn, tx, agents_tx),
        View::Question => {
            on_question_key(key, ui, state, turn, tx);
            false
        }
        _ => {
            on_list_key(key, ui, state, agents_tx);
            false
        }
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
) -> bool {
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
                return false;
            }
            KeyCode::Down => {
                ui.slash_sel = (ui.slash_sel + 1).min(count.saturating_sub(1));
                return false;
            }
            KeyCode::Enter if !newline => {
                run_slash_selected(ui, state, editor, agents_tx);
                return false;
            }
            KeyCode::Esc => {
                editor.clear();
                return false;
            }
            _ => {}
        }
    }
    match key.code {
        KeyCode::Esc => {
            if state.busy {
                if let Some(turn) = turn.take() {
                    turn.abort();
                }
                state.cancel(&t!("Ai.Cancelled"));
            } else {
                return true;
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
    false
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
        "clear" => {
            session_store::clear();
            ui.sessions.clear();
            ui.notice = Some(t!("Ai.HistoryCleared").to_string());
        }
        "history" => ui.switch(View::Sessions),
        "settings" => ui.switch(View::Settings),
        "agent" => open_agents(ui, agents_tx),
        "help" => state.messages.push(Message {
            role: Role::System,
            text: t!("Ai.HelpText").to_string(),
        }),
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
        MouseEventKind::ScrollUp => scroll(ui, state, true),
        MouseEventKind::ScrollDown => scroll(ui, state, false),
        MouseEventKind::Down(MouseButton::Left) => {
            let (col, row) = (m.column, m.row);
            if let Some(view) = ui
                .tabs
                .iter()
                .find(|(_, r)| hit(*r, col, row))
                .map(|(v, _)| *v)
            {
                ui.switch(view);
                return;
            }
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
        _ => {}
    }
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

fn cycle_view(ui: &mut Ui) {
    let next = match ui.view {
        View::Chat => View::Sessions,
        View::Sessions => View::Settings,
        _ => View::Chat,
    };
    ui.switch(next);
}

/// Run the selected row's action in the active list view.
fn activate(ui: &mut Ui, state: &mut ChatState, agents_tx: &UnboundedSender<Vec<AgentInfo>>) {
    match ui.view {
        View::Sessions => {
            if let Some(summary) = ui.sessions.get(ui.sel) {
                if let Some(session) = session_store::load(&summary.id) {
                    session_store::restore(session, state);
                    ui.switch(View::Chat);
                }
            }
        }
        View::Settings => match SETTINGS.get(ui.sel) {
            Some(Setting::Agent) => open_agents(ui, agents_tx),
            Some(Setting::NewChat) => {
                state.reset(t!("Ai.Welcome").to_string());
                ui.switch(View::Chat);
            }
            Some(Setting::ClearHistory) => {
                session_store::clear();
                ui.sessions.clear();
                ui.notice = Some(t!("Ai.HistoryCleared").to_string());
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

    let mut constraints = vec![
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ];
    if has_meta {
        constraints.push(Constraint::Length(meta_h));
    }
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Length(footer_h));
    let chunks = Layout::vertical(constraints).split(area);

    let (title, tabs, body) = (chunks[0], chunks[1], chunks[2]);
    let mut idx = 3;
    let meta = has_meta.then(|| {
        let m = chunks[idx];
        idx += 1;
        m
    });
    let status = chunks[idx];
    idx += 1;
    let footer = chunks[idx];

    render_title(f, title, state);
    render_tabs(f, tabs, ui);
    match ui.view {
        View::Chat => render_chat(f, body, state),
        View::Sessions => render_sessions(f, body, ui),
        View::Settings => render_settings(f, body, ui, state),
        View::Agents => render_agents(f, body, ui),
        View::Question => render_question(f, body, ui),
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
    let title_line = Line::from(vec![
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
    ]);
    f.render_widget(Paragraph::new(title_line), area);
}

/// Draw the tab bar and record each tab's hit rectangle for mouse clicks.
fn render_tabs(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui) {
    ui.tabs.clear();
    let mut spans = Vec::new();
    let mut x = area.x;
    for view in TABS {
        let label = format!(" {} ", tab_label(view));
        let width = label.width() as u16;
        let selected = view == ui.view;
        let style = if selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
        ui.tabs.push((
            view,
            Rect {
                x,
                y: area.y,
                width,
                height: 1,
            },
        ));
        x += width + 1;
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn tab_label(view: View) -> String {
    match view {
        View::Chat => t!("Ai.TabChat").to_string(),
        View::Sessions => t!("Ai.TabSessions").to_string(),
        _ => t!("Ai.TabSettings").to_string(),
    }
}

fn render_chat(f: &mut ratatui::Frame, area: Rect, state: &ChatState) {
    let width = area.width.max(1) as usize;
    let mut lines = transcript_lines(state, width);
    let height = area.height as usize;
    let total = lines.len();
    let bottom = total.saturating_sub(state.scroll as usize);
    let start = bottom.saturating_sub(height);
    let window: Vec<Line> = lines.drain(start..bottom).collect();
    f.render_widget(Paragraph::new(Text::from(window)), area);
}

/// The slash-command palette: a menu of matching commands floating above the
/// prompt, with the highlighted entry ↑/↓ can move and Enter/click can run.
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
    let h = matches.len() as u16;
    let box_area = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(h),
        width: area.width.min(48),
        height: h,
    };
    let mut lines = Vec::new();
    for (row, &idx) in matches.iter().enumerate() {
        let (name, desc) = SLASH[idx];
        let selected = row == ui.slash_sel;
        let line = if selected {
            Line::from(Span::styled(
                format!("{name}  {}", t!(desc)),
                row_style(true),
            ))
        } else {
            Line::from(vec![
                Span::styled(
                    format!("{name} "),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(t!(desc).to_string(), Style::default().fg(Color::DarkGray)),
            ])
        };
        lines.push(line);
        ui.slash_rows.push((
            idx,
            Rect {
                x: box_area.x,
                y: box_area.y + row as u16,
                width: box_area.width,
                height: 1,
            },
        ));
    }
    f.render_widget(Clear, box_area);
    f.render_widget(Paragraph::new(Text::from(lines)), box_area);
}

/// Render the History list and record a hit rectangle per visible row.
fn render_sessions(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui) {
    if ui.sessions.is_empty() {
        ui.rows.clear();
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                t!("Ai.SessionsEmpty").to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            area,
        );
        return;
    }
    let rows: Vec<(usize, String)> = ui
        .sessions
        .iter()
        .enumerate()
        .map(|(i, s)| (i, s.title.clone()))
        .collect();
    render_rows(f, area, ui, &rows);
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
                Setting::ClearHistory => t!("Ai.ClearHistory").to_string(),
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
        let marker = if selected { "› " } else { "  " };
        let text = row_text(ui, option, avail, rect, selected);
        lines.push(Line::from(Span::styled(
            format!("{marker}{text}"),
            row_style(selected),
        )));
        ui.rows.push((i, rect));
    }
    f.render_widget(Paragraph::new(Text::from(lines)), area);
}

/// Shared list renderer: draws `rows` with the selected one highlighted and
/// records a hit rectangle per visible row. Windows around the selection.
fn render_rows(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui, rows: &[(usize, String)]) {
    ui.rows.clear();
    ui.clamp_sel();
    let height = area.height.max(1) as usize;
    let start = ui.sel.saturating_sub(height.saturating_sub(1));
    let avail = (area.width as usize).saturating_sub(2);
    let mut lines = Vec::new();
    for (offset, (idx, label)) in rows.iter().skip(start).take(height).enumerate() {
        let selected = *idx == ui.sel;
        let rect = Rect {
            x: area.x,
            y: area.y + offset as u16,
            width: area.width,
            height: 1,
        };
        let marker = if selected { "› " } else { "  " };
        let text = row_text(ui, label, avail, rect, selected);
        lines.push(Line::from(Span::styled(
            format!("{marker}{text}"),
            row_style(selected),
        )));
        ui.rows.push((*idx, rect));
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
            let text = row_text(ui, &full, area.width as usize, rect, false);
            lines.push(Line::from(Span::styled(
                text,
                Style::default().fg(Color::Blue),
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
            let text = row_text(ui, &full, area.width as usize, rect, false);
            lines.push(Line::from(Span::styled(
                text,
                Style::default().fg(Color::Green),
            )));
            ui.chips.push((Chip::Further(i), rect));
            y += 1;
        }
    }
    f.render_widget(Paragraph::new(Text::from(lines)), area);
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
    let block = Block::bordered();
    let inner = block.inner(area);
    f.render_widget(block, area);
    if ui.view != View::Chat {
        return;
    }
    let lines: Vec<Line> = editor
        .lines()
        .iter()
        .map(|l| Line::from(l.clone()))
        .collect();
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
    let (cy, col) = editor.cursor();
    let cy = (cy as u16).min(inner.height.saturating_sub(1));
    let col = (col as u16).min(inner.width.saturating_sub(1));
    f.set_cursor_position((inner.x + col, inner.y + cy));
}

/// Flatten the transcript (and any in-progress answer) into styled, width-wrapped
/// lines. Assistant text is rendered as Markdown; user/system text stays plain.
fn transcript_lines(state: &ChatState, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for m in &state.messages {
        push_message(&mut lines, m, width);
    }
    if let Some(text) = &state.streaming {
        push_message(
            &mut lines,
            &Message {
                role: Role::Assistant,
                text: text.clone(),
            },
            width,
        );
    }
    lines
}

fn push_message(lines: &mut Vec<Line<'static>>, message: &Message, width: usize) {
    let (label, style) = match message.role {
        Role::User => (
            t!("Ai.You").to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Role::Assistant => (
            t!("Ai.Assistant").to_string(),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Role::System => (String::new(), Style::default().fg(Color::DarkGray)),
    };
    if !label.is_empty() {
        lines.push(Line::from(Span::styled(label, style)));
    }
    if message.role == Role::Assistant {
        lines.extend(markdown::render(&message.text, width));
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
}
