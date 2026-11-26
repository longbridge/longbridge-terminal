//! Response shaping for the account money-movement commands (bank cards,
//! eDDA, stock transfers, deposits, withdrawals), shared with the
//! reward-center commands.
//!
//! The data comes from the SDK's typed `AssetContext` account APIs, which
//! already drop the sensitive upstream fields (holder names, addresses,
//! contact people, document keys, receipt images) and decode placeholder
//! dates as unset. The JSON these commands print is read by AI agents as well
//! as people, so it mirrors the Longbridge MCP server's account tools field
//! for field: account numbers cut to their last four digits, status codes
//! named, and every time rendered in Hong Kong time (`+08:00`).

use anyhow::{bail, Result};
use longbridge::asset::{
    AuthorizationStatus, BankCard, BankCardAuthorization, BankCardBindState, BankCardStatus,
    BankCardWitnessState, Deposit, DepositState, EddaApplication, StockTransferIn,
    StockTransferInResponse, StockTransferInState, StockTransferOut, StockTransferOutState,
    TransferStock, TransferStockState, Withdrawal, WithdrawalState, WithdrawalsResponse,
};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use time::{format_description::well_known::Rfc3339, Date, OffsetDateTime, Time};

/// eDDA flows in a card's `auth_list`: bank-side and in-app eDDA.
const EDDA_FLOWS: &[&str] = &["hk_edda", "hk_sdda"];

/// Deposit `--states` codes and their names, as listed in the error message.
const DEPOSIT_STATE_HELP: &str =
    "0 processing, 1 credited, 2 failed, 3 cancelled, 4 invalidated, 5 rejected";

/// Transfer-in `--state` codes, as listed in the error message.
const TRANSFER_IN_STATE_HELP: &str =
    "0 submitted, 1 cancelled, 3 completed, 5 processing, 7 partially completed, 8 failed";

/// Transfer-out `--state` codes, as listed in the error message.
const TRANSFER_OUT_STATE_HELP: &str = "0 submitted, 1 processing, 2 executing, 3 completed, \
     4 cancelled, 5 partially completed, 6 failed, 7 rejected";

/// A time as RFC3339 in Hong Kong time (`2026-11-25T17:59:06+08:00`):
/// AP-account users read dates as Hong Kong days. `null` when unset.
pub(crate) fn hk_time(time: Option<OffsetDateTime>) -> Value {
    time.and_then(|t| t.to_offset(time::macros::offset!(+8)).format(&Rfc3339).ok())
        .map_or(Value::Null, Value::String)
}

/// A decimal as its text (`"38.5"`), `null` when unset.
pub(crate) fn decimal(value: Option<Decimal>) -> Value {
    value.map_or(Value::Null, |d| Value::String(d.to_string()))
}

/// The server's raw code of a status, `null` when it sent none.
pub(crate) fn code(raw: Option<i32>) -> Value {
    raw.map_or(Value::Null, Value::from)
}

/// The raw code when there is no label for it, so an undocumented code is
/// still shown; `null` when the label carries it.
fn code_if_unlabelled(name: Option<&'static str>, raw: Option<i32>) -> Value {
    if name.is_some() {
        Value::Null
    } else {
        code(raw)
    }
}

/// A label, `null` when there is none.
pub(crate) fn label(name: Option<&'static str>) -> Value {
    name.map_or(Value::Null, Value::from)
}

/// Remove `null` and empty-string entries at any depth.
fn strip_empty(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|_, v| !(v.is_null() || matches!(v, Value::String(s) if s.is_empty())));
            map.values_mut().for_each(strip_empty);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_empty),
        _ => {}
    }
}

/// Final pass shared by every account command: drop unset (`null`) and empty
/// values.
pub(crate) fn finish(mut value: Value) -> Value {
    strip_empty(&mut value);
    value
}

/// Parse the start of a date range: RFC3339, or `yyyy-mm-dd` read as the
/// start of that day in Hong Kong (UTC+8).
pub(crate) fn parse_hong_kong_start(s: &str) -> Result<OffsetDateTime> {
    parse_hong_kong(s, Time::MIDNIGHT)
}

/// Parse the end of a date range: RFC3339, or `yyyy-mm-dd` read as the end of
/// that day (23:59:59) in Hong Kong.
pub(crate) fn parse_hong_kong_end(s: &str) -> Result<OffsetDateTime> {
    parse_hong_kong(s, time::macros::time!(23:59:59))
}

fn parse_hong_kong(s: &str, time_of_day: Time) -> Result<OffsetDateTime> {
    let s = s.trim();
    if let Ok(t) = OffsetDateTime::parse(s, &Rfc3339) {
        return Ok(t);
    }
    let format = time::macros::format_description!("[year]-[month]-[day]");
    match Date::parse(s, format) {
        Ok(date) => Ok(date
            .with_time(time_of_day)
            .assume_offset(time::macros::offset!(+8))),
        Err(_) => bail!(
            "invalid date '{s}': expected a date such as 2026-09-01 (Hong Kong time) \
             or RFC3339 such as 2026-09-01T00:00:00+08:00"
        ),
    }
}

/// Bounds for optional `--start` / `--end` filters.
pub(crate) fn date_range(
    start: Option<&str>,
    end: Option<&str>,
) -> Result<(Option<OffsetDateTime>, Option<OffsetDateTime>)> {
    let start = start.map(parse_hong_kong_start).transpose()?;
    let end = end.map(parse_hong_kong_end).transpose()?;
    Ok((start, end))
}

/// Whole days between two times (`to` defaults to now), never negative.
fn days_between(from: OffsetDateTime, to: Option<OffsetDateTime>) -> i64 {
    (to.unwrap_or_else(OffsetDateTime::now_utc) - from)
        .whole_days()
        .max(0)
}

