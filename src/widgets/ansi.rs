use ansi_parser::AnsiParser;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::Widget,
};
use unicode_width::UnicodeWidthStr;

pub struct Ansi<'a>(pub &'a str);

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
                            fn color(v: &[u8]) -> Option<Color> {
                                if v.len() < 2 {
                                    return None;
                                }
                                match v[1] {
                                    2 if v.len() >= 5 => Some(Color::Rgb(v[2], v[3], v[4])),
                                    5 if v.len() >= 3 => Some(Color::Indexed(v[2])),
                                    _ => None,
                                }
                            }

                            s = match v.first() {
                                Some(0) => Style::default(),
                                Some(1) => s.add_modifier(Modifier::BOLD),
                                Some(2) => s.remove_modifier(Modifier::BOLD),
                                // Standard foreground colors (30-37)
                                Some(30) => s.fg(Color::Black),
                                Some(31) => s.fg(Color::Red),
                                Some(32) => s.fg(Color::Green),
                                Some(33) => s.fg(Color::Yellow),
                                Some(34) => s.fg(Color::Blue),
                                Some(35) => s.fg(Color::Magenta),
                                Some(36) => s.fg(Color::Cyan),
                                Some(37 | 97) => s.fg(Color::White),
                                // Bright foreground colors (90-97)
                                Some(90) => s.fg(Color::DarkGray),
                                Some(91) => s.fg(Color::LightRed),
                                Some(92) => s.fg(Color::LightGreen),
                                Some(93) => s.fg(Color::LightYellow),
                                Some(94) => s.fg(Color::LightBlue),
                                Some(95) => s.fg(Color::LightMagenta),
                                Some(96) => s.fg(Color::LightCyan),
                                // 256-color/RGB foreground
                                Some(38) => {
                                    if let Some(c) = color(&v) {
                                        s.fg(c)
                                    } else {
                                        s
                                    }
                                }
                                // 256-color/RGB background
                                Some(48) => {
                                    if let Some(c) = color(&v) {
                                        s.bg(c)
                                    } else {
                                        s
                                    }
                                }
                                _ => s,
                            };
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
