use anyhow::{Context, Result};
use rust_decimal::Decimal;

use super::{
    output::{print_json_value, print_table},
    GridCmd, GridRuleArgs, OutputFormat,
};
use crate::openapi;

fn parse_dec(s: &str, field: &str) -> Result<Decimal> {
    s.parse::<Decimal>()
        .with_context(|| format!("invalid decimal for {field}: {s}"))
}

/// Render an optional response decimal for table display (empty when absent).
fn dec(v: Option<Decimal>) -> String {
    v.map(|d| d.to_string()).unwrap_or_default()
}

/// Build a `GridTradeRule` from the shared CLI rule flags. `trigger-up/down` are
/// interpreted as percent or spread according to `--trigger-type`; every
/// enum-like field is always sent explicitly (the engine rejects partial rules).
fn build_rule(r: &GridRuleArgs) -> Result<longbridge::grid::GridTradeRule> {
    use longbridge::grid::GridTradeRule;

    let base = parse_dec(&r.base_price, "base-price")?;
    let upper = parse_dec(&r.upper_price, "upper-price")?;
    let lower = parse_dec(&r.lower_price, "lower-price")?;
    let qty = parse_dec(&r.quantity, "quantity")?;
    let upper_qty = parse_dec(&r.upper_quantity, "upper-quantity")?;
    let lower_qty = parse_dec(&r.lower_quantity, "lower-quantity")?;
    let trig_up = parse_dec(&r.trigger_up, "trigger-up")?;
    let trig_down = parse_dec(&r.trigger_down, "trigger-down")?;

    // Local pre-flight: reject mathematically-invalid rules before hitting the
    // gateway, so callers (including agents) get an actionable message instead
    // of a bare gateway code. Only invariant relations are checked here; the
    // gateway remains the final authority on strategy-specific rules.
    let zero = Decimal::ZERO;
    if lower <= zero {
        anyhow::bail!("--lower-price ({lower}) must be positive");
    }
    if upper <= lower {
        anyhow::bail!("--upper-price ({upper}) must be greater than --lower-price ({lower})");
    }
    if base < lower || base > upper {
        anyhow::bail!("--base-price ({base}) must be within [{lower}, {upper}]");
    }
    if qty <= zero {
        anyhow::bail!("--quantity ({qty}) must be positive");
    }
    if lower_qty <= zero {
        anyhow::bail!("--lower-quantity ({lower_qty}) must be positive");
    }
    if upper_qty <= lower_qty {
        anyhow::bail!(
            "--upper-quantity ({upper_qty}) must be greater than --lower-quantity ({lower_qty})"
        );
    }
    if trig_up <= zero || trig_down <= zero {
        anyhow::bail!("--trigger-up ({trig_up}) / --trigger-down ({trig_down}) must be positive");
    }
    // The gateway reports out-of-range enum-like ints as a generic "parameter
    // missing" (602080), which is misleading to callers. Validate them locally
    // so agents get an actionable message naming the offending flag.
    if !(0..=2).contains(&r.rth) {
        anyhow::bail!("--rth ({}) must be 0, 1, or 2", r.rth);
    }
    for (flag, v) in [
        ("--upper-event", r.upper_event),
        ("--lower-event", r.lower_event),
    ] {
        if v != 1 && v != 2 {
            anyhow::bail!("{flag} ({v}) must be 1 (ignore) or 2 (close at last price)");
        }
    }
    for (flag, v) in [("--sell-depth", r.sell_depth), ("--buy-depth", r.buy_depth)] {
        if !(-5..=5).contains(&v) {
            anyhow::bail!("{flag} ({v}) must be within [-5, 5]");
        }
    }

    // trigger-up/down interpreted by trigger-type (percent vs spread enum)
    let trigger = match r.trigger_type {
        super::GridTriggerTypeArg::Percent => longbridge::grid::GridTrigger::Percent {
            up: trig_up,
            down: trig_down,
        },
        super::GridTriggerTypeArg::Spread => longbridge::grid::GridTrigger::Spread {
            up: trig_up,
            down: trig_down,
        },
    };

    // order type: --order-type applies to both sides; --order-type-up/down override
    let both = r.order_type.as_str().to_string();
    let order_up = r
        .order_type_up
        .map_or_else(|| both.clone(), |o| o.as_str().to_string());
    let order_down = r.order_type_down.map_or(both, |o| o.as_str().to_string());

    let mut rule = GridTradeRule::new(
        base,
        upper,
        lower,
        trigger,
        qty,
        upper_qty,
        lower_qty,
        longbridge::grid::GridTimeInForce::from(r.tif.as_i32()),
    )
    .limit_events(
        longbridge::grid::GridLimitEvent::from(r.upper_event),
        longbridge::grid::GridLimitEvent::from(r.lower_event),
    )
    .depths(r.sell_depth, r.buy_depth)
    .order_types(order_up, order_down)
    .support_shortsell(r.support_shortsell)
    .multiple_trigger(r.multiple_trigger)
    .rth(r.rth);
    if let Some(expire) = &r.expire {
        rule = rule.expire_time(parse_expire(expire)?);
    }

    Ok(rule)
}

