//! Money-movement commands: bank cards and their eDDA authorizations,
//! deposits, withdrawals, and stock transfers in and out of the account.
//!
//! JSON output matches the Longbridge MCP server's account tools; the shaping
//! lives in [`super::atm_shape`].

use anyhow::Result;
use serde_json::Value;

use super::api::http_get;
use super::atm_shape::{
    bank_cards_output, check_bank_card_id, check_deposit_states, clamp_page_size, date_range,
    edda_output, shape_cards, shape_deposits, shape_edda_status, shape_transfers,
    shape_withdrawals, TransferKind, TRANSFER_IN, TRANSFER_OUT,
};
use super::output::print_table;
use super::OutputFormat;

/// Print a shaped value as pretty JSON, unchanged.
pub(crate) fn print_shaped_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
}

/// Table cell for a JSON value: strings as-is, `-` for a missing value.
pub(crate) fn cell(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "-".to_string(),
        other => other.to_string(),
    }
}

/// Table cell for an RFC3339 time (`2026-11-25T17:59:06+08:00`) as
/// `2026-11-25 17:59`; anything else as [`cell`].
pub(crate) fn time_cell(v: &Value) -> String {
    match v.as_str() {
        Some(t) if t.len() >= 16 && t.as_bytes().get(10) == Some(&b'T') => {
            format!("{} {}", &t[..10], &t[11..16])
        }
        _ => cell(v),
    }
}

/// Preferred label: `label_key` when present, else `raw_key`.
pub(crate) fn label_cell(item: &Value, label_key: &str, raw_key: &str) -> String {
    match item.get(label_key) {
        Some(v) if !v.is_null() => cell(v),
        _ => cell(&item[raw_key]),
    }
}

/// Fetch and shape the bank-card list (`GET /v2/account/bank-cards`).
async fn fetch_cards(verbose: bool) -> Result<Vec<Value>> {
    let channel = crate::auth::account_channel_or_default();
    let raw = http_get(
        "/v2/account/bank-cards",
        &[
            ("from_app_page", "cards"),
            ("account_channel", channel.as_str()),
        ],
        verbose,
    )
    .await?;
    Ok(shape_cards(raw))
}

