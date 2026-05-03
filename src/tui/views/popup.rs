use crate::{
    data::Counter,
    openapi,
    tui::ui::styles,
    tui::widgets::{LocalSearch, Search},
};

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table},
    Frame,
};

pub fn render(
    frame: &mut Frame,
    rect: Rect,
    account: &mut LocalSearch<crate::data::Account>,
    currency: &mut LocalSearch<openapi::account::CurrencyInfo>,
    search: &mut Search<openapi::search::StockItem>,
    watchlist: &mut LocalSearch<crate::data::WatchlistGroup>,
    watchlist_search: &mut LocalSearch<Counter>,
) {
    let popup = crate::tui::app::POPUP.load(std::sync::atomic::Ordering::Relaxed);
    if popup == crate::tui::app::POPUP_ACCOUNT {
        switch_account(frame, rect, account);
    } else if popup == crate::tui::app::POPUP_CURRENCY {
        switch_currency(frame, rect, currency);
    } else if popup == crate::tui::app::POPUP_WATCHLIST {
        switch_watchlist(frame, rect, watchlist);
    } else if popup == crate::tui::app::POPUP_HELP {
        crate::tui::views::help::render(frame, rect);
    } else if popup == crate::tui::app::POPUP_SEARCH {
        searching(frame, rect, search);
    } else if popup == crate::tui::app::POPUP_WATCHLIST_SEARCH {
        search_watchlist(frame, rect, watchlist_search);
    } else if popup & crate::tui::app::POPUP_ORDER_ENTRY != 0 {
        crate::tui::systems::render_order_entry_popup(frame, rect);
    } else if popup & crate::tui::app::POPUP_CANCEL_ORDER != 0 {
        crate::tui::systems::render_cancel_order_popup(frame, rect);
    } else if popup & crate::tui::app::POPUP_REPLACE_ORDER != 0 {
        crate::tui::systems::render_replace_order_popup(frame, rect);
    } else if popup & crate::tui::app::POPUP_DATE_FILTER != 0 {
        crate::tui::systems::render_date_filter_popup(frame, rect);
    }
}

fn switch_account(frame: &mut Frame, rect: Rect, account: &mut LocalSearch<crate::data::Account>) {
    const MAX_SIZE: (u16, u16) = (50, 30);
    let rect = crate::tui::ui::rect::centered(MAX_SIZE.0, MAX_SIZE.1, rect);
    frame.render_widget(Clear, rect);

    let chunks = Layout::default()
        .margin(1)
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Percentage(100)].as_ref())
        .split(rect);

    let input = &account.input;
    // one line, without scroll
    let paragraph = Paragraph::new(input.value()).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(styles::border())
            .title(t!("SwitchAccount.title")),
    );
    frame.render_widget(paragraph, chunks[0]);
    frame.set_cursor_position((
        // Put cursor past the end of the input text
        chunks[0].x + u16::try_from(input.visual_cursor()).unwrap() + 1,
        // Move one line down, from the border to the input line
        chunks[0].y + 1,
    ));

    let column_widths = [12, 34];

    let rows = account
        .options()
        .iter()
        .map(|account| {
            Row::new(vec![
                Cell::from(Span::styled(account.account_name.clone(), styles::popup())),
                Cell::from(account.org.name.clone()),
            ])
        })
        .collect::<Vec<_>>();

    let column_constraints = column_widths.map(|w| Constraint::Length(u16::try_from(w).unwrap()));

    let table = Table::new(rows, column_constraints)
        .block(
            Block::default()
                .borders(Borders::all())
                .border_style(styles::border()),
        )
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .column_spacing(2);

    frame.render_stateful_widget(table, chunks[1], &mut account.table);
    *crate::tui::mouse::POPUP_LIST_RECT.lock().expect("poison") = chunks[1];
}

fn switch_currency(
    frame: &mut Frame,
    rect: Rect,
    currency: &mut LocalSearch<openapi::account::CurrencyInfo>,
) {
    const MAX_SIZE: (u16, u16) = (50, 30);
    let rect = crate::tui::ui::rect::centered(MAX_SIZE.0, MAX_SIZE.1, rect);
    frame.render_widget(Clear, rect);

    let chunks = Layout::default()
        .margin(1)
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Percentage(100)].as_ref())
        .split(rect);

    let input = &currency.input;
    // one line, without scroll
    let paragraph = Paragraph::new(input.value()).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(styles::border())
            .title(t!("SwitchCurrency.title")),
    );
    frame.render_widget(paragraph, chunks[0]);
    frame.set_cursor_position((
        // Put cursor past the end of the input text
        chunks[0].x + u16::try_from(input.visual_cursor()).unwrap() + 1,
        // Move one line down, from the border to the input line
        chunks[0].y + 1,
    ));

    let column_widths = [12, 34];

    let rows = currency
        .options()
        .iter()
        .map(|currency| {
            Row::new(vec![Cell::from(Span::styled(
                currency.currency_iso.clone(),
                styles::popup(),
            ))])
        })
        .collect::<Vec<_>>();

    let column_constraints = column_widths.map(|w| Constraint::Length(u16::try_from(w).unwrap()));

    let table = Table::new(rows, column_constraints)
        .block(
            Block::default()
                .borders(Borders::all())
                .border_style(styles::border()),
        )
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .column_spacing(2);

    frame.render_stateful_widget(table, chunks[1], &mut currency.table);
    *crate::tui::mouse::POPUP_LIST_RECT.lock().expect("poison") = chunks[1];
}

