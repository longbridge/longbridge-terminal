use ratatui::{
    prelude::{Alignment, Constraint, Layout, Rect},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use rust_decimal::Decimal;
use unicode_width::UnicodeWidthStr;

use crate::data::{Counter, ReadyState, STOCKS};
use crate::utils::DecimalExt;
use crate::{tui::systems::WsState, tui::ui::styles};

/// Columns reserved for the connection light, plus a space before it.
const STATUS_WIDTH: u16 = 4;
/// Set between two index quotes.
const SEPARATOR: &str = "  ·  ";

pub fn render(frame: &mut Frame, rect: Rect, indexes: &[Counter; 3], state: &WsState) {
    let [index_area, status_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(STATUS_WIDTH)]).areas(rect);

    // Each quote takes only the width it needs, packed from the left. Splitting
    // the bar into three equal shares left most of each one empty, which is
    // what made the row read as gaps with numbers in it.
    let mut spans: Vec<Span> = Vec::new();
    let mut footer_rects = [Rect::default(); 3];
    let mut x = index_area.x;

    for (i, (counter, toggle_key)) in indexes.iter().zip(['Q', 'W', 'E']).enumerate() {
        let (last_done, prev_close) = STOCKS
            .get(counter)
            .map(|s| (s.quote.last_done, s.quote.prev_close))
            .unwrap_or_default();
        let (ordering, numbers) = last_done
            .zip(prev_close.filter(|v| !v.is_zero()))
            .map_or_else(
                || (std::cmp::Ordering::Equal, " -- -- --".to_string()),
                |(last_done, prev_close)| {
                    let increase = last_done - prev_close;
                    let increase_percent = increase / prev_close;
                    let numbers = format!(
                        " {} {} {}",
                        last_done.format_quote_by_counter(counter),
                        increase.format_quote_by_counter(counter),
                        increase_percent.format_percent()
                    );
                    (increase.cmp(&Decimal::ZERO), numbers)
                },
            );
        let color = styles::up(ordering);
        let key = format!("StockIndex.{counter}");
        let name = t!(&key).to_string();
        let group = [
            Span::styled(name, color),
            Span::styled(numbers, color),
            Span::styled(format!(" [{toggle_key}]"), styles::dark_gray()),
        ];

        let width: u16 = group.iter().map(|s| s.width() as u16).sum();
        let separator = u16::from(i > 0) * SEPARATOR.width() as u16;
        // Drop a whole quote rather than clip one mid-number.
        if x + separator + width > index_area.right() {
            break;
        }
        if i > 0 {
            spans.push(Span::styled(SEPARATOR, styles::dark_gray()));
            x += separator;
        }
        spans.extend(group);
        footer_rects[i] = Rect {
            x,
            y: index_area.y,
            width,
            height: 1,
        };
        x += width;
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), index_area);
    *crate::tui::mouse::FOOTER_INDEX_RECTS
        .lock()
        .expect("poison") = footer_rects;

    let (status, status_style) = match state.0 {
        ReadyState::Open => {
            if crate::tui::app::QUOTE_BMP.load(atomic::Ordering::Relaxed) {
                ("□□■", styles::bmp()) // Semi-automatic
            } else {
                ("■■■", styles::online())
            }
        }
        ReadyState::Closed => ("□□□", styles::offline()),
        _ => ("···", styles::text()),
    };

    frame.render_widget(
        Paragraph::new(Span::styled(status, status_style)).alignment(Alignment::Right),
        status_area,
    );
}
