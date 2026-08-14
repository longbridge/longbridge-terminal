use ansi_parser::AnsiParser;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Color, Style},
    text::Text,
    widgets::{Paragraph, Widget},
};

static LOGO_STR: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/logo.ascii"));

static BANNER_STR: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let banner = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/banner.txt"));
    banner.replace("%{version}", env!("CARGO_PKG_VERSION"))
});

pub const BANNER_HEIGHT: u16 = 23;

/// The Longbridge mark: a small, colour-accurate version of the app icon.
///
/// The icon is a seven-bar chart in a 69x69 box — two full-height bars on the
/// left (a thin white one and a thick teal one), then a staircase climbing to the
/// right. It is re-drawn from that geometry instead of being sampled down from
/// `logo.ascii`, because scaling block art small enough to sit above a chat's
/// welcome copy fuses neighbouring bars into one solid run and the chart is lost.
///
/// Heights are measured in half rows and drawn with a lower half block, so a bar
/// can end mid-cell. That extra step is what lets the tall bars be 7.5 rows: at
/// 8 they read as too long, and at 7 the whole mark looks squat.
///
/// The size is fixed at [`MARK_WIDTH`]x[`MARK_HEIGHT`]: the smallest that keeps
/// every bar, every gap, and the thin/thick contrast.
#[must_use]
pub fn logo_mark() -> Vec<ratatui::text::Line<'static>> {
    use ratatui::text::{Line, Span};

    (0..MARK_HEIGHT)
        .map(|row| {
            // Bars stand on a common baseline, so a cell's fill depends only on
            // how far it is from the bottom.
            let from_bottom = (MARK_HEIGHT - row) * 2;
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (i, &(width, halves, color)) in MARK_BARS.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw(" "));
                }
                let glyph = if halves >= from_bottom {
                    "█"
                } else if halves + 1 == from_bottom {
                    "▄"
                } else {
                    " "
                };
                spans.push(Span::styled(
                    glyph.repeat(usize::from(width)),
                    Style::default().fg(color),
                ));
            }
            Line::from(spans)
        })
        .collect()
}

/// Rows the mark occupies.
pub const MARK_HEIGHT: u16 = 8;

/// Columns the mark occupies: the bars plus one blank column between each pair.
pub const MARK_WIDTH: u16 = {
    let mut total = MARK_BARS.len() as u16 - 1;
    let mut i = 0;
    while i < MARK_BARS.len() {
        total += MARK_BARS[i].0;
        i += 1;
    }
    total
};

/// `(width in columns, height in half rows, colour)` per bar, left to right.
///
/// Taken from the icon's rects — widths 3, 10, 9, 3, 10, 9, 3 and heights 69, 69,
/// 9, 9, 17, 26, 43 over 69 units — scaled to the grid above. Thin bars get one
/// column and thick ones two, which is the narrowest layout that still tells them
/// apart.
const MARK_BARS: [(u16, u16, Color); 7] = [
    (1, 15, MARK_WHITE),
    (2, 15, MARK_TEAL),
    (2, 2, MARK_YELLOW),
    (1, 2, MARK_WHITE),
    (2, 4, MARK_ORANGE),
    (2, 6, MARK_WHITE),
    (1, 10, MARK_ORANGE),
];

// The brand palette, as literal RGB rather than the nearest ANSI slot, so the
// mark is the icon's colour and not an approximation of it.
const MARK_WHITE: Color = Color::Rgb(0xFF, 0xFF, 0xFF);
const MARK_TEAL: Color = Color::Rgb(0x00, 0xDB, 0xB6);
const MARK_YELLOW: Color = Color::Rgb(0xFF, 0xE0, 0x00);
const MARK_ORANGE: Color = Color::Rgb(0xFC, 0x52, 0x00);

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
        let banner_height = banner_lines as u16;

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

        // Render banner text (without ANSI, centered)
        let banner_area = Rect {
            x: area.x,
            y: area.y + logo_height + spacing_height,
            width: area.width,
            height: banner_height,
        };
        let banner_text = Text::raw(BANNER_STR.as_str());
        Paragraph::new(banner_text)
            .alignment(Alignment::Center)
            .style(self.style)
            .render(banner_area, buf);
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
