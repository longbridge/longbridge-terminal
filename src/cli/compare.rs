use std::collections::{HashMap, HashSet};

use anyhow::{bail, Result};
use futures::future::try_join_all;
use longbridge::httpclient::Json;
use longbridge::quote::{CalcIndex, SecurityCalcIndex, SecurityQuote, SecurityStaticInfo};
use reqwest::Method;
use rust_decimal::Decimal;
use serde_json::Value;

use super::{
    output::{fmt_decimal, print_table},
    OutputFormat,
};
use crate::utils::{counter::symbol_to_counter_id, number::format_financial_value};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum CompareField {
    Price,
    Change,
    Pe,
    Pb,
    Eps,
    Revenue,
    Rating,
}

impl CompareField {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "price" | "last" | "last_done" => Some(Self::Price),
            "change" | "chg" | "change_rate" => Some(Self::Change),
            "pe" | "pe_ttm" => Some(Self::Pe),
            "pb" => Some(Self::Pb),
            "eps" => Some(Self::Eps),
            "revenue" | "rev" => Some(Self::Revenue),
            "rating" | "recommend" => Some(Self::Rating),
            _ => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Price => "price",
            Self::Change => "change",
            Self::Pe => "pe",
            Self::Pb => "pb",
            Self::Eps => "eps",
            Self::Revenue => "revenue",
            Self::Rating => "rating",
        }
    }

    fn header(self) -> String {
        match self {
            Self::Price => t!("compare.headers.price").to_string(),
            Self::Change => t!("compare.headers.change").to_string(),
            Self::Pe => t!("compare.headers.pe").to_string(),
            Self::Pb => t!("compare.headers.pb").to_string(),
            Self::Eps => t!("compare.headers.eps").to_string(),
            Self::Revenue => t!("compare.headers.revenue").to_string(),
            Self::Rating => t!("compare.headers.rating").to_string(),
        }
    }
}

fn parse_compare_fields(fields: &[String]) -> Result<Vec<CompareField>> {
    let mut parsed = Vec::new();
    let mut seen = HashSet::new();

    for field in fields {
        let Some(field_kind) = CompareField::parse(field) else {
            bail!(t!("compare.error.unknown_field", field = field));
        };
        if seen.insert(field_kind) {
            parsed.push(field_kind);
        }
    }

    if parsed.is_empty() {
        bail!(t!("compare.error.no_fields"));
    }

    Ok(parsed)
}

async fn http_get(path: &str, params: &[(&str, &str)], verbose: bool) -> Result<Value> {
    if verbose {
        let qs = params
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("&");
        eprintln!("* GET {path}?{qs}");
    }

    let client = crate::openapi::http_client();
    let resp = client
        .request(Method::GET, path)
        .query_params(params.to_vec())
        .response::<Json<Value>>()
        .send()
        .await
        .map_err(anyhow::Error::from)?;
    Ok(resp.0)
}

fn val_str(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "-".to_string(),
        other => other.to_string(),
    }
}

fn change_pct(last: Decimal, prev_close: Decimal) -> String {
    if prev_close.is_zero() {
        return "-".to_string();
    }
    let pct = (last - prev_close) / prev_close * Decimal::ONE_HUNDRED;
    format!("{pct:+.2}%")
}

fn extract_consensus_metric(data: &Value, key: &str) -> String {
    let Some(periods) = data["list"].as_array() else {
        return "-".to_string();
    };

    for period in periods {
        let Some(details) = period["details"].as_array() else {
            continue;
        };

        let Some(metric) = details
            .iter()
            .find(|item| item["key"].as_str() == Some(key))
        else {
            continue;
        };

        let raw = if metric["is_released"].as_bool().unwrap_or(false) {
            val_str(&metric["actual"])
        } else {
            val_str(&metric["estimate"])
        };

        if raw != "-" && !raw.is_empty() {
            return format_financial_value(&raw, false);
        }
    }

    "-".to_string()
}

