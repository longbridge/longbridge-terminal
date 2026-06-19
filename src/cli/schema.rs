use anyhow::Result;
use clap::{Command, CommandFactory};
use serde_json::{json, Map, Value};
use std::ffi::OsString;

use super::{
    asset, atm, auth, check, completion, dca, fundamental, init, insider_trades, investors, ipo,
    news, quote, run_script, screener, sharelist, statement, topic, trade, watchlist, Cli,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RootKind {
    Object,
    Array,
    Json,
    Text,
}

#[derive(Clone, Debug)]
pub(crate) struct ResponseSchema {
    pub(crate) summary: String,
    pub(crate) root: RootKind,
    pub(crate) fields: Vec<Field>,
}

#[derive(Clone, Debug)]
pub(crate) struct Field {
    pub(crate) name: String,
    pub(crate) ty: String,
    pub(crate) description: String,
}

pub enum SchemaOutcome {
    NotRequested,
    Handled,
    Error,
}

pub fn handle_schema_args(args: impl IntoIterator<Item = OsString>) -> Result<SchemaOutcome> {
    let args = args.into_iter().collect::<Vec<_>>();
    if !args.iter().any(|arg| arg == "--schema") {
        return Ok(SchemaOutcome::NotRequested);
    }

    let mut root = Cli::command();
    root.build();
    let (selected, path) = selected_command_and_path_for_args(&root, args.iter().skip(1));

    if path.is_empty() {
        print_no_schema_error(&path);
        return Ok(SchemaOutcome::Error);
    }

    if let Some(schema) = schema_for_path(&path) {
        print_response_schema(&schema);
        Ok(SchemaOutcome::Handled)
    } else if selected.has_subcommands() {
        let mut help_cmd = selected.clone();
        help_cmd.print_help()?;
        println!();
        Ok(SchemaOutcome::Handled)
    } else {
        print_no_schema_error(&path);
        Ok(SchemaOutcome::Error)
    }
}

fn selected_command_and_path_for_args<'a, I>(
    root: &'a Command,
    args: I,
) -> (&'a Command, Vec<String>)
where
    I: IntoIterator<Item = &'a OsString>,
{
    let mut current = root;
    let mut path = Vec::new();
    let mut skip_next_value = false;

    for arg in args {
        if arg == "--schema" {
            break;
        }
        if skip_next_value {
            skip_next_value = false;
            continue;
        }

        if let Some(raw) = arg.to_str() {
            if raw == "--" {
                break;
            }
            if let Some(long) = raw.strip_prefix("--") {
                let (name, has_inline_value) = long
                    .split_once('=')
                    .map_or((long, false), |(name, _)| (name, true));
                skip_next_value = !has_inline_value && long_option_takes_value(current, name);
                continue;
            }
            if raw.starts_with('-') && raw != "-" {
                skip_next_value = short_option_takes_value(current, raw);
                continue;
            }
        }

        if let Some(subcommand) = current.find_subcommand(arg) {
            if subcommand.get_name() == "help" {
                break;
            }
            current = subcommand;
            path.push(current.get_name().to_string());
        }
    }

    (current, path)
}

fn long_option_takes_value(command: &Command, name: &str) -> bool {
    command.get_arguments().any(|arg| {
        !arg.is_positional()
            && arg.get_action().takes_values()
            && (arg.get_long() == Some(name)
                || arg
                    .get_all_aliases()
                    .unwrap_or_default()
                    .into_iter()
                    .any(|alias| alias == name))
    })
}

fn short_option_takes_value(command: &Command, raw: &str) -> bool {
    let mut chars = raw.trim_start_matches('-').chars().peekable();
    while let Some(short) = chars.next() {
        let Some(arg) = command.get_arguments().find(|arg| {
            !arg.is_positional()
                && (arg.get_short() == Some(short)
                    || arg
                        .get_all_short_aliases()
                        .unwrap_or_default()
                        .into_iter()
                        .any(|alias| alias == short))
        }) else {
            continue;
        };

        if arg.get_action().takes_values() {
            return chars.peek().is_none();
        }
    }
    false
}

fn print_response_schema(schema: &ResponseSchema) {
    println!(
        "{}",
        serde_json::to_string_pretty(&response_json_schema(schema)).expect("schema JSON")
    );
}

fn response_json_schema(schema: &ResponseSchema) -> Value {
    let root = match schema.root {
        RootKind::Object => object_json_schema(&schema.fields),
        RootKind::Array => json!({
            "type": "array",
            "items": object_json_schema(&schema.fields),
        }),
        RootKind::Json => json!({
            "anyOf": [
                object_json_schema(&schema.fields),
                {
                    "type": "array",
                    "items": object_json_schema(&schema.fields),
                },
            ],
        }),
        RootKind::Text => json!({
            "type": "string",
        }),
    };

    let mut root = root;
    let root_obj = root
        .as_object_mut()
        .expect("root schema constructors produce objects");
    root_obj.insert(
        "$schema".to_string(),
        Value::String("https://json-schema.org/draft/2020-12/schema".to_string()),
    );
    root_obj.insert("title".to_string(), Value::String(schema.summary.clone()));
    root_obj.insert(
        "description".to_string(),
        Value::String(schema.summary.clone()),
    );
    root
}

fn object_json_schema(fields: &[Field]) -> Value {
    let properties = fields
        .iter()
        .map(|field| (field.name.clone(), field_json_schema(field)))
        .collect::<Map<_, _>>();

    json!({
        "type": "object",
        "properties": properties,
        "additionalProperties": true,
    })
}

fn field_json_schema(field: &Field) -> Value {
    let mut schema = type_json_schema(&field.ty);
    schema
        .as_object_mut()
        .expect("field schema constructors produce objects")
        .insert(
            "description".to_string(),
            Value::String(field.description.clone()),
        );
    schema
}

fn type_json_schema(ty: &str) -> Value {
    if let Some(item_ty) = ty.strip_suffix("[]") {
        return json!({
            "type": "array",
            "items": type_json_schema(item_ty),
        });
    }

    let variants = ty
        .split('|')
        .map(str::trim)
        .filter(|variant| !variant.is_empty())
        .collect::<Vec<_>>();

    if variants.len() > 1 {
        let mut types = Vec::new();
        let mut has_object = false;
        for variant in variants {
            if variant == "object" {
                has_object = true;
            }
            types.push(Value::String(json_schema_type_name(variant).to_string()));
        }

        let mut schema = Map::new();
        schema.insert("type".to_string(), Value::Array(types));
        if has_object {
            schema.insert("additionalProperties".to_string(), Value::Bool(true));
        }
        return Value::Object(schema);
    }

    match ty {
        "array" => json!({
            "type": "array",
        }),
        "object" => json!({
            "type": "object",
            "additionalProperties": true,
        }),
        "string" | "number" | "boolean" | "null" => json!({
            "type": ty,
        }),
        _ => json!({}),
    }
}

fn json_schema_type_name(ty: &str) -> &'static str {
    match ty {
        "number" => "number",
        "boolean" => "boolean",
        "array" => "array",
        "object" => "object",
        "null" => "null",
        _ => "string",
    }
}

