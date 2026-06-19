use serde_json::Value;
use std::collections::BTreeSet;
use std::process::Command;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Root {
    Object,
    Array,
    Json,
}

struct Probe {
    path: &'static [&'static str],
    args: &'static [&'static str],
}

#[test]
#[ignore = "requires authenticated Longbridge account and live read-only API access"]
fn read_only_live_json_shapes_match_schema() {
    for probe in read_only_probes() {
        let schema = run_schema(probe.path);
        let schema_root = parse_schema_root(&schema);
        let schema_keys = parse_schema_keys(&schema);

        let json = run_json(probe.args);
        assert_root_matches(schema_root, &json, probe.path);

        for key in top_level_keys(&json) {
            assert!(
                schema_keys.contains(key),
                "schema for `{}` missing live JSON key `{key}`; stdout schema:\n{schema}",
                probe.path.join(" "),
                schema = serde_json::to_string_pretty(&schema).expect("format schema"),
            );
        }

        for (key, value) in top_level_entries(&json) {
            let field_schema = schema_for_key(&schema, key).unwrap_or_else(|| {
                panic!(
                    "schema for `{}` missing live JSON key `{key}`",
                    probe.path.join(" ")
                )
            });
            assert!(
                schema_allows_value(field_schema, value),
                "schema for `{}` has wrong type for live JSON key `{key}`: schema {}\nvalue type: {}\nvalue: {}",
                probe.path.join(" "),
                serde_json::to_string(field_schema).expect("format field schema"),
                value_type(value),
                serde_json::to_string(value).expect("format value"),
            );
        }
    }
}

