use crate::{candle_set::CandleSet, info_bar::InfoBar, y_axis::YAxis, Candle};
use colored::{Color, Colorize};

// Braille dot-to-bit mapping (Unicode braille U+2800):
//   col 0  col 1
//   dot1   dot4   row 0   0x01  0x08
//   dot2   dot5   row 1   0x02  0x10
//   dot3   dot6   row 2   0x04  0x20
//   dot7   dot8   row 3   0x40  0x80
const fn dot_bit(dx: usize, dy: usize) -> u8 {
    match (dx, dy) {
        (0, 0) => 0x01,
        (0, 1) => 0x02,
        (0, 2) => 0x04,
        (0, 3) => 0x40,
        (1, 0) => 0x08,
        (1, 1) => 0x10,
        (1, 2) => 0x20,
        (1, 3) => 0x80,
        _ => 0,
    }
}

fn braille_char(bits: u8) -> char {
    char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ')
}

/// Shade block for the area wash, `depth` character rows below the curve.
///
/// Shading rather than a blended background colour: a smooth gradient needs
/// truecolor, which ignores the user's terminal theme. These three blocks
/// dither the line's own colour at 75 / 50 / 25% coverage, so the wash stays on
/// the terminal palette and still reads as densest right under the curve.
fn shade_char(depth: usize) -> char {
    match depth {
        0..=1 => '\u{2593}', // ▓
        2..=4 => '\u{2592}', // ▒
        _ => '\u{2591}',     // ░
    }
}

/// High-resolution braille line chart for short-term periods.
///
/// Uses Unicode braille characters (2×4 dot grid per char) to render a price
/// curve with 4× the vertical resolution of block-character approaches.
/// The area below the price line is filled with a gradient background.
pub struct LineChart {
    pub bullish_color: Color,
    pub bearish_color: Color,
    pub vol_bullish_color: Color,
    pub vol_bearish_color: Color,
    candles: Vec<Candle>,
    size: (u16, u16),
}

impl LineChart {
    pub fn new_with_size(candles: Vec<Candle>, size: (u16, u16)) -> Self {
        Self {
            bullish_color: Color::BrightGreen,
            bearish_color: Color::BrightRed,
            vol_bullish_color: Color::BrightGreen,
            vol_bearish_color: Color::BrightRed,
            candles,
            size,
        }
    }

    pub fn set_bull_color(&mut self, color: Color) {
        self.bullish_color = color;
    }

    pub fn set_bear_color(&mut self, color: Color) {
        self.bearish_color = color;
    }

    pub fn set_vol_bull_color(&mut self, color: Color) {
        self.vol_bullish_color = color;
    }

    pub fn set_vol_bear_color(&mut self, color: Color) {
        self.vol_bearish_color = color;
    }

