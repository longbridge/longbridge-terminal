use std::error::Error;

use ansi_parser::AnsiParser;
use cli_candlestick_chart::{Candle, Chart};
use tui::{style::Modifier, widgets::Widget};
use unicode_width::UnicodeWidthStr;

pub struct AnsiEscape<'a>(&'a str);

impl<'a> Widget for AnsiEscape<'a> {
    fn render(self, area: tui::layout::Rect, buf: &mut tui::buffer::Buffer) {
        debug_assert!(self.0.lines().count() <= area.height as usize);
        for (h, line) in self.0.lines().enumerate() {
            let h = area.top() + h as u16;
            let mut w = area.left();
            let mut s = tui::style::Style::default();
            for block in line.ansi_parse() {
                match block {
                    ansi_parser::Output::TextBlock(text) => {
                        if w < buf.area.width {
                            buf.set_string(w, h, text, s);
                            w += text.width() as u16;
                        }
                    }
                    ansi_parser::Output::Escape(escape) => match escape {
                        ansi_parser::AnsiSequence::SetGraphicsMode(v) => {
                            fn color(v: &[u8]) -> tui::style::Color {
                                match v[1] {
                                    2 => tui::style::Color::Rgb(v[2], v[3], v[4]),
                                    5 => tui::style::Color::Indexed(v[2]),
                                    _ => panic!("unsupport color"),
                                }
                            }

                            s = match v[0] {
                                0 => tui::style::Style::default(),
                                1 => s.add_modifier(Modifier::BOLD),
                                2 => s.remove_modifier(Modifier::BOLD),
                                38 => tui::style::Style::default().fg(color(&v)),
                                48 => tui::style::Style::default().bg(color(&v)),
                                v => panic!("unsupport attribute: {v}"),
                            };
                        }
                        ansi_parser::AnsiSequence::ResetMode(_) => {
                            s = tui::style::Style::default();
                        }
                        _ => panic!("unssport escape sequence"),
                    },
                }
            }
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
    let mut terminal = tui::Terminal::new(tui::backend::CrosstermBackend::new(stdout))?;

    let mut candles: Vec<Candle> = Vec::new();
    let mut rdr = csv::Reader::from_path("./examples/BTC-USD.csv")?;
    for result in rdr.deserialize() {
        let candle: Candle = result?;
        candles.push(candle);
    }

    terminal.draw(|frame| {
        let area = frame.size().inner(&tui::layout::Margin {
            vertical: 2,
            horizontal: 2,
        });

        let mut chart = Chart::new_with_size(candles, (area.width, area.height));
        chart.set_name(String::from("BTC/USDT"));
        chart.set_bear_color(1, 205, 254);
        chart.set_bull_color(255, 107, 153);

        frame.render_widget(AnsiEscape(&chart.render()), area);
    })?;

    crossterm::event::read()?;
    crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;
    crossterm::terminal::disable_raw_mode()?;

    Ok(())
}
