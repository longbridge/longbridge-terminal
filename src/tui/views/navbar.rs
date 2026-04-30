use ratatui::{
    prelude::{Alignment, Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Paragraph, Tabs},
    Frame,
};

use crate::{tui::app::{AppState, ACCOUNT_TYPE}, tui::ui::styles};

pub fn render(frame: &mut Frame, rect: Rect, state: AppState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(rect);

    let tabs = vec![
        Line::from(format!(" {} [1] ", t!("tabs.Watchlist"))),
        Line::from(format!(" {} [2] ", t!("tabs.Portfolio"))),
        Line::from(format!(" {} [3] ", t!("tabs.Orders"))),
    ];

    let tabs = Tabs::new(tabs)
        .style(styles::text())
        .highlight_style(styles::text_selected())
        .divider("|")
        .select(match state {
            AppState::Watchlist | AppState::WatchlistStock | AppState::Stock => 0,
            AppState::Portfolio => 1,
            AppState::Orders => 2,
            _ => 0,
        });

    // Simplified implementation: use fixed username
    let nickname = "User".to_string();
    let dark_gray_style = styles::dark_gray();
    let name = Span::styled(t!("Welcome, %{name}", name = nickname), dark_gray_style);
    let search = Span::styled(t!("Keyboard.Search"), dark_gray_style);
    let help = Span::styled(t!("Keyboard.Help"), dark_gray_style);
    let log = Span::styled(t!("Keyboard.Console"), dark_gray_style);
    let quit = Span::styled(t!("Keyboard.Quit"), dark_gray_style);

    let account_type = ACCOUNT_TYPE.read().expect("poison").clone();
    let mut spans = vec![name];
    if let Some(ref t) = account_type {
        let is_real = matches!(t.as_str(), "CashAccount" | "MarginAccount");
        let badge_style = if is_real {
            styles::online()
        } else {
            styles::bmp()
        };
        spans.push(Span::styled(" ", dark_gray_style));
        spans.push(Span::styled(format!("[{t}]"), badge_style));
    }
    spans.extend([
        Span::styled(" | ", dark_gray_style),
        search,
        Span::styled(" ", dark_gray_style),
        help,
        Span::styled(" ", dark_gray_style),
        log,
        Span::styled(" ", dark_gray_style),
        quit,
    ]);
    let user_info = Paragraph::new(Line::from(spans)).alignment(Alignment::Right);

    frame.render_widget(tabs, chunks[0]);
    frame.render_widget(user_info, chunks[1]);
}