fn switch_watchlist(
    frame: &mut Frame,
    rect: Rect,
    groups: &mut LocalSearch<crate::data::WatchlistGroup>,
) {
    const MAX_SIZE: (u16, u16) = (50, 30);
    let rect = crate::tui::ui::rect::centered(MAX_SIZE.0, MAX_SIZE.1, rect);
    frame.render_widget(Clear, rect);

    let chunks = Layout::default()
        .margin(1)
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Percentage(100)].as_ref())
        .split(rect);

    let input = &groups.input;
    // one line, without scroll
    let paragraph = Paragraph::new(input.value()).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(styles::border())
            .title(t!("SwitchWatchlist.title")),
    );
    frame.render_widget(paragraph, chunks[0]);
    frame.set_cursor_position((
        // Put cursor past the end of the input text
        chunks[0].x + u16::try_from(input.visual_cursor()).unwrap() + 1,
        // Move one line down, from the border to the input line
        chunks[0].y + 1,
    ));

    let column_widths = [12, 34];

    let rows = groups
        .options()
        .iter()
        .map(|group| {
            Row::new(vec![Cell::from(Span::styled(
                group.name.clone(),
                styles::popup(),
            ))])
        })
        .collect::<Vec<_>>();

    let column_constraints = column_widths.map(|w| Constraint::Length(u16::try_from(w).unwrap()));

    let table = Table::new(rows, column_constraints)
        .block(
            Block::default()
                .borders(Borders::all())
                .border_style(styles::border()),
        )
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .column_spacing(2);

    frame.render_stateful_widget(table, chunks[1], &mut groups.table);
    *crate::tui::mouse::POPUP_LIST_RECT.lock().expect("poison") = chunks[1];
}

fn searching(frame: &mut Frame, rect: Rect, search: &mut Search<openapi::search::StockItem>) {
    const MAX_SIZE: (u16, u16) = (50, 3);
    let rect = crate::tui::ui::rect::centered(MAX_SIZE.0, MAX_SIZE.1, rect);
    frame.render_widget(Clear, rect);

    let input = &search.input;
    let paragraph = Paragraph::new(input.value()).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(styles::border())
            .title(t!("SearchStock.title")),
    );
    frame.render_widget(paragraph, rect);
    frame.set_cursor_position((
        rect.x + u16::try_from(input.visual_cursor()).unwrap() + 1,
        rect.y + 1,
    ));
}

fn search_watchlist(frame: &mut Frame, rect: Rect, search: &mut LocalSearch<Counter>) {
    const MAX_SIZE: (u16, u16) = (60, 25);
    const COLUMN_WIDTHS: [Constraint; 3] = [
        Constraint::Length(4),
        Constraint::Length(12),
        Constraint::Min(10),
    ];
    let rect = crate::tui::ui::rect::centered(MAX_SIZE.0, MAX_SIZE.1, rect);
    frame.render_widget(Clear, rect);

    let chunks = Layout::default()
        .margin(1)
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)].as_ref())
        .split(rect);

    let input = &search.input;
    let (title, border_style) = if let Some(err) = search.error() {
        (format!(" {err} "), Style::default().fg(Color::LightRed))
    } else {
        (t!("WatchlistSearch.title").to_string(), styles::border())
    };
    let paragraph = Paragraph::new(input.value()).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title),
    );
    frame.render_widget(paragraph, chunks[0]);
    frame.set_cursor_position((
        chunks[0].x + u16::try_from(input.visual_cursor()).unwrap() + 1,
        chunks[0].y + 1,
    ));

    let rows = search
        .options()
        .iter()
        .map(|counter| {
            let name = crate::data::STOCKS
                .get(counter)
                .map(|s| s.display_name().to_string())
                .unwrap_or_default();
            Row::new(vec![
                Cell::from(Span::styled(
                    counter.market().to_string(),
                    styles::market(counter.region()),
                )),
                Cell::from(counter.code().to_string()),
                Cell::from(name),
            ])
        })
        .collect::<Vec<_>>();

    let table = Table::new(rows, COLUMN_WIDTHS)
        .block(
            Block::default()
                .borders(Borders::all())
                .border_style(styles::border()),
        )
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .column_spacing(1);

    frame.render_stateful_widget(table, chunks[1], &mut search.table);
    *crate::tui::mouse::POPUP_LIST_RECT.lock().expect("poison") = chunks[1];
}
