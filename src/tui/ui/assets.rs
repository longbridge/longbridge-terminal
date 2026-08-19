use ansi_parser::AnsiParser;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::Text,
    widgets::{Paragraph, Widget},
};

static LOGO_STR: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/logo.ascii"));

/// The site the banner links to, and the prefix that marks the banner's link
/// line so it is styled and hit-tested as one.
pub const HOME_URL: &str = "https://longbridge.com";

static BANNER_STR: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let banner = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/banner.txt"));
    let year = time::OffsetDateTime::now_local()
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
        .year();
    banner
        .replace("%{version}", env!("CARGO_PKG_VERSION"))
        .replace("%{year}", &year.to_string())
});

pub const BANNER_HEIGHT: u16 = 24;

/// The Longbridge mark: a small, colour-accurate version of the app icon.
///
/// Loaded from `assets/logo-mark.ansi`, a hand-tuned 17x8 rendering of the icon's
/// seven-bar chart in its own colours. It is a separate asset rather than a
/// scaled-down `logo.ascii`: reducing that block art far enough to sit above a
/// chat's welcome copy fuses neighbouring bars into one solid run and the chart
/// stops reading as a chart. The tall bars end on a half block, so they measure
/// 7.5 rows — at 8 they read as too long, at 7 the mark looks squat.
#[must_use]
pub fn logo_mark() -> Vec<ratatui::text::Line<'static>> {
    MARK.clone()
}

/// Rows the mark occupies.
#[must_use]
pub fn mark_height() -> u16 {
    MARK.len() as u16
}

/// Columns the mark occupies.
#[must_use]
pub fn mark_width() -> u16 {
    MARK.first().map_or(0, line_width)
}

static MARK_STR: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/logo-mark.ansi"
));

static MARK: std::sync::LazyLock<Vec<ratatui::text::Line<'static>>> =
    std::sync::LazyLock::new(|| {
        use ansi_to_tui::IntoText;
        use ratatui::text::Span;

        let mut lines = MARK_STR.into_text().map(|t| t.lines).unwrap_or_default();
        // Every row is padded to the same width. The mark is drawn centre-aligned
        // as a block of lines, and a short row would be centred on its own and
        // knock the bars out of column.
        let width = lines.iter().map(line_width).max().unwrap_or(0);
        for line in &mut lines {
            let pad = width - line_width(line);
            if pad > 0 {
                line.spans.push(Span::raw(" ".repeat(usize::from(pad))));
            }
        }
        lines
    });

fn line_width(line: &ratatui::text::Line<'_>) -> u16 {
    line.spans
        .iter()
        .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()) as u16)
        .sum()
}

/// Banner widget that properly renders ANSI-colored logo and text banner
pub struct BannerWidget {
    style: Style,
}

impl BannerWidget {
    pub fn new(style: Style) -> Self {
        Self { style }
    }
}

impl Widget for BannerWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let logo_lines = LOGO_STR.lines().count();
        let banner_lines = BANNER_STR.lines().count();
        let total_lines = logo_lines + 2 + banner_lines; // +2 for spacing

        let logo_height = logo_lines as u16;
        let spacing_height = 2;

        if area.height < total_lines as u16 {
            // If area is too small, just render what we can
            center_ansi(LOGO_STR, area, buf);
            return;
        }

        // Render logo with ANSI support (centered)
        let logo_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: logo_height,
        };
        center_ansi(LOGO_STR, logo_area, buf);

        // Render banner text (without ANSI, centered). Drawn line by line so
        // the home-page URL can be styled as a link and registered as a click
        // target.
        let top = area.y + logo_height + spacing_height;
        for (y, line) in (top..).zip(BANNER_STR.lines()) {
            if y >= area.bottom() {
                break;
            }
            let line_area = centered_line(line, area, y);
            if line.trim() == HOME_URL {
                Paragraph::new(Text::styled(line, crate::tui::ui::styles::link()))
                    .render(line_area, buf);
                crate::tui::mouse::register_link(line_area, HOME_URL);
            } else {
                Paragraph::new(Text::raw(line))
                    .style(self.style)
                    .render(line_area, buf);
            }
        }
    }
}

/// The one-row rect a centered `line` occupies on row `y` of `area`.
fn centered_line(line: &str, area: Rect, y: u16) -> Rect {
    use unicode_width::UnicodeWidthStr;

    let width = (line.width() as u16).min(area.width);
    Rect {
        x: area.x + (area.width - width) / 2,
        y,
        width,
        height: 1,
    }
}

/// Helper function to center ANSI text within an area
fn center_ansi(text: &str, area: Rect, buf: &mut Buffer) {
    use unicode_width::UnicodeWidthStr;

    for (line_idx, line) in text.lines().enumerate() {
        let y = area.y + line_idx as u16;
        if y >= area.bottom() {
            break;
        }

        // Calculate visible width (without ANSI sequences)
        let mut visible_text = String::new();
        for block in line.ansi_parse() {
            if let ansi_parser::Output::TextBlock(t) = block {
                visible_text.push_str(t);
            }
        }

        let text_width = visible_text.width() as u16;
        let offset = if text_width < area.width {
            (area.width - text_width) / 2
        } else {
            0
        };

        // Render the line with offset for centering
        let line_area = Rect {
            x: area.x + offset,
            y,
            width: area.width.saturating_sub(offset),
            height: 1,
        };

        crate::tui::widgets::Ansi(line).render(line_area, buf);
    }
}

/// Legacy function for backward compatibility
pub fn banner(style: Style) -> BannerWidget {
    BannerWidget::new(style)
}