/// Parse an `--expire` value into unix seconds. Accepts RFC3339 (matching the
/// `expire_time` field emitted by `detail`, so a value can be round-tripped) or
/// a bare unix-seconds integer.
fn parse_expire(s: &str) -> Result<i64> {
    if let Ok(dt) = time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339) {
        return Ok(dt.unix_timestamp());
    }
    s.parse::<i64>().with_context(|| {
        format!(
            "invalid --expire ({s}): expected RFC3339 (e.g. 2026-11-10T08:00:00Z) or unix seconds"
        )
    })
}

/// Print a write-command result: structured JSON under `--format json`, a
/// human-readable line otherwise. Keeps mutation output parseable for agents
/// while staying friendly on a terminal.
fn print_mutation(format: &OutputFormat, json: &serde_json::Value, human: &str) {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(json).unwrap_or_default());
        }
        OutputFormat::Pretty => println!("{human}"),
    }
}

pub async fn cmd_grid(
    cmd: Option<GridCmd>,
    ids: Vec<String>,
    symbol: Option<String>,
    status: Option<String>,
    page: Option<i32>,
    limit: Option<i32>,
    sort_by: Option<String>,
    sort_order: Option<String>,
    format: &OutputFormat,
) -> Result<()> {
    match cmd {
        None => {
            if ids.is_empty() {
                cmd_list(symbol, status, page, limit, sort_by, sort_order, format).await
            } else {
                cmd_by_ids(ids, format).await
            }
        }
        Some(GridCmd::Submit {
            symbol,
            currency,
            rule,
            agree_terms,
        }) => cmd_submit(symbol, currency, &rule, agree_terms, format).await,
        Some(GridCmd::Replace { order_id, rule }) => cmd_replace(order_id, &rule, format).await,
        Some(GridCmd::Detail { order_id }) => cmd_detail(order_id, format).await,
        Some(GridCmd::Triggers {
            order_id,
            page,
            limit,
        }) => cmd_triggers(order_id, page, limit, format).await,
        Some(GridCmd::Cancel { order_id }) => {
            openapi::grid().cancel(order_id.clone()).await?;
            print_mutation(
                format,
                &serde_json::json!({ "status": "cancelled", "order_id": order_id }),
                &format!("Grid order {order_id} cancelled."),
            );
            Ok(())
        }
        Some(GridCmd::Suspend { order_id }) => {
            openapi::grid().suspend(order_id.clone()).await?;
            print_mutation(
                format,
                &serde_json::json!({ "status": "suspended", "order_id": order_id }),
                &format!("Grid order {order_id} suspended."),
            );
            Ok(())
        }
        Some(GridCmd::Restart { order_id }) => {
            openapi::grid().restart(order_id.clone()).await?;
            print_mutation(
                format,
                &serde_json::json!({ "status": "restarted", "order_id": order_id }),
                &format!("Grid order {order_id} restarted."),
            );
            Ok(())
        }
        Some(GridCmd::Info { symbol }) => cmd_info(symbol, format).await,
        Some(GridCmd::Questionnaire) => {
            openapi::grid()
                .submit_strategy_questionnaire(
                    longbridge::grid::SubmitStrategyQuestionnaireOptions::new(),
                )
                .await?;
            print_mutation(
                format,
                &serde_json::json!({ "status": "submitted" }),
                "Strategy risk-disclosure questionnaire submitted.",
            );
            Ok(())
        }
    }
}

