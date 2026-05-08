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

/// Dim the base color by `factor` (0.0 = black, 1.0 = original). Used for the
/// gradient area fill below the price line.
fn dim_color(base: Color, factor: f64) -> Color {
    match base {
        Color::TrueColor { r, g, b } => Color::TrueColor {
            r: (f64::from(r) * factor).round() as u8,
            g: (f64::from(g) * factor).round() as u8,
            b: (f64::from(b) * factor).round() as u8,
        },
        c => c,
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
            bullish_color: Color::TrueColor {
                r: 52,
                g: 208,
                b: 88,
            },
            bearish_color: Color::TrueColor {
                r: 234,
                g: 74,
                b: 90,
            },
            vol_bullish_color: Color::TrueColor {
                r: 52,
                g: 208,
                b: 88,
            },
            vol_bearish_color: Color::TrueColor {
                r: 234,
                g: 74,
                b: 90,
            },
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

        // line_bits: braille dots for the price line strokes
        // fill_bits: braille dots for the gradient area below the line
        let mut line_bits = vec![vec![0u8; chart_char_width]; chart_char_height];
        let mut fill_bits = vec![vec![0u8; chart_char_width]; chart_char_height];

        if px_w > 0 && px_h > 1 {
            let step = n as f64 / px_w as f64;

            for px_x in 0..px_w {
                let i0 = ((px_x as f64 * step) as usize).min(n - 1);
                let i1 = (((px_x + 1) as f64 * step) as usize).min(n - 1);
                let y0 = px_y(close_prices[i0]);
                let y1 = px_y(close_prices[i1]);

                // Fill vertical stroke between adjacent samples to avoid gaps
                for y in y0.min(y1)..=y0.max(y1) {
                    let char_row = y / 4;
                    let dy = y % 4;
                    let col = px_x / 2;
                    let dx = px_x % 2;
                    if char_row < chart_char_height {
                        line_bits[char_row][col] |= dot_bit(dx, dy);
                    }
                }

                // Gradient fill: all pixels below the line stroke
                let y_below = y0.max(y1) + 1;
                for y in y_below..px_h {
                    let char_row = y / 4;
                    let dy = y % 4;
                    let col = px_x / 2;
                    let dx = px_x % 2;
                    if char_row < chart_char_height {
                        fill_bits[char_row][col] |= dot_bit(dx, dy);
                    }
                }
            }
        }

        let y_axis_empty = {
            let cell = " ".repeat((YAxis::CHAR_PRECISION + YAxis::DEC_PRECISION + 2) as usize);
            let margin = " ".repeat((YAxis::MARGIN_RIGHT + 1) as usize);
            format!("{cell}│{margin}")
        };

        let mut output = String::new();

        for (row, (line_row, fill_row)) in line_bits.iter().zip(fill_bits.iter()).enumerate() {
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

            for (lb, fb) in line_row.iter().zip(fill_row.iter()) {
                let combined = lb | fb;
                if combined == 0 {
                    output.push(' ');
                } else if *lb != 0 {
                    // Cell contains part of the price line — render at full line color
                    output += &braille_char(combined)
                        .to_string()
                        .color(line_color)
                        .to_string();
                } else {
                    // Cell is in the fill area — gradient: bright near line, dim at bottom
                    let t = row as f64 / chart_char_height.max(1) as f64;
                    let factor = 0.55 - 0.35 * t; // ~55% at top, ~20% at bottom
                    let fill_color = dim_color(line_color, factor);
                    output += &braille_char(*fb).to_string().color(fill_color).to_string();
                }
            }
        }

        // Volume pane: braille-filled bars from bottom up, colored per candle direction
        if has_volume && vol_height > 0 {
            let max_vol = candle_set.max_volume;
            let vol_h_usize = vol_height as usize;
            let vol_px_h = vol_h_usize * 4;
            let mut vol_bits = vec![vec![0u8; chart_char_width]; vol_h_usize];
            let mut vol_is_bullish = vec![vec![true; chart_char_width]; vol_h_usize];

            if max_vol > 0.0 && px_w > 0 {
                let step = n as f64 / px_w as f64;

                for px_x in 0..px_w {
                    let i = ((px_x as f64 * step) as usize).min(n.saturating_sub(1));
                    let candle = &self.candles[i];
                    let vol = candle.volume.unwrap_or(0.0);
                    if vol <= 0.0 {
                        continue;
                    }
                    let is_bullish = candle.close >= candle.open;
                    let fill =
                        ((vol / max_vol) * (vol_px_h.saturating_sub(1)) as f64).round() as usize;

                    let col = px_x / 2;
                    let dx = px_x % 2;

                    // Fill from the bottom of the volume pane upward
                    for py in 0..=fill {
                        let y_from_top = vol_px_h - 1 - py;
                        let char_row = y_from_top / 4;
                        let dy = y_from_top % 4;
                        if char_row < vol_h_usize {
                            vol_bits[char_row][col] |= dot_bit(dx, dy);
                            vol_is_bullish[char_row][col] = is_bullish;
                        }
                    }
                }
            }

            for (vol_row, vol_bull_row) in vol_bits.iter().zip(vol_is_bullish.iter()) {
                output.push('\n');
                output += &y_axis_empty;
                for (b, is_bull) in vol_row.iter().zip(vol_bull_row.iter()) {
                    if *b == 0 {
                        output.push(' ');
                    } else {
                        let color = if *is_bull {
                            self.vol_bullish_color
                        } else {
                            self.vol_bearish_color
                        };
                        output += &braille_char(*b).to_string().color(color).to_string();
                    }
                }
            }
        }

        // Info bar: separator + price statistics
        output.push('\n');
        output += &"─".repeat(chart_char_width + YAxis::WIDTH as usize);
        output.push('\n');

        let arrow = if candle_set.variation > 0.0 {
            "↖"
        } else {
            "↙"
        };
        let var_color = if candle_set.variation > 0.0 {
            "green"
        } else {
            "red"
        };

        let avg_str = format!("{:.2}", candle_set.average);
        let avg_colored = if candle_set.last_price > candle_set.average {
            avg_str.bold().red()
        } else if candle_set.last_price < candle_set.average {
            avg_str.bold().green()
        } else {
            avg_str.bold().yellow()
        }
        .to_string();

        output += &format!(
            "Price: {price} | Highest: {high} | Lowest: {low} | Var.: {var} | Avg.: {avg} │ Cum. Vol: {vol}",
            price = format!("{:.2}", candle_set.last_price).green().bold(),
            high = format!("{:.2}", candle_set.max_price).green().bold(),
            low = format!("{:.2}", candle_set.min_price).red().bold(),
            var = format!("{arrow} {:>+.2}%", candle_set.variation)
                .color(var_color)
                .bold(),
            avg = avg_colored,
            vol = format!("{:.0}", candle_set.cumulative_volume).green().bold(),
        );

        output
    }
}
