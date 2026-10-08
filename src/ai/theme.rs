//! Theme-aware palette for the AI chat TUI.
//!
//! The chat used to hardcode ANSI names (`DarkGray`, `Gray`, `White`) and a
//! handful of fixed dark-tuned RGB values for its chrome. Those assume a dark
//! terminal: `DarkGray` (bright-black, palette 8) collapses toward the
//! background on many dark themes, and `Gray`/`White` vanish on a light one.
//!
//! Detection runs once at startup — [`detect`] queries the terminal's actual
//! background colour (OSC 11, via `terminal-colorsaurus`) and picks the dark or
//! light [`Palette`]. Everything reads it through [`pal`], which falls back to
//! the dark palette until detection lands (and in tests, which never call
//! `detect`), so the default matches the historical look.
//!
//! Only the roles that actually break with the background are routed here.
//! Brand-accent cyan is an ANSI colour the terminal already maps to its theme,
//! and self-contained pairs that set both `fg` and `bg` (the code-block shading
//! in `markdown`, the index badges) read on any background, so they stay put.

use std::sync::OnceLock;

use ratatui::style::Color;

/// The theme-dependent colours the chat chrome draws with.
#[derive(Clone, Copy)]
pub struct Palette {
    /// Faint chrome: hints, placeholders, inactive borders, disabled labels.
    /// Replaces `Color::DarkGray`, which sat too close to a dark background.
    pub dim: Color,
    /// Readable secondary text: welcome copy, sample prompts, field labels.
    /// Replaces `Color::Gray`, which vanished on a light background.
    pub muted: Color,
    /// The strongest emphasis: selected / hovered / active rows and values.
    /// Replaces `Color::White`, which vanished on a light background.
    pub strong: Color,
    /// The reader's own message band, and the light text set on it.
    pub user_bg: Color,
    pub user_fg: Color,
    /// A selected history row.
    pub sel_bg: Color,
    /// The ground under a hovered slash-command / dropdown row.
    pub hover_bg: Color,
    /// The ground under the ticker entry whose quote drawer is open.
    pub tab_bg: Color,
}

impl Palette {
    /// Tuned for a dark terminal. `dim`/`muted`/`strong` are lifted off the
    /// old ANSI names so faint text stays legible instead of melting into black.
    pub const fn dark() -> Self {
        Self {
            dim: Color::Rgb(122, 130, 140),
            muted: Color::Rgb(190, 196, 204),
            strong: Color::Rgb(240, 242, 245),
            user_bg: Color::Rgb(38, 45, 60),
            user_fg: Color::Rgb(226, 232, 240),
            sel_bg: Color::Rgb(45, 50, 62),
            hover_bg: Color::Rgb(48, 48, 48),
            tab_bg: Color::Rgb(30, 58, 66),
        }
    }

    /// Tuned for a light terminal: dark text on pale grounds so nothing washes
    /// out against a white background.
    pub const fn light() -> Self {
        Self {
            dim: Color::Rgb(122, 128, 136),
            muted: Color::Rgb(74, 80, 88),
            strong: Color::Rgb(17, 20, 26),
            user_bg: Color::Rgb(224, 231, 242),
            user_fg: Color::Rgb(28, 38, 54),
            sel_bg: Color::Rgb(219, 226, 238),
            hover_bg: Color::Rgb(226, 229, 234),
            tab_bg: Color::Rgb(207, 231, 240),
        }
    }
}

static PALETTE: OnceLock<Palette> = OnceLock::new();

/// The active palette. Falls back to the dark palette until [`detect`] runs, so
/// the look is unchanged before detection and in tests that never call it.
pub fn pal() -> Palette {
    PALETTE.get().copied().unwrap_or_else(Palette::dark)
}

/// Query the terminal's background once and latch the matching palette. Must run
/// before the event-reader thread is spawned, or the terminal's OSC 11 reply is
/// swallowed as an input event. A terminal that does not answer (or is not a
/// tty) leaves the dark fallback in place.
pub fn detect() {
    use terminal_colorsaurus::{theme_mode, QueryOptions, ThemeMode};

    let palette = match theme_mode(QueryOptions::default()) {
        Ok(ThemeMode::Light) => Palette::light(),
        _ => Palette::dark(),
    };
    let _ = PALETTE.set(palette);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Until detection runs — and in every test, which never calls `detect` — the
    /// dark palette stands in, so the historical look is the default.
    #[test]
    fn the_palette_defaults_to_dark() {
        assert_eq!(pal().dim, Palette::dark().dim);
        assert_eq!(pal().user_bg, Palette::dark().user_bg);
    }

    /// The two palettes are genuinely different: swapping in the light one has to
    /// actually change the chrome colours, or a light terminal is no better off.
    #[test]
    fn dark_and_light_disagree_on_every_role() {
        let (d, l) = (Palette::dark(), Palette::light());
        assert_ne!(d.dim, l.dim);
        assert_ne!(d.muted, l.muted);
        assert_ne!(d.strong, l.strong);
        assert_ne!(d.user_bg, l.user_bg);
        assert_ne!(d.user_fg, l.user_fg);
        assert_ne!(d.sel_bg, l.sel_bg);
        assert_ne!(d.hover_bg, l.hover_bg);
        assert_ne!(d.tab_bg, l.tab_bg);
    }
}