async fn cmd_list(
    symbol: Option<String>,
    status: Option<String>,
    page: Option<i32>,
    limit: Option<i32>,
    sort_by: Option<String>,
    sort_order: Option<String>,
    format: &OutputFormat,
) -> Result<()> {
    let mut opts = longbridge::grid::GetGridOrdersOptions::new();
    if let Some(s) = symbol {
        opts = opts.symbol(s);
    }
    if let Some(s) = status {
        opts = opts.status(s);
    }
    if let Some(p) = page {
        opts = opts.page(p);
    }
    if let Some(l) = limit {
        opts = opts.limit(l);
    }
    if let Some(s) = sort_by {
        opts = opts.sort_by(s);
    }
    if let Some(s) = sort_order {
        opts = opts.sort_order(s);
    }

    let resp = openapi::grid().list(opts).await?;
    render_orders(&resp.grid_order, format);
    Ok(())
}

async fn cmd_by_ids(ids: Vec<String>, format: &OutputFormat) -> Result<()> {
    let orders = openapi::grid()
        .list_by_ids(longbridge::grid::GetGridOrdersByIdsOptions::new(ids))
        .await?;
    render_orders(&orders, format);
    Ok(())
}

fn render_orders(orders: &[longbridge::grid::GridOrder], format: &OutputFormat) {
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(orders).unwrap_or_default()
        ),
        OutputFormat::Pretty => {
            if orders.is_empty() {
                println!("No grid orders found.");
                return;
            }
            let headers = &[
                "Order ID", "Symbol", "Name", "Status", "Base", "Upper", "Lower", "Qty", "Trigger",
                "Type↑", "Type↓", "TIF",
            ];
            let rows: Vec<Vec<String>> = orders
                .iter()
                .map(|o| {
                    let trigger =
                        if o.trigger_price_type == longbridge::grid::TriggerPriceType::Spread {
                            format!(
                                "±{}/{}",
                                dec(o.trigger_spread_up),
                                dec(o.trigger_spread_down)
                            )
                        } else {
                            format!(
                                "±{}%/{}%",
                                dec(o.trigger_percent_up),
                                dec(o.trigger_percent_down)
                            )
                        };
                    vec![
                        o.order_id.clone(),
                        o.symbol.clone(),
                        o.stock_name.clone(),
                        o.status.clone(),
                        dec(o.submitted_base_price),
                        dec(o.upper_limit_price),
                        dec(o.lower_limit_price),
                        dec(o.trigger_quantity),
                        trigger,
                        o.grid_order_type_up.clone(),
                        o.grid_order_type_down.clone(),
                        tif_label(o.time_in_force),
                    ]
                })
                .collect();
            print_table(headers, rows, format);
        }
    }
}

/// Print the payload a write command would send, for `--dry-run`. Always JSON
/// (both formats), since dry-run exists to be inspected by a caller or agent.
fn print_dry_run(payload: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(payload).unwrap_or_default()
    );
}

async fn cmd_submit(
    symbol: String,
    currency: String,
    rule: &GridRuleArgs,
    agree_terms: bool,
    format: &OutputFormat,
) -> Result<()> {
    // Build (and validate) the rule first so --dry-run reports the same errors a
    // real submit would, before any terms prompt or gateway call.
    let built = build_rule(rule)?;
    if rule.dry_run {
        print_dry_run(&serde_json::json!({
            "dry_run": true,
            "action": "submit",
            "symbol": symbol,
            "currency": currency,
            "rule": serde_json::to_value(&built).unwrap_or_default(),
        }));
        return Ok(());
    }
    if !agree_terms && !confirm_terms()? {
        println!("Grid order submission cancelled.");
        return Ok(());
    }
    let resp = openapi::grid()
        .submit(longbridge::grid::SubmitGridOrderOptions::new(
            symbol, currency, built,
        ))
        .await?;
    print_mutation(
        format,
        &serde_json::json!({ "status": "submitted", "order_id": resp.order_id }),
        &format!("Grid order submitted. Order ID: {}", resp.order_id),
    );
    Ok(())
}

