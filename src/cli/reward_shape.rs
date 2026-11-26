//! Response shaping for the reward-center commands (`rewards`, `coupons`,
//! `coupon-orders`), mirroring the Longbridge MCP server's reward tools.

use anyhow::{bail, Result};
use serde_json::{json, Map, Value};
use time::OffsetDateTime;

use super::atm_shape::{
    code_name, convert_unix_paths, drop_keys, finish, normalize, parse_hong_kong_start,
    retain_fields, timestamp_to_rfc3339, UNIX_SECONDS_MAX, UNIX_SECONDS_MIN,
};

/// Fields kept from each reward sub-object (`card` / `cash` / `prize`). This is
/// an allowlist: prize rewards also carry redemption secrets (`coupon_secret`)
/// and the shipping contact (`express_info`, `addresses`).
const REWARD_FIELDS: &[&str] = &[
    "record_id",
    "activity_title",
    "title",
    "description",
    "card_type",
    "card_status",
    "prize_type",
    "amount",
    "currency",
    "status",
    "deliver_status",
    "exit_status",
    "send_time",
    "start_time",
    "end_time",
    "effective_time",
    "receive_deadline",
];

/// Reward-center `status` codes (from the app's reward page).
const REWARD_STATUS: &[(i64, &str)] = &[
    (0, "upcoming"),
    (1, "cancelled"),
    (2, "exited_activity"),
    (8, "unclaimed"),
    (10, "issuing"),
    (12, "issued"),
    (13, "failed"),
    (16, "exchanged"),
    (17, "expired"),
    (18, "refunded"),
];

/// Reward-center `deliver_status` codes: what happened after `issued`.
const DELIVER_STATUS: &[(i64, &str)] = &[
    (0, "not_activated"),
    (1, "activated_or_shipping"),
    (2, "consumed"),
    (5, "e_voucher_activating"),
    (6, "e_voucher_expired_before_activation"),
    (7, "e_voucher_invalid"),
    (8, "address_missing"),
    (9, "address_filled"),
    (10, "shipping"),
    (11, "address_filled"),
];

/// Reward-center `reward_type` codes, which the `--kind` filter maps onto.
const REWARD_TYPES: &[(i64, &str)] = &[(0, "cash"), (1, "gift"), (2, "coupon"), (3, "task")];

/// Coupon `card_status` codes (from the app's coupon list).
const CARD_STATUS: &[(i64, &str)] = &[
    (0, "unused"),
    (1, "used"),
    (2, "expired"),
    // LOCKED in the app's code, shown as being redeemed.
    (3, "redeeming"),
    (4, "in_use"),
    (5, "processing"),
];

/// Longest `description` kept for a coupon when its terms are not requested.
pub(crate) const SHORT_DESCRIPTION_CHARS: usize = 200;

/// Most upstream pages read when filtering rewards by date.
pub(crate) const MAX_REWARD_PAGES: u32 = 6;

/// Page size used when paging rewards for a date filter.
pub(crate) const DATED_PAGE_SIZE: usize = 50;

/// Prize `prize_type` codes: only physical goods are shipped.
const PRIZE_TYPES: &[(i64, &str)] = &[(0, "physical")];

/// Commission-card `bz_types`: the instruments the card applies to.
const TARGET_TYPES: &[(i64, &str)] = &[
    (0, "stocks"),
    (1, "options"),
    (2, "warrants"),
    (3, "ETF"),
    (4, "LOF"),
];

/// Reward fields that are unix seconds.
const REWARD_TIME_KEYS: &[&str] = &[
    "send_time",
    "start_time",
    "end_time",
    "effective_time",
    "receive_deadline",
];

/// Coupon-card fields that only style the app's card list.
const CARD_UI_KEYS: &[&str] = &[
    "cover",
    "background",
    "short_background",
    "style",
    "new_flag",
    "invisible",
    "member_id",
];

