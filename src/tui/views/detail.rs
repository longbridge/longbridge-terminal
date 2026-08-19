//! Right-hand detail panel shared by the Portfolio and Orders screens.
//!
//! Clicking a row in either list opens this panel beside the list and keeps it
//! in sync with the selection. It renders a flat label/value sheet so both
//! screens present record details the same way.

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::tui::ui::styles;

/// One line of the sheet.
pub enum Row<'a> {
    /// A section heading, e.g. `EXECUTION`.
    Section(String),
    /// A label on the left, its value right-aligned on the same line.
    Field(String, Span<'a>),
    /// A full-width line of free text (wraps are the caller's business).
    Text(Line<'a>),
    /// Vertical breathing room.
    Blank,
}

impl Row<'_> {
    /// A field whose value uses the default text style.
    pub fn field(label: impl Into<String>, value: impl Into<String>) -> Self {
        Row::Field(label.into(), Span::styled(value.into(), styles::text()))
    }

    /// A field whose value carries its own style (P/L, status, …).
    pub fn styled(label: impl Into<String>, value: impl Into<String>, style: Style) -> Self {
        Row::Field(label.into(), Span::styled(value.into(), style))
    }
}

/// Split `area` into (list, panel). The panel takes about a third of the width,
/// and the whole area on a terminal too narrow to show both side by side.
pub fn split(area: Rect) -> (Rect, Rect) {
    let panel_width = if area.width < 56 {
        area.width
    } else {
        (area.width / 3).clamp(32, 50)
    };
    let [list, panel] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(panel_width)]).areas(area);
    (list, panel)
}

/// Draw the panel and register its click targets (the panel body, so clicks do
/// not fall through to the list, and the `✕` close button).
pub fn render(frame: &mut Frame, rect: Rect, title: &str, rows: &[Row], hints: Vec<Span<'_>>) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(styles::active_border())
        .padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {title} "), styles::title()))
        .title_top(Line::from(Span::styled(" ✕ ", styles::dark_gray())).right_aligned());
    let block = if hints.is_empty() {
        block
    } else {
        block.title_bottom(Line::from(hints).right_aligned())
    };

    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);

    let width = inner.width as usize;
    let lines: Vec<Line> = rows.iter().map(|row| render_row(row, width)).collect();
    frame.render_widget(Paragraph::new(lines), inner);

    *crate::tui::mouse::DETAIL_PANEL_RECT.lock().expect("poison") = rect;
    *crate::tui::mouse::DETAIL_CLOSE_RECT.lock().expect("poison") = Rect {
        x: rect.x + rect.width.saturating_sub(4),
        y: rect.y,
        width: 3,
        height: 1,
    };
}

/// Clear the panel's click targets — called when the panel is not on screen so
/// a stale rect cannot swallow clicks meant for the list.
pub fn clear_hit_areas() {
    *crate::tui::mouse::DETAIL_PANEL_RECT.lock().expect("poison") = Rect::default();
    *crate::tui::mouse::DETAIL_CLOSE_RECT.lock().expect("poison") = Rect::default();
}

fn render_row<'a>(row: &Row<'a>, width: usize) -> Line<'a> {
    match row {
        Row::Blank => Line::from(""),
        Row::Text(line) => line.clone(),
        Row::Section(title) => {
            Line::from(vec![Span::styled(title.to_uppercase(), styles::header())])
        }
        Row::Field(label, value) => {
            // Right-align the value against the panel's inner edge; if the pair
            // is wider than the panel, fall back to a single space between.
            let used = label.width() + value.width();
            let pad = width.saturating_sub(used).max(1);
            Line::from(vec![
                Span::styled(label.clone(), styles::label()),
                Span::raw(" ".repeat(pad)),
                value.clone(),
            ])
        }
    }
}
