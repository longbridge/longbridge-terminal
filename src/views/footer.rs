use ratatui::{
    prelude::{Alignment, Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use rust_decimal::Decimal;

use crate::data::{Counter, ReadyState, STOCKS};
use crate::helper::DecimalExt;
use crate::{system::WsState, ui::styles};

pub fn render(frame: &mut Frame, rect: Rect, indexes: &[Counter; 3], state: &WsState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(90), Constraint::Percentage(10)])
        .split(rect);

    let mut spans = Vec::with_capacity(9);
    for (counter, toggle_key) in indexes.iter().zip(['Q', 'W', 'E']) {
        let (last_done, prev_close) = STOCKS
            .get(counter)
            .map(|s| (s.quote.last_done, s.quote.prev_close))
            .unwrap_or_default();
        let (ordering, numbers) = last_done
            .zip(prev_close.filter(|v| !v.is_zero()))
            .map_or_else(
                || (std::cmp::Ordering::Equal, " -- -- -- ".to_string()),
                |(last_done, prev_close)| {
                    let increase = last_done - prev_close;
                    let increase_percent = increase / prev_close;
                    let numbers = format!(
                        " {} {} {} ",
                        last_done.format_quote_by_counter(counter),
                        increase.format_quote_by_counter(counter),
                        increase_percent.format_percent()
                    );
                    (increase.cmp(&Decimal::ZERO), numbers)
                },
            );
        let color = styles::up(ordering);
        // todo: add reversed modifier for chosen stock
        let name = t!(&format!("StockIndex.{counter}"));
        let index_name = Span::styled(name, color);
        let index_num = Span::styled(numbers, color);
        let toggle_key = Span::styled(format!("[{toggle_key}]  "), styles::dark_gray());
        spans.extend([index_name, index_num, toggle_key]);
    }
    let indexes = Paragraph::new(Line::from(spans));
    frame.render_widget(indexes, chunks[0]);

    let (status, status_style) = match state.0 {
        ReadyState::Open => {
            if crate::app::QUOTE_BMP.load(atomic::Ordering::Relaxed) {
                ("□□■", styles::bmp()) // Semi-automatic
            } else {
                ("■■■", styles::online())
            }
        }
        ReadyState::Closed => ("□□□", styles::offline()),
        _ => ("···", styles::text()),
    };
    let text = Span::styled(status, status_style);

    frame.render_widget(Paragraph::new(text).alignment(Alignment::Right), chunks[1]);
}