/// Coupon-card fields that are unix seconds.
const CARD_TIME_PATHS: &[&str] = &[
    "cards.*.start_time",
    "cards.*.end_time",
    "cards.*.activate_time",
    "cards.*.quote.current_end_time",
    "cards.*.commission.commission_expired_time",
];

/// Card-level fields that duplicate the typed sub-object; upstream marks them
/// deprecated.
const CARD_DEPRECATED_KEYS: &[&str] = &[
    "receive_duration",
    "quote_duration",
    "markets",
    "rebate_rate",
    "bz_types",
    "commission_balance",
    "commission_duration",
];

/// Card fields (at any depth) upstream fills with `"0"` when unset.
const CARD_ZERO_UNSET: &[&str] = &[
    "activate_time",
    "commission_expired_time",
    "current_end_time",
    "settled_store_id",
];

/// Fields kept from a coupon's related order (allowlist).
const ORDER_FIELDS: &[&str] = &[
    "order_id",
    "state",
    "state_val",
    "executed_price",
    "executed_quantity",
    "currency",
    "settlement_currency",
    "rebate",
    "rebate_dt",
];

/// Remove `"0"` / `0` placeholders under `keys` at any depth.
fn drop_zero_placeholders(value: &mut Value, keys: &[&str]) {
    match value {
        Value::Object(map) => {
            map.retain(|k, v| {
                !(keys.contains(&k.as_str()) && (v.as_i64() == Some(0) || v.as_str() == Some("0")))
            });
            map.values_mut()
                .for_each(|v| drop_zero_placeholders(v, keys));
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|v| drop_zero_placeholders(v, keys)),
        _ => {}
    }
}

/// Clean one coupon card: drop deprecated duplicates and `"0"` placeholders,
/// and replace a never-expiring card's far-future `end_time` with
/// `no_expiry: true`. Without `with_terms`, `detail` is dropped and a long
/// `description` shortened.
pub(crate) fn shape_coupon(card: &mut Value, with_terms: bool) {
    let Some(obj) = card.as_object_mut() else {
        return;
    };
    if !with_terms {
        obj.remove("detail");
        if let Some(Value::String(text)) = obj.get_mut("description") {
            if text.chars().count() > SHORT_DESCRIPTION_CHARS {
                let short: String = text.chars().take(SHORT_DESCRIPTION_CHARS).collect();
                *text = format!("{short}…");
            }
        }
    }
    for key in CARD_DEPRECATED_KEYS {
        obj.remove(*key);
    }
    // A converted end_time is an RFC3339 string. One left as raw seconds is
    // unset (0), past 2100 (never expires), or in milliseconds.
    let raw_end = match obj.get("end_time") {
        Some(Value::String(t)) if t.chars().all(|c| c.is_ascii_digit()) => t.parse::<i64>().ok(),
        Some(Value::Number(n)) => n.as_i64(),
        _ => None,
    };
    if let Some(raw) = raw_end {
        obj.remove("end_time");
        match raw {
            ms if ms > 1_000_000_000_000 => {
                let seconds = ms / 1000;
                if (UNIX_SECONDS_MIN..UNIX_SECONDS_MAX).contains(&seconds) {
                    obj.insert(
                        "end_time".into(),
                        Value::String(timestamp_to_rfc3339(seconds)),
                    );
                }
            }
            seconds if seconds > UNIX_SECONDS_MAX => {
                obj.insert("no_expiry".into(), Value::Bool(true));
            }
            _ => {}
        }
    }
    drop_zero_placeholders(card, CARD_ZERO_UNSET);
    let Some(obj) = card.as_object_mut() else {
        return;
    };
    if let Some(name) = code_name(obj.get("card_status"), CARD_STATUS) {
        obj.insert("card_status_label".into(), Value::String(name.to_string()));
    }
    // Name the instruments a commission card applies to.
    if let Some(commission) = obj.get_mut("commission").and_then(Value::as_object_mut) {
        let targets: Vec<Value> = commission
            .get("bz_types")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|t| code_name(Some(t), TARGET_TYPES))
            .map(|name| Value::String(name.to_string()))
            .collect();
        if !targets.is_empty() {
            commission.insert("targets".into(), Value::Array(targets));
        }
    }
}