fn read_only_probes() -> Vec<Probe> {
    vec![
        p(&["auth", "status"], &["auth", "status", "--format", "json"]),
        p(&["check"], &["check", "--format", "json"]),
        p(&["quote"], &["quote", "TSLA.US", "--format", "json"]),
        p(&["depth"], &["depth", "TSLA.US", "--format", "json"]),
        p(&["brokers"], &["brokers", "700.HK", "--format", "json"]),
        p(
            &["trades"],
            &["trades", "TSLA.US", "--count", "3", "--format", "json"],
        ),
        p(&["intraday"], &["intraday", "TSLA.US", "--format", "json"]),
        p(
            &["kline"],
            &["kline", "TSLA.US", "--count", "3", "--format", "json"],
        ),
        p(
            &["kline", "history"],
            &[
                "kline", "history", "TSLA.US", "--period", "day", "--format", "json",
            ],
        ),
        p(&["static"], &["static", "TSLA.US", "--format", "json"]),
        p(
            &["calc-index"],
            &[
                "calc-index",
                "TSLA.US",
                "--fields",
                "pe,pb,dps_rate,turnover_rate,mktcap",
                "--format",
                "json",
            ],
        ),
        p(&["capital"], &["capital", "TSLA.US", "--format", "json"]),
        p(
            &["capital"],
            &["capital", "TSLA.US", "--flow", "--format", "json"],
        ),
        p(&["market-temp"], &["market-temp", "HK", "--format", "json"]),
        p(
            &["trading", "session"],
            &["trading", "session", "--format", "json"],
        ),
        p(
            &["trading", "days"],
            &[
                "trading",
                "days",
                "HK",
                "--start",
                "2026-06-19",
                "--end",
                "2026-06-30",
                "--format",
                "json",
            ],
        ),
        p(
            &["security-list"],
            &["security-list", "US", "--count", "3", "--format", "json"],
        ),
        p(&["participants"], &["participants", "--format", "json"]),
        p(&["subscriptions"], &["subscriptions", "--format", "json"]),
        p(
            &["option", "chain"],
            &["option", "chain", "AAPL.US", "--format", "json"],
        ),
        p(
            &["option", "quote"],
            &[
                "option",
                "quote",
                "AAPL260619C00200000.US",
                "--format",
                "json",
            ],
        ),
        p(
            &["option", "volume"],
            &["option", "volume", "AAPL.US", "--format", "json"],
        ),
        p(
            &["option", "volume", "daily"],
            &[
                "option", "volume", "daily", "AAPL.US", "--count", "3", "--format", "json",
            ],
        ),
        p(&["warrant"], &["warrant", "700.HK", "--format", "json"]),
        p(
            &["warrant", "issuers"],
            &["warrant", "issuers", "--format", "json"],
        ),
        p(
            &["financial-report"],
            &[
                "financial-report",
                "AAPL.US",
                "--kind",
                "IS",
                "--report",
                "af",
                "--format",
                "json",
            ],
        ),
        p(
            &["financial-report", "snapshot"],
            &[
                "financial-report",
                "snapshot",
                "AAPL.US",
                "--format",
                "json",
            ],
        ),
        p(
            &["business-segments"],
            &["business-segments", "AAPL.US", "--format", "json"],
        ),
        p(
            &["industry-rank"],
            &[
                "industry-rank",
                "--market",
                "US",
                "--indicator",
                "market-cap",
                "--limit",
                "5",
                "--format",
                "json",
            ],
        ),
        p(
            &["industry-peers"],
            &["industry-peers", "BK/US/IN00258", "--format", "json"],
        ),
        p(
            &["institution-rating"],
            &["institution-rating", "AAPL.US", "--format", "json"],
        ),
        p(
            &["institution-rating", "detail"],
            &[
                "institution-rating",
                "detail",
                "AAPL.US",
                "--format",
                "json",
            ],
        ),
        p(&["dividend"], &["dividend", "AAPL.US", "--format", "json"]),
        p(
            &["dividend", "detail"],
            &["dividend", "detail", "AAPL.US", "--format", "json"],
        ),
        p(
            &["forecast-eps"],
            &["forecast-eps", "AAPL.US", "--format", "json"],
        ),
        p(
            &["consensus"],
            &["consensus", "AAPL.US", "--format", "json"],
        ),
        p(
            &["finance-calendar", "report"],
            &[
                "finance-calendar",
                "report",
                "--limit",
                "3",
                "--format",
                "json",
            ],
        ),
        p(
            &["valuation"],
            &["valuation", "AAPL.US", "--format", "json"],
        ),
        p(
            &["shareholder"],
            &["shareholder", "AAPL.US", "--count", "5", "--format", "json"],
        ),
        p(&["company"], &["company", "AAPL.US", "--format", "json"]),
        p(
            &["executive"],
            &["executive", "AAPL.US", "--format", "json"],
        ),
        p(
            &["industry-valuation"],
            &["industry-valuation", "AAPL.US", "--format", "json"],
        ),
        p(
            &["industry-valuation", "dist"],
            &["industry-valuation", "dist", "AAPL.US", "--format", "json"],
        ),
        p(&["operating"], &["operating", "700.HK", "--format", "json"]),
        p(
            &["corp-action"],
            &["corp-action", "700.HK", "--format", "json"],
        ),
        p(
            &["invest-relation"],
            &["invest-relation", "700.HK", "--format", "json"],
        ),
        p(
            &["financial-statement"],
            &[
                "financial-statement",
                "AAPL.US",
                "--kind",
                "IS",
                "--report",
                "af",
                "--format",
                "json",
            ],
        ),
        p(
            &["valuation-rank"],
            &["valuation-rank", "AAPL.US", "--format", "json"],
        ),
        p(
            &["compare"],
            &["compare", "AAPL.US", "MSFT.US", "--format", "json"],
        ),
        p(
            &["fund-holder"],
            &["fund-holder", "AAPL.US", "--count", "5", "--format", "json"],
        ),
        p(
            &["news"],
            &["news", "AAPL.US", "--limit", "3", "--format", "json"],
        ),
        p(
            &["news", "search"],
            &["news", "search", "AAPL", "--limit", "3", "--format", "json"],
        ),
        p(
            &["filing"],
            &["filing", "AAPL.US", "--limit", "3", "--format", "json"],
        ),
        p(
            &["topic"],
            &["topic", "AAPL.US", "--limit", "3", "--format", "json"],
        ),
        p(
            &["topic", "mine"],
            &["topic", "mine", "--size", "3", "--format", "json"],
        ),
        p(
            &["topic", "search"],
            &[
                "topic", "search", "AAPL", "--limit", "3", "--format", "json",
            ],
        ),
        p(&["watchlist"], &["watchlist", "--format", "json"]),
        p(
            &["statement"],
            &["statement", "--limit", "3", "--format", "json"],
        ),
        p(
            &["statement", "list"],
            &["statement", "list", "--limit", "3", "--format", "json"],
        ),
        p(&["order"], &["order", "--format", "json"]),
        p(
            &["order", "executions"],
            &["order", "executions", "--format", "json"],
        ),
        p(&["assets"], &["assets", "--format", "json"]),
        p(&["cash-flow"], &["cash-flow", "--format", "json"]),
        p(&["portfolio"], &["portfolio", "--format", "json"]),
        p(
            &["portfolio", "short-margin"],
            &["portfolio", "short-margin", "--format", "json"],
        ),
        p(&["positions"], &["positions", "--format", "json"]),
        p(&["fund-positions"], &["fund-positions", "--format", "json"]),
        p(
            &["margin-ratio"],
            &["margin-ratio", "TSLA.US", "--format", "json"],
        ),
        p(
            &["max-qty"],
            &[
                "max-qty", "TSLA.US", "--side", "buy", "--price", "250", "--format", "json",
            ],
        ),
        p(&["exchange-rate"], &["exchange-rate", "--format", "json"]),
        p(&["alert"], &["alert", "--format", "json"]),
        p(
            &["profit-analysis"],
            &["profit-analysis", "--format", "json"],
        ),
        p(
            &["profit-analysis", "by-market"],
            &["profit-analysis", "by-market", "US", "--format", "json"],
        ),
        p(
            &["constituent"],
            &["constituent", "HSI.HK", "--limit", "5", "--format", "json"],
        ),
        p(&["market-status"], &["market-status", "--format", "json"]),
        p(
            &["broker-holding"],
            &["broker-holding", "700.HK", "--format", "json"],
        ),
        p(
            &["broker-holding", "detail"],
            &["broker-holding", "detail", "700.HK", "--format", "json"],
        ),
        p(
            &["broker-holding", "daily"],
            &[
                "broker-holding",
                "daily",
                "700.HK",
                "--broker",
                "B01224",
                "--format",
                "json",
            ],
        ),
        p(
            &["ah-premium"],
            &["ah-premium", "939.HK", "--limit", "5", "--format", "json"],
        ),
        p(
            &["ah-premium", "intraday"],
            &["ah-premium", "intraday", "939.HK", "--format", "json"],
        ),
        p(
            &["trade-stats"],
            &["trade-stats", "700.HK", "--format", "json"],
        ),
        p(
            &["anomaly"],
            &[
                "anomaly", "--market", "HK", "--limit", "5", "--format", "json",
            ],
        ),
        p(
            &["top-movers"],
            &[
                "top-movers",
                "--market",
                "HK",
                "--limit",
                "5",
                "--format",
                "json",
            ],
        ),
        p(&["rank"], &["rank", "--market", "US", "--format", "json"]),
        p(
            &["rank"],
            &[
                "rank",
                "--key",
                "ib_hot_all-us",
                "--limit",
                "5",
                "--format",
                "json",
            ],
        ),
        p(
            &["screener", "strategies"],
            &[
                "screener",
                "strategies",
                "--market",
                "US",
                "--format",
                "json",
            ],
        ),
        p(
            &["screener", "run"],
            &["screener", "run", "42", "--limit", "5", "--format", "json"],
        ),
        p(
            &["screener", "filter"],
            &[
                "screener",
                "filter",
                "pettm:10:50",
                "--market",
                "US",
                "--limit",
                "5",
                "--format",
                "json",
            ],
        ),
        p(
            &["screener", "indicators"],
            &["screener", "indicators", "--format", "json"],
        ),
        p(
            &["insider-trades"],
            &[
                "insider-trades",
                "TSLA.US",
                "--limit",
                "5",
                "--format",
                "json",
            ],
        ),
        p(
            &["investors"],
            &["investors", "--top", "5", "--format", "json"],
        ),
        p(
            &["investors"],
            &["investors", "0001067983", "--top", "5", "--format", "json"],
        ),
        p(
            &["investors", "changes"],
            &[
                "investors",
                "changes",
                "0001067983",
                "--top",
                "5",
                "--format",
                "json",
            ],
        ),
        p(&["dca"], &["dca", "--limit", "5", "--format", "json"]),
        p(&["dca", "stats"], &["dca", "stats", "--format", "json"]),
        p(
            &["dca", "calc-date"],
            &[
                "dca",
                "calc-date",
                "AAPL.US",
                "--frequency",
                "monthly",
                "--day-of-month",
                "15",
                "--format",
                "json",
            ],
        ),
        p(
            &["dca", "check"],
            &["dca", "check", "AAPL.US", "TSLA.US", "--format", "json"],
        ),
        p(
            &["short-positions"],
            &[
                "short-positions",
                "AAPL.US",
                "--limit",
                "5",
                "--format",
                "json",
            ],
        ),
        p(
            &["short-trades"],
            &[
                "short-trades",
                "AAPL.US",
                "--limit",
                "5",
                "--format",
                "json",
            ],
        ),
        p(
            &["sharelist"],
            &["sharelist", "--limit", "3", "--format", "json"],
        ),
        p(
            &["sharelist", "popular"],
            &["sharelist", "popular", "--limit", "3", "--format", "json"],
        ),
        p(&["bank-cards"], &["bank-cards", "--format", "json"]),
        p(
            &["withdrawals"],
            &["withdrawals", "--limit", "3", "--format", "json"],
        ),
        p(
            &["deposits"],
            &["deposits", "--limit", "3", "--format", "json"],
        ),
        p(
            &["ipo", "subscriptions"],
            &["ipo", "subscriptions", "--format", "json"],
        ),
        p(
            &["ipo", "wait-listing"],
            &["ipo", "wait-listing", "--format", "json"],
        ),
        p(
            &["ipo", "listed"],
            &["ipo", "listed", "--limit", "5", "--format", "json"],
        ),
        p(
            &["ipo", "calendar"],
            &["ipo", "calendar", "--format", "json"],
        ),
        p(
            &["ipo", "detail"],
            &[
                "ipo", "detail", "6810.HK", "--market", "HK", "--format", "json",
            ],
        ),
        p(
            &["ipo", "orders"],
            &["ipo", "orders", "--limit", "5", "--format", "json"],
        ),
        p(
            &["ipo", "profit-loss"],
            &["ipo", "profit-loss", "--limit", "5", "--format", "json"],
        ),
        p(
            &["ipo", "us-subscriptions"],
            &["ipo", "us-subscriptions", "--format", "json"],
        ),
        p(
            &["ipo", "us-wait-listing"],
            &["ipo", "us-wait-listing", "--format", "json"],
        ),
        p(
            &["ipo", "us-listed"],
            &["ipo", "us-listed", "--limit", "5", "--format", "json"],
        ),
    ]
}

