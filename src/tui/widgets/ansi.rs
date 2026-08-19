use ansi_parser::AnsiParser;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::Widget,
};
use unicode_width::UnicodeWidthStr;

pub struct Ansi<'a>(pub &'a str);

/// Apply one SGR escape's parameters to `style`.
///
/// Every parameter is walked, not just the first: a single `ESC[` sequence
/// carries several (`ESC[48;5;22;92m` sets a background *and* a foreground),
/// and reading only the head silently dropped the rest. Note that
/// `ansi_parser` caps a sequence at five parameters, so producers must split
/// two truecolor changes across two escapes.
fn apply_sgr(mut style: Style, params: &[u8]) -> Style {
    if params.is_empty() {
        return Style::default();
    }
    let mut i = 0;
    while i < params.len() {
        let p = params[i];
        style = match p {
            0 => Style::default(),
            1 => style.add_modifier(Modifier::BOLD),
            2 | 22 => style.remove_modifier(Modifier::BOLD),
            3 => style.add_modifier(Modifier::ITALIC),
            4 => style.add_modifier(Modifier::UNDERLINED),
            7 => style.add_modifier(Modifier::REVERSED),
            30..=37 => style.fg(ansi_color(p - 30, false)),
            39 => style.fg(Color::Reset),
            40..=47 => style.bg(ansi_color(p - 40, false)),
            49 => style.bg(Color::Reset),
            90..=97 => style.fg(ansi_color(p - 90, true)),
            100..=107 => style.bg(ansi_color(p - 100, true)),
            38 | 48 => {
                let Some((color, used)) = extended_color(&params[i + 1..]) else {
                    break;
                };
                i += used;
                if p == 38 {
                    style.fg(color)
                } else {
                    style.bg(color)
                }
            }
            _ => style,
        };
        i += 1;
    }
    style
}

/// One of the eight ANSI colors, in its normal or bright form.
fn ansi_color(index: u8, bright: bool) -> Color {
    match (index, bright) {
        (0, false) => Color::Black,
        (1, false) => Color::Red,
        (2, false) => Color::Green,
        (3, false) => Color::Yellow,
        (4, false) => Color::Blue,
        (5, false) => Color::Magenta,
        (6, false) => Color::Cyan,
        (7, false) => Color::Gray,
        (0, true) => Color::DarkGray,
        (1, true) => Color::LightRed,
        (2, true) => Color::LightGreen,
        (3, true) => Color::LightYellow,
        (4, true) => Color::LightBlue,
        (5, true) => Color::LightMagenta,
        (6, true) => Color::LightCyan,
        _ => Color::White,
    }
}

/// Parse the tail of a `38`/`48` parameter: `5;n` (indexed) or `2;r;g;b`
/// (truecolor). Returns the color and how many parameters it consumed.
fn extended_color(rest: &[u8]) -> Option<(Color, usize)> {
    match rest.first()? {
        2 if rest.len() >= 4 => Some((Color::Rgb(rest[1], rest[2], rest[3]), 4)),
        5 if rest.len() >= 2 => Some((Color::Indexed(rest[1]), 2)),
        _ => None,
    }
}

impl Widget for Ansi<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        for (h, line) in self.0.lines().enumerate() {
            let h = area.top() + h as u16;
            if h >= area.bottom() {
                break;
            }

            let mut w = area.left();
            let mut s = Style::default();

            for block in line.ansi_parse() {
                match block {
                    ansi_parser::Output::TextBlock(text) => {
                        if w < area.right() {
                            buf.set_string(w, h, text, s);
                            w += text.width() as u16;
                        }
                    }
                    ansi_parser::Output::Escape(escape) => match escape {
                        ansi_parser::AnsiSequence::SetGraphicsMode(v) => {
                            s = apply_sgr(s, &v);
                        }
                        ansi_parser::AnsiSequence::ResetMode(_) => {
                            s = Style::default();
                        }
                        _ => {}
                    },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_sgr, Ansi};
    use ratatui::{
        buffer::Buffer,
        layout::Rect,
        style::{Color, Modifier, Style},
        widgets::Widget,
    };

    #[test]
    fn every_parameter_of_a_sequence_is_applied() {
        // The bug this covers: only the first parameter used to be read, so
        // `ESC[102;92m` — which `colored` emits for a background plus a
        // foreground — silently produced no styling at all.
        let style = apply_sgr(Style::default(), &[102, 92]);
        assert_eq!(style.bg, Some(Color::LightGreen));
        assert_eq!(style.fg, Some(Color::LightGreen));
    }

    #[test]
    fn truecolor_consumes_its_own_parameters() {
        let style = apply_sgr(Style::default(), &[38, 2, 1, 2, 3]);
        assert_eq!(style.fg, Some(Color::Rgb(1, 2, 3)));

        let style = apply_sgr(Style::default(), &[48, 2, 9, 8, 7]);
        assert_eq!(style.bg, Some(Color::Rgb(9, 8, 7)));

        // An indexed colour is two parameters, and what follows still applies.
        let style = apply_sgr(Style::default(), &[38, 5, 42, 1]);
        assert_eq!(style.fg, Some(Color::Indexed(42)));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn reset_clears_and_defaults_are_honoured() {
        let styled = apply_sgr(Style::default(), &[31, 1]);
        assert_eq!(apply_sgr(styled, &[0]), Style::default());
        assert_eq!(apply_sgr(styled, &[39]).fg, Some(Color::Reset));
        assert_eq!(apply_sgr(styled, &[49]).bg, Some(Color::Reset));
    }

    #[test]
    fn foreground_and_background_render_into_the_buffer() {
        // Two escapes rather than one combined sequence: `ansi_parser` caps a
        // sequence at five parameters, so producers must split them.
        let text = "\x1b[38;2;10;20;30m\x1b[48;2;40;50;60m\u{2580}\x1b[0m";
        let area = Rect::new(0, 0, 3, 1);
        let mut buf = Buffer::empty(area);
        Ansi(text).render(area, &mut buf);

        let cell = buf.cell((0, 0)).expect("cell");
        assert_eq!(cell.symbol(), "\u{2580}");
        assert_eq!(cell.style().fg, Some(Color::Rgb(10, 20, 30)));
        assert_eq!(cell.style().bg, Some(Color::Rgb(40, 50, 60)));
    }
}
