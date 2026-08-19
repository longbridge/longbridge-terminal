use std::{cell::RefCell, rc::Rc};

use colored::{Color, Colorize};

use crate::{chart_data::ChartData, y_axis::YAxis};

pub struct InfoBar {
    pub name: String,
    /// Rise / fall colours, set from the caller's up-down convention. Under
    /// "red up" a gain is red, so this bar may not hardcode green/red.
    pub bullish_color: Color,
    pub bearish_color: Color,
    chart_data: Rc<RefCell<ChartData>>,
}

impl InfoBar {
    pub const HEIGHT: i64 = 2;

    pub fn new(name: String, chart_data: Rc<RefCell<ChartData>>) -> InfoBar {
        InfoBar {
            name,
            bullish_color: Color::BrightGreen,
            bearish_color: Color::BrightRed,
            chart_data,
        }
    }

    pub fn render(&self) -> String {
        let chart_data = self.chart_data.borrow();
        let main_set = chart_data.main_candle_set.clone();
        let visible_set = chart_data.visible_candle_set.clone();

        let candles = visible_set.candles;
        let mut output_str = String::new();

        output_str += "\n";
        output_str += &"─".repeat(candles.len() + YAxis::WIDTH as usize);
        output_str += "\n";

        let mut avg_format = format!("{:.2}", main_set.average);
        avg_format = match main_set.last_price {
            lp if lp > main_set.average => avg_format.bold().color(self.bullish_color),
            lp if lp < main_set.average => avg_format.bold().color(self.bearish_color),
            _ => avg_format.bold().yellow(),
        }
        .to_string();

        let (arrow, variation_color) = if main_set.variation > 0.0 {
            ("\u{2196}", self.bullish_color)
        } else {
            ("\u{2199}", self.bearish_color)
        };
        let price_color = if main_set.last_price >= main_set.average {
            self.bullish_color
        } else {
            self.bearish_color
        };

        output_str += &format!(
            "Price: {price} | Highest: {high} | Lowest: {low} | Var.: {var} | Avg.: {avg} │ Cum. Vol: {vol}",
            high = format!("{:.2}", main_set.max_price).color(self.bullish_color).bold(),
            low = format!("{:.2}", main_set.min_price).color(self.bearish_color).bold(),
            var = format!("{} {:>+.2}%", arrow, main_set.variation).color(variation_color).bold(),
            avg = avg_format,
            price = format!("{:.2}", main_set.last_price).color(price_color).bold(),
            vol = format!("{:.0}", main_set.cumulative_volume).bold(),
        );

        output_str
    }
}
