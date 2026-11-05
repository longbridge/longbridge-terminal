//! Theme-aware palette for the AI chat TUI.
//!
//! The chat used to hardcode ANSI names (`DarkGray`, `Gray`, `White`) and a
//! handful of fixed dark-tuned RGB values for its chrome. Those assume a dark
//! terminal: `DarkGray` (bright-black, palette 8) collapses toward the
//! background on many dark themes, and `Gray`/`White` vanish on a light one.
//!
//! Detection runs once at startup — [`detect`] queries the terminal's actual
//! background colour (OSC 11, via `terminal-colorsaurus`) and picks the dark or
//! light palette. A terminal that won't answer (non-tty) falls back to the dark
//! palette — the look the app is tuned for, and what a dark terminal wants.
//!
//! Colour depth: the palette is authored in 24-bit RGB. A terminal that does
//! not advertise truecolour via `COLORTERM` (notably Apple Terminal, which
//! renders RGB sequences as washed greyscale) has every RGB cell downgraded to
//! the nearest xterm-256 colour once per frame in [`downgrade_buffer`] — a
//! single pass over the finished buffer, so it covers every view, not just the
//! palette. Truecolour terminals keep the exact RGB.
//!
//! Everything reads it through [`pal`]. Brand-accent cyan is an ANSI colour the
//! terminal already maps to its theme, and self-contained `fg`+`bg` pairs (the
//! code-block shading in `markdown`, the index badges) stay put.

use std::sync::OnceLock;

use ratatui::style::{Color, Style};

/// Which background the palette is tuned for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Dark,
    Light,
}