pub(crate) fn schema_for_path(path: &[String]) -> Option<ResponseSchema> {
    let first = path.first()?.as_str();
    match first {
        "auth" => auth::schema_for_path(path),
        "init" => init::schema_for_path(path),
        "check" => check::schema_for_path(path),
        "update" => crate::update::schema_for_path(path),
        "completion" => completion::schema_for_path(path),
        "quote" | "depth" | "brokers" | "trades" | "intraday" | "kline" | "static"
        | "calc-index" | "capital" | "market-temp" | "trading" | "security-list"
        | "participants" | "subscriptions" | "option" | "warrant" | "constituent"
        | "market-status" | "broker-holding" | "ah-premium" | "trade-stats" | "anomaly"
        | "top-movers" | "rank" | "short-positions" | "short-trades" => {
            quote::schema_for_path(path)
        }
        "financial-report"
        | "business-segments"
        | "industry-rank"
        | "industry-peers"
        | "institution-rating"
        | "dividend"
        | "forecast-eps"
        | "consensus"
        | "finance-calendar"
        | "valuation"
        | "shareholder"
        | "company"
        | "executive"
        | "industry-valuation"
        | "operating"
        | "corp-action"
        | "invest-relation"
        | "fund-holder"
        | "financial-statement"
        | "valuation-rank"
        | "compare" => fundamental::schema_for_path(path),
        "news" | "filing" => news::schema_for_path(path),
        "topic" => topic::schema_for_path(path),
        "watchlist" => watchlist::schema_for_path(path),
        "statement" => statement::schema_for_path(path),
        "portfolio" if path.get(1).is_some_and(|sub| sub == "short-margin") => {
            asset::schema_for_path(path)
        }
        "order" | "assets" | "cash-flow" | "portfolio" | "positions" | "fund-positions"
        | "margin-ratio" | "max-qty" | "alert" => trade::schema_for_path(path),
        "exchange-rate" | "profit-analysis" => asset::schema_for_path(path),
        "insider-trades" => insider_trades::schema_for_path(path),
        "investors" => investors::schema_for_path(path),
        "dca" => dca::schema_for_path(path),
        "sharelist" => sharelist::schema_for_path(path),
        "quant" => run_script::schema_for_path(path),
        "screener" => screener::schema_for_path(path),
        "bank-cards" | "withdrawals" | "deposits" => atm::schema_for_path(path),
        "ipo" => ipo::schema_for_path(path),
        _ => None,
    }
}

pub(crate) fn object(summary: &str, keys: &[&str]) -> ResponseSchema {
    schema(summary, RootKind::Object, fields(keys))
}

pub(crate) fn array(summary: &str, keys: &[&str]) -> ResponseSchema {
    schema(summary, RootKind::Array, fields(keys))
}

