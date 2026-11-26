//! Response shaping for the account money-movement commands (bank cards,
//! eDDA, stock transfers, deposits, withdrawals) and the reward-center
//! commands.
//!
//! The JSON these commands print is read by AI agents as well as people, so it
//! mirrors the Longbridge MCP server's account tools field for field:
//! allowlisted fields only, account numbers cut to their last four digits,
//! status codes named, placeholder dates removed, and every time rendered in
//! Hong Kong time (`+08:00`).

use anyhow::{bail, Result};
use serde_json::{json, Map, Value};
use time::{format_description::well_known::Rfc3339, Date, OffsetDateTime, Time};

/// Plausible unix-seconds range: 2000-01-01..2100-01-01 UTC. Numbers outside it
/// are not treated as timestamps (sentinels such as `0`, ids, counts).
pub(crate) const UNIX_SECONDS_MIN: i64 = 946_684_800;
pub(crate) const UNIX_SECONDS_MAX: i64 = 4_102_444_800;

/// Bank-card fields passed through. This is an allowlist: the upstream card
/// also carries the holder's name and postal address (`payee_*`), the storage
/// keys of uploaded ID documents (`picture_urls`) and app styling.
const CARD_FIELDS: &[&str] = &[
    "id",
    "bank",
    "bank_en",
    "bank_code",
    "account",
    "account_type",
    "region",
    "region_name",
    "country",
    "country_display",
    "remark",
    "status",
    "witness_state",
    "support_witness",
    "bind_state",
    "bind_tip",
    "created_at",
    "auth_list",
];

/// Fields passed through from each entry of a card's `auth_list` (allowlist).
const AUTH_FIELDS: &[&str] = &[
    "flow_id",
    "name",
    "currency",
    "status",
    "message",
    "is_support",
    "max_amount",
    "max_amount_unlimited",
    "daily_max_count",
    "daily_max_count_unlimited",
    "expiry_date",
    "long_term_valid",
    "doc_reviewing",
    "no_prior_deposit_reviewing",
];

/// Raw card codes dropped once labelled, plus display-only fields.
const CARD_RAW_CODES: &[&str] = &[
    "status",
    "witness_state",
    "bind_state",
    "support_witness",
    "country_display",
];

/// Fields kept from an eDDA application, after labelling.
const EDDA_APPLICATION_FIELDS: &[&str] = &[
    "id",
    "flow_id",
    "name",
    "currency",
    "authorization",
    "failure_reason",
    "created_at",
    "updated_at",
];

/// Fields kept from a stock-transfer record, after shaping (allowlist).
const TRANSFER_FIELDS: &[&str] = &[
    "id",
    "order_id",
    "market",
    "broker",
    "broker_en",
    "code",
    "account",
    "state",
    "state_code",
    "state_label",
    "can_modify",
    "app_state",
    "user_notified_broker",
    "stocks",
    "created_at",
    "updated_at",
    "process_at",
    "exec_at",
    "agree_at",
    "refuse_at",
    "cancel_at",
    "days_in_process",
    "remark",
    "reason",
];

/// Fields kept for each stock in a transfer record (allowlist).
const TRANSFER_STOCK_FIELDS: &[&str] = &[
    "code",
    "name",
    "name_en",
    "market",
    "counter_id",
    "amount",
    "currency",
    "state",
    "state_label",
];

/// Fields kept from a deposit record, after shaping (allowlist).
pub(crate) const DEPOSIT_FIELDS: &[&str] = &[
    "id",
    "created_at",
    "updated_at",
    "currency",
    "amount",
    "receivable_amount",
    "state",
    "state_code",
    "state_label",
    "sub_state_code",
    "bank_operation_type_name",
    "reference_no",
    "can_cancel",
    "voucher_count",
];

/// Fields kept from a withdrawal record, after shaping (allowlist).
pub(crate) const WITHDRAWAL_FIELDS: &[&str] = &[
    "id",
    "order_id",
    "created_at",
    "currency",
    "amount",
    "fee",
    "settlement_amount",
    "state",
    "state_label",
    "state_description",
    "bank_account",
    "display_bank_account",
    "user_bank_swift_code",
];

/// Auth fields upstream fills with `"0"` / `0` when unset.
const AUTH_ZERO_UNSET: &[&str] = &["max_amount", "daily_max_count", "expiry_date"];

/// Stock-transfer fields that expose the counterparty broker's contact people
/// or the user's e-mail.
pub(crate) const TRANSFER_CONTACT_KEYS: &[&str] = &[
    "contact",
    "contact_phone",
    "contact_email",
    "user_email",
    "website",
    "email_direct",
    "is_has_attachment",
];

/// Stock-transfer timestamps that may arrive as unix-second strings.
pub(crate) const TRANSFER_TIME_PATHS: &[&str] = &[
    "records.*.process_at",
    "records.*.exec_at",
    "records.*.created_at",
    "records.*.updated_at",
    "records.*.cancel_at",
    "records.*.refuse_at",
    "records.*.agree_at",
];

