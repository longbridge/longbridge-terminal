use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::tui::settings::{self, SettingKind, SettingMeta};
use crate::tui::ui::styles;

/// Columns of padding inside the block, on each side.
const PAD: u16 = 2;
/// Width of the focus marker column (`▌ `).
const MARKER: u16 = 2;
/// Blank columns between the label column and the first value chip.
const LABEL_GAP: u16 = 2;
/// Rows a setting occupies: its label/values row, its description, and a gap.
const ROW_HEIGHT: u16 = 3;

/// Render the settings modal.
///
/// Rows are laid out as a two-column grid — every label padded to the widest
/// one — so the value chips line up in a column instead of starting wherever
/// each label happens to end. The focused row is marked in the gutter and its
/// label brightened; the *selected value* is a filled accent chip. Those are
/// two different things, and the old modal drew both the same way.
pub fn render(frame: &mut Frame, rect: Rect) {
    // The modal shows the market TUI's rows; the chat's own live in its
    // Settings view, off the same table.
    let metas = settings::modal_rows();
    let sel = settings::selected();

    let label_width = metas
        .iter()
        .map(|m| t!(m.label).width() as u16)
        .max()
        .unwrap_or(0);
    let values_x = MARKER + label_width + LABEL_GAP;
    let body_width = metas
        .iter()
        .map(|m| values_x + chips_width(m))
        .chain(
            metas
                .iter()
                .map(|m| MARKER + t!(m.description).width() as u16),
        )
        .max()
        .unwrap_or(0);

    let hint = t!("settings.hint").to_string();
    let title = t!("settings.title").to_string();
    // The block's own borders + padding, and enough width for the footer hint
    // and title, which sit on the border rows.
    let chrome = 2 + PAD * 2;
    let width = (body_width + chrome)
        .max(hint.width() as u16 + 4)
        .max(title.width() as u16 + 6)
        .min(rect.width);
    let height = (metas.len() as u16 * ROW_HEIGHT + 3).min(rect.height);
    let area = Rect::new(
        rect.x + rect.width.saturating_sub(width) / 2,
        rect.y + rect.height.saturating_sub(height) / 2,
        width,
        height,
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(styles::border())
        .padding(Padding::horizontal(PAD))
        .title(Span::styled(format!(" {title} "), styles::title()))
        .title_bottom(
            Line::from(Span::styled(format!(" {hint} "), styles::dark_gray())).centered(),
        );
    let inner = block.inner(area);

    let mut lines: Vec<Line> = vec![Line::from("")];
    let mut targets: Vec<(Rect, usize, usize)> = Vec::new();

    for (i, meta) in metas.iter().enumerate() {
        let focused = i == sel;
        let mut spans = vec![
            Span::styled(
                if focused { "▌ " } else { "  " },
                if focused {
                    styles::accent_text()
                } else {
                    styles::text()
                },
            ),
            Span::styled(
                pad_to(&t!(meta.label), label_width),
                if focused {
                    styles::title()
                } else {
                    styles::text()
                },
            ),
            Span::raw(" ".repeat(LABEL_GAP as usize)),
        ];

        // Chips, recording each one's rect so a click can pick that value.
        let row_y = inner.y + 1 + i as u16 * ROW_HEIGHT;
        let mut x = inner.x + values_x;
        match &meta.kind {
            SettingKind::Enum { choices } => {
                let current = meta.id.current();
                for (c, choice) in choices.iter().enumerate() {
                    if c > 0 {
                        spans.push(Span::raw(" "));
                        x += 1;
                    }
                    let text = format!(" {} ", t!(choice.label));
                    let chip_width = text.width() as u16;
                    spans.push(Span::styled(
                        text,
                        if choice.canonical == current {
                            styles::chip_active()
                        } else {
                            styles::chip_inactive()
                        },
                    ));
                    targets.push((
                        Rect {
                            x,
                            y: row_y,
                            width: chip_width,
                            height: 1,
                        },
                        i,
                        c,
                    ));
                    x += chip_width;
                }
            }
        }

        lines.push(Line::from(spans));
        lines.push(Line::styled(
            format!("  {}", t!(meta.description)),
            styles::dark_gray(),
        ));
        lines.push(Line::from(""));
    }

    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).style(styles::popup()).block(block),
        area,
    );
    *crate::tui::mouse::SETTINGS_CHIP_RECTS
        .lock()
        .expect("poison") = targets;
}

/// Total width of a row's value chips, separated by one space.
fn chips_width(meta: &SettingMeta) -> u16 {
    match &meta.kind {
        SettingKind::Enum { choices } => {
            let chips: u16 = choices.iter().map(|c| t!(c.label).width() as u16 + 2).sum();
            chips + choices.len().saturating_sub(1) as u16
        }
    }
}

/// Right-pad `text` to `width` display columns.
fn pad_to(text: &str, width: u16) -> String {
    let pad = (width as usize).saturating_sub(text.width());
    format!("{text}{}", " ".repeat(pad))
}
