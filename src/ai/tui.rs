//! Full-screen chat view for `longbridge ai`.
//!
//! Modeled on grok-build's `xai-grok-pager`: a scrollback of the transcript, a
//! status line, and an input box, driven by an async event loop that
//! multiplexes terminal input against the running turn's [`ChatEvent`] stream.
//! The chat view is a pure function of [`ChatState`]; all conversation mutation
//! goes through `state.apply(...)`.
//!
//! Beyond the chat, a clickable tab bar switches to two auxiliary views — a
//! History list of saved sessions and a Settings panel — both fully navigable
//! with the mouse (scroll to browse, click to select/activate) as well as the
//! keyboard.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use crossterm::event::{
    Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use futures::StreamExt;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph};
use rust_i18n::t;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::task::JoinHandle;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::runtime;
use super::session_store::{self, SessionSummary};
use super::state::{ChatEvent, ChatState, Message, Role};
use crate::tui::widgets::Terminal;

/// Which view is on screen. The tab bar switches between them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Chat,
    Sessions,
    Settings,
}

const TABS: [View; 3] = [View::Chat, View::Sessions, View::Settings];

/// Interactive rows in the Settings view, in display order.
#[derive(Clone, Copy)]
enum Setting {
    NewChat,
    ClearHistory,
}

const SETTINGS: [Setting; 2] = [Setting::NewChat, Setting::ClearHistory];

/// View/navigation state, kept separate from [`ChatState`] so the conversation
/// model stays about messages, not the UI. Hit-test rectangles are recorded
/// during each render so mouse clicks can be mapped back to tabs and rows.
struct Ui {
    view: View,
    sessions: Vec<SessionSummary>,
    /// Selected row index within the active list view (History / Settings).
    sel: usize,
    /// Transient one-line notice shown in the status bar (e.g. after an action).
    notice: Option<String>,
    tabs: Vec<(View, Rect)>,
    rows: Vec<(usize, Rect)>,
}

impl Ui {
    fn new() -> Self {
        Self {
            view: View::Chat,
            sessions: Vec::new(),
            sel: 0,
            notice: None,
            tabs: Vec::new(),
            rows: Vec::new(),
        }
    }

    /// Number of selectable rows in the active list view.
    fn row_count(&self) -> usize {
        match self.view {
            View::Chat => 0,
            View::Sessions => self.sessions.len(),
            View::Settings => SETTINGS.len(),
        }
    }

    /// Switch to `view`, refreshing the History list and clamping selection.
    fn switch(&mut self, view: View) {
        self.view = view;
        self.notice = None;
        if view == View::Sessions {
            self.sessions = session_store::list();
        }
        self.sel = self.sel.min(self.row_count().saturating_sub(1));
    }
}

/// Run the chat TUI until the user quits. The caller has already entered the
/// full-screen terminal (with mouse capture) and restores it afterwards.
pub async fn run(agent_uid: String) -> Result<()> {
    let mut terminal = Terminal::default();
    let mut state = ChatState::new(agent_uid, t!("Ai.Welcome").to_string());
    let mut ui = Ui::new();
    let mut input = String::new();
    let mut turn: Option<JoinHandle<()>> = None;
    let (tx, mut turn_rx) = unbounded_channel::<ChatEvent>();
    let mut events = EventStream::new();

    loop {
        terminal.draw(|f| view(f, &mut ui, &state, &input))?;
        tokio::select! {
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                        if on_key(key, &mut ui, &mut state, &mut input, &mut turn, &tx) {
                            break;
                        }
                    }
                    Some(Ok(Event::Mouse(m))) => on_mouse(m, &mut ui, &mut state),
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
                }
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

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

// ── input ─────────────────────────────────────────────────────────────────────

/// Handle one keypress. Returns `true` to quit.
fn on_key(
    key: crossterm::event::KeyEvent,
    ui: &mut Ui,
    state: &mut ChatState,
    input: &mut String,
    turn: &mut Option<JoinHandle<()>>,
    tx: &UnboundedSender<ChatEvent>,
) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && key.code == KeyCode::Char('c') {
        return true;
    }
    if key.code == KeyCode::Tab {
        cycle_view(ui);
        return false;
    }
    match ui.view {
        View::Chat => on_chat_key(key, ui, state, input, turn, tx),
        View::Sessions | View::Settings => {
            on_list_key(key, ui, state);
            false
        }
    }
}

