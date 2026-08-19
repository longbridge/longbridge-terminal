//! The primary tab strip.
//!
//! Drawn as a row of pills, the active one filled in the brand accent. Returns
//! the rect of every tab so mouse hit-testing uses the geometry that was
//! actually drawn instead of re-deriving label widths.

use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::tui::ui::styles;

/// Blank columns between two tabs.
const GAP: u16 = 1;

/// One tab: a label plus the key that activates it.
pub struct Tab {
    pub label: String,
    /// Shortcut key, without brackets (`"1"`, `"2"`, …).
    pub key: String,
}

impl Tab {
    pub fn new(label: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            key: key.into(),
        }
    }
}

/// Draw the strip as ` 1 WATCHLIST ` pills. Returns each tab's rect plus the
/// total width consumed.
pub fn pills(frame: &mut Frame, rect: Rect, tabs: &[Tab], selected: usize) -> (Vec<Rect>, u16) {
    let mut spans: Vec<Span> = Vec::with_capacity(tabs.len() * 3);
    let mut rects = Vec::with_capacity(tabs.len());
    let mut x = rect.x;

    for (i, tab) in tabs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" ".repeat(GAP as usize)));
            x += GAP;
        }
        let (key_style, label_style) = if i == selected {
            (styles::tab_active_key(), styles::tab_active())
        } else {
            (styles::tab_inactive_key(), styles::tab_inactive())
        };
        let key = format!(" {} ", tab.key);
        let name = format!("{} ", tab.label);
        let width = (key.width() + name.width()) as u16;

        spans.push(Span::styled(key, key_style));
        spans.push(Span::styled(name, label_style));
        rects.push(Rect {
            x,
            y: rect.y,
            width,
            height: 1,
        });
        x += width;
    }

    let width = x - rect.x;
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect {
            width: width.min(rect.width),
            height: 1,
            ..rect
        },
    );
    (rects, width)
}
