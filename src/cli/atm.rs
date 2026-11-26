//! Money-movement commands: bank cards and their eDDA authorizations,
//! deposits, withdrawals, and stock transfers in and out of the account.
//!
//! The data comes from the SDK's `AssetContext` account APIs; JSON output
//! matches the Longbridge MCP server's account tools, and the shaping lives in
//! [`super::atm_shape`].

use std::future::Future;

use anyhow::Result;
use longbridge::{
    asset::{
        BankCard, GetDepositsOptions, GetStockTransfersInOptions, GetStockTransfersOutOptions,
        GetWithdrawalsOptions,
    },
    httpclient::HttpClientError,
};
use serde_json::Value;

use super::api::retry_rate_limited;
use super::atm_shape::{
    bank_cards_output, check_bank_card_id, check_deposit_states, clamp_page_size, date_range,
    edda_output, page_number, shape_deposits, shape_edda_status, shape_transfers_in,
    shape_transfers_out, shape_withdrawals, split_list, transfer_in_state, transfer_out_state,
};
use super::output::print_table;
use super::OutputFormat;

/// The error for an account command run by an account in the US data center:
/// these routes (lb-portfolio-atm / lb-reward-center) exist only in the AP data
/// center, and there is no US route covering the same data.
fn us_unavailable(command: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "`longbridge {command}` is not available for accounts in the US data center. \
         There is no US equivalent; do not retry."
    )
}

/// Replace the SDK's data-center restriction error with [`us_unavailable`];
/// any other error passes through.
fn ap_only_error(command: &str, err: anyhow::Error) -> anyhow::Error {
    let http_error = err.downcast_ref::<HttpClientError>().or_else(|| {
        match err.downcast_ref::<longbridge::Error>() {
            Some(longbridge::Error::HttpClient(e)) => Some(e),
            _ => None,
        }
    });
    if matches!(http_error, Some(HttpClientError::DcRegionRestricted { .. })) {
        us_unavailable(command)
    } else {
        err
    }
}

/// Await an SDK call on an AP-only account route for `command`: the SDK
/// rejects a US-region session before sending, and the rejection names the
/// command. `path` is only echoed with `--verbose`.
pub(crate) async fn ap_call<T, Fut>(
    command: &str,
    path: &str,
    verbose: bool,
    call: Fut,
) -> Result<T>
where
    Fut: Future<Output = longbridge::Result<T>>,
{
    if verbose {
        eprintln!("* GET {path}");
    }
    call.await
        .map_err(|err| ap_only_error(command, anyhow::Error::from(err)))
}

