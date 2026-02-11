use bevy_ecs::prelude::*;
use ratatui::{
    backend::Backend,
    layout::{Alignment, Constraint, Direction, Layout, Margin},
    text::{Line, Span, Text},
    widgets::{Paragraph, Widget},
    Terminal,
};

#[derive(Clone, Debug, Default, Resource, Component)]
pub struct Content<'a> {
    heading: Text<'a>,
    content: Text<'a>,
}

impl<'a> Content<'a> {
    pub fn new(heading: impl Into<Text<'a>>, content: impl Into<Text<'a>>) -> Self {
        Self {
            heading: heading.into(),
            content: content.into(),
        }
    }

    pub fn anykey_exit<B: Backend>(mut self, terminal: &mut Terminal<B>) -> std::io::Result<()> {
        let anykey = Span::styled(t!("exit.any_key"), crate::ui::styles::gray());
        self.content.lines.push(Line::from(anykey));

        terminal.draw(|frame| {
            frame.render_widget(self, frame.size());
        })?;
        crossterm::event::read()?;
        Ok(())
    }
}

impl Widget for Content<'_> {
    fn render(self, rect: ratatui::layout::Rect, buf: &mut ratatui::buffer::Buffer) {
        // make vertical center
        let heading_len = u16::try_from(self.heading.lines.len()).unwrap_or(5);
        let content_len = u16::try_from(self.content.lines.len()).unwrap_or(10);
        let rect = rect
            .height
            .checked_sub(heading_len + content_len + 2)
            .map_or(rect, |h| {
                rect.inner(&Margin {
                    vertical: h / 2,
                    horizontal: 0,
                })
            });

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(heading_len),
                Constraint::Length(2),
                Constraint::Length(content_len),
            ])
            .split(rect);

        let heading = Paragraph::new(self.heading).alignment(Alignment::Center);
        let content = Paragraph::new(self.content).alignment(Alignment::Center);

        heading.render(chunks[0], buf);
        content.render(chunks[2], buf);
    }
}
