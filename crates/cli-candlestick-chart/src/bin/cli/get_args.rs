use clap::{App, Arg};

#[derive(PartialEq)]
pub enum CandlesRetrievalMode {
    Stdin,
    CsvFile,
}

pub struct CliOptions {
    pub mode: CandlesRetrievalMode,
    pub file_path: Option<String>,
    pub chart_name: Option<String>,
    pub bear_color: Option<String>,
    pub bull_color: Option<String>,
}

pub fn get_args() -> CliOptions {
    let matches = App::new(env!("CARGO_PKG_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .author(env!("CARGO_PKG_AUTHORS"))
        .arg(
            Arg::with_name("MODE")
                .short("m")
                .long("mode")
                .help("Select the method for retrieving the candles.")
                .possible_values(&["stdin", "csv-file", "json-file"])
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::with_name("FILE")
                .short("f")
                .long("file")
                .help("[MODE:*-file] File to read candles from.`")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("CHART_NAME")
                .long("chart-name")
                .help("Sets the chart name.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("BEAR_COLOR")
                .long("bear-color")
                .help("Sets the descending candles color in hexadecimal.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("BULL_COLOR")
                .long("bull-color")
                .help("Sets the ascending candles color in hexadecimal.")
                .takes_value(true),
        )
        .get_matches();

    return CliOptions {
        mode: match matches.value_of("MODE").unwrap() {
            "stdin" => CandlesRetrievalMode::Stdin,
            "csv-file" => CandlesRetrievalMode::CsvFile,
            _ => panic!("Invalid reading mode."),
        },
        file_path: matches.value_of("FILE").map(|s| s.to_string()),
        chart_name: matches.value_of("CHART_NAME").map(|s| s.to_string()),
        bear_color: matches.value_of("BEAR_COLOR").map(|s| s.to_string()),
        bull_color: matches.value_of("BULL_COLOR").map(|s| s.to_string()),
    };
}
