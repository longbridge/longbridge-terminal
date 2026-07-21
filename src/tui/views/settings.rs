use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::tui::settings::{self, SettingKind};
use crate::tui::ui::styles;

/// Render the settings modal: one row per setting, its choices shown inline
/// with the active one highlighted, plus a description and a key hint.
pub fn render(frame: &mut Frame, rect: Rect) {
    let metas = settings::all();
    let sel = settings::selected();

    let width = 64u16.min(rect.width);
    let height = (metas.len() as u16 * 3 + 5).min(rect.height);
    let x = rect.x + rect.width.saturating_sub(width) / 2;
    let y = rect.y + rect.height.saturating_sub(height) / 2;
    let area = Rect::new(x, y, width, height);

    let mut lines: Vec<Line> = vec![Line::from("")];
    for (i, meta) in metas.iter().enumerate() {
        let selected_row = i == sel;
        let label_style = if selected_row {
            styles::text_selected()
        } else {
            styles::text()
        };
        let mut spans = vec![Span::styled(
            format!("  {}   ", t!(meta.label)),
            label_style,
        )];
        match &meta.kind {
            SettingKind::Enum { choices } => {
                let cur = meta.id.current();
                for choice in *choices {
                    let style = if choice.canonical == cur {
                        styles::text_selected()
                    } else {
                        styles::dark_gray()
                    };
                    spans.push(Span::styled(format!("[ {} ] ", t!(choice.label)), style));
                }
            }
        }
        lines.push(Line::from(spans));
        lines.push(Line::styled(
            format!("    {}", t!(meta.description)),
            styles::dark_gray(),
        ));
        lines.push(Line::from(""));
    }
    lines.push(Line::styled(
        format!("  {}", t!("settings.hint")),
        styles::dark_gray(),
    ));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(styles::border())
        .title(Span::styled(t!("settings.title"), styles::title()));
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).style(styles::popup()).block(block),
        area,
    );
}