const TRANSFER_IN_STATE: &[(i64, &str)] = &[
    (0, "submitted"),
    (1, "cancelled"),
    (3, "completed"),
    (5, "processing"),
    (7, "partially_completed"),
    (8, "failed"),
];
const TRANSFER_OUT_STATE: &[(i64, &str)] = &[
    (0, "submitted"),
    (1, "processing"),
    (2, "executing"),
    (3, "completed"),
    (4, "cancelled"),
    (5, "partially_completed"),
    // The app shows 6 as failed; follow what the customer sees.
    (6, "failed"),
    (7, "rejected"),
];
/// Deposit `state_code` codes.
pub(crate) const DEPOSIT_STATE: &[(i64, &str)] = &[
    (0, "processing"),
    (1, "credited"),
    (2, "failed"),
    (3, "cancelled"),
    (4, "invalidated"),
    (5, "rejected"),
];
/// Withdrawal `state` codes, as the app's withdrawal records read them.
pub(crate) const WITHDRAWAL_STATE: &[(i64, &str)] = &[
    (0, "submitted"),
    (1, "processing"),
    (2, "rejected"),
    (3, "cancelled"),
    (4, "remitted"),
    (5, "failed"),
    (6, "refunded"),
    (7, "approved_awaiting_payout"),
    (8, "large_amount_review"),
    (9, "change_pending_approval"),
    (10, "deletion_pending_approval"),
    (11, "deleted"),
    (12, "rejection_pending_approval"),
    (15, "awaiting_confirmation"),
];

/// How a stock-transfer list labels its codes and which codes are final.
pub(crate) struct TransferKind {
    pub(crate) path: &'static str,
    states: &'static [(i64, &'static str)],
    /// Cancelled / rejected / failed: no execution date, so the elapsed days
    /// stop at the last update instead of running on to today.
    terminal: &'static [i64],
    /// Completed or partially completed: the count stops at the execution
    /// date, or at the last update when that date is unset.
    completed: &'static [i64],
    /// Whether `can_modify` means "can still be cancelled or changed". On
    /// transfer-out records it is 0 even for a cancellable submitted one, so
    /// it is dropped there and `state_code` 0 decides instead.
    keeps_can_modify: bool,
}

/// Stock transfers into the account (another broker to Longbridge).
pub(crate) const TRANSFER_IN: TransferKind = TransferKind {
    path: "/v1/account/stock-transfers/in",
    states: TRANSFER_IN_STATE,
    terminal: &[1, 8],
    completed: &[3, 7],
    keeps_can_modify: true,
};
/// Stock transfers out of the account (Longbridge to another broker).
pub(crate) const TRANSFER_OUT: TransferKind = TransferKind {
    path: "/v1/account/stock-transfers/out",
    states: TRANSFER_OUT_STATE,
    terminal: &[4, 6, 7],
    completed: &[3, 5],
    keeps_can_modify: false,
};

/// Stock-transfer record fields with no use to a reader: internal position
/// bookkeeping and an empty cash list.
const TRANSFER_RECORD_NOISE: &[&str] = &["cash", "market_symbol", "broker_id"];

/// Per-stock fields of a transfer: internal bookkeeping, plus the storage
/// keys of uploaded proof documents (`usi_attachments`).
const TRANSFER_STOCK_NOISE: &[&str] = &[
    "usi_attachments",
    "is_reality",
    "street_amount",
    "nominee_amount",
    "own_amount",
    "tax_enable",
    "effect_state",
    "certificate_count",
];

/// Per-stock transfer `state`, as the app's record detail reads it.
const TRANSFER_STOCK_STATE: &[(i64, &str)] = &[(0, "processing"), (1, "succeeded"), (2, "failed")];

/// Transfer timestamps upstream fills with a placeholder when unset (0, which
/// becomes 1970, or a negative value): anything before this year is dropped.
const TRANSFER_MIN_YEAR: &str = "2000";

/// eDDA flows in a card's `auth_list`: bank-side and in-app eDDA.
const EDDA_FLOWS: &[&str] = &["hk_edda", "hk_sdda"];

const CARD_STATUS: &[(i64, &str)] = &[(0, "unverified"), (1, "under_review"), (2, "verified")];
const WITNESS_STATE: &[(i64, &str)] = &[(1, "not_required"), (2, "pending"), (3, "witnessed")];
const BIND_STATE: &[(i64, &str)] = &[(0, "binding"), (1, "bound"), (2, "failed")];
const AUTH_STATUS: &[(i64, &str)] = &[
    (0, "unauthorized"),
    (1, "authorized"),
    (2, "authorizing"),
    (3, "failed"),
];

/// Normalize an upstream response the way the MCP server reads it: `snake_case`
/// field names, `*_at` unix timestamps as RFC3339 UTC, and `aaid` /
/// `account_channel` blanked.
pub(crate) fn normalize(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, v)| {
                    // A map keyed by data (e.g. `AAPL.US`) keeps its keys.
                    if !is_field_name(&key) {
                        return (key, normalize(v));
                    }
                    let snake = to_snake_case(&key);
                    let v = if snake.ends_with("_at") {
                        normalize_timestamp(v)
                    } else if snake == "aaid" || snake == "account_channel" {
                        Value::Null
                    } else {
                        normalize(v)
                    };
                    (snake, v)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(normalize).collect()),
        Value::String(s) => Value::String(datetime_str_to_rfc3339(&s).unwrap_or(s)),
        other => other,
    }
}

