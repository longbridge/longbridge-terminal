use anyhow::Result;
use longbridge::signal::{
    SecurityFact, SecurityFactsOptions, Signal, SignalsOptions, SignalsResponse,
};

use super::{
    output::{parse_datetime_end, parse_datetime_start, print_table},
    OutputFormat,
};
use crate::utils::{datetime::fmt_rfc3339, text::truncate_width};

/// Width the signal title is trimmed to in the table view, so a row stays on
/// one terminal line next to the other columns.
const TITLE_WIDTH: usize = 60;

fn signal_to_json(s: &Signal) -> serde_json::Value {
    serde_json::json!({
        "id": s.id,
        "symbol": s.symbol,
        "company_name": s.company_name,
        "market": s.market,
        "title": s.title,
        "summary": s.summary,
        "strategy_id": s.strategy_id,
        "strategy_name": s.strategy_name,
        "recommend_by": s.recommend_by,
        "expression": s.expression,
        "key_fact_id": s.key_fact_id,
        "key_catalyst": s.key_catalyst,
        "analysis_price": s.analysis_price,
        "conservative_price": s.conservative_price,
        "benchmark_price": s.benchmark_price,
        "optimistic_price": s.optimistic_price,
        "outlook": s.outlook.to_string(),
        "outlook_desc": s.outlook_desc,
        "status": format!("{:?}", s.status),
        "created_at": fmt_rfc3339(s.created_at),
        "updated_at": fmt_rfc3339(s.updated_at),
    })
}

fn signals_to_json(resp: &SignalsResponse) -> serde_json::Value {
    serde_json::json!({
        "signals": resp.signals.iter().map(signal_to_json).collect::<Vec<_>>(),
        "total": resp.total,
    })
}

/// List strategy signals.
pub async fn cmd_signals(
    symbol: Option<String>,
    strategy_id: Option<String>,
    strategy_name: Option<String>,
    catalyst: Option<String>,
    catalyst_type: Option<String>,
    start: Option<String>,
    end: Option<String>,
    limit: Option<i32>,
    offset: Option<i32>,
    format: &OutputFormat,
) -> Result<()> {
    let opts = SignalsOptions {
        symbol_name: symbol,
        strategy_id,
        strategy_name,
        catalyst_name: catalyst,
        catalyst_type,
        start_time: start.map(|s| parse_datetime_start(&s)).transpose()?,
        end_time: end.map(|s| parse_datetime_end(&s)).transpose()?,
        limit,
        offset,
    };
    let resp = crate::openapi::signal().signals(opts).await?;

    if matches!(format, OutputFormat::Json) {
        println!(
            "{}",
            serde_json::to_string_pretty(&signals_to_json(&resp)).unwrap_or_default()
        );
        return Ok(());
    }

    if resp.signals.is_empty() {
        println!("No signals found.");
        return Ok(());
    }

    let headers = &[
        "id",
        "symbol",
        "title",
        "strategy",
        "outlook",
        "catalyst",
        "created_at",
    ];
    let rows = resp
        .signals
        .iter()
        .map(|s| {
            vec![
                s.id.clone(),
                s.symbol.clone(),
                truncate_width(&s.title, TITLE_WIDTH),
                s.strategy_name.clone(),
                s.outlook_desc.clone(),
                s.key_catalyst.clone(),
                fmt_rfc3339(s.created_at),
            ]
        })
        .collect();

    print_table(headers, rows, format);
    println!(
        "\nShowing {} of {} signals.",
        resp.signals.len(),
        resp.total
    );
    Ok(())
}

/// Show one signal, including the strategy analysis behind it.
pub async fn cmd_signal_detail(signal_id: String, format: &OutputFormat) -> Result<()> {
    let s = crate::openapi::signal().signal(signal_id).await?;

    if matches!(format, OutputFormat::Json) {
        let mut json = signal_to_json(&s);
        // The analysis travels as a JSON document inside a string; hand it over
        // as real JSON so `jq` can reach into it. Anything that does not parse
        // is passed through as the original string rather than dropped.
        json["analysis"] = serde_json::from_str(&s.json_data)
            .unwrap_or_else(|_| serde_json::Value::String(s.json_data.clone()));
        println!(
            "{}",
            serde_json::to_string_pretty(&json).unwrap_or_default()
        );
        return Ok(());
    }

    println!("{}", s.title);
    println!();
    println!("{:<14}{} {}", "Symbol", s.symbol, s.company_name);
    println!("{:<14}{} ({})", "Strategy", s.strategy_name, s.strategy_id);
    println!("{:<14}{}", "Catalyst", s.key_catalyst);
    println!("{:<14}{}", "Outlook", s.outlook_desc);
    println!(
        "{:<14}{} (conservative {} / benchmark {} / optimistic {})",
        "Price", s.analysis_price, s.conservative_price, s.benchmark_price, s.optimistic_price
    );
    println!("{:<14}{}", "Created", fmt_rfc3339(s.created_at));
    if !s.summary.is_empty() {
        println!();
        println!("{}", s.summary);
    }
    Ok(())
}