fn p(path: &'static [&'static str], args: &'static [&'static str]) -> Probe {
    Probe { path, args }
}

fn run_schema(path: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_longbridge"))
        .args(path)
        .arg("--schema")
        .output()
        .unwrap_or_else(|e| panic!("run schema for {path:?}: {e}"));

    assert!(
        output.status.success(),
        "schema command failed for `{}`: {}",
        path.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "schema command did not return JSON for `{}`: {e}\nstdout:\n{}",
            path.join(" "),
            String::from_utf8_lossy(&output.stdout),
        )
    })
}

fn run_json(args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_longbridge"))
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run longbridge {args:?}: {e}"));

    assert!(
        output.status.success(),
        "live command failed `longbridge {}`:\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "live command did not return JSON `longbridge {}`: {e}\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn parse_schema_root(schema: &Value) -> Root {
    if schema.get("anyOf").is_some() {
        Root::Json
    } else {
        match schema.get("type").and_then(Value::as_str) {
            Some("array") => Root::Array,
            Some("object") => Root::Object,
            _ => Root::Json,
        }
    }
}

fn parse_schema_keys(schema: &Value) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    collect_schema_keys(schema, &mut keys);
    keys
}

fn schema_for_key<'a>(schema: &'a Value, key: &str) -> Option<&'a Value> {
    if let Some(field) = schema
        .get("properties")
        .and_then(Value::as_object)
        .and_then(|properties| properties.get(key))
    {
        return Some(field);
    }
    if let Some(field) = schema
        .get("items")
        .and_then(|items| schema_for_key(items, key))
    {
        return Some(field);
    }
    schema
        .get("anyOf")
        .and_then(Value::as_array)
        .and_then(|schemas| {
            schemas
                .iter()
                .find_map(|schema| schema_for_key(schema, key))
        })
}

