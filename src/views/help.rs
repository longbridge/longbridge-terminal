use ratatui::{
    prelude::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph},
    Frame,
};

use crate::ui::styles;

pub fn render(frame: &mut Frame, rect: Rect) {
    let rect = crate::ui::rect::centered(100, 40, rect);

    let mut spans = vec![
        Line::from("\n"),
        Line::styled(
            concat!("  Longbridge Terminal v", env!("CARGO_PKG_VERSION")),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::from("\n"),
        Line::from("  https://longbridge.com"),
        Line::from("\n"),
    ];
    let tips = t!("HelpTips");
    spans.extend(tips.split('\n').map(Line::from));
    let paragraph = Paragraph::new(spans).style(styles::popup()).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(styles::border())
            .padding(Padding::horizontal(2))
            .title(Span::styled(t!("Help"), styles::title())),
    );
    frame.render_widget(Clear, rect);
    frame.render_widget(paragraph, rect);
}