fn on_chat_key(
    key: crossterm::event::KeyEvent,
    ui: &mut Ui,
    state: &mut ChatState,
    input: &mut String,
    turn: &mut Option<JoinHandle<()>>,
    tx: &UnboundedSender<ChatEvent>,
) -> bool {
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
        KeyCode::Enter if !state.busy => {
            let query = input.trim().to_string();
            if !query.is_empty() {
                input.clear();
                ui.notice = None;
                let req = runtime::build_request(state, query.clone());
                state.apply(ChatEvent::UserPrompt(query));
                state.pending_interrupt = None;
                *turn = Some(runtime::spawn_turn(req, tx.clone()));
            }
        }
        KeyCode::Backspace => {
            input.pop();
        }
        KeyCode::PageUp => state.scroll = state.scroll.saturating_add(5),
        KeyCode::PageDown => state.scroll = state.scroll.saturating_sub(5),
        KeyCode::Char(c) => input.push(c),
        _ => {}
    }
    false
}

/// Keyboard navigation for the History / Settings list views.
fn on_list_key(key: crossterm::event::KeyEvent, ui: &mut Ui, state: &mut ChatState) {
    match key.code {
        KeyCode::Esc => ui.switch(View::Chat),
        KeyCode::Up => ui.sel = ui.sel.saturating_sub(1),
        KeyCode::Down => {
            let last = ui.row_count().saturating_sub(1);
            ui.sel = (ui.sel + 1).min(last);
        }
        KeyCode::Enter => activate(ui, state),
        _ => {}
    }
}

fn on_mouse(m: crossterm::event::MouseEvent, ui: &mut Ui, state: &mut ChatState) {
    match m.kind {
        MouseEventKind::ScrollUp => scroll(ui, state, true),
        MouseEventKind::ScrollDown => scroll(ui, state, false),
        MouseEventKind::Down(MouseButton::Left) => {
            let (col, row) = (m.column, m.row);
            // A tab click always wins, regardless of the current view.
            if let Some(view) = ui
                .tabs
                .iter()
                .find(|(_, r)| hit(*r, col, row))
                .map(|(v, _)| *v)
            {
                ui.switch(view);
            } else if matches!(ui.view, View::Sessions | View::Settings) {
                if let Some(idx) = ui
                    .rows
                    .iter()
                    .find(|(_, r)| hit(*r, col, row))
                    .map(|(i, _)| *i)
                {
                    ui.sel = idx;
                    activate(ui, state);
                }
            }
        }
        _ => {}
    }
}

/// Scroll wheel: pans the transcript in Chat, moves the selection in a list.
fn scroll(ui: &mut Ui, state: &mut ChatState, up: bool) {
    match ui.view {
        View::Chat => {
            state.scroll = if up {
                state.scroll.saturating_add(3)
            } else {
                state.scroll.saturating_sub(3)
            };
        }
        View::Sessions | View::Settings => {
            if up {
                ui.sel = ui.sel.saturating_sub(1);
            } else {
                let last = ui.row_count().saturating_sub(1);
                ui.sel = (ui.sel + 1).min(last);
            }
        }
    }
}

fn cycle_view(ui: &mut Ui) {
    let next = match ui.view {
        View::Chat => View::Sessions,
        View::Sessions => View::Settings,
        View::Settings => View::Chat,
    };
    ui.switch(next);
}

/// Run the selected row's action in the active list view.
fn activate(ui: &mut Ui, state: &mut ChatState) {
    match ui.view {
        View::Chat => {}
        View::Sessions => {
            if let Some(summary) = ui.sessions.get(ui.sel) {
                if let Some(session) = session_store::load(&summary.id) {
                    session_store::restore(session, state);
                    ui.view = View::Chat;
                }
            }
        }
        View::Settings => match SETTINGS.get(ui.sel) {
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
    }
}

fn hit(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
}

// ── rendering ────────────────────────────────────────────────────────────────

fn view(f: &mut ratatui::Frame, ui: &mut Ui, state: &ChatState, input: &str) {
    let [title, tabs, body, status, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(3),
    ])
    .areas(f.area());

    render_title(f, title, state);
    render_tabs(f, tabs, ui);

    match ui.view {
        View::Chat => render_chat(f, body, state),
        View::Sessions => render_sessions(f, body, ui),
        View::Settings => render_settings(f, body, ui, state),
    }

    render_status(f, status, ui, state);
    render_footer(f, footer, ui, input);
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
        View::Settings => t!("Ai.TabSettings").to_string(),
    }
}