/// Keep only the last four characters of an account number.
pub(crate) fn mask_account(account: &str) -> String {
    let chars: Vec<char> = account.chars().filter(|c| !c.is_whitespace()).collect();
    let tail: String = chars[chars.len().saturating_sub(4)..].iter().collect();
    format!("****{tail}")
}

/// A masked account number, `null` when there is none.
fn masked(account: &str) -> Value {
    if account.trim().is_empty() {
        Value::Null
    } else {
        Value::String(mask_account(account))
    }
}

fn card_status_label(status: BankCardStatus) -> Option<&'static str> {
    match status {
        BankCardStatus::Unverified => Some("unverified"),
        BankCardStatus::UnderReview => Some("under_review"),
        BankCardStatus::Verified => Some("verified"),
        BankCardStatus::Unknown => None,
    }
}

fn witness_label(state: BankCardWitnessState) -> Option<&'static str> {
    match state {
        BankCardWitnessState::NotRequired => Some("not_required"),
        BankCardWitnessState::Pending => Some("pending"),
        BankCardWitnessState::Witnessed => Some("witnessed"),
        BankCardWitnessState::Unknown => None,
    }
}

fn binding_label(state: BankCardBindState) -> Option<&'static str> {
    match state {
        BankCardBindState::Binding => Some("binding"),
        BankCardBindState::Bound => Some("bound"),
        BankCardBindState::Failed => Some("failed"),
        BankCardBindState::Unknown => None,
    }
}

fn authorization_label(status: AuthorizationStatus) -> Option<&'static str> {
    match status {
        AuthorizationStatus::Unauthorized => Some("unauthorized"),
        AuthorizationStatus::Authorized => Some("authorized"),
        AuthorizationStatus::Authorizing => Some("authorizing"),
        AuthorizationStatus::Failed => Some("failed"),
        AuthorizationStatus::Unknown => None,
    }
}

fn transfer_in_label(state: StockTransferInState) -> Option<&'static str> {
    match state {
        StockTransferInState::Submitted => Some("submitted"),
        StockTransferInState::Cancelled => Some("cancelled"),
        StockTransferInState::Completed => Some("completed"),
        StockTransferInState::Processing => Some("processing"),
        StockTransferInState::PartiallyCompleted => Some("partially_completed"),
        StockTransferInState::Failed => Some("failed"),
        StockTransferInState::Unknown => None,
    }
}

fn transfer_out_label(state: StockTransferOutState) -> Option<&'static str> {
    match state {
        StockTransferOutState::Submitted => Some("submitted"),
        StockTransferOutState::Processing => Some("processing"),
        StockTransferOutState::Executing => Some("executing"),
        StockTransferOutState::Completed => Some("completed"),
        StockTransferOutState::Cancelled => Some("cancelled"),
        StockTransferOutState::PartiallyCompleted => Some("partially_completed"),
        // The app shows 6 as failed; follow what the customer sees.
        StockTransferOutState::Failed => Some("failed"),
        StockTransferOutState::Rejected => Some("rejected"),
        StockTransferOutState::Unknown => None,
    }
}

fn transfer_stock_label(state: TransferStockState) -> Option<&'static str> {
    match state {
        TransferStockState::Processing => Some("processing"),
        TransferStockState::Succeeded => Some("succeeded"),
        TransferStockState::Failed => Some("failed"),
        TransferStockState::Unknown => None,
    }
}

fn deposit_label(state: DepositState) -> Option<&'static str> {
    match state {
        DepositState::Processing => Some("processing"),
        DepositState::Credited => Some("credited"),
        DepositState::Failed => Some("failed"),
        DepositState::Cancelled => Some("cancelled"),
        DepositState::Invalidated => Some("invalidated"),
        DepositState::Rejected => Some("rejected"),
        DepositState::Unknown => None,
    }
}

/// Withdrawal states, as the app's withdrawal records read them.
fn withdrawal_label(state: WithdrawalState) -> Option<&'static str> {
    match state {
        WithdrawalState::Submitted => Some("submitted"),
        WithdrawalState::Processing => Some("processing"),
        WithdrawalState::Rejected => Some("rejected"),
        WithdrawalState::Cancelled => Some("cancelled"),
        WithdrawalState::Remitted => Some("remitted"),
        WithdrawalState::Failed => Some("failed"),
        WithdrawalState::Refunded => Some("refunded"),
        WithdrawalState::ApprovedAwaitingPayout => Some("approved_awaiting_payout"),
        WithdrawalState::LargeAmountReview => Some("large_amount_review"),
        WithdrawalState::ChangePendingApproval => Some("change_pending_approval"),
        WithdrawalState::DeletionPendingApproval => Some("deletion_pending_approval"),
        WithdrawalState::Deleted => Some("deleted"),
        WithdrawalState::RejectionPendingApproval => Some("rejection_pending_approval"),
        WithdrawalState::AwaitingConfirmation => Some("awaiting_confirmation"),
        WithdrawalState::Unknown => None,
    }
}

/// One authorization of a bank card, with its status named and its unset
/// limits and expiry left out.
fn shape_auth(auth: &BankCardAuthorization) -> Value {
    // A long-term authorization never expires, whatever date it carries.
    let expiry = if auth.long_term_valid {
        None
    } else {
        auth.expiry_date
    };
    json!({
        "flow_id": auth.flow_id,
        "name": auth.name,
        "currency": auth.currency,
        "authorization": label(authorization_label(auth.status)),
        "status": code_if_unlabelled(authorization_label(auth.status), auth.status_raw),
        "failure_reason": auth.message,
        "is_support": auth.is_support,
        "max_amount": decimal(auth.max_amount),
        "max_amount_unlimited": auth.max_amount_unlimited,
        "daily_max_count": auth.daily_max_count,
        "daily_max_count_unlimited": auth.daily_max_count_unlimited,
        "expiry_date": hk_time(expiry),
        "long_term_valid": auth.long_term_valid,
        "doc_reviewing": auth.doc_reviewing,
        "no_prior_deposit_reviewing": auth.no_prior_deposit_reviewing,
    })
}