/// [`ap_call`] for a command that sends several requests to the same route:
/// rate limits are retried, as [`retry_rate_limited`].
pub(crate) async fn ap_call_paced<T, F, Fut>(
    command: &str,
    path: &str,
    verbose: bool,
    call: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = longbridge::Result<T>>,
{
    if verbose {
        eprintln!("* GET {path}");
    }
    retry_rate_limited(verbose, call)
        .await
        .map_err(|err| ap_only_error(command, err))
}

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

/// Fetch the bank-card list (`GET /v2/account/bank-cards`).
async fn fetch_cards(command: &str, verbose: bool) -> Result<Vec<BankCard>> {
    let ctx = crate::openapi::statement();
    ap_call(command, "/v2/account/bank-cards", verbose, ctx.bank_cards()).await
}

/// List linked bank cards with their verification, binding and authorization
/// status.
pub async fn cmd_bank_cards(format: &OutputFormat, verbose: bool) -> Result<()> {
    let value = bank_cards_output(&fetch_cards("bank-cards", verbose).await?);
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
                "witness",
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
                            let text = format!(
                                "{} {} {}",
                                cell(&auth["flow_id"]),
                                cell(&auth["currency"]),
                                cell(&auth["authorization"])
                            );
                            match auth["failure_reason"]
                                .as_str()
                                .filter(|r| !r.trim().is_empty())
                            {
                                Some(reason) => format!("{text} ({reason})"),
                                None => text,
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("; ");
                    vec![
                        cell(&card["id"]),
                        cell(&card["bank"]),
                        cell(&card["account"]),
                        cell(&card["account_type"]),
                        cell(&card["verification"]),
                        cell(&card["witness"]),
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
pub async fn cmd_edda_list(format: &OutputFormat, verbose: bool) -> Result<()> {
    let value = edda_output(&fetch_cards("edda list", verbose).await?);
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
    let ctx = crate::openapi::statement();
    let applications = ap_call(
        "edda status",
        "/v1/account/edda-status",
        verbose,
        ctx.edda_applications(card_id, currency),
    )
    .await?;
    let value = shape_edda_status(&applications);
    match format {
        OutputFormat::Json => print_shaped_json(&value),
        OutputFormat::Pretty => {
            let apps = value["applications"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if apps.is_empty() {
                println!(
                    "No eDDA application filed for this card; see `longbridge edda list` for its current eDDA state."
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

/// Filters shared by `stock-transfer in` and `stock-transfer out`.
pub struct TransferFilter<'a> {
    pub market: Option<&'a str>,
    pub state: Option<i32>,
    pub start: Option<&'a str>,
    pub end: Option<&'a str>,
    pub page: u32,
    pub size: u32,
}

/// Direction of a stock transfer list.
#[derive(Clone, Copy)]
enum TransferDirection {
    In,
    Out,
}

async fn transfer_records(
    direction: TransferDirection,
    filter: &TransferFilter<'_>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let (start, end) = date_range(filter.start, filter.end)?;
    let market = filter.market.map(str::trim).filter(|m| !m.is_empty());
    let page = page_number(filter.page);
    let size = clamp_page_size(filter.size).cast_signed();
    let ctx = crate::openapi::statement();
    let value = match direction {
        TransferDirection::In => {
            let mut opts = GetStockTransfersInOptions::new().page(page).size(size);
            if let Some(market) = market {
                opts = opts.market(market);
            }
            if let Some(state) = filter.state {
                opts = opts.state(transfer_in_state(state)?);
            }
            if let Some(start) = start {
                opts = opts.start(start);
            }
            if let Some(end) = end {
                opts = opts.end(end);
            }
            let resp = ap_call(
                "stock-transfer in",
                "/v1/account/stock-transfers/in",
                verbose,
                ctx.stock_transfers_in(opts),
            )
            .await?;
            shape_transfers_in(&resp)
        }
        TransferDirection::Out => {
            let mut opts = GetStockTransfersOutOptions::new().page(page).size(size);
            if let Some(market) = market {
                opts = opts.market(market);
            }
            if let Some(state) = filter.state {
                opts = opts.state(transfer_out_state(state)?);
            }
            if let Some(start) = start {
                opts = opts.start(start);
            }
            if let Some(end) = end {
                opts = opts.end(end);
            }
            let records = ap_call(
                "stock-transfer out",
                "/v1/account/stock-transfers/out",
                verbose,
                ctx.stock_transfers_out(opts),
            )
            .await?;
            shape_transfers_out(&records)
        }
    };
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
    transfer_records(TransferDirection::In, filter, format, verbose).await
}

/// List stock transfer-out applications (this account to another broker).
pub async fn cmd_stock_transfers_out(
    filter: &TransferFilter<'_>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    transfer_records(TransferDirection::Out, filter, format, verbose).await
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
    let size = clamp_page_size(limit);
    let (start, end) = date_range(start, end)?;
    let mut opts = GetWithdrawalsOptions::new()
        .page(page_number(page))
        .size(size.cast_signed());
    if let Some(start) = start {
        opts = opts.start(start);
    }
    if let Some(end) = end {
        opts = opts.end(end);
    }
    let ctx = crate::openapi::statement();
    let resp = ap_call(
        "withdrawals",
        "/v1/account/withdrawals",
        verbose,
        ctx.withdrawals(opts),
    )
    .await?;
    let value = shape_withdrawals(&resp, size as usize);
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
    let size = clamp_page_size(limit);
    let states = states.map(check_deposit_states).transpose()?;
    let (start, end) = date_range(start, end)?;
    let mut opts = GetDepositsOptions::new()
        .page(page_number(page))
        .size(size.cast_signed());
    if let Some(states) = states {
        opts = opts.states(states);
    }
    if let Some(currencies) = currencies {
        opts = opts.currencies(split_list(currencies));
    }
    if let Some(start) = start {
        opts = opts.start(start);
    }
    if let Some(end) = end {
        opts = opts.end(end);
    }
    let ctx = crate::openapi::statement();
    let items = ap_call(
        "deposits",
        "/v1/account/deposits",
        verbose,
        ctx.deposits(opts),
    )
    .await?;
    let value = shape_deposits(&items, size as usize);
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
    let schema = match command.as_str() {
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
                 long_term_valid, is_support (true: the bank supports this flow), doc_reviewing, \
                 no_prior_deposit_reviewing}}. A code with no known label is shown raw instead: \
                 status / witness_state / bind_state on a card, status on an authorization. \
                 Times are Hong Kong time (+08:00)",
            )],
        ),
        "edda list" => schema(
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
                 long_term_valid, is_support (true: the bank supports this flow), doc_reviewing, \
                 no_prior_deposit_reviewing, status (raw code, only when authorization has no \
                 label)}. Empty when no card has an eDDA authorization",
            )],
        ),
        "edda status" => schema(
            "eDDA authorization applications filed for one card and currency",
            RootKind::Object,
            vec![field(
                "applications",
                "object[]",
                "Applications: {id, flow_id, name, currency, authorization \
                 (unauthorized/authorized/authorizing/failed), status (raw code, only when \
                 authorization has no label), failure_reason, created_at, updated_at}. Usually \
                 empty: an eDDA pushed by the bank has no application",
            )],
        ),
        "stock-transfer in" | "stock-transfer out" => schema(
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
                field("total", "number", "Total records (transfers in only)"),
            ],
        ),
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
                field("total", "number", "Total records"),
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

#[cfg(test)]
mod tests {
    use longbridge::DcRegion;

    use super::*;

    #[test]
    fn dc_restriction_names_the_command_and_says_not_to_retry() {
        let err = anyhow::Error::from(HttpClientError::DcRegionRestricted {
            path: "/v1/account/deposits".to_string(),
            required: DcRegion::Ap,
            current: DcRegion::Us,
        });
        assert_eq!(
            ap_only_error("deposits", err).to_string(),
            "`longbridge deposits` is not available for accounts in the US data center. \
             There is no US equivalent; do not retry.",
            "US rejection replaces the SDK message"
        );
        let wrapped = anyhow::Error::from(longbridge::Error::HttpClient(
            HttpClientError::DcRegionRestricted {
                path: "/v1/account/cards".to_string(),
                required: DcRegion::Ap,
                current: DcRegion::Us,
            },
        ));
        assert!(
            ap_only_error("coupon list", wrapped)
                .to_string()
                .starts_with("`longbridge coupon list` is not available"),
            "an SDK-wrapped restriction is recognised too"
        );
    }

    #[test]
    fn other_errors_pass_through() {
        let err = ap_only_error("bank-cards", anyhow::anyhow!("boom"));
        assert_eq!(err.to_string(), "boom", "unrelated error unchanged");
    }
}