/// List linked bank cards with their verification, binding and authorization
/// status.
pub async fn cmd_bank_cards(format: &OutputFormat, verbose: bool) -> Result<()> {
    let value = bank_cards_output(fetch_cards(verbose).await?);
    match format {
        OutputFormat::Json => print_shaped_json(&value),
        OutputFormat::Pretty => {
            let cards = value["cards"].as_array().cloned().unwrap_or_default();
            if cards.is_empty() {
                println!("No bank cards found.");
                return Ok(());
            }
            let headers = [
                "id",
                "bank",
                "account",
                "type",
                "verification",
                "binding",
                "review days",
                "authorizations",
            ];
            let rows = cards
                .iter()
                .map(|card| {
                    let auths = card["auth_list"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|auth| {
                            format!(
                                "{} {} {}",
                                cell(&auth["flow_id"]),
                                cell(&auth["currency"]),
                                cell(&auth["authorization"])
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("; ");
                    vec![
                        cell(&card["id"]),
                        cell(&card["bank"]),
                        cell(&card["account"]),
                        cell(&card["account_type"]),
                        cell(&card["verification"]),
                        cell(&card["binding"]),
                        cell(&card["review_days"]),
                        if auths.is_empty() {
                            "-".to_string()
                        } else {
                            auths
                        },
                    ]
                })
                .collect();
            print_table(&headers, rows, &OutputFormat::Pretty);
        }
    }
    Ok(())
}

/// List the eDDA authorizations across all linked bank cards.
pub async fn cmd_edda(format: &OutputFormat, verbose: bool) -> Result<()> {
    let value = edda_output(&fetch_cards(verbose).await?);
    match format {
        OutputFormat::Json => print_shaped_json(&value),
        OutputFormat::Pretty => {
            let rows = value["authorizations"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if rows.is_empty() {
                println!("No eDDA authorizations found.");
                return Ok(());
            }
            let headers = [
                "card id",
                "bank",
                "account",
                "flow",
                "currency",
                "authorization",
                "max amount",
                "expiry",
                "failure reason",
            ];
            let rows = rows
                .iter()
                .map(|row| {
                    let max_amount = if row["max_amount_unlimited"] == Value::Bool(true) {
                        "unlimited".to_string()
                    } else {
                        cell(&row["max_amount"])
                    };
                    let expiry = if row["long_term_valid"] == Value::Bool(true) {
                        "long-term".to_string()
                    } else {
                        time_cell(&row["expiry_date"])
                    };
                    vec![
                        cell(&row["card_id"]),
                        cell(&row["bank"]),
                        cell(&row["account"]),
                        cell(&row["flow"]),
                        cell(&row["currency"]),
                        cell(&row["authorization"]),
                        max_amount,
                        expiry,
                        cell(&row["failure_reason"]),
                    ]
                })
                .collect();
            print_table(&headers, rows, &OutputFormat::Pretty);
        }
    }
    Ok(())
}

/// List the eDDA authorization applications filed for one card and currency.
pub async fn cmd_edda_status(
    card_id: &str,
    currency: &str,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let card_id = card_id.trim();
    check_bank_card_id(card_id)?;
    let channel = crate::auth::account_channel_or_default();
    let currency = currency.trim().to_uppercase();
    let raw = http_get(
        "/v1/account/edda-status",
        &[
            ("card_id", card_id),
            ("currency", currency.as_str()),
            ("account_channel", channel.as_str()),
        ],
        verbose,
    )
    .await?;
    let value = shape_edda_status(raw);
    match format {
        OutputFormat::Json => print_shaped_json(&value),
        OutputFormat::Pretty => {
            let apps = value["applications"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if apps.is_empty() {
                println!(
                    "No eDDA application filed for this card; see `longbridge edda` for its current eDDA state."
                );
                return Ok(());
            }
            let headers = [
                "id",
                "flow",
                "currency",
                "authorization",
                "failure reason",
                "created at",
                "updated at",
            ];
            let rows = apps
                .iter()
                .map(|app| {
                    vec![
                        cell(&app["id"]),
                        cell(&app["flow_id"]),
                        cell(&app["currency"]),
                        cell(&app["authorization"]),
                        cell(&app["failure_reason"]),
                        time_cell(&app["created_at"]),
                        time_cell(&app["updated_at"]),
                    ]
                })
                .collect();
            print_table(&headers, rows, &OutputFormat::Pretty);
        }
    }
    Ok(())
}

/// Filters shared by `stock-transfers-in` and `stock-transfers-out`.
pub struct TransferFilter<'a> {
    pub market: Option<&'a str>,
    pub state: Option<i32>,
    pub start: Option<&'a str>,
    pub end: Option<&'a str>,
    pub page: u32,
    pub size: u32,
}

async fn transfer_records(
    kind: &TransferKind,
    filter: &TransferFilter<'_>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let channel = crate::auth::account_channel_or_default();
    let (start, end) = date_range(filter.start, filter.end)?;
    let mut params: Vec<(&str, String)> = vec![
        ("page", filter.page.max(1).to_string()),
        ("size", clamp_page_size(filter.size).to_string()),
        ("account_channel", channel),
    ];
    if let Some(market) = filter.market {
        params.push(("market", market.trim().to_uppercase()));
    }
    if let Some(state) = filter.state {
        params.push(("state", state.to_string()));
    }
    if let Some(start) = start {
        params.push(("start", start));
    }
    if let Some(end) = end {
        params.push(("end", end));
    }
    let params: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let raw = http_get(kind.path, &params, verbose).await?;
    let value = shape_transfers(raw, kind);
    match format {
        OutputFormat::Json => print_shaped_json(&value),
        OutputFormat::Pretty => {
            let records = value["records"].as_array().cloned().unwrap_or_default();
            if records.is_empty() {
                println!("No stock transfer records.");
                return Ok(());
            }
            if let Some(total) = value.get("total") {
                println!("Total: {}\n", cell(total));
            }
            let headers = [
                "id",
                "created at",
                "broker",
                "market",
                "stocks",
                "state",
                "days",
                "notified broker",
                "reason",
            ];
            let rows = records
                .iter()
                .map(|record| {
                    let stocks = record["stocks"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|s| format!("{} x{}", cell(&s["code"]), cell(&s["amount"])))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let broker = match record.get("broker_en") {
                        Some(name) => cell(name),
                        None => cell(&record["broker"]),
                    };
                    let reason = match record.get("reason") {
                        Some(reason) => cell(reason),
                        None => cell(&record["remark"]),
                    };
                    vec![
                        cell(&record["id"]),
                        time_cell(&record["created_at"]),
                        broker,
                        cell(&record["market"]),
                        if stocks.is_empty() {
                            "-".to_string()
                        } else {
                            stocks
                        },
                        label_cell(record, "state_label", "state"),
                        cell(&record["days_in_process"]),
                        cell(&record["user_notified_broker"]),
                        reason,
                    ]
                })
                .collect();
            print_table(&headers, rows, &OutputFormat::Pretty);
        }
    }
    Ok(())
}

/// List stock transfer-in applications (another broker to this account).
pub async fn cmd_stock_transfers_in(
    filter: &TransferFilter<'_>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    transfer_records(&TRANSFER_IN, filter, format, verbose).await
}

/// List stock transfer-out applications (this account to another broker).
pub async fn cmd_stock_transfers_out(
    filter: &TransferFilter<'_>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    transfer_records(&TRANSFER_OUT, filter, format, verbose).await
}

fn print_has_more(value: &Value) {
    if value["has_more"] == Value::Bool(true) {
        println!("\nMore records may exist: request the next --page.");
    }
}

/// List withdrawal history for the current account.
pub async fn cmd_withdrawals(
    page: u32,
    limit: u32,
    start: Option<&str>,
    end: Option<&str>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let account_channel = crate::auth::account_channel_or_default();
    let size = clamp_page_size(limit);
    let page_str = page.to_string();
    let size_str = size.to_string();
    let (begin, end) = date_range(start, end)?;
    let mut params: Vec<(&str, &str)> = vec![
        ("page", page_str.as_str()),
        ("size", size_str.as_str()),
        ("account_channel", account_channel.as_str()),
    ];
    if let Some(b) = begin.as_deref() {
        params.push(("begin", b));
    }
    if let Some(e) = end.as_deref() {
        params.push(("end", e));
    }
    let raw = http_get("/v1/account/withdrawals", &params, verbose).await?;
    let value = shape_withdrawals(raw, size as usize);
    match format {
        OutputFormat::Json => print_shaped_json(&value),
        OutputFormat::Pretty => {
            let list = value["list"].as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                println!("No withdrawal records.");
                return Ok(());
            }
            let headers = [
                "id",
                "created at",
                "amount",
                "fee",
                "paid out",
                "currency",
                "state",
                "bank account",
            ];
            let rows = list
                .iter()
                .map(|item| {
                    let state = match item.get("state_description") {
                        Some(desc) => cell(desc),
                        None => label_cell(item, "state_label", "state"),
                    };
                    vec![
                        cell(&item["id"]),
                        time_cell(&item["created_at"]),
                        cell(&item["amount"]),
                        cell(&item["fee"]),
                        cell(&item["settlement_amount"]),
                        cell(&item["currency"]),
                        state,
                        cell(&item["bank_account"]),
                    ]
                })
                .collect();
            print_table(&headers, rows, &OutputFormat::Pretty);
            print_has_more(&value);
        }
    }
    Ok(())
}

/// List deposit history for the current account.
pub async fn cmd_deposits(
    page: u32,
    limit: u32,
    states: Option<&str>,
    currencies: Option<&str>,
    start: Option<&str>,
    end: Option<&str>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let account_channel = crate::auth::account_channel_or_default();
    let size = clamp_page_size(limit);
    let page_str = page.to_string();
    let size_str = size.to_string();
    let states = states.map(check_deposit_states).transpose()?;
    let (from_date, to_date) = date_range(start, end)?;
    let mut params: Vec<(&str, &str)> = vec![
        ("page", page_str.as_str()),
        ("size", size_str.as_str()),
        ("account_channel", account_channel.as_str()),
    ];
    if let Some(s) = states.as_deref() {
        params.push(("states", s));
    }
    if let Some(c) = currencies {
        params.push(("currencies", c));
    }
    if let Some(f) = from_date.as_deref() {
        params.push(("from_date", f));
    }
    if let Some(t) = to_date.as_deref() {
        params.push(("to_date", t));
    }
    let raw = http_get("/v1/account/deposits", &params, verbose).await?;
    let value = shape_deposits(raw, size as usize);
    match format {
        OutputFormat::Json => print_shaped_json(&value),
        OutputFormat::Pretty => {
            let items = value["items"].as_array().cloned().unwrap_or_default();
            if items.is_empty() {
                println!("No deposit records.");
                return Ok(());
            }
            let headers = [
                "id",
                "created at",
                "amount",
                "received",
                "currency",
                "method",
                "state",
            ];
            let rows = items
                .iter()
                .map(|item| {
                    vec![
                        cell(&item["id"]),
                        time_cell(&item["created_at"]),
                        cell(&item["amount"]),
                        cell(&item["receivable_amount"]),
                        cell(&item["currency"]),
                        cell(&item["bank_operation_type_name"]),
                        label_cell(item, "state_label", "state"),
                    ]
                })
                .collect();
            print_table(&headers, rows, &OutputFormat::Pretty);
            print_has_more(&value);
        }
    }
    Ok(())
}

pub(crate) fn schema_for_path(path: &[String]) -> Option<super::schema::ResponseSchema> {
    use super::schema::{field, schema, RootKind};

    let command = path.join(" ");
    let schema =
        match command.as_str() {
            "bank-cards" => schema(
                "Linked bank cards and their review state",
                RootKind::Object,
                vec![field(
                    "cards",
                    "object[]",
                    "Cards: {id, bank, bank_en, bank_code, account (last 4 digits), account_type, \
                 region, region_name, country, verification (unverified/under_review/verified), \
                 witness (not_required/pending/witnessed), binding (binding/bound/failed), \
                 review_days (days since the card was added, only while under_review), bind_tip, \
                 remark, created_at, auth_list[]{flow_id (hk_edda/hk_sdda: eDDA, hk_bank_sec: \
                 bank-securities transfer, hk_fps: FPS), name, currency, authorization \
                 (unauthorized/authorized/authorizing/failed), failure_reason, max_amount, \
                 max_amount_unlimited, daily_max_count, daily_max_count_unlimited, expiry_date, \
                 long_term_valid, is_support, doc_reviewing, no_prior_deposit_reviewing}}. \
                 Times are Hong Kong time (+08:00)",
                )],
            ),
            "edda" => schema(
                "eDDA authorizations across linked bank cards",
                RootKind::Object,
                vec![field(
                    "authorizations",
                    "object[]",
                    "Rows: {card_id, bank, bank_en, account (last 4 digits), card_binding \
                 (binding/bound/failed), flow (hk_edda: authorized at the bank, hk_sdda: \
                 authorized in the app), name, currency, authorization \
                 (unauthorized/authorized/authorizing/failed), failure_reason, max_amount, \
                 max_amount_unlimited, daily_max_count, daily_max_count_unlimited, expiry_date, \
                 long_term_valid, is_support, doc_reviewing, no_prior_deposit_reviewing}. \
                 Empty when no card has an eDDA authorization",
                )],
            ),
            "edda-status" => schema(
                "eDDA authorization applications filed for one card and currency",
                RootKind::Object,
                vec![field(
                    "applications",
                    "object[]",
                    "Applications: {id, flow_id, name, currency, authorization \
                 (unauthorized/authorized/authorizing/failed), failure_reason, created_at, \
                 updated_at}. Usually empty: an eDDA pushed by the bank has no application",
                )],
            ),
            "stock-transfers-in" | "stock-transfers-out" => {
                schema(
                    "Stock transfer applications, newest first",
                    RootKind::Object,
                    vec![
                field(
                    "records",
                    "object[]",
                    "Records: {id, order_id, market, broker, broker_en, code (broker code), \
                     account (account at the other broker, last 4 digits), state (status text), \
                     state_code, state_label, can_modify (transfers in only; 1: can still be \
                     cancelled or changed), app_state, user_notified_broker (the user tapped \
                     \"I have notified the broker\" in the app), stocks[]{code, name, name_en, \
                     market, counter_id, amount, currency, state, state_label \
                     (processing/succeeded/failed)}, created_at, updated_at, process_at, \
                     exec_at, agree_at, refuse_at, cancel_at, days_in_process (days from \
                     creation until execution; for a cancelled or failed one until it ended; \
                     for an open one until today), remark, reason}",
                ),
                field("total", "number | string", "Total records (when upstream returns it)"),
            ],
                )
            }
            "withdrawals" => schema(
                "Withdrawal history, newest first",
                RootKind::Object,
                vec![
                    field(
                        "list",
                        "object[]",
                        "Withdrawals: {id, order_id, created_at, currency, amount (applied), fee, \
                     settlement_amount (paid out), state, state_label (submitted/processing/\
                     rejected/cancelled/remitted/failed/refunded/approved_awaiting_payout/\
                     large_amount_review/awaiting_confirmation/…), state_description (status \
                     text as the app shows it), bank_account (last 4 digits), \
                     display_bank_account, user_bank_swift_code}",
                    ),
                    field("total", "number | string", "Total records"),
                    field(
                        "has_more",
                        "boolean",
                        "A full page came back; request the next page",
                    ),
                ],
            ),
            "deposits" => schema(
                "Deposit history, newest first",
                RootKind::Object,
                vec![
                field(
                    "items",
                    "object[]",
                    "Deposits: {id, created_at, updated_at, currency, amount (notified/applied), \
                     receivable_amount (received; 0 = not received yet), state (status text), \
                     state_code, state_label (processing/credited/failed/cancelled/invalidated/\
                     rejected), sub_state_code, bank_operation_type_name (deposit method), \
                     reference_no, can_cancel, voucher_count (receipts uploaded)}",
                ),
                field(
                    "has_more",
                    "boolean",
                    "A full page came back; request the next page (there is no reliable total)",
                ),
            ],
            ),
            _ => return None,
        };
    Some(schema)
}