/// One bank card: account number masked, status codes named, and the days
/// spent under review counted.
pub(crate) fn shape_card(card: &BankCard) -> Value {
    // Days a card has been waiting for manual review since it was added.
    let review_days = match (card.status, card.created_at) {
        (BankCardStatus::UnderReview, Some(created)) => Value::from(days_between(created, None)),
        _ => Value::Null,
    };
    json!({
        "id": card.id,
        "bank": card.bank,
        "bank_en": card.bank_en,
        "bank_code": card.bank_code,
        "account": masked(&card.account),
        "account_type": card.account_type,
        "region": card.region,
        "region_name": card.region_name,
        "country": card.country,
        "verification": label(card_status_label(card.status)),
        "status": code_if_unlabelled(card_status_label(card.status), card.status_raw),
        "review_days": review_days,
        "witness": label(witness_label(card.witness_state)),
        "witness_state": code_if_unlabelled(witness_label(card.witness_state), card.witness_state_raw),
        "binding": label(binding_label(card.bind_state)),
        "bind_state": code_if_unlabelled(binding_label(card.bind_state), card.bind_state_raw),
        "bind_tip": card.bind_tip,
        "remark": card.remark,
        "created_at": hk_time(card.created_at),
        "auth_list": card.auth_list.iter().map(shape_auth).collect::<Vec<_>>(),
    })
}

/// `{cards[]}` output of `bank-cards`.
pub(crate) fn bank_cards_output(cards: &[BankCard]) -> Value {
    finish(json!({ "cards": cards.iter().map(shape_card).collect::<Vec<_>>() }))
}

/// One eDDA authorization row: the card it belongs to plus the auth entry.
fn edda_row(card: &BankCard, auth: &BankCardAuthorization) -> Value {
    let shaped = shape_auth(auth);
    let field = |key: &str| shaped.get(key).cloned().unwrap_or(Value::Null);
    json!({
        "card_id": card.id,
        "bank": card.bank,
        "bank_en": card.bank_en,
        "account": masked(&card.account),
        "card_binding": label(binding_label(card.bind_state)),
        "flow": field("flow_id"),
        "name": field("name"),
        "currency": field("currency"),
        "authorization": field("authorization"),
        "status": field("status"),
        "failure_reason": field("failure_reason"),
        "max_amount": field("max_amount"),
        "max_amount_unlimited": field("max_amount_unlimited"),
        "daily_max_count": field("daily_max_count"),
        "daily_max_count_unlimited": field("daily_max_count_unlimited"),
        "expiry_date": field("expiry_date"),
        "long_term_valid": field("long_term_valid"),
        "is_support": field("is_support"),
        "doc_reviewing": field("doc_reviewing"),
        "no_prior_deposit_reviewing": field("no_prior_deposit_reviewing"),
    })
}

/// `{authorizations[]}` output of `edda list`: the eDDA entries of every
/// card.
pub(crate) fn edda_output(cards: &[BankCard]) -> Value {
    let rows: Vec<Value> = cards
        .iter()
        .flat_map(|card| {
            card.auth_list
                .iter()
                .filter(|auth| EDDA_FLOWS.contains(&auth.flow_id.as_str()))
                .map(move |auth| edda_row(card, auth))
        })
        .collect();
    finish(json!({ "authorizations": rows }))
}

/// Validate an `edda status` card id: the numeric id from `bank-cards`.
pub(crate) fn check_bank_card_id(card_id: &str) -> Result<()> {
    if card_id.is_empty() || !card_id.chars().all(|c| c.is_ascii_digit()) {
        bail!("invalid card id '{card_id}': expected the numeric id from bank-cards (cards[].id)");
    }
    Ok(())
}

/// `{applications[]}` output of `edda status`.
pub(crate) fn shape_edda_status(applications: &[EddaApplication]) -> Value {
    let applications: Vec<Value> = applications
        .iter()
        .map(|app| {
            json!({
                "id": app.id,
                "flow_id": app.flow_id,
                "name": app.name,
                "currency": app.currency,
                "authorization": label(authorization_label(app.status)),
                "status": code_if_unlabelled(authorization_label(app.status), app.status_raw),
                "failure_reason": app.message,
                "created_at": hk_time(app.created_at),
                "updated_at": hk_time(app.updated_at),
            })
        })
        .collect();
    finish(json!({ "applications": applications }))
}

/// The fields a transfer-in and a transfer-out record share, read off either.
struct TransferRecord<'a> {
    id: &'a str,
    order_id: &'a str,
    market: &'a str,
    broker: &'a str,
    broker_en: &'a str,
    broker_code: &'a str,
    account: &'a str,
    state: &'a str,
    state_code: Value,
    state_label: Option<&'static str>,
    /// Cancelled, rejected, failed or (partially) completed: the elapsed days
    /// stop at the end instead of running on to today.
    finished: bool,
    /// Transfers in only: on transfer-out records the server's `can_modify`
    /// is 0 even for a cancellable submitted one, so `state_code` 0 decides
    /// there instead.
    can_modify: Option<bool>,
    app_state: &'a str,
    user_notified_broker: bool,
    created_at: Option<OffsetDateTime>,
    updated_at: Option<OffsetDateTime>,
    process_at: Option<OffsetDateTime>,
    exec_at: Option<OffsetDateTime>,
    agree_at: Option<OffsetDateTime>,
    refuse_at: Option<OffsetDateTime>,
    cancel_at: Option<OffsetDateTime>,
    remark: &'a str,
    reason: &'a str,
    stocks: &'a [TransferStock],
}

