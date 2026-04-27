use std::sync::LazyLock;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use ratatui::{
    style::{Color, Style},
    text::Span,
    widgets::{Clear, Paragraph},
    Frame,
};
use ratatui::layout::Rect;

pub static TOAST: LazyLock<RwLock<Option<ToastMessage>>> =
    LazyLock::new(|| RwLock::new(None));

pub struct ToastMessage {
    pub text: String,
    pub kind: ToastKind,
    pub expires_at: Instant,
}

pub enum ToastKind {
    Success,
    Error,
    Info,
}

pub fn set_toast(kind: ToastKind, text: String) {
    let duration = match kind {
        ToastKind::Success => Duration::from_secs(2),
        ToastKind::Error => Duration::from_secs(4),
        ToastKind::Info => Duration::from_secs(3),
    };
    *TOAST.write().expect("poison") = Some(ToastMessage {
        text,
        kind,
        expires_at: Instant::now() + duration,
    });
}

pub fn render_toast(frame: &mut Frame, rect: Rect) {
    let mut toast = TOAST.write().expect("poison");
    if let Some(t) = &*toast {
        if Instant::now() > t.expires_at {
            *toast = None;
            return;
        }
        let color = match t.kind {
            ToastKind::Success => Color::Green,
            ToastKind::Error => Color::Red,
            ToastKind::Info => Color::Yellow,
        };
        let text_len = t.text.chars().count() as u16 + 4;
        let width = text_len.min(rect.width.saturating_sub(4));
        if width == 0 || rect.height < 2 {
            return;
        }
        let toast_rect = Rect {
            x: rect.x + rect.width.saturating_sub(width + 2),
            y: rect.y + rect.height - 2,
            width,
            height: 1,
        };
        frame.render_widget(Clear, toast_rect);
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(" {} ", t.text),
                Style::default().fg(color),
            )),
            toast_rect,
        );
    }
}
