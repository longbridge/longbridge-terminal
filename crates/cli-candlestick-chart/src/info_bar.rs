use std::{cell::RefCell, rc::Rc};

use colored::Colorize;

use crate::{chart_data::ChartData, y_axis::YAxis};

pub struct InfoBar {
    pub name: String,
    chart_data: Rc<RefCell<ChartData>>,
}

impl InfoBar {
    pub const HEIGHT: i64 = 2;

    pub fn new(name: String, chart_data: Rc<RefCell<ChartData>>) -> InfoBar {
        InfoBar { name, chart_data }
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
            lp if lp > main_set.average => avg_format.bold().red(),
            lp if lp < main_set.average => avg_format.bold().green(),
            _ => avg_format.bold().yellow(),
        }
        .to_string();

        let variation_output = if main_set.variation > 0.0 {
            ("↖", "green")
        } else {
            ("↙", "red")
        };

        output_str += &format!(
            "Price: {price} | Highest: {high} | Lowest: {low} | Var.: {var} | Avg.: {avg} │ Cum. Vol: {vol}",
            high = format!("{:.2}", main_set.max_price).green().bold(),
            low = format!("{:.2}", main_set.min_price).red().bold(),
            var = format!("{} {:>+.2}%", variation_output.0, main_set.variation).color(variation_output.1).bold(),
            avg = avg_format,
            price = format!("{:.2}", main_set.last_price).green().bold(),
            vol = format!("{:.0}", main_set.cumulative_volume).green().bold(),
        );

        output_str
    }
}
