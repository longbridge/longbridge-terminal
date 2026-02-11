use std::{borrow::Cow, cmp::Ordering};

use crate::data::{Market, StockColorMode};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::ListItem,
};

use crate::helper::Sign;

#[inline]
pub fn header() -> Style {
    Style::default().fg(Color::Gray)
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

#[inline]
pub fn popup() -> Style {
    text()
}

#[inline]
pub fn title() -> Style {
    text()
}

#[inline]
pub fn border() -> Style {
    Style::default().fg(Color::DarkGray)
}

#[inline]
pub fn market(m: Market) -> Style {
    use crate::data::Market as M;
    let color = match m {
        M::US => Color::Blue,
        M::HK => Color::Magenta,
        M::CN => Color::Red,
        M::SG => Color::Cyan,
    };
    Style::default().fg(color)
}

#[inline]
pub fn up(val: Ordering) -> Style {
    match val {
        Ordering::Less => bull_bear().1,
        Ordering::Equal => Style::default().fg(Color::Reset),
        Ordering::Greater => bull_bear().0,
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

#[inline]
pub fn stock_color_mode() -> StockColorMode {
    // Default to GreenUp mode (green for up, red for down - China mainland convention)
    // TODO: Read from user settings
    StockColorMode::GreenUp
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

#[allow(clippy::needless_pass_by_value)]
pub fn item<'a>(label: String, value: impl Into<Cow<'a, str>>) -> ListItem<'a> {
    let spans = Line::from(vec![
        Span::styled(format!("{label}: "), super::styles::label()),
        Span::styled(value, super::styles::text()),
    ]);
    ListItem::new(spans)
}

#[allow(clippy::needless_pass_by_value)]
pub fn item_up<'a>(label: String, value: impl Into<Cow<'a, str>>) -> ListItem<'a> {
    let value = value.into();
    let style = super::styles::up(value.sign());
    let spans = Line::from(vec![
        Span::styled(format!("{label}: "), super::styles::label()),
        Span::styled(value, style),
    ]);
    ListItem::new(spans)
}

#[allow(clippy::needless_pass_by_value)]
pub fn item_label(label: String) -> ListItem<'static> {
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

pub fn online() -> Style {
    Style::default().fg(Color::Green)
}

pub fn offline() -> Style {
    Style::default().fg(Color::Red)
}

pub fn bmp() -> Style {
    Style::default().fg(Color::Yellow)
}
