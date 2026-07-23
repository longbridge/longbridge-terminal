use std::{borrow::Cow, cmp::Ordering};

use crate::data::{Market, StockColorMode};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::ListItem,
};

use crate::utils::Sign;

#[inline]
pub fn header() -> Style {
    // Bold table headers to lift them off the data rows below.
    Style::default()
        .fg(Color::Gray)
        .add_modifier(Modifier::BOLD)
}

#[inline]
pub fn gray() -> Style {
    Style::default().fg(Color::Gray)
}

#[inline]
pub fn dark_gray() -> Style {
    Style::default().fg(Color::DarkGray)
}

#[inline]
pub fn label() -> Style {
    Style::default().fg(Color::Gray)
}

#[inline]
pub fn text() -> Style {
    Style::default().fg(Color::Reset)
}

#[inline]
pub fn primary() -> Style {
    Style::default().fg(Color::White)
}

#[inline]
pub fn text_selected() -> Style {
    text().add_modifier(Modifier::REVERSED)
}

#[inline]
pub fn keyboard() -> Style {
    text()
}

/// Style for the `[key]` part of a shortcut hint — brighter + bold so the key
/// stands out from its dimmed description (Grok-style hint hierarchy).
#[inline]
pub fn hint_key() -> Style {
    gray().add_modifier(Modifier::BOLD)
}

#[inline]
pub fn popup() -> Style {
    text()
}

#[inline]
pub fn title() -> Style {
    // Bold titles give panels/modals a clear visual anchor.
    text().add_modifier(Modifier::BOLD)
}

#[inline]
pub fn border() -> Style {
    Style::default().fg(Color::DarkGray)
}

#[inline]
pub fn active_border() -> Style {
    Style::default().fg(Color::Gray)
}

#[inline]
pub fn market_color(m: Market) -> Color {
    use crate::data::Market as M;
    match m {
        M::US => Color::Blue,
        M::HK => Color::Magenta,
        M::CN => Color::Red,
        M::SG => Color::Cyan,
    }
}

#[inline]
pub fn market(m: Market) -> Style {
    Style::default().fg(market_color(m))
}

#[inline]
pub fn up(val: Ordering) -> Style {
    match val {
        Ordering::Less => bull_bear().1,
        Ordering::Equal => Style::default().fg(Color::Reset),
        Ordering::Greater => bull_bear().0,
    }
}

/// Direction glyph for a value's sign — a quick up/down cue, independent of the
/// red/green color convention (▲ = up, ▼ = down, empty = flat).
#[inline]
pub fn trend_arrow(val: Ordering) -> &'static str {
    match val {
        Ordering::Greater => "▲ ",
        Ordering::Less => "▼ ",
        Ordering::Equal => "",
    }
}

#[inline]
pub fn up_color(val: Ordering) -> Color {
    let (red, green) = (Color::Red, Color::Green);
    match val {
        Ordering::Less => match stock_color_mode() {
            StockColorMode::RedUp => green,
            StockColorMode::GreenUp => red,
        },
        Ordering::Equal => Color::Reset,
        Ordering::Greater => match stock_color_mode() {
            StockColorMode::RedUp => red,
            StockColorMode::GreenUp => green,
        },
    }
}

/// Return a style for the curreny
#[inline]
pub fn currency(currency: &str) -> Style {
    let color = match currency {
        "USD" => Color::LightBlue,
        "HKD" => Color::LightMagenta,
        "CNY" => Color::LightRed,
        "SGD" => Color::LightCyan,
        _ => Color::Reset,
    };

    Style::default().fg(color)
}

// Global up/down color convention. Defaults to GreenUp (China mainland
// convention: green = up); overridden at startup from persisted user settings.
static STOCK_COLOR_MODE: atomic::Atomic<StockColorMode> =
    atomic::Atomic::new(StockColorMode::GreenUp);

/// Set the global stock up/down color convention (called from settings).
#[inline]
pub fn set_stock_color_mode(mode: StockColorMode) {
    STOCK_COLOR_MODE.store(mode, std::sync::atomic::Ordering::Relaxed);
}

#[inline]
pub fn stock_color_mode() -> StockColorMode {
    STOCK_COLOR_MODE.load(std::sync::atomic::Ordering::Relaxed)
}

#[inline]
pub fn bull_bear() -> (Style, Style) {
    let red = Style::default().fg(Color::LightRed);
    let green = Style::default().fg(Color::LightGreen);
    match stock_color_mode() {
        StockColorMode::RedUp => (red, green),
        StockColorMode::GreenUp => (green, red),
    }
}

#[inline]
pub fn bull_bear_color() -> (cli_candlestick_chart::Color, cli_candlestick_chart::Color) {
    let red = cli_candlestick_chart::Color::BrightRed;
    let green = cli_candlestick_chart::Color::BrightGreen;
    match stock_color_mode() {
        StockColorMode::RedUp => (red, green),
        StockColorMode::GreenUp => (green, red),
    }
}

pub fn item<'a>(label: impl Into<String>, value: impl Into<Cow<'a, str>>) -> ListItem<'a> {
    let label = label.into();
    let spans = Line::from(vec![
        Span::styled(format!("{label}: "), super::styles::label()),
        Span::styled(value, super::styles::text()),
    ]);
    ListItem::new(spans)
}

pub fn item_up<'a>(label: impl Into<String>, value: impl Into<Cow<'a, str>>) -> ListItem<'a> {
    let label = label.into();
    let value = value.into();
    let style = super::styles::up(value.sign());
    let spans = Line::from(vec![
        Span::styled(format!("{label}: "), super::styles::label()),
        Span::styled(value, style),
    ]);
    ListItem::new(spans)
}

pub fn item_label(label: impl Into<String>) -> ListItem<'static> {
    let label = label.into();
    let span = Span::styled(format!("{label}: "), super::styles::label());

    ListItem::new(span)
}

pub fn item_value<'a>(value: impl Into<Cow<'a, str>>) -> ListItem<'a> {
    let span = Span::styled(value, super::styles::text());

    ListItem::new(span)
}

pub fn item_value_up<'a>(value: impl Into<Cow<'a, str>>) -> ListItem<'a> {
    let value = value.into();
    let style = super::styles::up(value.sign());
    let span = Span::styled(value, style);

    ListItem::new(span)
}

pub fn risk_level(level: u8) -> (String, Style) {
    match level {
        0 => (
            t!("RiskLevel.Safe").to_string(),
            Style::default().fg(Color::Green),
        ),
        1 => (
            t!("RiskLevel.Middle").to_string(),
            Style::default().fg(Color::Yellow),
        ),
        2 => (
            t!("RiskLevel.Warning").to_string(),
            Style::default().fg(Color::Rgb(255, 140, 0)),
        ),
        3 => (
            t!("RiskLevel.Danger").to_string(),
            Style::default().fg(Color::Red),
        ),
        _ => (
            t!("RiskLevel.Unknown").to_string(),
            Style::default().fg(Color::Gray),
        ),
    }
}

pub fn online() -> Style {
    Style::default().fg(Color::Green)
}

pub fn offline() -> Style {
    Style::default().fg(Color::Red)
}

pub fn bmp() -> Style {
    Style::default().fg(Color::Yellow)
}

#[cfg(test)]
mod tests {
    use super::trend_arrow;
    use std::cmp::Ordering;

    #[test]
    fn trend_arrow_directions() {
        assert_eq!(trend_arrow(Ordering::Greater), "▲ ");
        assert_eq!(trend_arrow(Ordering::Less), "▼ ");
        assert_eq!(trend_arrow(Ordering::Equal), "");
    }
}
