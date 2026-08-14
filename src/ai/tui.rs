//! Full-screen chat view for `longbridge ai`.
//!
//! Modeled on grok-build's `xai-grok-pager`: a scrollback of the transcript, a
//! status line, and an input box, driven by an async event loop that
//! multiplexes terminal input against the running turn's [`ChatEvent`] stream.
//! The view is a pure function of [`ChatState`]; all mutation goes through
//! `state.apply(...)`.

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph};
use rust_i18n::t;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::task::JoinHandle;
use unicode_width::UnicodeWidthChar;

use super::runtime;
use super::state::{ChatEvent, ChatState, Message, Role};
use crate::tui::widgets::Terminal;

/// Run the chat TUI until the user quits. The caller has already entered the
/// full-screen terminal and restores it afterwards.
pub async fn run(agent_uid: String) -> Result<()> {
    let mut terminal = Terminal::default();
    let mut state = ChatState::new(agent_uid, t!("Ai.Welcome").to_string());
    let mut input = String::new();
    let mut turn: Option<JoinHandle<()>> = None;
    let (tx, mut turn_rx) = unbounded_channel::<ChatEvent>();
    let mut events = EventStream::new();

    loop {
        terminal.draw(|f| view(f, &state, &input))?;
        tokio::select! {
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                        if handle_key(key, &mut state, &mut input, &mut turn, &tx) {
                            break;
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => break,
                }
            }
            Some(event) = turn_rx.recv() => {
                let finished = matches!(event, ChatEvent::TurnFinished { .. });
                state.apply(event);
                if finished {
                    turn = None;
                }
            }
        }
    }
    if let Some(turn) = turn.take() {
        turn.abort();
    }
    Ok(())
}

/// Handle one keypress. Returns `true` to quit.
fn handle_key(
    key: crossterm::event::KeyEvent,
    state: &mut ChatState,
    input: &mut String,
    turn: &mut Option<JoinHandle<()>>,
    tx: &UnboundedSender<ChatEvent>,
) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('c') if ctrl => return true,
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

// ── rendering ────────────────────────────────────────────────────────────────

fn view(f: &mut ratatui::Frame, state: &ChatState, input: &str) {
    let [title, body, status, prompt] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(3),
    ])
    .areas(f.area());

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
    f.render_widget(Paragraph::new(title_line), title);

    // Wrap the whole transcript to the body width, then show the window ending
    // `state.scroll` lines above the bottom (0 = pinned to the latest).
    let width = body.width.max(1) as usize;
    let mut lines = transcript_lines(state, width);
    let height = body.height as usize;
    let total = lines.len();
    let bottom = total.saturating_sub(state.scroll as usize);
    let start = bottom.saturating_sub(height);
    let window: Vec<Line> = lines.drain(start..bottom).collect();
    f.render_widget(Paragraph::new(Text::from(window)), body);

    let (status_text, style) = if state.busy {
        (
            format!("● {}", state.status),
            Style::default().fg(Color::Yellow),
        )
    } else {
        (
            t!("Ai.InputHint").to_string(),
            Style::default().fg(Color::DarkGray),
        )
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(status_text, style))),
        status,
    );

    let prompt_line = Line::from(vec![
        Span::styled("> ", Style::default().fg(Color::Cyan)),
        Span::raw(input),
        Span::styled("▏", Style::default().fg(Color::Cyan)),
    ]);
    f.render_widget(Paragraph::new(prompt_line).block(Block::bordered()), prompt);
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
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
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