/// Shape a raw coupon-list reply. The pinned top / bottom sections come back
/// on every page, so they are merged into `cards` on the first page only.
pub(crate) fn shape_coupons(raw: Value, page: u32, with_terms: bool) -> Value {
    let mut raw = normalize(raw);
    let sections: &[&str] = if page <= 1 {
        &["top_list", "list", "bottom_list"]
    } else {
        &["list"]
    };
    let cards: Vec<Value> = sections
        .iter()
        .filter_map(|key| raw.get_mut(*key).map(Value::take))
        .filter_map(|section| match section {
            Value::Array(cards) => Some(cards),
            _ => None,
        })
        .flatten()
        .collect();
    let mut value = json!({
        "cards": cards,
        "total": raw.get("total"),
        "expired_count": raw.get("expired_count"),
    });
    drop_keys(&mut value, CARD_UI_KEYS);
    convert_unix_paths(&mut value, CARD_TIME_PATHS);
    if let Some(cards) = value.get_mut("cards").and_then(Value::as_array_mut) {
        for card in cards.iter_mut() {
            shape_coupon(card, with_terms);
        }
    }
    finish(value)
}

/// Flatten one reward-list item into `{reward_type, reward_type_label, …}`.
pub(crate) fn shape_reward(item: &Value) -> Option<Value> {
    let found = ["cash", "card", "prize"]
        .iter()
        // Upstream sends the unused detail objects as `null` next to the real
        // one (`"cash": null, "card": {…}`), so skip nulls.
        .find_map(|kind| {
            item.get(*kind)
                .filter(|detail| detail.is_object())
                .map(|detail| (*kind, detail))
        });
    let mut row = Map::new();
    if let Some(reward_type) = item.get("reward_type") {
        row.insert("reward_type".into(), reward_type.clone());
        if let Some(name) = code_name(Some(reward_type), REWARD_TYPES) {
            row.insert("reward_type_label".into(), Value::String(name.to_string()));
        }
    }
    match found {
        Some((kind, detail)) => {
            // No reward_type upstream: name the type after the detail object,
            // in the same words as the `--kind` filter.
            if !row.contains_key("reward_type_label") {
                let label = match kind {
                    "card" => "coupon",
                    "prize" => "gift",
                    other => other,
                };
                row.insert("reward_type_label".into(), Value::String(label.to_string()));
            }
            for key in REWARD_FIELDS {
                if let Some(v) = detail.get(*key) {
                    row.insert((*key).to_string(), v.clone());
                }
            }
        }
        // A record without a known detail object (e.g. task coins): keep the
        // type so the record is not silently lost.
        None if row.contains_key("reward_type") => {}
        None => return None,
    }
    if let Some(name) = code_name(row.get("status"), REWARD_STATUS) {
        row.insert("status_label".into(), Value::String(name.to_string()));
    }
    if let Some(name) = code_name(row.get("prize_type"), PRIZE_TYPES) {
        row.insert("prize_type_label".into(), Value::String(name.to_string()));
    }
    // Delivery states follow an issued (12) prize; elsewhere a 0 means nothing.
    let issued_prize = found.is_some_and(|(kind, _)| kind == "prize")
        && row.get("status").and_then(Value::as_i64) == Some(12);
    if issued_prize {
        if let Some(name) = code_name(row.get("deliver_status"), DELIVER_STATUS) {
            row.insert(
                "deliver_status_label".into(),
                Value::String(name.to_string()),
            );
        }
    }
    let mut row = Value::Object(row);
    convert_unix_paths(&mut row, REWARD_TIME_KEYS);
    drop_zero_placeholders(&mut row, REWARD_TIME_KEYS);
    Some(row)
}

