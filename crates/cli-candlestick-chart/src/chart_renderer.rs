use crate::{chart::CandleType, y_axis::YAxis, Candle, Chart};
use colored::*;

pub struct ChartRenderer {
    pub bearish_color: Color,
    pub bullish_color: Color,
}

impl ChartRenderer {
    const UNICODE_VOID: char = ' ';
    const UNICODE_BODY: char = '┃';
    const UNICODE_HALF_BODY_BOTTOM: char = '╻';
    const UNICODE_HALF_BODY_TOP: char = '╹';
    const UNICODE_WICK: char = '│';
    const UNICODE_TOP: char = '╽';
    const UNICODE_BOTTOM: char = '╿';
    const UNICODE_UPPER_WICK: char = '╷';
    const UNICODE_LOWER_WICK: char = '╵';

    pub fn new() -> ChartRenderer {
        #[cfg(target_os = "windows")]
        control::set_virtual_terminal(true).unwrap();

        ChartRenderer {
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
        }
    }

    fn colorize(&self, candle_type: CandleType, string: &str) -> String {
        let color = match candle_type {
            CandleType::Bearish => self.bearish_color,
            CandleType::Bullish => self.bullish_color,
        };
        string.color(color).to_string()
    }

    fn render_candle(&self, candle: &Candle, y: i32, y_axis: &YAxis) -> String {
        let height_unit = y as f64;
        let high_y = y_axis.price_to_height(candle.high);
        let low_y = y_axis.price_to_height(candle.low);
        let max_y = y_axis.price_to_height(candle.open.max(candle.close));
        let min_y = y_axis.price_to_height(candle.close.min(candle.open));

        let mut output = ChartRenderer::UNICODE_VOID;

        if high_y.ceil() >= height_unit && height_unit >= max_y.floor() {
            if max_y - height_unit > 0.75 {
                output = ChartRenderer::UNICODE_BODY;
            } else if (max_y - height_unit) > 0.25 {
                if (high_y - height_unit) > 0.75 {
                    output = ChartRenderer::UNICODE_TOP;
                } else {
                    output = ChartRenderer::UNICODE_HALF_BODY_BOTTOM;
                }
            } else if (high_y - height_unit) > 0.75 {
                output = ChartRenderer::UNICODE_WICK;
            } else if (high_y - height_unit) > 0.25 {
                output = ChartRenderer::UNICODE_UPPER_WICK;
            }
        } else if max_y.floor() >= height_unit && height_unit >= min_y.ceil() {
            output = ChartRenderer::UNICODE_BODY;
        } else if min_y.ceil() >= height_unit && height_unit >= low_y.floor() {
            if (min_y - height_unit) < 0.25 {
                output = ChartRenderer::UNICODE_BODY;
            } else if (min_y - height_unit) < 0.75 {
                if (low_y - height_unit) < 0.25 {
                    output = ChartRenderer::UNICODE_BOTTOM;
                } else {
                    output = ChartRenderer::UNICODE_HALF_BODY_TOP;
                }
            } else if low_y - height_unit < 0.25 {
                output = ChartRenderer::UNICODE_WICK;
            } else if low_y - height_unit < 0.75 {
                output = ChartRenderer::UNICODE_LOWER_WICK;
            }
        }

        if output == ChartRenderer::UNICODE_VOID {
            output.to_string()
        } else {
            let mut tmp = [0; 4];
            self.colorize(candle.get_type(), output.encode_utf8(&mut tmp))
        }
    }

    pub fn render(&self, chart: &Chart) -> String {
        let mut output_str = "".to_owned();

        let mut chart_data = chart.chart_data.borrow_mut();
        chart_data.compute_height(&chart.volume_pane);
        drop(chart_data);

        let chart_data = chart.chart_data.borrow();

        for y in (1..chart_data.height as u16).rev() {
            output_str += "\n";

            output_str += &chart.y_axis.render_line(y);

            for candle in chart_data.visible_candle_set.candles.iter() {
                output_str += &self.render_candle(candle, y.into(), &chart.y_axis);
            }
        }

        if chart.volume_pane.enabled {
            for y in (1..chart.volume_pane.height + 1).rev() {
                output_str += "\n";

                output_str += &chart.y_axis.render_empty();

                for candle in chart_data.visible_candle_set.candles.iter() {
                    output_str += &chart.volume_pane.render(candle, y);
                }
            }
        }

        output_str += &chart.info_bar.render();

        output_str
    }
}

impl Default for ChartRenderer {
    fn default() -> Self {
        Self::new()
    }
}
