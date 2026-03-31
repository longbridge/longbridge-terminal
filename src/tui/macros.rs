#[macro_export]
macro_rules! key {
    ($key:literal) => {
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char($key),
            modifiers: ::crossterm::event::KeyModifiers::NONE,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        }
    };
    ($key:tt) => {
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::$key,
            modifiers: ::crossterm::event::KeyModifiers::NONE,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        }
    };
}

#[macro_export]
macro_rules! ctrl {
    ($key:literal) => {
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char($key),
            modifiers: ::crossterm::event::KeyModifiers::CONTROL,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        }
    };
    ($key:tt) => {
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::$key,
            modifiers: ::crossterm::event::KeyModifiers::CONTROL,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        }
    };
}

#[macro_export]
macro_rules! shift {
    ($key:literal) => {
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::Char($key),
            modifiers: ::crossterm::event::KeyModifiers::SHIFT,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        }
    };
    ($key:tt) => {
        ::crossterm::event::KeyEvent {
            code: ::crossterm::event::KeyCode::$key,
            modifiers: ::crossterm::event::KeyModifiers::SHIFT,
            kind: ::crossterm::event::KeyEventKind::Press,
            state: ::crossterm::event::KeyEventState::NONE,
        }
    };
}
