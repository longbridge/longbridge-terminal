use cli_candlestick_chart::{Candle, Chart};
use std::error::Error;
use std::io::{self, BufRead};

mod utils;
use utils::hexa_to_rgb;

mod get_args;
use get_args::{get_args, CandlesRetrievalMode};

fn parse_candles_from_stdin() -> Vec<Candle> {
    let stdin = io::stdin();

    let mut stdin_lines = String::new();
    for line in stdin.lock().lines() {
        let line = line.expect("Could not read line from standard in");
        stdin_lines.push_str(&line);
    }

    let candles: Vec<Candle> = serde_json::from_str(&stdin_lines).expect("Could not parse json");
    candles
}

fn parse_candles_from_csv(filepath: &str) -> Result<Vec<Candle>, Box<dyn Error>> {
    let mut rdr = csv::Reader::from_path(filepath)?;
    let mut candles: Vec<Candle> = Vec::new();

    for result in rdr.deserialize() {
        let candle: Candle = result?;
        candles.push(candle);
    }

    Ok(candles)
}

fn main() {
    let options = get_args();

    let candles = match options.mode {
        CandlesRetrievalMode::Stdin => parse_candles_from_stdin(),
        CandlesRetrievalMode::CsvFile => {
            let filepath = options.file_path.expect("No file path provided.");
            parse_candles_from_csv(&filepath).unwrap()
        }
    };

    let mut chart = Chart::new(&candles);

    if let Some(chart_name) = options.chart_name {
        chart.set_name(chart_name);
    }

    if let Some(bear_color) = options.bear_color {
        let (r, g, b) = hexa_to_rgb(bear_color);
        chart.set_bear_color(r, g, b);
    }

    if let Some(bull_color) = options.bull_color {
        let (r, g, b) = hexa_to_rgb(bull_color);
        chart.set_bull_color(r, g, b);
    }

    chart.draw();
}
