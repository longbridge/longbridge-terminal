use std::error::Error;

use cli_candlestick_chart::{Candle, Chart, Color};

fn main() -> Result<(), Box<dyn Error>> {
    // Your CSV data must have "open,high,low,close" header fields.
    let mut rdr = csv::Reader::from_path("./examples/BTC-USD.csv")?;

    let mut candles: Vec<Candle> = Vec::new();

    for result in rdr.deserialize() {
        let candle: Candle = result?;
        candles.push(candle);
    }

    let mut chart = Chart::new(&candles);

    // Set the chart title
    chart.set_name(String::from("BTC/USDT"));

    // Set customs colors
    chart.set_bear_color(Color::TrueColor {
        r: 1,
        g: 205,
        b: 254,
    });
    chart.set_bull_color(Color::TrueColor {
        r: 255,
        g: 107,
        b: 153,
    });

    chart.draw();

    Ok(())
}
