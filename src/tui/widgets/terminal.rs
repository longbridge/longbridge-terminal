use std::ops::{Deref, DerefMut};

use bevy_ecs::prelude::*;
use ratatui::backend::CrosstermBackend;

#[derive(Resource)]
pub struct Terminal(ratatui::Terminal<CrosstermBackend<std::io::Stdout>>);

impl Deref for Terminal {
    type Target = ratatui::Terminal<CrosstermBackend<std::io::Stdout>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Terminal {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Default for Terminal {
    fn default() -> Self {
        let mut stdout = std::io::stdout();
        // tui doesn't clear buffer for different instances, so clear screen here
        _ = crossterm::execute!(
            stdout,
            crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
        );
        let backend = CrosstermBackend::new(stdout);
        let terminal = ratatui::Terminal::new(backend).unwrap();
        Self(terminal)
    }
}

impl Terminal {
    /// Draw a frame. Shadows [`ratatui::Terminal::draw`] so that per-frame hit
    /// areas — the clickable links the render pass re-registers — are reset
    /// once per frame instead of at every call site.
    pub fn draw<F>(&mut self, render: F) -> std::io::Result<ratatui::CompletedFrame<'_>>
    where
        F: FnOnce(&mut ratatui::Frame),
    {
        crate::tui::mouse::clear_links();
        self.0.draw(render)
    }

    pub fn enter_full_screen() {
        use crossterm::{cursor, event, terminal};

        _ = terminal::enable_raw_mode();
        _ = crossterm::execute!(
            std::io::stdout(),
            terminal::EnterAlternateScreen,
            terminal::Clear(terminal::ClearType::All),
            terminal::Clear(terminal::ClearType::Purge),
            cursor::MoveTo(0, 0),
            cursor::Hide,
            event::EnableMouseCapture,
            // Focus reporting: without it a TUI left open in a background pane
            // reports the same time-in-use as one somebody is watching.
            // Terminals that do not support it simply send nothing.
            event::EnableFocusChange,
        );
    }

    pub fn exit_full_screen() {
        use crossterm::{cursor, event, terminal};

        _ = crossterm::execute!(
            std::io::stdout(),
            event::DisableMouseCapture,
            event::DisableFocusChange,
            cursor::Show,
            terminal::LeaveAlternateScreen,
        );
        _ = terminal::disable_raw_mode();
    }

    /// Graceful exit - cleanup terminal and exit program
    ///
    /// Analytics is settled here rather than after the TUI returns, because it
    /// never does: this is `q` and Ctrl-C, and `process::exit` below ends the
    /// process without unwinding. Anything still queued — the page on screen,
    /// which has no leave yet, and whatever was reported on the way out — would
    /// go with it, silently, since a cancelled request logs nothing.
    ///
    /// The terminal is restored first so the wait happens against a normal
    /// prompt instead of a frozen full-screen frame.
    pub fn graceful_exit(code: i32) -> ! {
        Self::exit_full_screen();
        crate::analytics::leave_page();
        crate::analytics::flush_blocking();
        std::process::exit(code);
    }
}