pub(crate) fn json_schema(summary: &str, keys: &[&str]) -> ResponseSchema {
    schema(summary, RootKind::Json, fields(keys))
}

pub(crate) fn text(summary: &str) -> ResponseSchema {
    schema(
        summary,
        RootKind::Text,
        vec![field("output", "string", "Text written to stdout")],
    )
}

pub(crate) fn schema(summary: &str, root: RootKind, fields: Vec<Field>) -> ResponseSchema {
    ResponseSchema {
        summary: summary.to_string(),
        root,
        fields,
    }
}

pub(crate) fn fields(keys: &[&str]) -> Vec<Field> {
    keys.iter()
        .map(|key| field(key, inferred_type(key), "Response field"))
        .collect()
}

pub(crate) fn field(name: &str, ty: &str, description: &str) -> Field {
    Field {
        name: name.to_string(),
        ty: ty.to_string(),
        description: description.to_string(),
    }
}

fn inferred_type(key: &str) -> &'static str {
    match key {
        "trading_days" | "half_trading_days" | "opt_reports" | "opt_periods" | "empty_fields"
        | "file_urls" | "tags" | "bus_ids" | "reg_ids" => "string[]",
        "list" | "items" | "history" | "historical" | "stocks" | "holdings" | "market_accounts"
        | "hk" | "us" => "array | object",
        "pe" | "pb" | "ps" | "dvd" => "array | number | string | object | null",
        "asks"
        | "bids"
        | "sessions"
        | "securities"
        | "business"
        | "regionals"
        | "events"
        | "changes"
        | "exchanges"
        | "tickers"
        | "hashtags"
        | "images"
        | "orders"
        | "cash_infos"
        | "cash_balances"
        | "lists"
        | "sharelists"
        | "subscribed_sharelists"
        | "professional_list"
        | "shareholder_list"
        | "invest_securities"
        | "nearest_plans"
        | "klines"
        | "trades"
        | "stats"
        | "short_list"
        | "scopes"
        | "stock_topics"
        | "stock_items"
        | "plots"
        | "series"
        | "bars"
        | "errors"
        | "data"
        | "infos"
        | "records"
        | "plans"
        | "buy"
        | "sell" => "object[]",
        "id" | "rank" | "volume" | "quantity" | "available" | "shares" | "shares_after"
        | "value" | "price" | "total" | "page" | "count" | "active_count" | "finished_count"
        | "suspended_count" | "rest_days" | "trade_stock_num" | "total_holdings" | "win_qty"
        | "amount" | "c" | "p" | "sources" => "number | string",
        "enabled" | "is_traded" | "support_regular_saving" | "subscribed" | "all_off" | "bmp" => {
            "boolean"
        }
        "overview" | "timeline" | "eligibility" | "summary" | "token" | "session"
        | "connectivity" | "metrics" | "range" | "flows" | "position" | "group" | "sharelist"
        | "statistics" | "next_params" => "object",
        _ => "string | number | boolean | object | null",
    }
}

fn print_no_schema_error(path: &[String]) {
    let name = if path.is_empty() {
        "longport".to_string()
    } else {
        format!("longport {}", path.join(" "))
    };
    eprintln!(
        "{}",
        json!({
            "code": 0,
            "error": format!("no response schema available for \"{name}\""),
            "hint": "",
            "status": 0
        })
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real_leaf_paths(command: &Command) -> Vec<Vec<String>> {
        fn walk(command: &Command, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
            let real_subcommands = command
                .get_subcommands()
                .filter(|subcommand| subcommand.get_name() != "help")
                .collect::<Vec<_>>();

            if !prefix.is_empty() && real_subcommands.is_empty() {
                out.push(prefix.clone());
                return;
            }

            for subcommand in real_subcommands {
                prefix.push(subcommand.get_name().to_string());
                walk(subcommand, prefix, out);
                prefix.pop();
            }
        }

        let mut out = Vec::new();
        walk(command, &mut Vec::new(), &mut out);
        out
    }

    #[test]
    fn schema_path_preparse_selects_nested_command_without_required_args() {
        let mut root = Cli::command();
        root.build();
        let args = [
            OsString::from("kline"),
            OsString::from("history"),
            OsString::from("--schema"),
        ];

        let (selected, path) = selected_command_and_path_for_args(&root, args.iter());

        assert_eq!(selected.get_name(), "history");
        assert_eq!(path, vec!["kline".to_string(), "history".to_string()]);
    }

    #[test]
    fn every_real_leaf_command_has_schema_provider() {
        let mut root = Cli::command();
        root.build();
        let paths = real_leaf_paths(&root);
        assert_eq!(
            paths.len(),
            137,
            "real command count changed; review schema coverage"
        );

        let missing = paths
            .iter()
            .filter(|path| schema_for_path(path).is_none())
            .map(|path| path.join(" "))
            .collect::<Vec<_>>();

        assert!(missing.is_empty(), "missing schema coverage: {missing:#?}");
    }
}