impl<'a> TransferRecord<'a> {
    fn transfer_in(record: &'a StockTransferIn) -> Self {
        Self {
            id: &record.id,
            order_id: &record.order_id,
            market: &record.market,
            broker: &record.broker,
            broker_en: &record.broker_en,
            broker_code: &record.broker_code,
            account: &record.account,
            state: &record.state,
            state_code: code(record.state_code_raw),
            state_label: transfer_in_label(record.state_code),
            finished: matches!(
                record.state_code,
                StockTransferInState::Cancelled
                    | StockTransferInState::Failed
                    | StockTransferInState::Completed
                    | StockTransferInState::PartiallyCompleted
            ),
            can_modify: Some(record.can_modify),
            app_state: &record.app_state,
            user_notified_broker: record.user_notified_broker(),
            created_at: record.created_at,
            updated_at: record.updated_at,
            process_at: record.process_at,
            exec_at: record.exec_at,
            agree_at: None,
            refuse_at: None,
            cancel_at: None,
            remark: &record.remark,
            reason: &record.reason,
            stocks: &record.stocks,
        }
    }

    fn transfer_out(record: &'a StockTransferOut) -> Self {
        Self {
            id: &record.id,
            order_id: &record.order_id,
            market: &record.market,
            broker: &record.broker,
            broker_en: &record.broker_en,
            broker_code: &record.broker_code,
            account: &record.account,
            state: &record.state,
            state_code: code(record.state_code_raw),
            state_label: transfer_out_label(record.state_code),
            finished: matches!(
                record.state_code,
                StockTransferOutState::Cancelled
                    | StockTransferOutState::Failed
                    | StockTransferOutState::Rejected
                    | StockTransferOutState::Completed
                    | StockTransferOutState::PartiallyCompleted
            ),
            can_modify: None,
            app_state: &record.app_state,
            user_notified_broker: record.user_notified_broker(),
            created_at: record.created_at,
            updated_at: record.updated_at,
            process_at: record.process_at,
            exec_at: record.exec_at,
            agree_at: record.agree_at,
            refuse_at: record.refuse_at,
            cancel_at: record.cancel_at,
            remark: &record.remark,
            reason: &record.reason,
            stocks: &record.stocks,
        }
    }

    /// Days from creation until execution; for a finished one without an
    /// execution date until it ended (no count when that is unknown too); for
    /// an open one until now.
    fn days_in_process(&self) -> Option<i64> {
        let until = self.exec_at.or_else(|| {
            if self.finished {
                self.cancel_at.or(self.refuse_at).or(self.updated_at)
            } else {
                None
            }
        });
        if self.finished && until.is_none() {
            return None;
        }
        self.created_at.map(|created| days_between(created, until))
    }

    fn to_value(&self) -> Value {
        let stocks: Vec<Value> = self
            .stocks
            .iter()
            .map(|stock| {
                json!({
                    "code": stock.code,
                    "name": stock.name,
                    "name_en": stock.name_en,
                    "market": stock.market,
                    "counter_id": stock.counter_id,
                    "amount": decimal(stock.amount),
                    "currency": stock.currency,
                    "state": code(stock.state_code_raw),
                    "state_label": label(transfer_stock_label(stock.state_code)),
                })
            })
            .collect();
        // `noticed`: the user tapped "I have notified the broker" in the app.
        let notified = if self.app_state.is_empty() {
            Value::Null
        } else {
            Value::Bool(self.user_notified_broker)
        };
        json!({
            "id": self.id,
            "order_id": self.order_id,
            "market": self.market,
            "broker": self.broker,
            "broker_en": self.broker_en,
            "code": self.broker_code,
            // The customer's account number at the other broker.
            "account": masked(self.account),
            "state": self.state,
            "state_code": self.state_code,
            "state_label": label(self.state_label),
            "can_modify": self.can_modify.map(i32::from),
            "app_state": self.app_state,
            "user_notified_broker": notified,
            "stocks": stocks,
            "created_at": hk_time(self.created_at),
            "updated_at": hk_time(self.updated_at),
            "process_at": hk_time(self.process_at),
            "exec_at": hk_time(self.exec_at),
            "agree_at": hk_time(self.agree_at),
            "refuse_at": hk_time(self.refuse_at),
            "cancel_at": hk_time(self.cancel_at),
            "days_in_process": self.days_in_process(),
            "remark": self.remark,
            "reason": self.reason,
        })
    }
}

/// `{records[], total}` output of `stock-transfer in`.
pub(crate) fn shape_transfers_in(resp: &StockTransferInResponse) -> Value {
    let records: Vec<Value> = resp
        .records
        .iter()
        .map(|r| TransferRecord::transfer_in(r).to_value())
        .collect();
    finish(json!({ "records": records, "total": resp.total }))
}

/// `{records[]}` output of `stock-transfer out` (the list has no total).
pub(crate) fn shape_transfers_out(records: &[StockTransferOut]) -> Value {
    let records: Vec<Value> = records
        .iter()
        .map(|r| TransferRecord::transfer_out(r).to_value())
        .collect();
    finish(json!({ "records": records }))
}

/// Validate a `stock-transfer in --state` code.
pub(crate) fn transfer_in_state(code: i32) -> Result<StockTransferInState> {
    match StockTransferInState::from(code) {
        StockTransferInState::Unknown => {
            bail!("invalid --state {code}: expected one of {TRANSFER_IN_STATE_HELP}")
        }
        state => Ok(state),
    }
}

/// Validate a `stock-transfer out --state` code.
pub(crate) fn transfer_out_state(code: i32) -> Result<StockTransferOutState> {
    match StockTransferOutState::from(code) {
        StockTransferOutState::Unknown => {
            bail!("invalid --state {code}: expected one of {TRANSFER_OUT_STATE_HELP}")
        }
        state => Ok(state),
    }
}