fn fact_to_json(f: &SecurityFact) -> serde_json::Value {
    serde_json::json!({
        "fact_id": f.fact_id,
        "fact_type": f.fact_type.to_string(),
        "direction": f.direction.to_string(),
        "occur_time": fmt_rfc3339(f.occur_time),
        "symbols": f.symbols_info.iter().map(|s| serde_json::json!({
            "symbol": s.symbol,
            "security_name": s.security_name,
        })).collect::<Vec<_>>(),
        "factors": f.factors.iter().map(|factor| serde_json::json!({
            "name": factor.name,
            "factor_groups": factor.factor_groups,
            "long_short_direction": factor.long_short_direction.to_string(),
            "trigger_condition": factor.trigger_condition,
            "anomaly_detection": {
                "anomaly_result": factor.anomaly_detection.anomaly_result,
                "significance_level": factor.anomaly_detection.significance_level,
                "test_method": factor.anomaly_detection.test_method,
                "thresholds": {
                    "low": factor.anomaly_detection.thresholds.low,
                    "medium": factor.anomaly_detection.thresholds.medium,
                    "high": factor.anomaly_detection.thresholds.high,
                },
            },
        })).collect::<Vec<_>>(),
        "data_source": f.data_source.iter().map(|d| serde_json::json!({
            "source_name": d.source_name,
            "type": d.source_type.to_string(),
            "url": d.url,
        })).collect::<Vec<_>>(),
        "nl_info": {
            "title": f.nl_info.title,
            "sub_title": f.nl_info.sub_title,
            "summary": f.nl_info.summary_tags().iter().map(|t| serde_json::json!({
                "tag": t.tag,
                "value": t.value,
            })).collect::<Vec<_>>(),
            "invest_anal": f.nl_info.invest_anal_tags().iter().map(|t| serde_json::json!({
                "tag": t.tag,
                "value": t.value,
            })).collect::<Vec<_>>(),
            "eli_explain": f.nl_info.eli_explain_tags().iter().map(|t| serde_json::json!({
                "tag": t.tag,
                "value": t.value,
            })).collect::<Vec<_>>(),
        },
    })
}

fn facts_to_json(facts: &[SecurityFact]) -> serde_json::Value {
    serde_json::Value::Array(facts.iter().map(fact_to_json).collect())
}

/// List the fact (catalyst) events for one security.
pub async fn cmd_security_facts(
    symbol: String,
    begin: Option<String>,
    end: Option<String>,
    limit: Option<i32>,
    format: &OutputFormat,
) -> Result<()> {
    let opts = SecurityFactsOptions {
        symbol,
        begin_time: begin.map(|s| parse_datetime_start(&s)).transpose()?,
        end_time: end.map(|s| parse_datetime_end(&s)).transpose()?,
        limit,
    };
    let facts = crate::openapi::signal().security_facts(opts).await?;

    if matches!(format, OutputFormat::Json) {
        println!(
            "{}",
            serde_json::to_string_pretty(&facts_to_json(&facts)).unwrap_or_default()
        );
        return Ok(());
    }

    if facts.is_empty() {
        println!("No facts found.");
        return Ok(());
    }

    let headers = &["occur_time", "type", "dir", "factors", "title"];
    let rows = facts
        .iter()
        .map(|f| {
            let factors = f
                .factors
                .iter()
                .map(|factor| factor.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            vec![
                fmt_rfc3339(f.occur_time),
                f.fact_type.to_string(),
                f.direction.to_string(),
                factors,
                truncate_width(&f.nl_info.title, TITLE_WIDTH),
            ]
        })
        .collect();

    print_table(headers, rows, format);
    Ok(())
}

pub(crate) fn schema_for_path(path: &[String]) -> Option<super::schema::ResponseSchema> {
    use super::schema::{field, schema, RootKind};

    match path.join(" ").as_str() {
        "signals" => Some(schema(
            "Strategy signal list",
            RootKind::Object,
            vec![
                field("signals", "object[]", "Signals on this page"),
                field("total", "number", "Total signals matching the filters"),
            ],
        )),
        "signal" => Some(schema(
            "Strategy signal detail",
            RootKind::Object,
            vec![
                field("id", "string", "Signal ID"),
                field("symbol", "string", "Security symbol"),
                field("company_name", "string", "Company name"),
                field("market", "string", "Trading market"),
                field("title", "string", "Signal headline"),
                field("summary", "string", "Signal summary in Markdown"),
                field("strategy_id", "string", "Strategy ID"),
                field("strategy_name", "string", "Strategy name"),
                field("recommend_by", "string", "Signal recommender"),
                field("expression", "string", "Strategy expression"),
                field("key_fact_id", "string", "Triggering fact ID"),
                field("key_catalyst", "string", "Triggering catalyst label"),
                field("analysis_price", "number", "Analysis price"),
                field("conservative_price", "number", "Conservative target price"),
                field("benchmark_price", "number", "Benchmark target price"),
                field("optimistic_price", "number", "Optimistic target price"),
                field("outlook", "string", "Strategy outlook"),
                field("outlook_desc", "string", "Localized outlook description"),
                field("status", "string", "Signal lifecycle status"),
                field("created_at", "string", "Creation time (RFC3339)"),
                field("updated_at", "string", "Last update time (RFC3339)"),
                field(
                    "analysis",
                    "object | string",
                    "Full strategy-specific analysis",
                ),
            ],
        )),
        "facts" => Some(schema(
            "Security catalyst facts",
            RootKind::Array,
            vec![
                field("fact_id", "string", "Fact ID"),
                field("fact_type", "string", "Fact type"),
                field("direction", "string", "Fact direction"),
                field("occur_time", "string", "Occurrence time (RFC3339)"),
                field("symbols", "object[]", "Related securities"),
                field("factors", "object[]", "Contributing factors"),
                field("data_source", "object[]", "Fact data sources"),
                field("nl_info", "object", "Natural-language fact details"),
            ],
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_signals_remain_valid_json_with_total() {
        let response = SignalsResponse {
            signals: Vec::new(),
            total: 0,
        };

        assert_eq!(
            signals_to_json(&response),
            json!({ "signals": [], "total": 0 })
        );
    }

    #[test]
    fn empty_facts_remain_a_valid_json_array() {
        assert_eq!(facts_to_json(&[]), json!([]));
    }
}