async fn cmd_replace(order_id: String, rule: &GridRuleArgs, format: &OutputFormat) -> Result<()> {
    let built = build_rule(rule)?;
    if rule.dry_run {
        print_dry_run(&serde_json::json!({
            "dry_run": true,
            "action": "replace",
            "order_id": order_id,
            "rule": serde_json::to_value(&built).unwrap_or_default(),
        }));
        return Ok(());
    }
    openapi::grid()
        .replace(longbridge::grid::ReplaceGridOrderOptions::new(
            order_id.clone(),
            built,
        ))
        .await?;
    print_mutation(
        format,
        &serde_json::json!({ "status": "replaced", "order_id": order_id }),
        &format!("Grid order {order_id} replaced."),
    );
    Ok(())
}

async fn cmd_detail(order_id: String, format: &OutputFormat) -> Result<()> {
    let d = openapi::grid()
        .detail(longbridge::grid::GetGridOrderDetailOptions::new(order_id))
        .await?;
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&d).unwrap_or_default()),
        OutputFormat::Pretty => {
            print_json_value(
                &serde_json::json!({
                    "order_id": d.order_id,
                    "symbol": d.symbol,
                    "stock_name": d.stock_name,
                    "status": d.status,
                    "grid_status": d.grid_status,
                    "submitted_base_price": d.submitted_base_price,
                    "current_base_price": d.current_base_price,
                    "upper_limit_price": d.upper_limit_price,
                    "lower_limit_price": d.lower_limit_price,
                    "trigger_price_type": d.trigger_price_type,
                    "trigger_quantity": d.trigger_quantity,
                    "settlement_currency": d.settlement_currency,
                    "time_in_force": tif_label(d.time_in_force),
                    "gtd": d.gtd,
                    "sub_orders": d.grid_sub_orders.len(),
                    "history_entries": d.grid_order_history.len(),
                }),
                format,
            );
        }
    }
    Ok(())
}

async fn cmd_triggers(
    order_id: String,
    page: Option<i32>,
    limit: Option<i32>,
    format: &OutputFormat,
) -> Result<()> {
    let mut opts = longbridge::grid::GetGridTriggerHistoryOptions::new(order_id);
    if let Some(p) = page {
        opts = opts.page(p);
    }
    if let Some(l) = limit {
        opts = opts.limit(l);
    }
    let resp = openapi::grid().trigger_history(opts).await?;
    match format {
        OutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&resp.trigger_orders).unwrap_or_default()
        ),
        OutputFormat::Pretty => {
            if resp.trigger_orders.is_empty() {
                println!("No trigger history found.");
                return Ok(());
            }
            let headers = &[
                "ID",
                "Symbol",
                "Status",
                "Price",
                "Qty",
                "Exec Price",
                "Exec Qty",
                "Type",
            ];
            let rows: Vec<Vec<String>> = resp
                .trigger_orders
                .iter()
                .map(|t| {
                    vec![
                        t.id.clone(),
                        t.symbol.clone(),
                        t.status.clone(),
                        dec(t.price),
                        dec(t.quantity),
                        dec(t.executed_price),
                        dec(t.executed_qty),
                        t.order_type.clone(),
                    ]
                })
                .collect();
            print_table(headers, rows, format);
        }
    }
    Ok(())
}

async fn cmd_info(symbol: String, format: &OutputFormat) -> Result<()> {
    let i = openapi::grid().order_info(symbol).await?;
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&i).unwrap_or_default()),
        OutputFormat::Pretty => {
            print_json_value(
                &serde_json::json!({
                    "name": i.name,
                    "last_done": i.last_done,
                    "lot_size": i.lot_size,
                    "buy_lot_size": i.buy_lot_size,
                    "sell_lot_size": i.sell_lot_size,
                    "strategy_granted": i.channel_infos.strategy_granted,
                    "support_rth": i.channel_infos.support_rth,
                    "currency": i.channel_infos.currency,
                    "settlement_currency": i.channel_infos.settlement_currency,
                }),
                format,
            );
        }
    }
    Ok(())
}