/// Insert `has_more` into an output object: a full page means there may be
/// more (the deposit list's server `total` follows the page size, not the
/// record count, so this is the reliable signal).
fn with_has_more(mut value: Value, count: usize, page_size: usize) -> Value {
    if let Some(obj) = value.as_object_mut() {
        obj.insert("has_more".into(), Value::Bool(count >= page_size));
    }
    value
}

/// One withdrawal record.
fn shape_withdrawal(item: &Withdrawal) -> Value {
    // Usually a short tail like "尾号 0578"; mask a longer run of digits.
    let display = if item
        .display_bank_account
        .chars()
        .filter(char::is_ascii_digit)
        .count()
        > 4
    {
        mask_account(&item.display_bank_account)
    } else {
        item.display_bank_account.clone()
    };
    json!({
        "id": item.id,
        "order_id": item.order_id,
        "created_at": hk_time(item.created_at),
        "currency": item.currency,
        "amount": decimal(item.amount),
        "fee": decimal(item.fee),
        "settlement_amount": decimal(item.settlement_amount),
        "state": code(item.state_code_raw),
        "state_label": label(withdrawal_label(item.state_code)),
        "state_description": item.state_description,
        "bank_account": masked(&item.bank_account),
        "display_bank_account": display,
        "user_bank_swift_code": item.user_bank_swift_code,
    })
}

/// `{list[], total, has_more}` output of `withdrawals`.
pub(crate) fn shape_withdrawals(resp: &WithdrawalsResponse, page_size: usize) -> Value {
    let list: Vec<Value> = resp.list.iter().map(shape_withdrawal).collect();
    let value = json!({ "list": list, "total": resp.total });
    finish(with_has_more(value, resp.list.len(), page_size))
}

/// One deposit record.
fn shape_deposit(item: &Deposit) -> Value {
    json!({
        "id": item.id,
        "created_at": hk_time(item.created_at),
        "updated_at": hk_time(item.updated_at),
        "currency": item.currency,
        "amount": decimal(item.amount),
        "receivable_amount": decimal(item.receivable_amount),
        "state": item.state,
        "state_code": code(item.state_code_raw),
        "state_label": label(deposit_label(item.state_code)),
        "sub_state_code": item.sub_state_code,
        "bank_operation_type_name": item.bank_operation_type_name,
        "reference_no": item.reference_no,
        "can_cancel": item.can_cancel,
        "voucher_count": item.voucher_count,
    })
}

/// `{items[], has_more}` output of `deposits`. The server's `total` is capped
/// at the page size, so it would read as a wrong record count: `has_more`
/// replaces it.
pub(crate) fn shape_deposits(items: &[Deposit], page_size: usize) -> Value {
    let list: Vec<Value> = items.iter().map(shape_deposit).collect();
    finish(with_has_more(
        json!({ "items": list }),
        items.len(),
        page_size,
    ))
}

/// Parse a deposit `--states` filter: comma-separated numeric codes 0..5.
/// Upstream fails on state names, so they are rejected here with the codes.
pub(crate) fn check_deposit_states(states: &str) -> Result<Vec<DepositState>> {
    let parsed: Option<Vec<DepositState>> = states
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|c| {
            c.parse::<i32>()
                .ok()
                .map(DepositState::from)
                .filter(|s| *s != DepositState::Unknown)
        })
        .collect();
    match parsed {
        Some(codes) if !codes.is_empty() => Ok(codes),
        _ => bail!(
            "invalid --states '{states}': expected comma-separated codes ({DEPOSIT_STATE_HELP})"
        ),
    }
}