async fn fetch_revenue(symbol: &str, verbose: bool) -> Result<(String, String)> {
    let counter_id = symbol_to_counter_id(symbol);
    let data = http_get(
        "/v1/quote/consensus",
        &[("counter_id", counter_id.as_str())],
        verbose,
    )
    .await?;
    Ok((
        symbol.to_string(),
        extract_consensus_metric(&data, "revenue"),
    ))
}

async fn fetch_rating(symbol: &str, verbose: bool) -> Result<(String, String)> {
    let counter_id = symbol_to_counter_id(symbol);
    let data = http_get(
        "/v1/quote/institution-ratings",
        &[("counter_id", counter_id.as_str())],
        verbose,
    )
    .await?;
    Ok((symbol.to_string(), val_str(&data["recommend"])))
}

fn format_field_value(
    symbol: &str,
    field: CompareField,
    quotes: &HashMap<String, SecurityQuote>,
    calc_indexes: &HashMap<String, SecurityCalcIndex>,
    static_infos: &HashMap<String, SecurityStaticInfo>,
    revenues: &HashMap<String, String>,
    ratings: &HashMap<String, String>,
) -> String {
    match field {
        CompareField::Price => quotes
            .get(symbol)
            .map_or_else(|| "-".to_string(), |quote| quote.last_done.to_string()),
        CompareField::Change => quotes.get(symbol).map_or_else(
            || "-".to_string(),
            |quote| change_pct(quote.last_done, quote.prev_close),
        ),
        CompareField::Pe => calc_indexes
            .get(symbol)
            .map_or_else(|| "-".to_string(), |calc| fmt_decimal(&calc.pe_ttm_ratio)),
        CompareField::Pb => calc_indexes
            .get(symbol)
            .map_or_else(|| "-".to_string(), |calc| fmt_decimal(&calc.pb_ratio)),
        CompareField::Eps => static_infos
            .get(symbol)
            .map_or_else(|| "-".to_string(), |info| info.eps.to_string()),
        CompareField::Revenue => revenues
            .get(symbol)
            .cloned()
            .unwrap_or_else(|| "-".to_string()),
        CompareField::Rating => ratings
            .get(symbol)
            .cloned()
            .unwrap_or_else(|| "-".to_string()),
    }
}

