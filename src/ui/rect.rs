use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};

pub fn centered(width: u16, height: u16, r: Rect) -> Rect {
    let horizontal = if width == 0 {
        0
    } else {
        r.width.saturating_sub(width) / 2
    };
    let vertical = if height == 0 {
        0
    } else {
        r.height.saturating_sub(height) / 2
    };
    r.inner(&Margin {
        horizontal,
        vertical,
    })
}

pub fn centered_percent(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Percentage((100 - percent_y) / 2),
                Constraint::Percentage(percent_y),
                Constraint::Percentage((100 - percent_y) / 2),
            ]
            .as_ref(),
        )
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints(
            [
                Constraint::Percentage((100 - percent_x) / 2),
                Constraint::Percentage(percent_x),
                Constraint::Percentage((100 - percent_x) / 2),
            ]
            .as_ref(),
        )
        .split(popup_layout[1])[1]
}
