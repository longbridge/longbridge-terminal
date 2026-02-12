use crate::{
    openapi,
    ui::styles,
    widgets::{LocalSearch, Search},
};

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
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
) {
    let popup = crate::app::POPUP.load(std::sync::atomic::Ordering::Relaxed);
    if popup == crate::app::POPUP_ACCOUNT {
        switch_account(frame, rect, account);
    } else if popup == crate::app::POPUP_CURRENCY {
        switch_currency(frame, rect, currency);
    } else if popup == crate::app::POPUP_WATCHLIST {
        switch_watchlist(frame, rect, watchlist);
    } else if popup == crate::app::POPUP_HELP {
        crate::views::help::render(frame, rect);
    } else if popup == crate::app::POPUP_SEARCH {
        searching(frame, rect, search);
    }
}

fn switch_account(frame: &mut Frame, rect: Rect, account: &mut LocalSearch<crate::data::Account>) {
    const MAX_SIZE: (u16, u16) = (50, 30);
    let rect = crate::ui::rect::centered(MAX_SIZE.0, MAX_SIZE.1, rect);
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
    frame.set_cursor(
        // Put cursor past the end of the input text
        chunks[0].x + u16::try_from(input.visual_cursor()).unwrap() + 1,
        // Move one line down, from the border to the input line
        chunks[0].y + 1,
    );

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

    let table = Table::new(rows)
        .block(
            Block::default()
                .borders(Borders::all())
                .border_style(styles::border()),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .widths(&column_constraints)
        .column_spacing(2);

    frame.render_stateful_widget(table, chunks[1], &mut account.table);
}

fn switch_currency(
    frame: &mut Frame,
    rect: Rect,
    currency: &mut LocalSearch<openapi::account::CurrencyInfo>,
) {
    const MAX_SIZE: (u16, u16) = (50, 30);
    let rect = crate::ui::rect::centered(MAX_SIZE.0, MAX_SIZE.1, rect);
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
    frame.set_cursor(
        // Put cursor past the end of the input text
        chunks[0].x + u16::try_from(input.visual_cursor()).unwrap() + 1,
        // Move one line down, from the border to the input line
        chunks[0].y + 1,
    );

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

    let table = Table::new(rows)
        .block(
            Block::default()
                .borders(Borders::all())
                .border_style(styles::border()),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .widths(&column_constraints)
        .column_spacing(2);

    frame.render_stateful_widget(table, chunks[1], &mut currency.table);
}

fn switch_watchlist(
    frame: &mut Frame,
    rect: Rect,
    groups: &mut LocalSearch<crate::data::WatchlistGroup>,
) {
    const MAX_SIZE: (u16, u16) = (50, 30);
    let rect = crate::ui::rect::centered(MAX_SIZE.0, MAX_SIZE.1, rect);
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
    frame.set_cursor(
        // Put cursor past the end of the input text
        chunks[0].x + u16::try_from(input.visual_cursor()).unwrap() + 1,
        // Move one line down, from the border to the input line
        chunks[0].y + 1,
    );

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

    let table = Table::new(rows)
        .block(
            Block::default()
                .borders(Borders::all())
                .border_style(styles::border()),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .widths(&column_constraints)
        .column_spacing(2);

    frame.render_stateful_widget(table, chunks[1], &mut groups.table);
}

fn searching(frame: &mut Frame, rect: Rect, search: &mut Search<openapi::search::StockItem>) {
    const MAX_SIZE: (u16, u16) = (50, 30);
    let rect = crate::ui::rect::centered(MAX_SIZE.0, MAX_SIZE.1, rect);
    frame.render_widget(Clear, rect);

    let chunks = Layout::default()
        .margin(1)
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Percentage(100)].as_ref())
        .split(rect);

    let input = &search.input;
    // one line, without scroll
    let paragraph = Paragraph::new(input.value()).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(styles::border())
            .title(t!("SearchStock.title")),
    );
    frame.render_widget(paragraph, chunks[0]);
    frame.set_cursor(
        // Put cursor past the end of the input text
        chunks[0].x + u16::try_from(input.visual_cursor()).unwrap() + 1,
        // Move one line down, from the border to the input line
        chunks[0].y + 1,
    );

    let column_widths = [12, 34];

    let rows = search
        .options()
        .into_iter()
        .map(|stock| {
            let market = stock.market.as_str().into();
            Row::new(vec![
                Cell::from(Line::from(vec![
                    Span::styled(stock.market, styles::market(market)),
                    Span::raw(" "),
                    Span::raw(stock.code),
                ])),
                Cell::from(stock.name),
            ])
        })
        .collect::<Vec<_>>();

    let column_constraints = column_widths.map(|w| Constraint::Length(u16::try_from(w).unwrap()));

    let table = Table::new(rows)
        .block(
            Block::default()
                .borders(Borders::all())
                .border_style(styles::border()),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .widths(&column_constraints)
        .column_spacing(2);

    frame.render_stateful_widget(table, chunks[1], &mut search.table);
}