pub async fn cmd_compare(
    symbols: Vec<String>,
    fields: Vec<String>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    if !(2..=5).contains(&symbols.len()) {
        bail!(t!("compare.error.symbol_count"));
    }

    let fields = parse_compare_fields(&fields)?;
    let quote_ctx = crate::openapi::quote();

    let need_quotes = fields
        .iter()
        .any(|field| matches!(field, CompareField::Price | CompareField::Change));
    let need_calc_indexes = fields
        .iter()
        .any(|field| matches!(field, CompareField::Pe | CompareField::Pb));
    let need_static_info = fields.contains(&CompareField::Eps);
    let need_revenue = fields.contains(&CompareField::Revenue);
    let need_rating = fields.contains(&CompareField::Rating);

    let calc_index_fields: Vec<CalcIndex> = fields
        .iter()
        .filter_map(|field| match field {
            CompareField::Pe => Some(CalcIndex::PeTtmRatio),
            CompareField::Pb => Some(CalcIndex::PbRatio),
            _ => None,
        })
        .collect();

    let quote_symbols = symbols.clone();
    let calc_symbols = symbols.clone();
    let static_symbols = symbols.clone();
    let revenue_symbols = symbols.clone();
    let rating_symbols = symbols.clone();

    let quote_future = async {
        if need_quotes {
            quote_ctx
                .quote(quote_symbols)
                .await
                .map_err(anyhow::Error::from)
        } else {
            Ok(Vec::<SecurityQuote>::new())
        }
    };

    let calc_future = async {
        if need_calc_indexes {
            quote_ctx
                .calc_indexes(calc_symbols, calc_index_fields)
                .await
                .map_err(anyhow::Error::from)
        } else {
            Ok(Vec::<SecurityCalcIndex>::new())
        }
    };

    let static_future = async {
        if need_static_info {
            quote_ctx
                .static_info(static_symbols)
                .await
                .map_err(anyhow::Error::from)
        } else {
            Ok(Vec::<SecurityStaticInfo>::new())
        }
    };

    let revenue_future = async move {
        if need_revenue {
            try_join_all(
                revenue_symbols
                    .iter()
                    .map(|symbol| fetch_revenue(symbol, verbose)),
            )
            .await
        } else {
            Ok(Vec::<(String, String)>::new())
        }
    };

    let rating_future = async move {
        if need_rating {
            try_join_all(
                rating_symbols
                    .iter()
                    .map(|symbol| fetch_rating(symbol, verbose)),
            )
            .await
        } else {
            Ok(Vec::<(String, String)>::new())
        }
    };

    let (quotes, calc_indexes, static_infos, revenues, ratings) = tokio::try_join!(
        quote_future,
        calc_future,
        static_future,
        revenue_future,
        rating_future
    )?;

    let quote_map: HashMap<String, SecurityQuote> = quotes
        .into_iter()
        .map(|quote| (quote.symbol.clone(), quote))
        .collect();
    let calc_index_map: HashMap<String, SecurityCalcIndex> = calc_indexes
        .into_iter()
        .map(|calc| (calc.symbol.clone(), calc))
        .collect();
    let static_info_map: HashMap<String, SecurityStaticInfo> = static_infos
        .into_iter()
        .map(|info| (info.symbol.clone(), info))
        .collect();
    let revenue_map: HashMap<String, String> = revenues.into_iter().collect();
    let rating_map: HashMap<String, String> = ratings.into_iter().collect();

    match format {
        OutputFormat::Json => {
            let records: Vec<Value> = symbols
                .iter()
                .map(|symbol| {
                    let mut record = serde_json::Map::new();
                    record.insert("symbol".to_string(), Value::String(symbol.clone()));
                    for field in &fields {
                        record.insert(
                            field.key().to_string(),
                            Value::String(format_field_value(
                                symbol,
                                *field,
                                &quote_map,
                                &calc_index_map,
                                &static_info_map,
                                &revenue_map,
                                &rating_map,
                            )),
                        );
                    }
                    Value::Object(record)
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&records)?);
        }
        OutputFormat::Pretty => {
            let mut headers: Vec<String> = vec![t!("compare.headers.symbol").to_string()];
            headers.extend(fields.iter().map(|field| field.header()));
            let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();
            let rows: Vec<Vec<String>> = symbols
                .iter()
                .map(|symbol| {
                    let mut row = vec![symbol.clone()];
                    row.extend(fields.iter().map(|field| {
                        format_field_value(
                            symbol,
                            *field,
                            &quote_map,
                            &calc_index_map,
                            &static_info_map,
                            &revenue_map,
                            &rating_map,
                        )
                    }));
                    row
                })
                .collect();
            print_table(&header_refs, rows, format);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{extract_consensus_metric, parse_compare_fields, CompareField};
    use serde_json::json;

    #[test]
    fn compare_fields_are_deduplicated() {
        let fields = parse_compare_fields(&[
            "price".to_string(),
            "last_done".to_string(),
            "pe".to_string(),
        ])
        .unwrap();
        assert_eq!(fields, vec![CompareField::Price, CompareField::Pe]);
    }

    #[test]
    fn compare_fields_reject_unknown_values() {
        let err = parse_compare_fields(&["mystery".to_string()]).unwrap_err();
        assert!(format!("{err}").contains("mystery"));
    }

    #[test]
    fn consensus_metric_prefers_actual_when_released() {
        let data = json!({
            "list": [
                {
                    "details": [
                        {
                            "key": "revenue",
                            "is_released": true,
                            "actual": "2500000000",
                            "estimate": "2000000000"
                        }
                    ]
                }
            ]
        });
        assert_eq!(extract_consensus_metric(&data, "revenue"), "2.50B");
    }

    #[test]
    fn consensus_metric_uses_estimate_when_not_released() {
        let data = json!({
            "list": [
                {
                    "details": [
                        {
                            "key": "revenue",
                            "is_released": false,
                            "actual": "2500000000",
                            "estimate": "1600000000"
                        }
                    ]
                }
            ]
        });
        assert_eq!(extract_consensus_metric(&data, "revenue"), "1.60B");
    }
}
