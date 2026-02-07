use ratatui::{
    prelude::{Alignment, Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Paragraph, Tabs},
    Frame,
};

use crate::{
    app::AppState,
    ui::styles,
};

pub fn render(frame: &mut Frame, rect: Rect, state: AppState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(rect);

    let tabs = vec![
        Line::from(format!(" {} [1] ", t!("tabs.Watchlist"))),
        Line::from(format!(" {} [2] ", t!("tabs.Portfolio"))),
    ];

    let tabs = Tabs::new(tabs)
        .style(styles::text())
        .highlight_style(styles::text_selected())
        .divider("|")
        .select(match state {
            AppState::Watchlist | AppState::WatchlistStock | AppState::Stock => 0,
            AppState::Portfolio => 1,
            _ => panic!("invalid state"),
        });

    // Simplified implementation: use fixed username
    let nickname = "User".to_string();
    let name = Span::raw(t!("Welcome, %{name}", name = nickname));
    let help = Span::styled(t!("Keyboard.Help"), styles::keyboard());
    let search = Span::styled(t!("Keyboard.Search"), styles::keyboard());
    let quit = Span::styled(t!("Keyboard.Quit"), styles::keyboard());
    let user_info = Paragraph::new(Line::from(vec![
        name,
        " | ".into(),
        help,
        " ".into(),
        search,
        " ".into(),
        quit,
    ]))
    .alignment(Alignment::Right);

    frame.render_widget(tabs, chunks[0]);
    frame.render_widget(user_info, chunks[1]);
}
