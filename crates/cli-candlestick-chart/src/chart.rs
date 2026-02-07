use crate::{
    chart_data::ChartData, chart_renderer::ChartRenderer, info_bar::InfoBar,
    volume_pane::VolumePane, y_axis::YAxis,
};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize))]
pub struct Candle {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: Option<f64>,
    pub timestamp: Option<i64>,
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum CandleType {
    Bearish,
    Bullish,
}

impl Candle {
    #[allow(dead_code)]
    pub fn new(
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        volume: Option<f64>,
        timestamp: Option<i64>,
    ) -> Candle {
        Candle {
            open,
            high,
            low,
            close,
            volume,
            timestamp,
        }
    }

    pub(crate) fn get_type(&self) -> CandleType {
        match self.open < self.close {
            true => CandleType::Bullish,
            false => CandleType::Bearish,
        }
    }
}

pub struct Chart {
    pub(crate) renderer: ChartRenderer,
    pub(crate) y_axis: YAxis,
    pub(crate) chart_data: Rc<RefCell<ChartData>>,
    pub(crate) info_bar: InfoBar,
    pub(crate) volume_pane: VolumePane,
}

impl Chart {
    #[cfg(feature = "terminal_size")]
    pub fn new(candles: &[Candle]) -> Self {
        let (terminal_size::Width(w), terminal_size::Height(h)) =
            terminal_size::terminal_size().unwrap();
        Self::new_with_size(candles.to_vec(), (w, h))
    }

    pub fn new_with_size(candles: Vec<Candle>, size: (u16, u16)) -> Self {
        let renderer = ChartRenderer::new();
        let chart_data = Rc::new(RefCell::new(ChartData::new(candles, size)));
        let y_axis = YAxis::new(chart_data.clone());
        let info_bar = InfoBar::new("APPLE".to_string(), chart_data.clone());

        let volume_pane = VolumePane::new(
            chart_data.clone(),
            (chart_data.borrow().terminal_size.1 / 6) as i64,
        );

        chart_data.borrow_mut().compute_height(&volume_pane);

        Chart {
            renderer,
            y_axis,
            chart_data,
            info_bar,
            volume_pane,
        }
    }

    /// Renders the chart to multiline string
    pub fn render(&self) -> String {
        self.renderer.render(self)
    }

    /// Draws the chart by outputting multiples strings in the terminal.
    pub fn draw(&self) {
        println!("{}", self.render());
    }

    /// Set the name of the chart in the info bar.
    pub fn set_name(&mut self, name: String) {
        self.info_bar.name = name;
    }

    /// Set the color of the bearish candle
    /// The default color is  (234, 74, 90).
    pub fn set_bear_color(&mut self, color: colored::Color) {
        self.renderer.bearish_color = color;
    }

    /// Set the color of the bullish candle
    /// The default color is  (52, 208, 88).
    pub fn set_bull_color(&mut self, color: colored::Color) {
        self.renderer.bullish_color = color;
    }

    /// Sets the color of the volume when the candle is bearish.
    /// The default color is  (234, 74, 90).
    pub fn set_vol_bear_color(&mut self, color: colored::Color) {
        self.volume_pane.bearish_color = color;
    }

    /// Sets the color of the volume when the candle is bullish.
    /// The default color is  (52, 208, 88).
    pub fn set_vol_bull_color(&mut self, color: colored::Color) {
        self.volume_pane.bullish_color = color;
    }

    /// Hide or show the volume pane.
    pub fn set_volume_pane_enabled(&mut self, enabled: bool) {
        self.volume_pane.enabled = enabled;
    }

    /// Set the character for drawing the volume bars.
    pub fn set_volume_pane_unicode_fill(&mut self, unicode_fill: char) {
        self.volume_pane.unicode_fill = unicode_fill;
    }

    /// Set the volume pane height.
    /// Default is 1/6 of the terminal height.
    pub fn set_volume_pane_height(&mut self, height: i64) {
        self.volume_pane.height = height;
    }
}