    pub fn render(&self) -> String {
        if self.candles.is_empty() {
            return String::new();
        }

        let w = i64::from(self.size.0);
        let h = i64::from(self.size.1);

        if w <= YAxis::WIDTH || h <= InfoBar::HEIGHT + 1 {
            return String::new();
        }

        let chart_char_width = (w - YAxis::WIDTH) as usize;

        let has_volume = self.candles.iter().any(|c| c.volume.unwrap_or(0.0) > 0.0);
        let vol_height = if has_volume { (h / 6).max(1) } else { 0 };

        let chart_char_height = ((h - InfoBar::HEIGHT - vol_height).max(1)) as usize;

        let candle_set = CandleSet::new(self.candles.clone());
        let min_price = candle_set.min_price;
        let max_price = candle_set.max_price;
        let price_span = (max_price - min_price).max(1e-9);

        let line_color = if candle_set.variation >= 0.0 {
            self.bullish_color
        } else {
            self.bearish_color
        };

        let close_prices: Vec<f64> = self.candles.iter().map(|c| c.close).collect();
        let n = close_prices.len();

        let px_h = chart_char_height * 4;
        let px_w = chart_char_width * 2;

        // px_y: maps price → pixel row from top (0 = top of chart, px_h-1 = bottom)
        let px_y = |v: f64| -> usize {
            let norm = (v.clamp(min_price, max_price) - min_price) / price_span;
            ((1.0 - norm) * (px_h - 1) as f64).round() as usize
        };

        // The price curve keeps braille's 2x4 resolution. The area under it does
        // NOT: braille can only set a foreground, so a braille "fill" is a dot
        // texture, which is what made this chart read as stippled rather than
        // filled. The area is drawn with shade blocks instead — full-cell,
        // evenly dithered, and in the line's own terminal colour.
        let mut line_bits = vec![vec![0u8; chart_char_width]; chart_char_height];
        // The character row each column's wash starts at; `chart_char_height`
        // means the column has no wash at all.
        let mut fill_from = vec![chart_char_height; chart_char_width];

        if px_w > 0 && px_h > 1 {
            let step = n as f64 / px_w as f64;

            for px_x in 0..px_w {
                let i0 = ((px_x as f64 * step) as usize).min(n - 1);
                let i1 = (((px_x + 1) as f64 * step) as usize).min(n - 1);
                let y0 = px_y(close_prices[i0]);
                let y1 = px_y(close_prices[i1]);
                let col = px_x / 2;
                let dx = px_x % 2;

                // Fill vertical stroke between adjacent samples to avoid gaps
                for y in y0.min(y1)..=y0.max(y1) {
                    let char_row = y / 4;
                    let dy = y % 4;
                    if char_row < chart_char_height {
                        line_bits[char_row][col] |= dot_bit(dx, dy);
                    }
                }

                // The wash starts under the lowest dot of the stroke. A cell
                // spans two pixel columns, so the shallower of the two wins and
                // the wash meets the curve instead of notching it.
                let start = (y0.max(y1) + 1) / 4;
                fill_from[col] = fill_from[col].min(start.min(chart_char_height));
            }
        }

        let y_axis_empty = {
            let cell = " ".repeat((YAxis::CHAR_PRECISION + YAxis::DEC_PRECISION + 2) as usize);
            let margin = " ".repeat((YAxis::MARGIN_RIGHT + 1) as usize);
            format!("{cell}│{margin}")
        };

        let mut output = String::new();

        for (row, line_row) in line_bits.iter().enumerate() {
            output.push('\n');

            // Y-axis tick every 4 character rows (from bottom), matching YAxis convention
            let y_from_bottom = chart_char_height - 1 - row;
            if y_from_bottom.is_multiple_of(4) {
                let price =
                    min_price + y_from_bottom as f64 * price_span / chart_char_height as f64;
                let cell_len = (YAxis::CHAR_PRECISION + YAxis::DEC_PRECISION + 1) as usize;
                let margin = " ".repeat(YAxis::MARGIN_RIGHT as usize);
                output += &format!(
                    "{0:<cell_len$.2} │┈{margin}",
                    price,
                    cell_len = cell_len,
                    margin = margin
                );
            } else {
                output += &y_axis_empty;
            }

            for (col, lb) in line_row.iter().enumerate() {
                if *lb != 0 {
                    // The curve wins the cell.
                    output += &braille_char(*lb).to_string().color(line_color).to_string();
                } else if row >= fill_from[col] {
                    output += &shade_char(row - fill_from[col])
                        .to_string()
                        .color(line_color)
                        .to_string();
                } else {
                    output.push(' ');
                }
            }
        }

        // Volume pane: half-block bars from the bottom up, coloured per candle
        // direction. Half blocks rather than braille so neighbouring bars form
        // one continuous mass — braille glyphs leave a gap at every cell edge,
        // which reads as a dotted texture instead of a bar.
        if has_volume && vol_height > 0 {
            let max_vol = candle_set.max_volume;
            let vol_h_usize = vol_height as usize;
            let vol_half_h = vol_h_usize * 2;
            // Half-cell rows filled per column, and that column's direction.
            let mut vol_fill = vec![0usize; chart_char_width];
            let mut vol_is_bullish = vec![true; chart_char_width];

            if max_vol > 0.0 && px_w > 0 {
                let step = n as f64 / px_w as f64;

                for px_x in 0..px_w {
                    let i = ((px_x as f64 * step) as usize).min(n.saturating_sub(1));
                    let candle = &self.candles[i];
                    let vol = candle.volume.unwrap_or(0.0);
                    if vol <= 0.0 {
                        continue;
                    }
                    let col = px_x / 2;
                    // A bar at least one half-cell tall, so a small but real
                    // volume never vanishes entirely.
                    let filled = (((vol / max_vol) * vol_half_h as f64).round() as usize)
                        .clamp(1, vol_half_h);
                    if filled >= vol_fill[col] {
                        vol_fill[col] = filled;
                        vol_is_bullish[col] = candle.close >= candle.open;
                    }
                }
            }

            for row in 0..vol_h_usize {
                output.push('\n');
                output += &y_axis_empty;
                // Half-cell rows counted from the bottom of the pane.
                let top_from_bottom = (vol_h_usize - row) * 2 - 1;
                let bottom_from_bottom = (vol_h_usize - row - 1) * 2;
                for (col, filled) in vol_fill.iter().enumerate() {
                    let color = if vol_is_bullish[col] {
                        self.vol_bullish_color
                    } else {
                        self.vol_bearish_color
                    };
                    match (*filled > top_from_bottom, *filled > bottom_from_bottom) {
                        (true, _) => {
                            output += &"\u{2588}".color(color).to_string();
                        }
                        (false, true) => {
                            output += &"\u{2584}".color(color).to_string();
                        }
                        (false, false) => output.push(' '),
                    }
                }
            }
        }

        // Info bar: separator + price statistics
        output.push('\n');
        output += &"─".repeat(chart_char_width + YAxis::WIDTH as usize);
        output.push('\n');

        // Rise and fall follow the caller's configured convention — under
        // "red up" a gain is red — so nothing here may hardcode green/red.
        let (arrow, var_color) = if candle_set.variation > 0.0 {
            ("\u{2196}", self.bullish_color)
        } else {
            ("\u{2199}", self.bearish_color)
        };

        let avg_str = format!("{:.2}", candle_set.average);
        let avg_colored = match candle_set.last_price {
            lp if lp > candle_set.average => avg_str.bold().color(self.bullish_color),
            lp if lp < candle_set.average => avg_str.bold().color(self.bearish_color),
            _ => avg_str.bold().yellow(),
        }
        .to_string();

        output += &format!(
            "Price: {price} | Highest: {high} | Lowest: {low} | Var.: {var} | Avg.: {avg} │ Cum. Vol: {vol}",
            price = format!("{:.2}", candle_set.last_price)
                .color(line_color)
                .bold(),
            high = format!("{:.2}", candle_set.max_price)
                .color(self.bullish_color)
                .bold(),
            low = format!("{:.2}", candle_set.min_price)
                .color(self.bearish_color)
                .bold(),
            var = format!("{arrow} {:>+.2}%", candle_set.variation)
                .color(var_color)
                .bold(),
            avg = avg_colored,
            vol = format!("{:.0}", candle_set.cumulative_volume).bold(),
        );

        output
    }
}