/// Normalize the value of an `*_at` field: any integer becomes an RFC3339
/// date (so `0` reads 1970 and is later dropped as a placeholder), a string
/// only when it is plausible unix seconds.
fn normalize_timestamp(value: Value) -> Value {
    match value {
        Value::Number(n) => {
            if let Some(ts) = n.as_i64() {
                Value::String(timestamp_to_rfc3339(ts))
            } else if let Some(ts) = n.as_u64() {
                Value::String(
                    i64::try_from(ts).map_or_else(|_| ts.to_string(), timestamp_to_rfc3339),
                )
            } else {
                Value::Number(n)
            }
        }
        Value::String(s) => {
            match datetime_str_to_rfc3339(&s)
                .or_else(|| try_parse_unix_string(&s).map(timestamp_to_rfc3339))
            {
                Some(converted) => Value::String(converted),
                None => Value::String(s),
            }
        }
        Value::Array(items) => Value::Array(items.into_iter().map(normalize_timestamp).collect()),
        other => normalize(other),
    }
}

fn is_field_name(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn to_snake_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.extend(c.to_lowercase());
        } else {
            result.push(c);
        }
    }
    result
}

/// Unix seconds as an RFC3339 UTC string (`…Z`), or the number itself when it
/// is out of range for a date.
pub(crate) fn timestamp_to_rfc3339(ts: i64) -> String {
    OffsetDateTime::from_unix_timestamp(ts)
        .ok()
        .and_then(|dt| dt.format(&Rfc3339).ok())
        .unwrap_or_else(|| ts.to_string())
}

/// Convert `time`'s default `OffsetDateTime` text (`2026-06-02 20:00:00.0
/// +00:00:00`) into RFC3339; `None` for anything else.
fn datetime_str_to_rfc3339(s: &str) -> Option<String> {
    let b = s.as_bytes();
    if b.len() < 19 || !b[0].is_ascii_digit() || b.get(4) != Some(&b'-') || b.get(10) != Some(&b' ')
    {
        return None;
    }
    let format = time::macros::format_description!(
        "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond] [offset_hour sign:mandatory]:[offset_minute]:[offset_second]"
    );
    OffsetDateTime::parse(s, format)
        .ok()
        .and_then(|dt| dt.format(&Rfc3339).ok())
}

fn unix_seconds_in_range(n: i64) -> Option<i64> {
    (UNIX_SECONDS_MIN..=UNIX_SECONDS_MAX)
        .contains(&n)
        .then_some(n)
}

fn try_parse_unix_string(s: &str) -> Option<i64> {
    unix_seconds_in_range(s.trim().parse().ok()?)
}

/// Convert unix-seconds values at dot paths (`*` matches every array element
/// or map value) to RFC3339. Out-of-range values (`0`, far-future sentinels,
/// milliseconds) are left untouched.
pub(crate) fn convert_unix_paths(value: &mut Value, paths: &[&str]) {
    for path in paths {
        let segments: Vec<&str> = path.split('.').collect();
        walk_convert(value, &segments);
    }
}

fn walk_convert(value: &mut Value, segments: &[&str]) {
    let Some((seg, rest)) = segments.split_first() else {
        let ts = match &*value {
            Value::String(s) => try_parse_unix_string(s),
            Value::Number(n) => n.as_i64().and_then(unix_seconds_in_range),
            _ => None,
        };
        if let Some(ts) = ts {
            *value = Value::String(timestamp_to_rfc3339(ts));
        }
        return;
    };
    match value {
        Value::Object(map) => {
            if *seg == "*" {
                map.values_mut().for_each(|v| walk_convert(v, rest));
            } else if let Some(v) = map.get_mut(*seg) {
                walk_convert(v, rest);
            }
        }
        Value::Array(items) if *seg == "*" => {
            for v in items.iter_mut() {
                walk_convert(v, rest);
            }
        }
        _ => {}
    }
}

/// Remove entries whose key is in `keys`, at any depth.
pub(crate) fn drop_keys(value: &mut Value, keys: &[&str]) {
    match value {
        Value::Object(map) => {
            map.retain(|k, _| !keys.contains(&k.as_str()));
            map.values_mut().for_each(|v| drop_keys(v, keys));
        }
        Value::Array(items) => items.iter_mut().for_each(|v| drop_keys(v, keys)),
        _ => {}
    }
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

/// Drop a `counter_id` next to a usable `symbol` (and `counter_ids` next to
/// `symbols`), at any depth.
fn drop_redundant_counter_ids(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if matches!(map.get("symbol"), Some(Value::String(s)) if !s.is_empty()) {
                map.remove("counter_id");
            }
            if matches!(map.get("symbols"), Some(Value::Array(a)) if !a.is_empty()) {
                map.remove("counter_ids");
            }
            map.values_mut().for_each(drop_redundant_counter_ids);
        }
        Value::Array(items) => items.iter_mut().for_each(drop_redundant_counter_ids),
        _ => {}
    }
}