/// Map `--kind` to the upstream `reward_type` code.
pub(crate) fn reward_type_code(kind: Option<&str>) -> Result<Option<&'static str>> {
    match kind.map(|k| k.trim().to_ascii_lowercase()).as_deref() {
        None | Some("" | "all") => Ok(None),
        Some("cash") => Ok(Some("0")),
        Some("gift" | "prize") => Ok(Some("1")),
        Some("coupon" | "card") => Ok(Some("2")),
        Some("task") => Ok(Some("3")),
        Some(other) => bail!("invalid kind '{other}': expected cash, gift, coupon or task"),
    }
}

/// Whether a shaped reward's `send_time` falls inside `[start, end]`. A record
/// without a send time is kept: it has not been issued yet.
pub(crate) fn in_range(
    row: &Value,
    start: Option<&OffsetDateTime>,
    end: Option<&OffsetDateTime>,
) -> bool {
    let Some(sent) = row
        .get("send_time")
        .and_then(Value::as_str)
        .and_then(|t| parse_hong_kong_start(t).ok())
    else {
        return true;
    };
    start.is_none_or(|s| sent >= *s) && end.is_none_or(|e| sent <= *e)
}

/// Accumulates reward pages for a dated search, newest first.
pub(crate) struct DatedRewards {
    start: Option<OffsetDateTime>,
    end: Option<OffsetDateTime>,
    rewards: Vec<Value>,
}

impl DatedRewards {
    pub(crate) fn new(start: Option<OffsetDateTime>, end: Option<OffsetDateTime>) -> Self {
        Self {
            start,
            end,
            rewards: Vec::new(),
        }
    }

    /// Add one raw upstream page. Returns `true` when no later page can match:
    /// the page was short, or it already ends before `start`.
    pub(crate) fn add_page(&mut self, raw: Value) -> bool {
        let raw = normalize(raw);
        let items = raw
            .get("list")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let fetched = items.len();
        let rows: Vec<Value> = items.iter().filter_map(shape_reward).collect();
        let past_start = rows
            .last()
            .is_some_and(|last| self.start.is_some() && !in_range(last, self.start.as_ref(), None));
        let (start, end) = (self.start, self.end);
        self.rewards.extend(
            rows.into_iter()
                .filter(|r| in_range(r, start.as_ref(), end.as_ref())),
        );
        past_start || fetched < DATED_PAGE_SIZE
    }

    /// `{rewards[], searched_all}`: `searched_all` is false when the page cap
    /// or a failed page cut the search short.
    pub(crate) fn output(self, searched_all: bool) -> Value {
        finish(json!({ "rewards": self.rewards, "searched_all": searched_all }))
    }
}

/// `{rewards[], total}` for one undated page.
pub(crate) fn shape_reward_page(raw: Value) -> Value {
    let raw = normalize(raw);
    let rewards: Vec<Value> = raw
        .get("list")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(shape_reward)
        .collect();
    finish(json!({ "rewards": rewards, "total": raw.get("total") }))
}

/// Map `--status` to the upstream `card_status` filter.
pub(crate) fn card_status_code(status: Option<&str>) -> Result<&'static str> {
    match status.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        None | Some("" | "valid") => Ok("-1"),
        Some("expired") => Ok("2"),
        Some(other) => bail!("invalid status '{other}': expected \"valid\" or \"expired\""),
    }
}

/// Validate a coupon card id (32 hex characters), with a hint when a bank
/// card id is given instead.
pub(crate) fn check_coupon_id(card_id: &str) -> Result<()> {
    if card_id.len() == 32 && card_id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(());
    }
    let hint = if !card_id.is_empty() && card_id.chars().all(|c| c.is_ascii_digit()) {
        " — this looks like a bank card id from bank-cards"
    } else {
        ""
    };
    bail!(
        "invalid card id '{card_id}': expected the 32-character coupon id from coupons (cards[].card_id){hint}"
    )
}

