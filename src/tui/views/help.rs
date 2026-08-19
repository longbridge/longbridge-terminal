use ratatui::{
    prelude::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::tui::ui::{assets::HOME_URL, styles};

/// Row of the popup body the home-page URL sits on. The rows above it are a
/// blank line, the product name, and another blank line.
const URL_ROW: u16 = 3;
/// Columns the body indents its text by, on top of the block's own padding.
const INDENT: u16 = 2;

pub fn render(frame: &mut Frame, rect: Rect) {
    let rect = crate::tui::ui::rect::centered(100, 40, rect);

    let mut spans = vec![
        Line::from("\n"),
        Line::styled(
            concat!("  Longbridge Terminal v", env!("CARGO_PKG_VERSION")),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::from("\n"),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(HOME_URL, styles::link()),
        ]),
        Line::from("\n"),
    ];
    let tips = t!("HelpTips");
    spans.extend(tips.split('\n').map(Line::from));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(styles::border())
        .padding(Padding::horizontal(2))
        .title(Span::styled(t!("Help"), styles::title()));
    let inner = block.inner(rect);
    let paragraph = Paragraph::new(spans).style(styles::popup()).block(block);

    frame.render_widget(Clear, rect);
    frame.render_widget(paragraph, rect);

    if inner.height > URL_ROW {
        crate::tui::mouse::register_link(
            Rect {
                x: inner.x + INDENT,
                y: inner.y + URL_ROW,
                width: (HOME_URL.width() as u16).min(inner.width.saturating_sub(INDENT)),
                height: 1,
            },
            HOME_URL,
        );
    }
}