/// Rewrite every RFC3339 UTC timestamp (`…Z`) as Hong Kong time (`…+08:00`),
/// at any depth: AP-account users read dates as Hong Kong days.
fn localize_hong_kong(value: &mut Value) {
    match value {
        Value::String(text) if text.ends_with('Z') => {
            if let Ok(utc) = OffsetDateTime::parse(text, &Rfc3339) {
                if let Ok(local) = utc.to_offset(time::macros::offset!(+8)).format(&Rfc3339) {
                    *text = local;
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(localize_hong_kong),
        Value::Object(map) => map.values_mut().for_each(localize_hong_kong),
        _ => {}
    }
}

/// Final pass shared by every account command: drop empty values, drop
/// redundant counter ids, and render times in Hong Kong time.
pub(crate) fn finish(mut value: Value) -> Value {
    strip_empty(&mut value);
    drop_redundant_counter_ids(&mut value);
    localize_hong_kong(&mut value);
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

/// Unix-second bounds for optional `--start` / `--end` filters.
pub(crate) fn date_range(
    start: Option<&str>,
    end: Option<&str>,
) -> Result<(Option<String>, Option<String>)> {
    let start = start
        .map(|s| parse_hong_kong_start(s).map(|t| t.unix_timestamp().to_string()))
        .transpose()?;
    let end = end
        .map(|e| parse_hong_kong_end(e).map(|t| t.unix_timestamp().to_string()))
        .transpose()?;
    Ok((start, end))
}

/// Whole days between two RFC3339 timestamps (`to` defaults to now).
fn days_between(from: &str, to: Option<&str>) -> Option<i64> {
    let from = parse_hong_kong_start(from).ok()?;
    let to = match to {
        Some(t) => parse_hong_kong_start(t).ok()?,
        None => OffsetDateTime::now_utc(),
    };
    Some((to - from).whole_days().max(0))
}

/// Keep only the last four characters of an account number.
pub(crate) fn mask_account(account: &str) -> String {
    let chars: Vec<char> = account.chars().filter(|c| !c.is_whitespace()).collect();
    let tail: String = chars[chars.len().saturating_sub(4)..].iter().collect();
    format!("****{tail}")
}

/// Readable name for an integer code, or `None` for an unknown code.
pub(crate) fn code_name(
    value: Option<&Value>,
    names: &[(i64, &'static str)],
) -> Option<&'static str> {
    let code = value?.as_i64()?;
    names
        .iter()
        .find_map(|(c, name)| (*c == code).then_some(*name))
}

/// Insert `key: label` next to an integer code when the code is known.
fn add_label(
    obj: &mut Map<String, Value>,
    code_key: &str,
    key: &str,
    names: &[(i64, &'static str)],
) {
    if let Some(name) = code_name(obj.get(code_key), names) {
        obj.insert(key.to_string(), Value::String(name.to_string()));
    }
}

/// Keep only `keys` of `obj`, in place.
pub(crate) fn retain_fields(obj: &mut Map<String, Value>, keys: &[&str]) {
    obj.retain(|k, _| keys.contains(&k.as_str()));
}

fn is_zero_placeholder(value: &Value) -> bool {
    value.as_i64() == Some(0) || value.as_str() == Some("0")
}

/// Reshape one bank card: keep the allowlisted fields, mask the account
/// number, name the status codes and count the days under review.
pub(crate) fn shape_card(card: &mut Value) {
    let Some(obj) = card.as_object_mut() else {
        return;
    };
    retain_fields(obj, CARD_FIELDS);
    if let Some(account) = obj
        .remove("account")
        .and_then(|a| a.as_str().map(mask_account))
    {
        obj.insert("account".into(), Value::String(account));
    }
    add_label(obj, "status", "verification", CARD_STATUS);
    // Days a card has been waiting for manual review since it was added.
    if obj.get("status").and_then(Value::as_i64) == Some(1) {
        if let Some(days) = obj
            .get("created_at")
            .and_then(Value::as_str)
            .and_then(|c| days_between(c, None))
        {
            obj.insert("review_days".into(), Value::from(days));
        }
    }
    add_label(obj, "witness_state", "witness", WITNESS_STATE);
    add_label(obj, "bind_state", "binding", BIND_STATE);
    // The labels carry these codes; the raw numbers are ambiguous next to an
    // auth entry's own `status` (1 = under review here, authorized there).
    for key in CARD_RAW_CODES {
        obj.remove(*key);
    }
    if let Some(auths) = obj.get_mut("auth_list").and_then(Value::as_array_mut) {
        for auth in auths.iter_mut().filter_map(Value::as_object_mut) {
            shape_auth(auth);
        }
    }
}

fn shape_auth(auth: &mut Map<String, Value>) {
    retain_fields(auth, AUTH_FIELDS);
    auth.retain(|k, v| !(AUTH_ZERO_UNSET.contains(&k.as_str()) && is_zero_placeholder(v)));
    // A long-term authorization carries the 9999-12-31 sentinel as its expiry.
    if auth.get("long_term_valid").and_then(Value::as_bool) == Some(true) {
        auth.remove("expiry_date");
    }
    if let Some(expiry) = auth.get_mut("expiry_date") {
        walk_convert(expiry, &[]);
    }
    add_label(auth, "status", "authorization", AUTH_STATUS);
    auth.remove("status");
    if let Some(message) = auth.remove("message") {
        auth.insert("failure_reason".into(), message);
    }
}

/// Shape the raw `/v2/account/bank-cards` reply into its card list.
pub(crate) fn shape_cards(raw: Value) -> Vec<Value> {
    let mut value = normalize(raw);
    let mut cards = match value.get_mut("list").map(Value::take) {
        Some(Value::Array(cards)) => cards,
        _ => Vec::new(),
    };
    cards.iter_mut().for_each(shape_card);
    cards
}

/// `{cards[]}` output of `bank-cards`.
pub(crate) fn bank_cards_output(cards: Vec<Value>) -> Value {
    finish(json!({ "cards": cards }))
}

/// One eDDA authorization row: the card it belongs to plus the auth entry.
pub(crate) fn edda_row(card: &Value, auth: &Value) -> Value {
    let card_field = |key: &str| card.get(key).cloned().unwrap_or(Value::Null);
    json!({
        "card_id": card_field("id"),
        "bank": card_field("bank"),
        "bank_en": card_field("bank_en"),
        "account": card_field("account"),
        "card_binding": card_field("binding"),
        "flow": auth.get("flow_id"),
        "name": auth.get("name"),
        "currency": auth.get("currency"),
        "authorization": auth.get("authorization"),
        "failure_reason": auth.get("failure_reason"),
        "max_amount": auth.get("max_amount"),
        "max_amount_unlimited": auth.get("max_amount_unlimited"),
        "daily_max_count": auth.get("daily_max_count"),
        "daily_max_count_unlimited": auth.get("daily_max_count_unlimited"),
        "expiry_date": auth.get("expiry_date"),
        "long_term_valid": auth.get("long_term_valid"),
        "is_support": auth.get("is_support"),
        "doc_reviewing": auth.get("doc_reviewing"),
        "no_prior_deposit_reviewing": auth.get("no_prior_deposit_reviewing"),
    })
}

/// `{authorizations[]}` output of `edda`: the eDDA entries of every card.
pub(crate) fn edda_output(cards: &[Value]) -> Value {
    let rows: Vec<Value> = cards
        .iter()
        .flat_map(|card| {
            card.get("auth_list")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|auth| {
                    auth.get("flow_id")
                        .and_then(Value::as_str)
                        .is_some_and(|flow| EDDA_FLOWS.contains(&flow))
                })
                .map(move |auth| edda_row(card, auth))
        })
        .collect();
    finish(json!({ "authorizations": rows }))
}

/// Validate an `edda-status` card id: the numeric id from `bank-cards`.
pub(crate) fn check_bank_card_id(card_id: &str) -> Result<()> {
    if card_id.is_empty() || !card_id.chars().all(|c| c.is_ascii_digit()) {
        bail!("invalid card id '{card_id}': expected the numeric id from bank-cards (cards[].id)");
    }
    Ok(())
}

/// Normalize an eDDA status reply to `{applications[]}`. The RPC returns
/// `{list[]}`; older builds returned a single `{id, status, message}`.
pub(crate) fn shape_edda_status(raw: Value) -> Value {
    let mut applications = match normalize(raw) {
        Value::Object(mut obj) => match obj.remove("list") {
            Some(Value::Array(list)) => list,
            None if obj.contains_key("status") => vec![Value::Object(obj)],
            Some(_) | None => Vec::new(),
        },
        _ => Vec::new(),
    };
    for app in applications.iter_mut().filter_map(Value::as_object_mut) {
        add_label(app, "status", "authorization", AUTH_STATUS);
        if let Some(message) = app.remove("message") {
            app.insert("failure_reason".into(), message);
        }
        retain_fields(app, EDDA_APPLICATION_FIELDS);
    }
    finish(json!({ "applications": applications }))
}

/// Whether a converted timestamp is an unset placeholder: a date before
/// [`TRANSFER_MIN_YEAR`], or a raw `0` / `"0"` / negative number.
fn is_placeholder_time(value: &Value) -> bool {
    match value {
        Value::String(t) => t == "0" || t.get(..4).is_some_and(|year| year < TRANSFER_MIN_YEAR),
        Value::Number(n) => n.as_i64().is_some_and(|n| n <= 0),
        _ => false,
    }
}

/// Shape a raw stock-transfer reply (`{records[], total}`).
pub(crate) fn shape_transfers(raw: Value, kind: &TransferKind) -> Value {
    let mut value = normalize(raw);
    drop_keys(&mut value, TRANSFER_CONTACT_KEYS);
    convert_unix_paths(&mut value, TRANSFER_TIME_PATHS);
    if let Some(records) = value.get_mut("records").and_then(Value::as_array_mut) {
        records
            .iter_mut()
            .filter_map(Value::as_object_mut)
            .for_each(|record| shape_transfer(record, kind));
    }
    finish(value)
}

/// Shape one stock-transfer record: drop the proof-document storage keys and
/// the account holder's name at the other broker, then add `state_label`,
/// `user_notified_broker` and `days_in_process`.
pub(crate) fn shape_transfer(record: &mut Map<String, Value>, kind: &TransferKind) {
    record.remove("attachments");
    record.remove("name");
    for key in TRANSFER_RECORD_NOISE {
        record.remove(*key);
    }
    // The customer's account number at the other broker: last 4 digits only.
    if let Some(account) = record
        .remove("account")
        .and_then(|a| a.as_str().filter(|a| !a.is_empty()).map(mask_account))
    {
        record.insert("account".into(), Value::String(account));
    }
    // Unset dates arrive as 1970 (0) or earlier (negative): drop them.
    record.retain(|key, value| !(key.ends_with("_at") && is_placeholder_time(value)));
    // Transfer-out records name the broker's English name `name_en`.
    if !record.contains_key("broker_en") {
        if let Some(name) = record.remove("name_en") {
            record.insert("broker_en".into(), name);
        }
    }
    if let Some(stocks) = record.get_mut("stocks").and_then(Value::as_array_mut) {
        for stock in stocks.iter_mut().filter_map(Value::as_object_mut) {
            for key in TRANSFER_STOCK_NOISE {
                stock.remove(*key);
            }
            add_label(stock, "state", "state_label", TRANSFER_STOCK_STATE);
        }
    }
    add_label(record, "state_code", "state_label", kind.states);
    // `noticed`: the user tapped "I have notified the broker" in the app.
    if let Some(state) = record.get("app_state").and_then(Value::as_str) {
        let noticed = state == "noticed";
        record.insert("user_notified_broker".into(), Value::Bool(noticed));
    }
    let text = |key: &str| {
        record
            .get(key)
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
    };
    let code = record.get("state_code").and_then(Value::as_i64);
    let finished =
        code.is_some_and(|code| kind.terminal.contains(&code) || kind.completed.contains(&code));
    // Executed: until execution. Finished without an execution date: until it
    // ended. Still open: until now.
    let until = text("exec_at").or_else(|| {
        if finished {
            ["cancel_at", "refuse_at", "updated_at"]
                .iter()
                .find_map(|key| text(key))
        } else {
            None
        }
    });
    if !(finished && until.is_none()) {
        if let Some(days) = text("created_at").and_then(|c| days_between(&c, until.as_deref())) {
            record.insert("days_in_process".into(), Value::from(days));
        }
    }
    if !kind.keeps_can_modify {
        record.remove("can_modify");
    }
    retain_fields(record, TRANSFER_FIELDS);
    if let Some(stocks) = record.get_mut("stocks").and_then(Value::as_array_mut) {
        for stock in stocks.iter_mut().filter_map(Value::as_object_mut) {
            retain_fields(stock, TRANSFER_STOCK_FIELDS);
        }
    }
}

/// Mask `bank_account`, summarize `vouchers`, label states and keep the
/// allowlisted fields of deposit / withdrawal records; add `has_more`.
pub(crate) fn shape_money_records(
    value: &mut Value,
    list_key: &str,
    state_key: &str,
    states: &[(i64, &'static str)],
    page_size: usize,
    fields: &[&str],
) {
    // A full page means there may be more; the deposit list's `total` follows
    // the page size, not the record count, so this is the reliable signal.
    let full_page = value
        .get(list_key)
        .and_then(Value::as_array)
        .is_some_and(|items| items.len() >= page_size);
    if let Some(obj) = value.as_object_mut() {
        obj.insert("has_more".into(), Value::Bool(full_page));
    }
    let Some(items) = value.get_mut(list_key).and_then(Value::as_array_mut) else {
        return;
    };
    for item in items.iter_mut().filter_map(Value::as_object_mut) {
        add_label(item, state_key, "state_label", states);
        if let Some(account) = item
            .remove("bank_account")
            .and_then(|a| a.as_str().filter(|a| !a.is_empty()).map(mask_account))
        {
            item.insert("bank_account".into(), Value::String(account));
        }
        // Usually a short tail like "尾号 0578"; mask a longer run of digits.
        let long_display = item
            .get("display_bank_account")
            .and_then(Value::as_str)
            .filter(|d| d.chars().filter(char::is_ascii_digit).count() > 4)
            .map(mask_account);
        if let Some(masked) = long_display {
            item.insert("display_bank_account".into(), Value::String(masked));
        }
        if let Some(vouchers) = item.remove("vouchers") {
            let count = vouchers.as_array().map_or(0, Vec::len);
            item.insert("voucher_count".into(), Value::from(count));
        }
        retain_fields(item, fields);
    }
}

/// Shape a raw withdrawal reply (`{list[], total}`).
pub(crate) fn shape_withdrawals(raw: Value, page_size: usize) -> Value {
    let mut value = normalize(raw);
    shape_money_records(
        &mut value,
        "list",
        "state",
        WITHDRAWAL_STATE,
        page_size,
        WITHDRAWAL_FIELDS,
    );
    finish(value)
}

/// Shape a raw deposit reply (`{items[], total}`). Upstream `total` is capped
/// at the page size, so it reads as a wrong record count: `has_more` replaces it.
pub(crate) fn shape_deposits(raw: Value, page_size: usize) -> Value {
    let mut value = normalize(raw);
    shape_money_records(
        &mut value,
        "items",
        "state_code",
        DEPOSIT_STATE,
        page_size,
        DEPOSIT_FIELDS,
    );
    if let Some(obj) = value.as_object_mut() {
        obj.remove("total");
    }
    finish(value)
}

/// Validate a deposit `--states` filter: comma-separated numeric codes 0..5.
/// Upstream fails on state names, so they are rejected here with the codes.
pub(crate) fn check_deposit_states(states: &str) -> Result<String> {
    let codes: Vec<&str> = states
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let valid = !codes.is_empty()
        && codes.iter().all(|c| {
            c.parse::<i64>()
                .is_ok_and(|n| DEPOSIT_STATE.iter().any(|(code, _)| *code == n))
        });
    if !valid {
        bail!(
            "invalid --states '{states}': expected comma-separated codes \
             (0 processing, 1 credited, 2 failed, 3 cancelled, 4 invalidated, 5 rejected)"
        );
    }
    Ok(codes.join(","))
}

/// Page size sent upstream: default 20, between 1 and 50.
pub(crate) fn clamp_page_size(size: u32) -> u32 {
    size.clamp(1, 50)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_account_keeps_last_four() {
        assert_eq!(mask_account("012 345 678901"), "****8901", "spaces ignored");
        assert_eq!(mask_account("12"), "****12", "short numbers stay whole");
    }

    #[test]
    fn normalize_converts_at_fields_and_blanks_private_ones() {
        let value = normalize(json!({
            "createdAt": 1_764_064_746,
            "exec_at": 0,
            "process_at": "1764064746",
            "send_time": "1764064746",
            "account_channel": "lb",
            "symbols": {"AAPL.US": {"updated_at": "0"}},
        }));
        assert_eq!(
            value["created_at"], "2025-11-25T09:59:06Z",
            "camelCase snake-cased and converted"
        );
        assert_eq!(
            value["exec_at"], "1970-01-01T00:00:00Z",
            "numeric zero becomes 1970"
        );
        assert_eq!(
            value["process_at"], "2025-11-25T09:59:06Z",
            "string seconds converted"
        );
        assert_eq!(value["send_time"], "1764064746", "non-_at field left alone");
        assert!(
            value["account_channel"].is_null(),
            "account_channel blanked"
        );
        assert_eq!(
            value["symbols"]["AAPL.US"]["updated_at"], "0",
            "data keys kept, \"0\" left"
        );
    }

    #[test]
    fn finish_localizes_and_strips() {
        let value =
            finish(json!({"a": "2026-01-01T00:00:00Z", "b": "", "c": null, "d": [{"e": ""}]}));
        assert_eq!(
            value,
            json!({"a": "2026-01-01T08:00:00+08:00", "d": [{}]}),
            "HK time, empties gone"
        );
    }

    #[test]
    fn shape_card_masks_labels_and_drops_personal_data() {
        // Shape of a real canary response, 2026-11-25.
        let raw = json!({"list": [{
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
        }], "show_add_card_button": true});
        let out = bank_cards_output(shape_cards(raw));
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
        assert!(
            out.get("show_add_card_button").is_none(),
            "only cards are returned"
        );
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
        let mut card = json!({"id": 2, "status": 2, "created_at": "2020-01-01T00:00:00Z"});
        shape_card(&mut card);
        assert!(card.get("review_days").is_none(), "only cards under review");
    }

    #[test]
    fn edda_rows_take_card_context_and_skip_other_flows() {
        let cards = shape_cards(json!({"list": [{
            "id": 7, "bank": "HSBC", "account": "123456789", "bind_state": 1,
            "auth_list": [
                {"flow_id": "hk_edda", "currency": "HKD", "status": 1, "doc_reviewing": true},
                {"flow_id": "hk_fps", "currency": "HKD", "status": 1},
            ],
        }]}));
        let out = edda_output(&cards);
        let rows = out["authorizations"].as_array().expect("rows");
        assert_eq!(rows.len(), 1, "FPS is not eDDA");
        assert_eq!(rows[0]["card_id"], 7, "card id carried over");
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
        assert!(rows[0].get("failure_reason").is_none(), "nulls stripped");
    }

    #[test]
    fn shape_edda_status_accepts_list_and_object() {
        let listed = shape_edda_status(json!({"list": [{"id": 1, "status": 3, "message": "x",
            "created_at": 1_764_064_746, "extra": "y"}]}));
        let app = &listed["applications"][0];
        assert_eq!(app["authorization"], "failed", "list shape labelled");
        assert_eq!(app["failure_reason"], "x", "message renamed");
        assert_eq!(
            app["created_at"], "2025-11-25T17:59:06+08:00",
            "Hong Kong time"
        );
        assert!(app.get("extra").is_none(), "unknown field dropped");
        assert!(app.get("status").is_none(), "raw status dropped");
        let single = shape_edda_status(json!({"id": 2, "status": 1}));
        assert_eq!(
            single["applications"][0]["authorization"], "authorized",
            "object wrapped"
        );
        assert_eq!(
            shape_edda_status(json!({"list": []}))["applications"],
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
        let out = shape_transfers(
            json!({"records": [{
                "id": "7053", "market_symbol": "US", "broker_id": "9", "account": "11222333444555",
                "name": "CHAN TAI MAN", "contact_phone": "12345678", "user_email": "a@b.c",
                "attachments": "[\"ai/x\"]",
                "created_at": 1_795_600_746, "exec_at": 0, "cancel_at": -2_005_000_000,
                "process_at": "0", "app_state": "default", "state_code": 0, "state": "已提交",
                "can_modify": 1, "cash": [],
                "stocks": [{"code": "AAPL", "name": "苹果", "amount": "100", "state": 0,
                    "counter_id": "ST/US/AAPL", "usi_attachments": ["k"], "is_reality": 0,
                    "street_amount": "0"}],
            }], "total": 1}),
            &TRANSFER_IN,
        );
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
        assert_eq!(record["can_modify"], 1, "transfer-in keeps can_modify");
        let stock = &record["stocks"][0];
        assert!(stock.get("usi_attachments").is_none(), "proof keys dropped");
        assert!(stock.get("street_amount").is_none(), "bookkeeping dropped");
        assert_eq!(stock["name"], "苹果", "stock name kept");
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
        let mut record = json!({"name_en": "Futu Securities", "state_code": 0, "can_modify": 0});
        shape_transfer(record.as_object_mut().expect("object"), &TRANSFER_OUT);
        assert_eq!(record["broker_en"], "Futu Securities", "renamed");
        assert!(record.get("name_en").is_none(), "old name gone");
        assert!(
            record.get("can_modify").is_none(),
            "transfer-out drops can_modify"
        );
    }

    #[test]
    fn days_in_process_stops_the_clock_at_the_end() {
        let mut cancelled = json!({"state_code": 1, "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-11T00:00:00Z"});
        shape_transfer(cancelled.as_object_mut().expect("object"), &TRANSFER_IN);
        assert_eq!(
            cancelled["days_in_process"], 10,
            "cancelled: until the last update"
        );

        let mut done = json!({"state_code": 3, "created_at": "2024-01-01T00:00:00Z",
            "exec_at": "2024-01-04T00:00:00Z", "app_state": "noticed"});
        shape_transfer(done.as_object_mut().expect("object"), &TRANSFER_IN);
        assert_eq!(done["days_in_process"], 3, "executed: until execution");
        assert_eq!(done["user_notified_broker"], true, "notified flag");

        let mut no_exec = json!({"state_code": 3, "created_at": "2024-01-01T00:00:00Z",
            "exec_at": "0", "updated_at": "2024-01-06T00:00:00Z"});
        shape_transfer(no_exec.as_object_mut().expect("object"), &TRANSFER_IN);
        assert!(
            no_exec.get("exec_at").is_none(),
            "raw \"0\" placeholder dropped"
        );
        assert_eq!(
            no_exec["days_in_process"], 5,
            "completed: until the last update"
        );

        let mut rejected = json!({"state_code": 7, "created_at": "2024-01-01T00:00:00Z"});
        shape_transfer(rejected.as_object_mut().expect("object"), &TRANSFER_OUT);
        assert!(
            rejected.get("days_in_process").is_none(),
            "no end date: no count"
        );

        let mut open = json!({"state_code": 1, "created_at": "2024-01-01T00:00:00Z"});
        shape_transfer(open.as_object_mut().expect("object"), &TRANSFER_OUT);
        assert!(
            open["days_in_process"].as_i64().is_some_and(|d| d > 300),
            "open: until now"
        );
    }

    #[test]
    fn withdrawals_are_masked_allowlisted_and_paged() {
        let out = shape_withdrawals(
            json!({"list": [
                {"id": 1, "state": 4, "bank_account": "012345678901", "fee": "5",
                 "raw_state": 4, "payee_name": "CHAN TAI MAN", "created_at": 1_764_064_746,
                 "display_bank_account": "012345678901", "account_channel": "lb"},
                {"id": 2, "state": 99, "display_bank_account": "尾号 0578"},
            ], "total": 2}),
            2,
        );
        let item = &out["list"][0];
        assert_eq!(item["bank_account"], "****8901", "account masked");
        assert_eq!(
            item["display_bank_account"], "****8901",
            "long display masked"
        );
        assert_eq!(item["fee"], "5", "allowlisted field kept");
        assert_eq!(item["state_label"], "remitted", "state labelled");
        assert_eq!(
            item["created_at"], "2025-11-25T17:59:06+08:00",
            "Hong Kong time"
        );
        assert!(item.get("payee_name").is_none(), "unknown field dropped");
        assert!(item.get("raw_state").is_none(), "raw_state dropped");
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
    fn deposits_summarize_vouchers_and_drop_total() {
        let out = shape_deposits(
            json!({"items": [{"id": 1, "state_code": 1, "state": "已到账", "sub_state_code": 0,
                "bank_operation_type_id": 3, "disable_link": true,
                "vouchers": [{"key": "k", "img_url": "https://x/y.png"}]}], "total": 20}),
            20,
        );
        let item = &out["items"][0];
        assert_eq!(item["voucher_count"], 1, "vouchers summarized");
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
    fn deposit_states_take_codes_only() {
        assert_eq!(
            check_deposit_states("0, 2").ok().as_deref(),
            Some("0,2"),
            "codes accepted"
        );
        assert!(check_deposit_states("Pending").is_err(), "names rejected");
        assert!(check_deposit_states("9").is_err(), "unknown code rejected");
    }

    #[test]
    fn date_range_reads_hong_kong_days() {
        let (s, e) = date_range(Some("2026-01-01"), Some("2026-01-01")).expect("valid dates");
        assert_eq!(s.as_deref(), Some("1767196800"), "start of day, Hong Kong");
        assert_eq!(e.as_deref(), Some("1767283199"), "end of day, Hong Kong");
        assert!(
            date_range(Some("yesterday"), None).is_err(),
            "bad date rejected"
        );
        let (s, _) = date_range(Some("2026-01-01T00:00:00Z"), None).expect("rfc3339");
        assert_eq!(s.as_deref(), Some("1767225600"), "RFC3339 kept as given");
    }

    #[test]
    fn days_between_counts_whole_days() {
        assert_eq!(
            days_between("2026-11-01T00:00:00Z", Some("2026-11-04T12:00:00Z")),
            Some(3),
            "partial days dropped"
        );
        assert_eq!(days_between("not a date", None), None, "bad input skipped");
    }
}