/// `{card_id, locked[], settled[]}` output of `coupon-orders`.
pub(crate) fn shape_coupon_orders(card_id: &str, locked: Value, settled: Value) -> Value {
    let orders = |v: Value| -> Vec<Value> {
        let mut orders = match normalize(v).get_mut("orders").map(Value::take) {
            Some(Value::Array(orders)) => orders,
            _ => Vec::new(),
        };
        for order in orders.iter_mut().filter_map(Value::as_object_mut) {
            retain_fields(order, ORDER_FIELDS);
        }
        orders
    };
    finish(json!({
        "card_id": card_id,
        "locked": orders(locked),
        "settled": orders(settled),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::atm_shape::parse_hong_kong_end;

    #[test]
    fn shape_reward_drops_secrets() {
        let item = json!({
            "reward_type": 1,
            "prize": {
                "activity_title": "Open account", "title": "Coffee", "status": 2,
                "send_time": 1_790_000_000,
                "coupon_secret": {"secret": "ABC-123"}, "coupon_secret_id": "9",
                "cash_card_code": "XYZ", "express_info": {"phone": "12345678"},
                "addresses": [{"detail": "somewhere"}]
            }
        });
        let row = shape_reward(&item).expect("a prize item must shape");
        assert_eq!(row["reward_type"], 1, "reward_type carried over");
        assert_eq!(row["reward_type_label"], "gift", "type labelled");
        assert_eq!(row["status_label"], "exited_activity", "status labelled");
        for key in [
            "coupon_secret",
            "coupon_secret_id",
            "cash_card_code",
            "express_info",
            "addresses",
        ] {
            assert!(row.get(key).is_none(), "{key} must be dropped");
        }
        assert!(
            row["send_time"]
                .as_str()
                .is_some_and(|t| t.starts_with("2026-")),
            "send_time must be RFC3339"
        );
    }

    #[test]
    fn shape_reward_labels_type_like_the_filter() {
        let row = shape_reward(&json!({"card": {"title": "t"}})).expect("shapes");
        assert_eq!(
            row["reward_type_label"], "coupon",
            "card detail reads as coupon"
        );
        let row = shape_reward(&json!({"reward_type": 3})).expect("typed record kept");
        assert_eq!(row["reward_type_label"], "task", "task labelled");
        assert!(
            shape_reward(&json!({"other": 1})).is_none(),
            "nothing to show"
        );
    }

    #[test]
    fn deliver_label_only_for_issued_prizes() {
        let cash =
            shape_reward(&json!({"cash": {"status": 12, "deliver_status": 0}})).expect("shapes");
        assert!(
            cash.get("deliver_status_label").is_none(),
            "cash has no delivery"
        );
        let pending =
            shape_reward(&json!({"prize": {"status": 0, "deliver_status": 0}})).expect("shapes");
        assert!(
            pending.get("deliver_status_label").is_none(),
            "not issued yet"
        );
        let issued = shape_reward(&json!({"reward_type": 1,
            "prize": {"status": 12, "deliver_status": 8, "prize_type": 0}}))
        .expect("shapes");
        assert_eq!(
            issued["deliver_status_label"], "address_missing",
            "deliver label"
        );
        assert_eq!(issued["prize_type_label"], "physical", "physical prize");
    }

    #[test]
    fn shape_reward_skips_null_detail_objects() {
        // Shape of a real production coupon reward, 2026-11-26.
        let row = shape_reward(&json!({"reward_type": 2, "cash": null, "prize": null,
            "card": {"record_id": "18071952", "activity_title": "Options Campaign",
                "card_type": "commission", "status": 12, "send_time": "1794459188",
                "cover": "https://x/c.png", "receive_duration": "0"}}))
        .expect("shapes");
        assert_eq!(
            row["activity_title"], "Options Campaign",
            "card fields read"
        );
        assert_eq!(row["status_label"], "issued", "status labelled");
        assert!(row["send_time"].is_string(), "send_time converted");
        assert!(row.get("cover").is_none(), "styling not allowlisted");
    }

    #[test]
    fn reward_send_time_zero_is_dropped() {
        let row =
            shape_reward(&json!({"reward_type": 0, "cash": {"send_time": "0", "amount": 38}}))
                .expect("shapes");
        assert!(
            row.get("send_time").is_none(),
            "placeholder send_time dropped"
        );
    }

    #[test]
    fn reward_page_reports_total_in_hong_kong_time() {
        let out = shape_reward_page(json!({"list": [
            {"reward_type": 0, "cash": {"status": 12, "amount": 38, "currency": "HKD",
                "send_time": 1_764_064_746}},
        ], "total": 41}));
        assert_eq!(out["total"], 41, "total kept without dates");
        assert_eq!(
            out["rewards"][0]["send_time"], "2025-11-25T17:59:06+08:00",
            "Hong Kong time"
        );
        assert_eq!(out["rewards"][0]["status_label"], "issued", "12 is issued");
    }

    #[test]
    fn dated_rewards_filter_and_stop() {
        let start = parse_hong_kong_start("2026-08-01").expect("date");
        let end = parse_hong_kong_end("2026-08-31").expect("date");
        let mut dated = DatedRewards::new(Some(start), Some(end));
        let page = json!({"list": [
            {"reward_type": 0, "cash": {"title": "unsent"}},
            {"reward_type": 0, "cash": {"title": "inside", "send_time": 1_786_000_000}},
            {"reward_type": 0, "cash": {"title": "before", "send_time": 1_783_000_000}},
        ]});
        assert!(dated.add_page(page), "short page ends the search");
        let out = dated.output(true);
        let titles: Vec<&str> = out["rewards"]
            .as_array()
            .expect("rewards")
            .iter()
            .filter_map(|r| r["title"].as_str())
            .collect();
        assert_eq!(titles, ["unsent", "inside"], "unsent kept, older dropped");
        assert_eq!(out["searched_all"], true, "search finished");
        assert!(out.get("total").is_none(), "no total with dates");
    }

    #[test]
    fn coupon_end_time_variants() {
        let mut ms = json!({"end_time": 1_798_732_800_000_i64});
        shape_coupon(&mut ms, false);
        assert!(
            ms.get("no_expiry").is_none(),
            "milliseconds are not forever"
        );
        assert_eq!(
            ms["end_time"], "2026-12-31T16:00:00Z",
            "milliseconds converted"
        );
        let mut zero = json!({"end_time": "0"});
        shape_coupon(&mut zero, false);
        assert!(
            zero.get("end_time").is_none() && zero.get("no_expiry").is_none(),
            "0 is unset"
        );
        let mut forever = json!({"end_time": 8_061_747_303_i64});
        shape_coupon(&mut forever, false);
        assert_eq!(forever["no_expiry"], true, "far future is forever");
    }

    #[test]
    fn coupons_merge_pinned_sections_on_first_page_only() {
        // Shape of a real production lifetime commission card, 2026-11-25.
        let raw = json!({
            "top_list": [{"card_id": "top"}],
            "list": [{
                "card_id": "bd6b40914cd411f08becae6ae6fae931", "card_type": "commission",
                "start_time": 1_750_313_704, "end_time": "8061747303", "card_status": 4,
                "activate_time": "0", "cover": "https://x/c.png", "member_id": "1",
                "commission": {"markets": ["US"], "bz_types": [0, 2, 3],
                    "commission_expired_time": "0"},
                "markets": ["US"], "rebate_rate": "1", "quote_duration": "0",
                "available_trade_session": "夜盘、盘中+盘前盘后",
                "description": "x".repeat(300), "detail": "terms",
            }],
            "bottom_list": [{"card_id": "bottom"}],
            "total": 3, "expired_count": 5, "tags": ["a"],
        });
        let first = shape_coupons(raw.clone(), 1, false);
        let ids: Vec<&str> = first["cards"]
            .as_array()
            .expect("cards")
            .iter()
            .filter_map(|c| c["card_id"].as_str())
            .collect();
        assert_eq!(
            ids,
            ["top", "bd6b40914cd411f08becae6ae6fae931", "bottom"],
            "pinned merged"
        );
        assert_eq!(first["total"], 3, "total kept");
        assert_eq!(first["expired_count"], 5, "expired count kept");
        assert!(first.get("tags").is_none(), "tags dropped");
        let card = &first["cards"][1];
        assert_eq!(
            card["no_expiry"], true,
            "far-future end_time becomes no_expiry"
        );
        assert!(card.get("end_time").is_none(), "raw end_time removed");
        assert!(
            card.get("markets").is_none(),
            "deprecated duplicate removed"
        );
        assert!(card.get("cover").is_none(), "styling removed");
        assert!(card.get("member_id").is_none(), "member id removed");
        assert!(
            card.get("activate_time").is_none(),
            "zero placeholder removed"
        );
        assert!(
            card["commission"].get("commission_expired_time").is_none(),
            "nested zero removed"
        );
        assert_eq!(
            card["commission"]["targets"],
            json!(["stocks", "warrants", "ETF"]),
            "targets named"
        );
        assert_eq!(
            card["start_time"], "2025-06-19T14:15:04+08:00",
            "start_time in Hong Kong time"
        );
        assert_eq!(card["card_status_label"], "in_use", "status labelled");
        assert!(card.get("detail").is_none(), "detail dropped by default");
        assert_eq!(
            card["description"].as_str().map(|d| d.chars().count()),
            Some(SHORT_DESCRIPTION_CHARS + 1),
            "description shortened"
        );

        let second = shape_coupons(raw.clone(), 2, true);
        assert_eq!(
            second["cards"].as_array().map(Vec::len),
            Some(1),
            "pinned sections only on page 1"
        );
        assert_eq!(
            second["cards"][0]["detail"], "terms",
            "with terms keeps detail"
        );
    }

    #[test]
    fn filters_map_and_reject() {
        assert_eq!(
            reward_type_code(Some("coupon")).ok(),
            Some(Some("2")),
            "coupon"
        );
        assert_eq!(reward_type_code(None).ok(), Some(None), "all");
        assert!(reward_type_code(Some("stock")).is_err(), "unknown kind");
        assert_eq!(card_status_code(None).ok(), Some("-1"), "default is valid");
        assert_eq!(
            card_status_code(Some("Expired")).ok(),
            Some("2"),
            "case-insensitive"
        );
        assert!(
            card_status_code(Some("used")).is_err(),
            "unsupported filter"
        );
    }

    #[test]
    fn coupon_ids_are_checked() {
        assert!(
            check_coupon_id("bd6b40914cd411f08becae6ae6fae931").is_ok(),
            "real coupon id"
        );
        let err = check_coupon_id("493082").expect_err("bank card id");
        assert!(err.to_string().contains("bank card id"), "hint given");
    }

    #[test]
    fn coupon_orders_are_allowlisted() {
        let order = json!({"orders": [{"order_id": "1", "state": "Filled", "state_val": "FilledStatus",
            "executed_price": "10", "rebate": "1.5", "state_color": "#fff", "rebate_dt": ""}]});
        let out = shape_coupon_orders(
            "bd6b40914cd411f08becae6ae6fae931",
            order,
            json!({"orders": []}),
        );
        assert_eq!(out["locked"][0]["rebate"], "1.5", "kept field");
        assert!(
            out["locked"][0].get("state_color").is_none(),
            "styling dropped"
        );
        assert!(
            out["locked"][0].get("rebate_dt").is_none(),
            "empty string stripped"
        );
        assert_eq!(out["settled"], json!([]), "empty settled list");
    }
}
