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
    pub fn enter_full_screen() {
        use crossterm::{cursor, terminal};

        _ = terminal::enable_raw_mode();
        _ = crossterm::execute!(
            std::io::stdout(),
            terminal::EnterAlternateScreen,
            terminal::Clear(terminal::ClearType::All),
            terminal::Clear(terminal::ClearType::Purge),
            cursor::MoveTo(0, 0),
            cursor::Hide
        );
    }

    pub fn exit_full_screen() {
        use crossterm::{cursor, terminal};

        // 恢复终端状态
        _ = crossterm::execute!(
            std::io::stdout(),
            cursor::Show,  // 显示光标
            terminal::LeaveAlternateScreen,  // 离开备用屏幕
        );
        _ = terminal::disable_raw_mode();  // 禁用 raw mode
    }

    /// 优雅退出 - 清理终端并退出程序
    pub fn graceful_exit(code: i32) -> ! {
        Self::exit_full_screen();
        std::process::exit(code);
    }
}