fn schema_allows_value(schema: &Value, value: &Value) -> bool {
    if let Some(any_of) = schema.get("anyOf").and_then(Value::as_array) {
        return any_of
            .iter()
            .any(|schema| schema_allows_value(schema, value));
    }

    match schema.get("type") {
        Some(Value::String(ty)) => json_type_matches(ty, value),
        Some(Value::Array(types)) => types
            .iter()
            .filter_map(Value::as_str)
            .any(|ty| json_type_matches(ty, value)),
        Some(_) => false,
        None => true,
    }
}

fn json_type_matches(ty: &str, value: &Value) -> bool {
    match ty {
        "array" => value.is_array(),
        "boolean" => value.is_boolean(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "object" => value.is_object(),
        "string" => value.is_string(),
        _ => false,
    }
}

fn collect_schema_keys(schema: &Value, keys: &mut BTreeSet<String>) {
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        keys.extend(properties.keys().cloned());
    }
    if let Some(items) = schema.get("items") {
        collect_schema_keys(items, keys);
    }
    if let Some(any_of) = schema.get("anyOf").and_then(Value::as_array) {
        for schema in any_of {
            collect_schema_keys(schema, keys);
        }
    }
}

fn assert_root_matches(root: Root, value: &Value, path: &[&str]) {
    let ok = match root {
        Root::Object => value.is_object(),
        Root::Array => value.is_array(),
        Root::Json => value.is_object() || value.is_array(),
    };
    assert!(
        ok,
        "schema root mismatch for `{}`: expected {root:?}, got {}",
        path.join(" "),
        value_type(value)
    );
}

fn top_level_keys(value: &Value) -> BTreeSet<&str> {
    match value {
        Value::Object(map) => map.keys().map(String::as_str).collect(),
        Value::Array(items) => items
            .first()
            .and_then(Value::as_object)
            .map_or_else(BTreeSet::new, |map| {
                map.keys().map(String::as_str).collect()
            }),
        _ => BTreeSet::new(),
    }
}

fn top_level_entries(value: &Value) -> Vec<(&str, &Value)> {
    match value {
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| (key.as_str(), value))
            .collect(),
        Value::Array(items) => {
            items
                .first()
                .and_then(Value::as_object)
                .map_or_else(Vec::new, |map| {
                    map.iter()
                        .map(|(key, value)| (key.as_str(), value))
                        .collect()
                })
        }
        _ => Vec::new(),
    }
}

fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