fn render_chat(f: &mut ratatui::Frame, area: Rect, state: &ChatState) {
    // Wrap the whole transcript to the body width, then show the window ending
    // `state.scroll` lines above the bottom (0 = pinned to the latest).
    let width = area.width.max(1) as usize;
    let mut lines = transcript_lines(state, width);
    let height = area.height as usize;
    let total = lines.len();
    let bottom = total.saturating_sub(state.scroll as usize);
    let start = bottom.saturating_sub(height);
    let window: Vec<Line> = lines.drain(start..bottom).collect();
    f.render_widget(Paragraph::new(Text::from(window)), area);
}

/// Render the History list and record a hit rectangle per visible row.
fn render_sessions(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui) {
    ui.rows.clear();
    if ui.sessions.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                t!("Ai.SessionsEmpty").to_string(),
                Style::default().fg(Color::DarkGray),
            ))),
            area,
        );
        return;
    }
    let height = area.height as usize;
    let start = ui.sel.saturating_sub(height.saturating_sub(1));
    let mut lines = Vec::new();
    for (offset, (idx, summary)) in ui
        .sessions
        .iter()
        .enumerate()
        .skip(start)
        .take(height)
        .enumerate()
    {
        let selected = idx == ui.sel;
        let style = row_style(selected);
        let marker = if selected { "› " } else { "  " };
        lines.push(Line::from(Span::styled(
            format!("{marker}{}", summary.title),
            style,
        )));
        ui.rows.push((
            idx,
            Rect {
                x: area.x,
                y: area.y + offset as u16,
                width: area.width,
                height: 1,
            },
        ));
    }
    f.render_widget(Paragraph::new(Text::from(lines)), area);
}

/// Render the Settings panel and record a hit rectangle per action row.
fn render_settings(f: &mut ratatui::Frame, area: Rect, ui: &mut Ui, state: &ChatState) {
    ui.rows.clear();
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{}: ", t!("Ai.SettingAgent")),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(state.agent_uid.clone(), Style::default().fg(Color::Gray)),
        ]),
        Line::from(""),
    ];
    // Action rows start two lines below the agent info.
    let base = area.y + lines.len() as u16;
    for (idx, setting) in SETTINGS.iter().enumerate() {
        let selected = idx == ui.sel;
        let marker = if selected { "› " } else { "  " };
        lines.push(Line::from(Span::styled(
            format!("{marker}{}", setting_label(*setting)),
            row_style(selected),
        )));
        ui.rows.push((
            idx,
            Rect {
                x: area.x,
                y: base + idx as u16,
                width: area.width,
                height: 1,
            },
        ));
    }
    f.render_widget(Paragraph::new(Text::from(lines)), area);
}

fn setting_label(setting: Setting) -> String {
    match setting {
        Setting::NewChat => t!("Ai.NewChat").to_string(),
        Setting::ClearHistory => t!("Ai.ClearHistory").to_string(),
    }
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

fn render_status(f: &mut ratatui::Frame, area: Rect, ui: &Ui, state: &ChatState) {
    let (text, style) = if let Some(notice) = &ui.notice {
        (notice.clone(), Style::default().fg(Color::Green))
    } else if state.busy && ui.view == View::Chat {
        (
            format!("● {}", state.status),
            Style::default().fg(Color::Yellow),
        )
    } else {
        let hint = match ui.view {
            View::Chat => t!("Ai.InputHint"),
            View::Sessions => t!("Ai.SessionsHint"),
            View::Settings => t!("Ai.SettingsHint"),
        };
        (hint.to_string(), Style::default().fg(Color::DarkGray))
    };
    f.render_widget(Paragraph::new(Line::from(Span::styled(text, style))), area);
}

fn render_footer(f: &mut ratatui::Frame, area: Rect, ui: &Ui, input: &str) {
    if ui.view == View::Chat {
        let prompt_line = Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Cyan)),
            Span::raw(input),
            Span::styled("▏", Style::default().fg(Color::Cyan)),
        ]);
        f.render_widget(Paragraph::new(prompt_line).block(Block::bordered()), area);
    } else {
        f.render_widget(Paragraph::new("").block(Block::bordered()), area);
    }
}

/// Flatten the transcript (and any in-progress answer) into styled, width-wrapped
/// lines.
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