/// The theme-dependent colours the chat chrome draws with.
#[derive(Clone, Copy)]
pub struct Palette {
    mode: Mode,
    /// Faint chrome: hints, placeholders, inactive borders. Was `DarkGray`.
    dim: Color,
    /// Readable secondary text: welcome copy, labels. Was `Gray`.
    muted: Color,
    /// Strongest emphasis: selected / hovered / active. Was `White`.
    strong: Color,
    /// The reader's own message band, and the text set on it.
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
    /// Tuned for a dark terminal.
    pub const fn dark() -> Self {
        Self {
            mode: Mode::Dark,
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

    /// Tuned for a light terminal: dark text on pale grounds.
    pub const fn light() -> Self {
        Self {
            mode: Mode::Light,
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

    /// Foreground colour of the faint-chrome role. Replaces `Color::DarkGray`.
    pub fn dim_fg(&self) -> Color {
        self.dim
    }

    /// Foreground colour of readable secondary text. Replaces `Color::Gray`.
    pub fn muted_fg(&self) -> Color {
        self.muted
    }

    /// Foreground colour of the strongest emphasis. Replaces `Color::White`.
    pub fn strong_fg(&self) -> Color {
        self.strong
    }

    /// Faint chrome as a full style.
    pub fn dim(&self) -> Style {
        Style::new().fg(self.dim)
    }

    /// Readable secondary text as a full style.
    pub fn muted(&self) -> Style {
        Style::new().fg(self.muted)
    }

    /// The strongest emphasis as a full style.
    pub fn strong(&self) -> Style {
        Style::new().fg(self.strong)
    }

    /// Whether a light background was detected. The welcome logo uses this to
    /// pick the official icon variant: its white bars become black on a light
    /// terminal (as in `app-icon-dark.svg`) so they don't vanish.
    pub fn is_light(&self) -> bool {
        self.mode == Mode::Light
    }

    /// Name of the chosen mode, for the detection log.
    fn mode_name(&self) -> &'static str {
        match self.mode {
            Mode::Dark => "dark",
            Mode::Light => "light",
        }
    }
}

/// Whether the terminal advertises 24-bit colour through `COLORTERM`. Apple
/// Terminal does not, and renders RGB sequences as washed greyscale — so when
/// this is false, colours are downgraded to xterm-256 (which it does render).
pub fn supports_truecolor() -> bool {
    truecolor_from(std::env::var("COLORTERM").ok().as_deref())
}

fn truecolor_from(colorterm: Option<&str>) -> bool {
    matches!(colorterm, Some("truecolor" | "24bit"))
}

/// Downgrade every RGB cell in the frame to its nearest xterm-256 colour, for a
/// terminal that renders 256 colours but not 24-bit RGB (Apple Terminal). A
/// single pass over the finished buffer, so it covers *every* view — welcome,
/// sessions, charts, markdown, the ticker — not just the palette. No-op on a
/// truecolour terminal, which keeps the exact RGB.
pub fn downgrade_buffer(frame: &mut ratatui::Frame) {
    if supports_truecolor() {
        return;
    }
    for cell in &mut frame.buffer_mut().content {
        // A full block (`█`) doesn't cover its whole cell on Apple Terminal,
        // leaving gaps — so bars (charts, treemap, columns) read as separated
        // squares. Backing each full block with its own colour fills the cell.
        // Partial blocks (`▊`, `▄`, …) are left alone: their partial coverage
        // encodes a fractional value or shape.
        if cell.symbol() == "█" && cell.bg == Color::Reset {
            cell.bg = cell.fg;
        }
        cell.fg = to_256(cell.fg);
        cell.bg = to_256(cell.bg);
    }
}

/// Nearest xterm-256 colour to an RGB value (a no-op for non-RGB colours).
/// Maps near-greys onto the 232–255 ramp and everything else onto the 6×6×6
/// cube — the standard reduction, good enough for chrome tints and the logo.
pub fn to_256(color: Color) -> Color {
    // The 6×6×6 colour cube's per-axis levels.
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let Color::Rgb(red, green, blue) = color else {
        return color;
    };
    let nearest_level = |value: u8| -> usize {
        LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, level)| (i16::from(**level) - i16::from(value)).abs())
            .map_or(0, |(idx, _)| idx)
    };
    let (ri, gi, bi) = (
        nearest_level(red),
        nearest_level(green),
        nearest_level(blue),
    );
    let cube = 16 + 36 * ri + 6 * gi + bi;
    // The 232–255 greyscale ramp runs value 8..=238 in steps of 10.
    let gray_step = (((u16::from(red) + u16::from(green) + u16::from(blue)) / 3).saturating_sub(8)
        / 10)
        .min(23) as u8;
    let gray_value = 8 + 10 * gray_step;
    // Pick the cube colour or the grey, whichever is actually closer — so a near
    // grey stays grey instead of snapping to a tinted cube cell.
    let dist2 = |cr: u8, cg: u8, cb: u8| {
        let dr = i32::from(cr) - i32::from(red);
        let dg = i32::from(cg) - i32::from(green);
        let db = i32::from(cb) - i32::from(blue);
        dr * dr + dg * dg + db * db
    };
    let idx =
        if dist2(LEVELS[ri], LEVELS[gi], LEVELS[bi]) <= dist2(gray_value, gray_value, gray_value) {
            cube as u8
        } else {
            232 + gray_step
        };
    Color::Indexed(idx)
}

static PALETTE: OnceLock<Palette> = OnceLock::new();

/// The active palette. Falls back to the dark palette until [`detect`] runs, so
/// the look is unchanged before detection and in tests that never call it.
pub fn pal() -> Palette {
    PALETTE.get().copied().unwrap_or_else(Palette::dark)
}

/// Query the terminal's background once and latch the matching palette. Must run
/// before the event-reader thread is spawned, or the terminal's OSC 11 reply is
/// swallowed as an input event. A light background latches the light palette;
/// anything else latches the dark palette. On a terminal without truecolour the
/// palette is downgraded to xterm-256 so its colours still render.
pub fn detect() {
    use terminal_colorsaurus::{theme_mode, QueryOptions, ThemeMode};

    let result = theme_mode(QueryOptions::default());
    let palette = match result {
        Ok(ThemeMode::Light) => Palette::light(),
        _ => Palette::dark(),
    };
    // Logged so a "colours look wrong in terminal X" report can be traced to
    // which branch ran, without guessing. The RGB→256 downgrade for terminals
    // without truecolour happens per-frame in `downgrade_buffer`, not here.
    tracing::info!(
        "ai theme: TERM_PROGRAM={:?} COLORTERM={:?} colorsaurus={:?} -> {} ({})",
        std::env::var("TERM_PROGRAM").ok(),
        std::env::var("COLORTERM").ok(),
        result,
        palette.mode_name(),
        if supports_truecolor() {
            "truecolor"
        } else {
            "256"
        },
    );
    let _ = PALETTE.set(palette);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Until detection runs — and in every test, which never calls `detect` — the
    /// dark palette stands in, so the historical look is the default.
    #[test]
    fn the_palette_defaults_to_dark() {
        assert_eq!(pal().dim(), Palette::dark().dim());
        assert_eq!(pal().user_bg, Palette::dark().user_bg);
    }

    /// The two palettes are genuinely different: swapping in the light one has
    /// to actually change the chrome, or a light terminal is no better off.
    #[test]
    fn dark_and_light_disagree_on_every_role() {
        let (d, l) = (Palette::dark(), Palette::light());
        assert_ne!(d.dim(), l.dim());
        assert_ne!(d.muted(), l.muted());
        assert_ne!(d.strong(), l.strong());
        assert_ne!(d.user_bg, l.user_bg);
        assert_ne!(d.user_fg, l.user_fg);
        assert_ne!(d.sel_bg, l.sel_bg);
        assert_ne!(d.hover_bg, l.hover_bg);
        assert_ne!(d.tab_bg, l.tab_bg);
    }

    #[test]
    fn truecolor_is_recognised_only_from_the_known_colorterm_values() {
        assert!(truecolor_from(Some("truecolor")));
        assert!(truecolor_from(Some("24bit")));
        assert!(!truecolor_from(Some("256")));
        assert!(!truecolor_from(None));
    }

    /// The 256-downgrade turns every RGB colour into an indexed one (so a
    /// no-truecolour terminal shows colour), and leaves non-RGB colours alone.
    #[test]
    fn the_indexed_downgrade_maps_rgb_onto_the_256_palette() {
        for c in [
            Palette::dark().dim,
            Palette::dark().strong,
            Palette::dark().user_bg,
        ] {
            assert!(matches!(to_256(c), Color::Indexed(_)));
        }
        assert_eq!(to_256(Color::Cyan), Color::Cyan);
        // A pure grey lands on the 232–255 greyscale ramp.
        assert!(matches!(to_256(Color::Rgb(128, 128, 128)), Color::Indexed(n) if n >= 232));
    }
}
