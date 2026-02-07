use crate::Candle;

#[derive(Debug, Clone)]
pub struct CandleSet {
    pub candles: Vec<Candle>,

    pub min_price: f64,
    pub max_price: f64,

    pub min_volume: f64,
    pub max_volume: f64,

    pub variation: f64,
    pub average: f64,
    pub last_price: f64,
    pub cumulative_volume: f64,
}

impl CandleSet {
    pub fn new(candles: Vec<Candle>) -> CandleSet {
        let mut cs = CandleSet {
            candles: Vec::new(),
            min_price: 0.0,
            max_price: 0.0,
            min_volume: 0.0,
            max_volume: 0.0,
            variation: 0.0,
            average: 0.0,
            last_price: 0.0,
            cumulative_volume: 0.0,
        };

        cs.set_candles(candles);

        cs
    }

    pub fn set_candles(&mut self, candles: Vec<Candle>) {
        self.candles = candles;

        if self.candles.is_empty() {
            return;
        }

        self.compute_min_and_max_values();
        self.compute_variation();
        self.compute_average();
        self.compute_last_price();
        self.compute_cumulative_volume();
    }

    fn compute_cumulative_volume(&mut self) {
        self.cumulative_volume = self
            .candles
            .iter()
            .fold(0.0, |acc, candle| acc + candle.volume.unwrap_or_default());
    }

    fn compute_last_price(&mut self) {
        let last_candle = self.candles.last().unwrap();
        self.last_price = last_candle.close;
    }

    fn compute_variation(&mut self) {
        let open = self.candles.first().unwrap().open;
        let close = self.candles.last().unwrap().close;

        self.variation = ((close - open) / open) * 100.0;
    }

    fn compute_average(&mut self) {
        let sum = self.candles.iter().fold(0.0, |acc, c| acc + c.close);
        self.average = sum / self.candles.len() as f64;
    }

    fn compute_min_and_max_values(&mut self) {
        self.max_price = self
            .candles
            .iter()
            .fold(f64::NEG_INFINITY, |a, b| a.max(b.high));

        self.min_price = self.candles.iter().fold(f64::INFINITY, |a, b| a.min(b.low));

        self.max_volume = self.candles.iter().fold(f64::NEG_INFINITY, |a, b| {
            a.max(b.volume.unwrap_or_default())
        });

        self.min_volume = self
            .candles
            .iter()
            .fold(f64::INFINITY, |a, b| a.min(b.volume.unwrap_or_default()));
    }
}