fn tif_label(v: longbridge::grid::GridTimeInForce) -> String {
    use longbridge::grid::GridTimeInForce::{Day, GoodTilCanceled, GoodTilDate, Unknown};
    match v {
        Day => "Day",
        GoodTilCanceled => "GTC",
        GoodTilDate => "GTD",
        Unknown(_) => "-",
    }
    .to_string()
}

fn confirm_terms() -> Result<bool> {
    use std::io::{IsTerminal, Write};
    // Non-interactive (agent / pipe / CI): don't hang reading stdin. Fail with an
    // actionable message so the caller re-runs with --agree-terms explicitly.
    if !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "Grid submission needs risk-disclosure confirmation, but stdin is not a terminal. \
             Re-run with --agree-terms to confirm non-interactively."
        );
    }
    println!(
        "\nGrid trading involves risk. By submitting you confirm you have read and \
         agreed to the strategy risk disclosure.\nTip: pass --agree-terms to skip this prompt."
    );
    print!("Proceed? [y/N]: ");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(input.trim().eq_ignore_ascii_case("y"))
}

pub(crate) fn schema_for_path(path: &[String]) -> Option<super::schema::ResponseSchema> {
    use super::schema::{array, object, text};

    let command = path.join(" ");
    let order_fields = &[
        "order_id",
        "symbol",
        "stock_name",
        "status",
        "grid_status",
        "market",
        "submitted_base_price",
        "current_base_price",
        "upper_limit_price",
        "lower_limit_price",
        "trigger_price_type",
        "trigger_percent_up",
        "trigger_percent_down",
        "trigger_spread_up",
        "trigger_spread_down",
        "trigger_quantity",
        "upper_limit_quantity",
        "lower_limit_quantity",
        "grid_order_type_up",
        "grid_order_type_down",
        "time_in_force",
        "rth",
        "gtd",
        "created_at",
    ];
    // `grid detail` serializes the full GridOrderDetail struct, which is a
    // superset of the list fields; describe every key so agents relying on the
    // schema see the complete shape (sub-orders, history, timestamps, reasons).
    let detail_fields = &[
        "order_id",
        "symbol",
        "stock_name",
        "status",
        "grid_status",
        "suspend_reason",
        "sleeping_reason",
        "submitted_base_price",
        "current_base_price",
        "upper_limit_price",
        "lower_limit_price",
        "trigger_price_type",
        "trigger_spread_up",
        "trigger_spread_down",
        "trigger_percent_up",
        "trigger_percent_down",
        "pullback_percent",
        "pullback_spread",
        "rebound_percent",
        "rebound_spread",
        "multiple_trigger",
        "time_in_force",
        "trigger_quantity",
        "trigger_sell_quantity",
        "trigger_buy_quantity",
        "upper_limit_quantity",
        "lower_limit_quantity",
        "upper_limit_event",
        "lower_limit_event",
        "trigger_sell_depth",
        "trigger_buy_depth",
        "created_at",
        "updated_at",
        "settlement_currency",
        "expire_time",
        "gtd",
        "grid_sub_orders",
        "sub_has_more",
        "grid_order_history",
        "history_has_more",
        "support_shortsell",
        "rth",
    ];
    let schema = match command.as_str() {
        "grid" => array("Grid trading orders", order_fields),
        "grid detail" => object("Grid trading order detail", detail_fields),
        "grid triggers" => array(
            "Grid trigger history",
            &[
                "id",
                "symbol",
                "status",
                "price",
                "quantity",
                "executed_price",
                "executed_qty",
                "order_type",
                "trigger_price",
            ],
        ),
        "grid info" => object(
            "Grid order info",
            &[
                "name",
                "last_done",
                "lot_size",
                "buy_lot_size",
                "sell_lot_size",
                "bid_sizes",
                "channel_infos",
            ],
        ),
        "grid submit" | "grid replace" | "grid cancel" | "grid suspend" | "grid restart"
        | "grid questionnaire" => text("Grid trading mutation status message"),
        _ => return None,
    };
    Some(schema)
}