/// Split a comma-separated `--currencies` filter.
pub(crate) fn split_list(list: &str) -> Vec<String> {
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Page size sent upstream: default 20, between 1 and 50.
pub(crate) fn clamp_page_size(size: u32) -> u32 {
    size.clamp(1, 50)
}

/// Page size sent to the deposit list: between 1 and 20. The deposit list
/// honours at most 20 and returns a short 10-record page for a larger size,
/// which would read as the last page (`has_more: false`) and hide records.
pub(crate) fn clamp_deposit_page_size(size: u32) -> u32 {
    size.clamp(1, 20)
}

/// Page number sent upstream: at least 1.
pub(crate) fn page_number(page: u32) -> i32 {
    i32::try_from(page.max(1)).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode<T>(value: Value) -> T
    where
        T: serde::de::DeserializeOwned,
    {
        serde_json::from_value(value).expect("fixture decodes into the SDK type")
    }

    #[test]
    fn mask_account_keeps_last_four() {
        assert_eq!(mask_account("012 345 678901"), "****8901", "spaces ignored");
        assert_eq!(mask_account("12"), "****12", "short numbers stay whole");
    }

    #[test]
    fn finish_strips_unset_values() {
        let value = finish(json!({"a": 1, "b": "", "c": null, "d": [{"e": ""}]}));
        assert_eq!(value, json!({"a": 1, "d": [{}]}), "empties gone");
    }

    #[test]
    fn times_render_in_hong_kong() {
        let t = OffsetDateTime::from_unix_timestamp(1_764_064_746).expect("valid timestamp");
        assert_eq!(
            hk_time(Some(t)),
            "2025-11-25T17:59:06+08:00",
            "Hong Kong time"
        );
        assert!(hk_time(None).is_null(), "unset time is null");
    }

    #[test]
    fn shape_card_masks_labels_and_drops_personal_data() {
        // Shape of a real canary response, 2026-11-25.
        let cards: Vec<BankCard> = decode(json!([{
            "id": "434810", "bank_id": "79", "account_type": "ALL",
            "account": "266382841694216", "region": "HONGKONG", "bank": "389 MOX Bank",
            "region_name": "香港地区银行", "witness_state": 1, "status": 1, "bind_state": 1,
            "bind_tip": "pending review", "country": "Hong Kong", "country_display": "HK",
            "picture_urls": "[\"id/abc\"]", "payee_street": "Nathan Road",
            "payee_postcode": "000000", "name": "CHAN TAI MAN", "created_at": "1577836800",
            "auth_list": [{
                "flow_id": "hk_edda", "name": "eDDA", "status": 3, "message": "bank rejected",
                "custom": "789", "id": "0", "created_at": "0", "need_sms": 0,
                "daily_max_count": "0", "max_amount_unlimited": false,
                "expiry_date": 253_402_214_400_i64, "long_term_valid": true,
            }, {
                "flow_id": "hk_fps", "status": 1, "expiry_date": "1798732800",
                "long_term_valid": false, "max_amount": "0",
            }],
        }]));
        let out = bank_cards_output(&cards);
        let card = &out["cards"][0];
        assert_eq!(card["account"], "****4216", "account masked");
        assert_eq!(card["country"], "Hong Kong", "country passes through");
        assert_eq!(card["verification"], "under_review", "status label");
        assert_eq!(card["witness"], "not_required", "witness label");
        assert_eq!(card["binding"], "bound", "bind label");
        assert_eq!(
            card["created_at"], "2020-01-01T08:00:00+08:00",
            "Hong Kong time"
        );
        assert!(
            card["review_days"].as_i64().is_some_and(|d| d > 14),
            "review days counted"
        );
        for key in [
            "picture_urls",
            "payee_street",
            "payee_postcode",
            "name",
            "bank_id",
            "status",
            "witness_state",
            "bind_state",
            "country_display",
        ] {
            assert!(card.get(key).is_none(), "card field {key} must be dropped");
        }
        let auth = &card["auth_list"][0];
        assert_eq!(auth["authorization"], "failed", "auth status label");
        assert_eq!(auth["failure_reason"], "bank rejected", "message renamed");
        assert!(
            auth.get("expiry_date").is_none(),
            "long-term sentinel dropped"
        );
        for key in [
            "custom",
            "id",
            "created_at",
            "need_sms",
            "daily_max_count",
            "status",
        ] {
            assert!(auth.get(key).is_none(), "auth field {key} must be dropped");
        }
        assert_eq!(auth["max_amount_unlimited"], false, "limit flags kept");
        let fps = &card["auth_list"][1];
        assert_eq!(
            fps["expiry_date"], "2027-01-01T00:00:00+08:00",
            "real expiry converted"
        );
        assert!(fps.get("max_amount").is_none(), "zero max_amount dropped");
    }

    #[test]
    fn verified_card_has_no_review_days() {
        let card: BankCard = decode(json!({"id": 2, "status": 2, "created_at": 1_577_836_800}));
        assert!(
            shape_card(&card)
                .get("review_days")
                .is_none_or(Value::is_null),
            "only cards under review"
        );
    }

    #[test]
    fn edda_rows_take_card_context_and_skip_other_flows() {
        let cards: Vec<BankCard> = decode(json!([{
            "id": 7, "bank": "HSBC", "account": "123456789", "bind_state": 1,
            "auth_list": [
                {"flow_id": "hk_edda", "currency": "HKD", "status": 1, "doc_reviewing": true},
                {"flow_id": "hk_fps", "currency": "HKD", "status": 1},
            ],
        }]));
        let out = edda_output(&cards);
        let rows = out["authorizations"].as_array().expect("rows");
        assert_eq!(rows.len(), 1, "FPS is not eDDA");
        assert_eq!(rows[0]["card_id"], "7", "card id carried over");
        assert_eq!(
            rows[0]["account"], "****6789",
            "masked account carried over"
        );
        assert_eq!(
            rows[0]["card_binding"], "bound",
            "card binding carried over"
        );
        assert_eq!(rows[0]["flow"], "hk_edda", "flow renamed");
        assert_eq!(
            rows[0]["authorization"], "authorized",
            "auth label carried over"
        );
        assert_eq!(rows[0]["doc_reviewing"], true, "review flags carried over");
        assert!(rows[0].get("failure_reason").is_none(), "empties stripped");
    }

    #[test]
    fn shape_edda_status_labels_applications() {
        let apps: Vec<EddaApplication> = decode(json!([{"id": 1, "status": 3, "message": "x",
            "flow_id": "hk_edda", "created_at": 1_764_064_746, "extra": "y"}]));
        let listed = shape_edda_status(&apps);
        let app = &listed["applications"][0];
        assert_eq!(app["authorization"], "failed", "labelled");
        assert_eq!(app["failure_reason"], "x", "message renamed");
        assert_eq!(app["flow_id"], "hk_edda", "flow kept");
        assert_eq!(
            app["created_at"], "2025-11-25T17:59:06+08:00",
            "Hong Kong time"
        );
        assert!(app.get("extra").is_none(), "unknown field dropped");
        assert!(app.get("status").is_none(), "raw status dropped");
        assert_eq!(
            shape_edda_status(&[])["applications"],
            json!([]),
            "empty stays empty"
        );
        assert!(check_bank_card_id("434810").is_ok(), "numeric id accepted");
        assert!(
            check_bank_card_id("bd6b40914cd411f08becae6ae6fae931").is_err(),
            "coupon id rejected"
        );
    }

    #[test]
    fn shape_transfers_cleans_a_real_record() {
        // Shape of a real canary transfer-in record, 2026-11-25.
        let resp: StockTransferInResponse = decode(json!({"records": [{
            "id": "7053", "market_symbol": "US", "broker_id": "9", "account": "11222333444555",
            "name": "CHAN TAI MAN", "contact_phone": "12345678", "user_email": "a@b.c",
            "attachments": "[\"ai/x\"]",
            "created_at": 1_795_600_746, "exec_at": 0, "cancel_at": -2_005_000_000,
            "process_at": "0", "app_state": "default", "state_code": 0, "state": "已提交",
            "can_modify": 1, "cash": [],
            "stocks": [{"code": "AAPL", "name": "苹果", "amount": "100", "state": 0,
                "counter_id": "ST/US/AAPL", "usi_attachments": ["k"], "is_reality": 0,
                "street_amount": "0"}],
        }], "total": 1}));
        let out = shape_transfers_in(&resp);
        let record = &out["records"][0];
        assert_eq!(out["total"], 1, "total kept");
        assert_eq!(record["account"], "****4555", "other-broker account masked");
        assert_eq!(
            record["created_at"], "2026-11-25T17:59:06+08:00",
            "Hong Kong time"
        );
        for key in [
            "exec_at",
            "cancel_at",
            "process_at",
            "cash",
            "market_symbol",
            "broker_id",
            "name",
            "contact_phone",
            "user_email",
            "attachments",
        ] {
            assert!(record.get(key).is_none(), "{key} dropped");
        }
        assert!(
            record["days_in_process"].as_i64().is_some_and(|d| d >= 0),
            "open: counted"
        );
        assert_eq!(
            record["user_notified_broker"], false,
            "default: not notified"
        );
        assert_eq!(record["state_label"], "submitted", "state labelled");
        assert_eq!(record["state_code"], 0, "state code kept");
        assert_eq!(record["can_modify"], 1, "transfer-in keeps can_modify");
        let stock = &record["stocks"][0];
        assert!(stock.get("usi_attachments").is_none(), "proof keys dropped");
        assert!(stock.get("street_amount").is_none(), "bookkeeping dropped");
        assert_eq!(stock["name"], "苹果", "stock name kept");
        assert_eq!(stock["amount"], "100", "quantity as text");
        assert_eq!(
            stock["counter_id"], "ST/US/AAPL",
            "counter id kept as in the MCP"
        );
        assert_eq!(
            stock["state_label"], "processing",
            "per-stock state labelled"
        );
    }

    #[test]
    fn shape_transfer_out_names_the_broker_like_transfer_in() {
        let records: Vec<StockTransferOut> =
            decode(json!([{"name_en": "Futu Securities", "state_code": 0, "can_modify": 0}]));
        let out = shape_transfers_out(&records);
        let record = &out["records"][0];
        assert_eq!(record["broker_en"], "Futu Securities", "renamed");
        assert!(record.get("name_en").is_none(), "old name gone");
        assert!(
            record.get("can_modify").is_none(),
            "transfer-out drops can_modify"
        );
        assert!(out.get("total").is_none(), "transfer-out has no total");
    }

    #[test]
    fn days_in_process_stops_the_clock_at_the_end() {
        let shape_in = |record: Value| {
            let resp: StockTransferInResponse = decode(json!({"records": [record]}));
            shape_transfers_in(&resp)["records"][0].clone()
        };
        let shape_out = |record: Value| {
            let records: Vec<StockTransferOut> = decode(json!([record]));
            shape_transfers_out(&records)["records"][0].clone()
        };
        let cancelled = shape_in(
            json!({"state_code": 1, "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-11T00:00:00Z"}),
        );
        assert_eq!(
            cancelled["days_in_process"], 10,
            "cancelled: until the last update"
        );

        let done = shape_in(
            json!({"state_code": 3, "created_at": "2024-01-01T00:00:00Z",
            "exec_at": "2024-01-04T00:00:00Z", "app_state": "noticed"}),
        );
        assert_eq!(done["days_in_process"], 3, "executed: until execution");
        assert_eq!(done["user_notified_broker"], true, "notified flag");

        let no_exec = shape_in(
            json!({"state_code": 3, "created_at": "2024-01-01T00:00:00Z",
            "exec_at": "0", "updated_at": "2024-01-06T00:00:00Z"}),
        );
        assert!(
            no_exec.get("exec_at").is_none(),
            "raw \"0\" placeholder dropped"
        );
        assert_eq!(
            no_exec["days_in_process"], 5,
            "completed: until the last update"
        );

        let rejected = shape_out(json!({"state_code": 7, "created_at": "2024-01-01T00:00:00Z"}));
        assert!(
            rejected.get("days_in_process").is_none(),
            "no end date: no count"
        );

        let open = shape_out(json!({"state_code": 1, "created_at": "2024-01-01T00:00:00Z"}));
        assert!(
            open["days_in_process"].as_i64().is_some_and(|d| d > 300),
            "open: until now"
        );
    }

    #[test]
    fn undocumented_codes_are_shown_raw() {
        let cards: Vec<BankCard> = decode(json!([{"id": 1, "status": 9, "witness_state": 1,
            "bind_state": 7, "auth_list": [{"flow_id": "hk_edda", "status": 8},
                {"flow_id": "hk_sdda", "status": 1}]}]));
        let card = &bank_cards_output(&cards)["cards"][0];
        assert_eq!(card["status"], 9, "unknown card status kept raw");
        assert!(card.get("verification").is_none(), "no label");
        assert!(
            card.get("witness_state").is_none(),
            "labelled code not repeated"
        );
        assert_eq!(card["bind_state"], 7, "unknown bind state kept raw");
        assert_eq!(card["auth_list"][0]["status"], 8, "unknown auth status raw");
        assert!(
            card["auth_list"][1].get("status").is_none(),
            "labelled auth"
        );
        let rows = edda_output(&cards);
        assert_eq!(rows["authorizations"][0]["status"], 8, "eDDA row too");
        let apps: Vec<EddaApplication> = decode(json!([{"id": 1, "status": 6}]));
        assert_eq!(shape_edda_status(&apps)["applications"][0]["status"], 6);
        let resp: WithdrawalsResponse = decode(json!({"list": [{"id": 1, "state": 42}]}));
        let item = &shape_withdrawals(&resp, 20)["list"][0];
        assert_eq!(item["state"], 42, "raw withdrawal state");
        assert!(item.get("state_label").is_none(), "unlabelled");
        let items: Vec<Deposit> = decode(json!([{"id": 1, "state_code": 9}]));
        assert_eq!(shape_deposits(&items, 20)["items"][0]["state_code"], 9);
        let transfers: StockTransferInResponse =
            decode(json!({"records": [{"state_code": 11, "stocks": [{"state": 6}]}]}));
        let record = &shape_transfers_in(&transfers)["records"][0];
        assert_eq!(record["state_code"], 11, "raw transfer state");
        assert_eq!(record["stocks"][0]["state"], 6, "raw stock state");
    }

    #[test]
    fn transfer_states_are_checked_per_list() {
        assert_eq!(
            transfer_in_state(8).ok(),
            Some(StockTransferInState::Failed),
            "transfer-in failed is 8"
        );
        assert!(transfer_in_state(2).is_err(), "2 is not a transfer-in code");
        assert_eq!(
            transfer_out_state(2).ok(),
            Some(StockTransferOutState::Executing),
            "transfer-out executing is 2"
        );
        assert!(
            transfer_out_state(8).is_err(),
            "8 is not a transfer-out code"
        );
    }

    #[test]
    fn withdrawals_are_masked_and_paged() {
        let resp: WithdrawalsResponse = decode(json!({"list": [
            {"id": 1, "state": 4, "bank_account": "012345678901", "fee": "5",
             "raw_state": 4, "payee_name": "CHAN TAI MAN", "created_at": 1_764_064_746,
             "display_bank_account": "012345678901", "account_channel": "lb"},
            {"id": 2, "state": 99, "display_bank_account": "尾号 0578"},
        ], "total": 2}));
        let out = shape_withdrawals(&resp, 2);
        let item = &out["list"][0];
        assert_eq!(item["bank_account"], "****8901", "account masked");
        assert_eq!(
            item["display_bank_account"], "****8901",
            "long display masked"
        );
        assert_eq!(item["fee"], "5", "fee kept");
        assert_eq!(item["state"], 4, "state code");
        assert_eq!(item["state_label"], "remitted", "state labelled");
        assert_eq!(
            item["created_at"], "2025-11-25T17:59:06+08:00",
            "Hong Kong time"
        );
        for key in ["payee_name", "raw_state", "account_channel"] {
            assert!(item.get(key).is_none(), "{key} dropped");
        }
        assert_eq!(
            out["list"][1]["display_bank_account"], "尾号 0578",
            "tail kept"
        );
        assert!(
            out["list"][1].get("state_label").is_none(),
            "unknown code unlabelled"
        );
        assert_eq!(out["total"], 2, "withdrawal total kept");
        assert_eq!(out["has_more"], true, "full page: maybe more");
    }

    #[test]
    fn deposits_count_vouchers_and_drop_total() {
        let items: Vec<Deposit> = decode(json!([{"id": 1, "state_code": 1, "state": "已到账",
            "sub_state_code": 0, "bank_operation_type_id": 3, "disable_link": true,
            "vouchers": [{"key": "k", "img_url": "https://x/y.png"}]}]));
        let out = shape_deposits(&items, 20);
        let item = &out["items"][0];
        assert_eq!(item["voucher_count"], 1, "vouchers counted");
        assert!(item.get("vouchers").is_none(), "voucher urls dropped");
        assert!(
            item.get("bank_operation_type_id").is_none(),
            "unlisted id dropped"
        );
        assert_eq!(item["state_label"], "credited", "state labelled");
        assert!(out.get("total").is_none(), "unreliable total dropped");
        assert_eq!(out["has_more"], false, "short page: no more");
    }

    #[test]
    fn deposit_page_size_is_capped_at_20() {
        assert_eq!(clamp_deposit_page_size(50), 20, "capped");
        assert_eq!(clamp_deposit_page_size(15), 15, "kept");
        assert_eq!(clamp_deposit_page_size(0), 1, "at least one");
        let items: Vec<Deposit> = decode(json!(vec![json!({"id": 1}); 20]));
        assert_eq!(
            shape_deposits(&items, clamp_deposit_page_size(50) as usize)["has_more"],
            true,
            "a full capped page may have more"
        );
    }

    #[test]
    fn deposit_states_take_codes_only() {
        assert_eq!(
            check_deposit_states("0, 2").ok(),
            Some(vec![DepositState::Processing, DepositState::Failed]),
            "codes accepted"
        );
        assert!(check_deposit_states("Pending").is_err(), "names rejected");
        assert!(check_deposit_states("9").is_err(), "unknown code rejected");
        assert_eq!(split_list(" HKD, ,USD"), ["HKD", "USD"], "currencies split");
    }

    #[test]
    fn date_range_reads_hong_kong_days() {
        let (s, e) = date_range(Some("2026-01-01"), Some("2026-01-01")).expect("valid dates");
        assert_eq!(
            s.map(OffsetDateTime::unix_timestamp),
            Some(1_767_196_800),
            "start of day, Hong Kong"
        );
        assert_eq!(
            e.map(OffsetDateTime::unix_timestamp),
            Some(1_767_283_199),
            "end of day, Hong Kong"
        );
        assert!(
            date_range(Some("yesterday"), None).is_err(),
            "bad date rejected"
        );
        let (s, _) = date_range(Some("2026-01-01T00:00:00Z"), None).expect("rfc3339");
        assert_eq!(
            s.map(OffsetDateTime::unix_timestamp),
            Some(1_767_225_600),
            "RFC3339 kept as given"
        );
    }

    #[test]
    fn days_between_counts_whole_days() {
        let parse = |s: &str| OffsetDateTime::parse(s, &Rfc3339).expect("valid time");
        assert_eq!(
            days_between(
                parse("2026-11-01T00:00:00Z"),
                Some(parse("2026-11-04T12:00:00Z"))
            ),
            3,
            "partial days dropped"
        );
    }
}
