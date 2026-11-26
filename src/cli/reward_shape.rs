//! Response shaping for the reward-center commands (`reward list`,
//! `coupon list`, `coupon orders`), mirroring the Longbridge MCP server's
//! reward tools.
//!
//! The data comes from the SDK's typed `AssetContext` APIs, which already drop
//! prize redemption secrets, shipping contacts and coupon styling, and decode
//! placeholder and "never expires" times as unset.

use anyhow::{bail, Result};
use longbridge::asset::{
    Coupon, CouponAiBenefit, CouponCash, CouponCoffee, CouponCommission, CouponCrypto,
    CouponFinanceInterest, CouponFundDeduct, CouponIpo, CouponIpoCommission, CouponIpoInterest,
    CouponOrder, CouponPlatformFeeDeduct, CouponQuote, CouponStatus, CouponStock,
    CouponStockFragment, CouponTargetType, CouponsResponse, RewardDeliverStatus, RewardRecord,
    RewardRecordsResponse, RewardStatus, RewardType,
};
use serde_json::{json, Value};
use time::OffsetDateTime;

use crate::cli::atm_shape::{code, decimal, finish, hk_time, label};

/// Longest `description` kept for a coupon when its terms are not requested.
pub(crate) const SHORT_DESCRIPTION_CHARS: usize = 200;

/// Most upstream pages read when filtering rewards by date.
pub(crate) const MAX_REWARD_PAGES: u32 = 6;

/// Page size used when paging rewards for a date filter.
pub(crate) const DATED_PAGE_SIZE: u32 = 50;

/// Reward-center `status` names (from the app's reward page).
fn reward_status_label(status: RewardStatus) -> Option<&'static str> {
    match status {
        RewardStatus::Upcoming => Some("upcoming"),
        RewardStatus::Cancelled => Some("cancelled"),
        RewardStatus::ExitedActivity => Some("exited_activity"),
        RewardStatus::Unclaimed => Some("unclaimed"),
        RewardStatus::Issuing => Some("issuing"),
        RewardStatus::Issued => Some("issued"),
        RewardStatus::Failed => Some("failed"),
        RewardStatus::Exchanged => Some("exchanged"),
        RewardStatus::Expired => Some("expired"),
        RewardStatus::Refunded => Some("refunded"),
        RewardStatus::Unknown => None,
    }
}

/// Reward-center `deliver_status` names: what happened after `issued`.
fn deliver_status_label(status: RewardDeliverStatus) -> Option<&'static str> {
    match status {
        RewardDeliverStatus::NotActivated => Some("not_activated"),
        RewardDeliverStatus::ActivatedOrShipping => Some("activated_or_shipping"),
        RewardDeliverStatus::Consumed => Some("consumed"),
        RewardDeliverStatus::EVoucherActivating => Some("e_voucher_activating"),
        RewardDeliverStatus::EVoucherExpired => Some("e_voucher_expired_before_activation"),
        RewardDeliverStatus::EVoucherInvalid => Some("e_voucher_invalid"),
        RewardDeliverStatus::AddressMissing => Some("address_missing"),
        RewardDeliverStatus::AddressFilled => Some("address_filled"),
        RewardDeliverStatus::Shipping => Some("shipping"),
        RewardDeliverStatus::Unknown => None,
    }
}

/// Reward types, in the words of the `--kind` filter.
fn reward_type_label(reward_type: RewardType) -> Option<&'static str> {
    match reward_type {
        RewardType::Cash => Some("cash"),
        RewardType::Gift => Some("gift"),
        RewardType::Coupon => Some("coupon"),
        RewardType::Task => Some("task"),
        RewardType::Unknown => None,
    }
}

/// Coupon `card_status` names (from the app's coupon list).
fn coupon_status_label(status: CouponStatus) -> Option<&'static str> {
    match status {
        CouponStatus::Unused => Some("unused"),
        CouponStatus::Used => Some("used"),
        CouponStatus::Expired => Some("expired"),
        // LOCKED in the app's code, shown as being redeemed.
        CouponStatus::Redeeming => Some("redeeming"),
        CouponStatus::InUse => Some("in_use"),
        CouponStatus::Processing => Some("processing"),
        CouponStatus::Unknown => None,
    }
}

/// Commission-card `bz_types`: the instruments the card applies to.
fn target_type_label(target: CouponTargetType) -> Option<&'static str> {
    match target {
        CouponTargetType::Stock => Some("stocks"),
        CouponTargetType::UsOption => Some("options"),
        CouponTargetType::HkWarrant => Some("warrants"),
        CouponTargetType::Etf => Some("ETF"),
        CouponTargetType::Lof => Some("LOF"),
        CouponTargetType::Unknown => None,
    }
}

/// The server codes of typed states, leaving out undocumented ones.
fn codes<T>(values: &[T]) -> Vec<i32>
where
    T: Into<i32> + Default + PartialEq + Copy,
{
    values
        .iter()
        .filter(|v| **v != T::default())
        .map(|v| (*v).into())
        .collect()
}

/// A per-type terms object, or `null` when the coupon has none of this type.
fn terms<T>(terms: Option<&T>, shape: fn(&T) -> Value) -> Value {
    terms.map_or(Value::Null, shape)
}

/// Prize `prize_type` names: only physical goods are shipped.
fn prize_type_label(prize_type: Option<i32>) -> Option<&'static str> {
    (prize_type == Some(0)).then_some("physical")
}

/// One reward record as `{reward_type, reward_type_label, …}`, or `None` for
/// a record with neither a known type nor any detail.
pub(crate) fn shape_reward(record: &RewardRecord) -> Option<Value> {
    let has_detail = !(record.record_id.is_empty()
        && record.title.is_empty()
        && record.activity_title.is_empty());
    if record.reward_type == RewardType::Unknown && !has_detail {
        return None;
    }
    // Delivery states follow an issued prize; elsewhere a 0 means nothing.
    let issued_prize =
        record.reward_type == RewardType::Gift && record.status == RewardStatus::Issued;
    let deliver_label = if issued_prize {
        deliver_status_label(record.deliver_status)
    } else {
        None
    };
    Some(json!({
        "reward_type": code(record.reward_type),
        "reward_type_label": label(reward_type_label(record.reward_type)),
        "record_id": record.record_id,
        "activity_title": record.activity_title,
        "title": record.title,
        "description": record.description,
        "card_type": record.card_type,
        "card_status": code(record.card_status),
        "prize_type": record.prize_type,
        "prize_type_label": label(prize_type_label(record.prize_type)),
        "amount": decimal(record.amount),
        "currency": record.currency,
        "status": code(record.status),
        "status_label": label(reward_status_label(record.status)),
        "deliver_status": code(record.deliver_status),
        "deliver_status_label": label(deliver_label),
        "exit_status": (record.exit_status != 0).then_some(record.exit_status),
        "send_time": hk_time(record.send_time),
        "start_time": hk_time(record.start_time),
        "end_time": hk_time(record.end_time),
        "effective_time": hk_time(record.effective_time),
        "receive_deadline": hk_time(record.receive_deadline),
    }))
}

/// Map `--kind` to the reward type filter (`None` for all types).
pub(crate) fn reward_type_filter(kind: Option<&str>) -> Result<Option<RewardType>> {
    match kind.map(|k| k.trim().to_ascii_lowercase()).as_deref() {
        None | Some("" | "all") => Ok(None),
        Some("cash") => Ok(Some(RewardType::Cash)),
        Some("gift" | "prize") => Ok(Some(RewardType::Gift)),
        Some("coupon" | "card") => Ok(Some(RewardType::Coupon)),
        Some("task") => Ok(Some(RewardType::Task)),
        Some(other) => bail!("invalid kind '{other}': expected cash, gift, coupon or task"),
    }
}

/// Whether a reward's send time falls inside `[start, end]`. A record without
/// a send time is kept: it has not been issued yet.
pub(crate) fn in_range(
    record: &RewardRecord,
    start: Option<&OffsetDateTime>,
    end: Option<&OffsetDateTime>,
) -> bool {
    let Some(sent) = record.send_time else {
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

    /// Add one page. Returns `true` when no later page can match: the page was
    /// short, or it already ends before `start`.
    pub(crate) fn add_page(&mut self, page: &RewardRecordsResponse) -> bool {
        let (start, end) = (self.start, self.end);
        let shown: Vec<&RewardRecord> = page
            .list
            .iter()
            .filter(|r| shape_reward(r).is_some())
            .collect();
        let past_start = shown
            .last()
            .is_some_and(|last| start.is_some() && !in_range(last, start.as_ref(), None));
        self.rewards.extend(
            shown
                .into_iter()
                .filter(|r| in_range(r, start.as_ref(), end.as_ref()))
                .filter_map(shape_reward),
        );
        past_start || page.list.len() < DATED_PAGE_SIZE as usize
    }

    /// `{rewards[], searched_all}`: `searched_all` is false when the page cap
    /// or a failed page cut the search short.
    pub(crate) fn output(self, searched_all: bool) -> Value {
        finish(json!({ "rewards": self.rewards, "searched_all": searched_all }))
    }
}

/// `{rewards[], total}` for one undated page.
pub(crate) fn shape_reward_page(page: &RewardRecordsResponse) -> Value {
    let rewards: Vec<Value> = page.list.iter().filter_map(shape_reward).collect();
    finish(json!({ "rewards": rewards, "total": page.total }))
}

/// Map `--status` to the coupon filter: `false` for valid (not expired),
/// `true` for expired.
pub(crate) fn coupons_expired(status: Option<&str>) -> Result<bool> {
    match status.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        None | Some("" | "valid") => Ok(false),
        Some("expired") => Ok(true),
        Some(other) => bail!("invalid status '{other}': expected \"valid\" or \"expired\""),
    }
}

/// A coupon's commission terms.
fn shape_commission(commission: &CouponCommission) -> Value {
    // Name the instruments a commission card applies to.
    let targets: Vec<&str> = commission
        .bz_types
        .iter()
        .filter_map(|t| target_type_label(*t))
        .collect();
    json!({
        "markets": commission.markets,
        "targets": (!targets.is_empty()).then_some(targets),
        "rebate_rate": commission.rebate_rate,
        "bz_types": codes(&commission.bz_types),
        "commission_balance": decimal(commission.commission_balance),
        "commission_duration": commission.commission_duration,
        "unlimited": code(commission.unlimited),
        "commission_expired_time": hk_time(commission.commission_expired_time),
        "usage_count": commission.usage_count,
        "lave_usage_count": commission.lave_usage_count,
        "settlement_model": commission.settlement_model,
        "amount_limit": commission.amount_limit,
        "superposition": commission.superposition,
    })
}

fn shape_ai_benefit(t: &CouponAiBenefit) -> Value {
    json!({
        "benefit_days": t.benefit_days,
        "benefit_start": hk_time(t.benefit_start),
        "benefit_end": hk_time(t.benefit_end),
        "benefit_tag": t.benefit_tag,
    })
}

fn shape_cash(t: &CouponCash) -> Value {
    json!({ "receive_duration": t.receive_duration })
}

fn shape_coffee(t: &CouponCoffee) -> Value {
    let stores: Vec<Value> = t
        .store_list
        .iter()
        .map(|s| json!({ "store_id": s.store_id, "store_name": s.store_name }))
        .collect();
    json!({
        "coupon_channel": t.coupon_channel,
        "coupon_id": t.coupon_id,
        "coupon_no": t.coupon_no,
        "is_available_all_store": t.is_available_all_store,
        "period_type": t.period_type,
        // 0: not settled at a store yet.
        "settled_store_id": (t.settled_store_id != 0).then_some(t.settled_store_id),
        "store_id_list": t.store_id_list,
        "store_list": stores,
        "threshold": t.threshold,
        "use_limit": t.use_limit,
    })
}

fn shape_crypto(t: &CouponCrypto) -> Value {
    let counters: Vec<Value> = t
        .optional_counters
        .iter()
        .map(|c| json!({ "counter_id": c.counter_id }))
        .collect();
    json!({
        "fallback": {
            "enabled": t.fallback.enabled,
            "counter_id": t.fallback.counter_id,
        },
        "optional_counters": counters,
    })
}

fn shape_finance_interest(t: &CouponFinanceInterest) -> Value {
    json!({
        "discount_mode": t.discount_mode,
        "discount_rate": decimal(t.discount_rate),
        "superposition": t.superposition,
    })
}

fn shape_fund_deduct(t: &CouponFundDeduct) -> Value {
    json!({
        "max_risk_level": t.max_risk_level,
        "min_order_amount": t.min_order_amount,
        "order_types": t.order_types,
        "underlying_type": t.underlying_type,
        "underlying_values": t.underlying_values,
        "underlying_names": t.underlying_names,
    })
}

fn shape_quote(t: &CouponQuote) -> Value {
    json!({
        "current_end_time": hk_time(t.current_end_time),
        "inapplicable": t.inapplicable,
        "investor_type": code(t.investor_type),
        "manual_activate": t.manual_activate,
        "markets": t.markets,
        "member_investor_type": t.member_investor_type,
        "pair_spu_no": t.pair_spu_no,
        "quote_duration": t.quote_duration,
        "quote_market": t.quote_market,
        "quote_tags": t.quote_tags,
        "superposition": t.superposition,
    })
}

fn shape_ipo(t: &CouponIpo) -> Value {
    json!({ "subscription_id": t.subscription_id })
}

fn shape_ipo_commission(t: &CouponIpoCommission) -> Value {
    json!({
        "bz_types": codes(&t.bz_types),
        "counter_id": t.counter_id,
        "counter_name": t.counter_name,
        "financing_limit": t.financing_limit,
    })
}

fn shape_ipo_interest(t: &CouponIpoInterest) -> Value {
    json!({ "discount_rate": t.discount_rate })
}

fn shape_platform_fee_deduct(t: &CouponPlatformFeeDeduct) -> Value {
    json!({
        "bz_types": codes(&t.bz_types),
        "counter_id": t.counter_id,
        "counter_name": t.counter_name,
        "markets": t.markets,
        "usage_count": t.usage_count,
        "lave_usage_count": t.lave_usage_count,
        "superposition": t.superposition,
    })
}

fn shape_stock(t: &CouponStock) -> Value {
    let counters: Vec<Value> = t
        .optional_counters
        .iter()
        .map(|c| json!({ "counter_id": c.counter_id, "shares": c.shares }))
        .collect();
    json!({
        "counter_id": t.counter_id,
        "counter_name": t.counter_name,
        "amount": decimal(t.amount),
        "redemption_type": t.redemption_type,
        "send_way": t.send_way,
        "optional_counters": counters,
        "fallback": {
            "enabled": t.fallback.enabled,
            "type": t.fallback.fallback_type,
            "counter_id": t.fallback.counter_id,
            "shares": t.fallback.shares,
        },
    })
}

fn shape_stock_fragment(t: &CouponStockFragment) -> Value {
    json!({
        "counter_id": t.counter_id,
        "counter_name": t.counter_name,
        "market": t.market,
        "markets": t.markets,
        "underlying_type": code(t.underlying_type),
        "amounts": t.amounts,
    })
}

/// One coupon card. Without `with_terms`, `detail` is dropped and a long
/// `description` shortened. A card that never expires has no `end_time` and
/// `no_expiry: true`.
pub(crate) fn shape_coupon(coupon: &Coupon, with_terms: bool) -> Value {
    let description = if !with_terms && coupon.description.chars().count() > SHORT_DESCRIPTION_CHARS
    {
        let short: String = coupon
            .description
            .chars()
            .take(SHORT_DESCRIPTION_CHARS)
            .collect();
        format!("{short}…")
    } else {
        coupon.description.clone()
    };
    json!({
        "card_id": coupon.card_id,
        "card_type": coupon.card_type,
        "title": coupon.title,
        "title_sec": coupon.title_sec,
        "description": description,
        "detail": if with_terms { coupon.detail.as_str() } else { "" },
        "card_status": code(coupon.card_status),
        "card_status_label": label(coupon_status_label(coupon.card_status)),
        "start_time": hk_time(coupon.start_time),
        "end_time": hk_time(coupon.end_time),
        "no_expiry": coupon.no_expiry.then_some(true),
        "activate_time": hk_time(coupon.activate_time),
        "amount": decimal(coupon.amount),
        "currency": coupon.currency,
        "available_trade_session": coupon.available_trade_session,
        "trade_sessions": coupon.trade_sessions,
        "subscription_id": coupon.subscription_id,
        "ai_benefit": terms(coupon.ai_benefit.as_ref(), shape_ai_benefit),
        "cash": terms(coupon.cash.as_ref(), shape_cash),
        "coffee_coupon": terms(coupon.coffee_coupon.as_ref(), shape_coffee),
        "commission": terms(coupon.commission.as_ref(), shape_commission),
        "crypto": terms(coupon.crypto.as_ref(), shape_crypto),
        "finance_interest": terms(coupon.finance_interest.as_ref(), shape_finance_interest),
        "fund_deduct": terms(coupon.fund_deduct.as_ref(), shape_fund_deduct),
        "fundamental": terms(coupon.fundamental.as_ref(), shape_quote),
        "ipo": terms(coupon.ipo.as_ref(), shape_ipo),
        "ipo_commission": terms(coupon.ipo_commission.as_ref(), shape_ipo_commission),
        "ipo_interest": terms(coupon.ipo_interest.as_ref(), shape_ipo_interest),
        "platform_fee_deduct": terms(
            coupon.platform_fee_deduct.as_ref(),
            shape_platform_fee_deduct
        ),
        "quote": terms(coupon.quote.as_ref(), shape_quote),
        "stock": terms(coupon.stock.as_ref(), shape_stock),
        "stock_fragment": terms(coupon.stock_fragment.as_ref(), shape_stock_fragment),
    })
}

/// `{cards[], total, expired_count}` output of `coupon list`. The pinned top /
/// bottom sections come back on every page, so they are merged into `cards`
/// on the first page only.
pub(crate) fn shape_coupons(resp: &CouponsResponse, page: u32, with_terms: bool) -> Value {
    let sections: Vec<&Vec<Coupon>> = if page <= 1 {
        vec![&resp.top_list, &resp.list, &resp.bottom_list]
    } else {
        vec![&resp.list]
    };
    let cards: Vec<Value> = sections
        .into_iter()
        .flatten()
        .map(|c| shape_coupon(c, with_terms))
        .collect();
    finish(json!({
        "cards": cards,
        "total": resp.total,
        "expired_count": resp.expired_count,
    }))
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
        "invalid card id '{card_id}': expected the 32-character coupon id from `coupon list` (cards[].card_id){hint}"
    )
}

fn shape_order(order: &CouponOrder) -> Value {
    json!({
        "order_id": order.order_id,
        "state": order.state,
        "state_val": order.state_val,
        "executed_price": decimal(order.executed_price),
        "executed_quantity": decimal(order.executed_quantity),
        "currency": order.currency,
        "settlement_currency": order.settlement_currency,
        "rebate": decimal(order.rebate),
        "rebate_dt": order.rebate_dt,
    })
}

/// `{card_id, locked[], settled[]}` output of `coupon orders`.
pub(crate) fn shape_coupon_orders(
    card_id: &str,
    locked: &[CouponOrder],
    settled: &[CouponOrder],
) -> Value {
    finish(json!({
        "card_id": card_id,
        "locked": locked.iter().map(shape_order).collect::<Vec<_>>(),
        "settled": settled.iter().map(shape_order).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::atm_shape::{parse_hong_kong_end, parse_hong_kong_start};

    fn rewards(list: Value) -> RewardRecordsResponse {
        serde_json::from_value(json!({ "list": list })).expect("rewards decode")
    }

    fn reward(item: Value) -> Option<Value> {
        shape_reward(&rewards(json!([item])).list[0])
    }

    #[test]
    fn shape_reward_drops_secrets() {
        let row = reward(json!({
            "reward_type": 1,
            "prize": {
                "activity_title": "Open account", "title": "Coffee", "status": 2,
                "send_time": 1_790_000_000,
                "coupon_secret": {"secret": "ABC-123"}, "coupon_secret_id": "9",
                "cash_card_code": "XYZ", "express_info": {"phone": "12345678"},
                "addresses": [{"detail": "somewhere"}]
            }
        }))
        .expect("a prize item must shape");
        assert_eq!(row["reward_type"], 1, "reward_type carried over");
        assert_eq!(row["reward_type_label"], "gift", "type labelled");
        assert_eq!(row["status_label"], "exited_activity", "status labelled");
        let text = row.to_string();
        for secret in ["ABC-123", "XYZ", "12345678", "somewhere"] {
            assert!(!text.contains(secret), "{secret} must be dropped");
        }
        assert!(
            row["send_time"]
                .as_str()
                .is_some_and(|t| t.starts_with("2026-") && t.ends_with("+08:00")),
            "send_time must be RFC3339 in Hong Kong time"
        );
    }

    #[test]
    fn shape_reward_labels_type_like_the_filter() {
        let row = reward(json!({"card": {"title": "t"}})).expect("shapes");
        assert_eq!(
            row["reward_type_label"], "coupon",
            "card detail reads as coupon"
        );
        let row = reward(json!({"reward_type": 3})).expect("typed record kept");
        assert_eq!(row["reward_type_label"], "task", "task labelled");
        assert!(reward(json!({"other": 1})).is_none(), "nothing to show");
    }

    #[test]
    fn deliver_label_only_for_issued_prizes() {
        let cash = reward(json!({"cash": {"status": 12, "deliver_status": 0}})).expect("shapes");
        assert!(
            cash.get("deliver_status_label").is_none_or(Value::is_null),
            "cash has no delivery"
        );
        let pending = reward(json!({"prize": {"status": 0, "deliver_status": 0}})).expect("shapes");
        assert!(
            pending
                .get("deliver_status_label")
                .is_none_or(Value::is_null),
            "not issued yet"
        );
        let issued = reward(json!({"reward_type": 1,
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
        let row = reward(json!({"reward_type": 2, "cash": null, "prize": null,
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
        assert!(row.get("cover").is_none(), "styling not modelled");
    }

    #[test]
    fn reward_send_time_zero_is_dropped() {
        let out = shape_reward_page(&rewards(
            json!([{"reward_type": 0, "cash": {"send_time": "0", "amount": 38}}]),
        ));
        assert!(
            out["rewards"][0].get("send_time").is_none(),
            "placeholder send_time dropped"
        );
        assert_eq!(out["rewards"][0]["amount"], "38", "amount as text");
    }

    #[test]
    fn reward_page_reports_total_in_hong_kong_time() {
        let page: RewardRecordsResponse = serde_json::from_value(json!({"list": [
            {"reward_type": 0, "cash": {"status": 12, "amount": 38, "currency": "HKD",
                "send_time": 1_764_064_746}},
        ], "total": 41}))
        .expect("decodes");
        let out = shape_reward_page(&page);
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
        let page = rewards(json!([
            {"reward_type": 0, "cash": {"title": "unsent"}},
            {"reward_type": 0, "cash": {"title": "inside", "send_time": 1_786_000_000}},
            {"reward_type": 0, "cash": {"title": "before", "send_time": 1_783_000_000}},
        ]));
        assert!(dated.add_page(&page), "short page ends the search");
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
    fn coupons_merge_pinned_sections_on_first_page_only() {
        // Shape of a real production lifetime commission card, 2026-11-25.
        let resp: CouponsResponse = serde_json::from_value(json!({
            "top_list": [{"card_id": "top"}],
            "list": [{
                "card_id": "bd6b40914cd411f08becae6ae6fae931", "card_type": "commission",
                "start_time": 1_750_313_704, "end_time": "8061747303", "card_status": 4,
                "activate_time": "0", "cover": "https://x/c.png", "member_id": "1",
                "commission": {"markets": ["US"], "bz_types": [0, 2, 3],
                    "commission_expired_time": "0"},
                "markets": ["US"], "rebate_rate": "1", "quote_duration": "0",
                "available_trade_session": "夜盘、盘中+盘前盘后",
                "coffee_coupon": {"code": "SECRET-1"},
                "description": "x".repeat(300), "detail": "terms",
            }, {"card_id": "ms", "end_time": 1_798_732_800_000_i64, "card_status": 0}],
            "bottom_list": [{"card_id": "bottom", "end_time": "0"}],
            "total": 3, "expired_count": 5, "tags": ["a"],
        }))
        .expect("coupons decode");
        let first = shape_coupons(&resp, 1, false);
        let ids: Vec<&str> = first["cards"]
            .as_array()
            .expect("cards")
            .iter()
            .filter_map(|c| c["card_id"].as_str())
            .collect();
        assert_eq!(
            ids,
            ["top", "bd6b40914cd411f08becae6ae6fae931", "ms", "bottom"],
            "pinned merged"
        );
        assert_eq!(first["total"], 3, "total kept");
        assert_eq!(first["expired_count"], 5, "expired count kept");
        assert!(first.get("tags").is_none(), "tags dropped");
        assert!(
            !first.to_string().contains("SECRET-1"),
            "redemption codes never shown"
        );
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
        let ms = &first["cards"][2];
        assert_eq!(
            ms["end_time"], "2027-01-01T00:00:00+08:00",
            "milliseconds converted"
        );
        assert!(
            ms.get("no_expiry").is_none(),
            "milliseconds are not forever"
        );
        assert!(ms.get("commission").is_none(), "no commission terms");
        let bottom = &first["cards"][3];
        assert!(
            bottom.get("end_time").is_none() && bottom.get("no_expiry").is_none(),
            "0 is unset"
        );

        let second = shape_coupons(&resp, 2, true);
        assert_eq!(
            second["cards"].as_array().map(Vec::len),
            Some(2),
            "pinned sections only on page 1"
        );
        assert_eq!(
            second["cards"][0]["detail"], "terms",
            "with terms keeps detail"
        );
    }

    #[test]
    fn coupons_keep_the_terms_of_their_type() {
        // Synthetic values in the server's wire format: every type key is
        // present, null for the types that do not apply.
        let resp: CouponsResponse = serde_json::from_value(json!({"list": [{
            "card_id": "c1", "card_type": "coffee_coupon", "card_status": 0,
            "subscription_id": "", "cash": null, "commission": null, "quote": null,
            "coffee_coupon": {"coupon_channel": "chan", "coupon_id": "", "coupon_no": "N1",
                "is_available_all_store": false, "period_type": "day", "settled_store_id": "0",
                "store_id_list": [], "store_list": [], "threshold": "2", "use_limit": "total",
                "cover": "https://x/c.png"},
        }, {
            "card_id": "c2", "card_type": "stock_fragment", "coffee_coupon": null,
            "stock_fragment": {"amounts": {}, "counter_id": "ST/US/X", "counter_name": "X",
                "market": "US", "markets": ["US"], "underlying_type": 0},
            "ai_benefit": null,
        }, {
            "card_id": "c3", "card_type": "ai_benefit",
            "ai_benefit": {"benefit_days": 30, "benefit_start": "1767196800",
                "benefit_end": "0", "benefit_tag": "t"},
            "commission": {"bz_types": [1], "markets": ["US"], "unlimited": 6,
                "commission_duration": "0", "usage_count": "1", "lave_usage_count": "0",
                "amount_limit": "0", "superposition": false, "commission_expired_time": "0"},
        }], "total": 3, "expired_count": 0}))
        .expect("coupons decode");
        let out = shape_coupons(&resp, 1, false);
        let coffee = &out["cards"][0];
        assert_eq!(
            coffee["coffee_coupon"]["coupon_no"], "N1",
            "coffee terms kept"
        );
        assert_eq!(coffee["coffee_coupon"]["threshold"], 2, "int64 as a number");
        assert!(
            coffee["coffee_coupon"].get("settled_store_id").is_none(),
            "unset store dropped"
        );
        assert!(
            coffee["coffee_coupon"].get("cover").is_none(),
            "styling dropped"
        );
        for key in [
            "cash",
            "commission",
            "quote",
            "stock_fragment",
            "subscription_id",
        ] {
            assert!(coffee.get(key).is_none(), "{key}: other types absent");
        }
        let fragment = &out["cards"][1]["stock_fragment"];
        assert_eq!(fragment["underlying_type"], 0, "underlying type code");
        assert_eq!(fragment["amounts"], json!({}), "amounts kept");
        let ai = &out["cards"][2];
        assert_eq!(
            ai["ai_benefit"]["benefit_start"], "2026-01-01T00:00:00+08:00",
            "Hong Kong time"
        );
        assert!(ai["ai_benefit"].get("benefit_end").is_none(), "0 is unset");
        assert_eq!(ai["commission"]["unlimited"], 6, "unlimited code");
        assert_eq!(ai["commission"]["bz_types"], json!([1]), "codes");
        assert_eq!(ai["commission"]["targets"], json!(["options"]), "named");
    }

    #[test]
    fn filters_map_and_reject() {
        assert_eq!(
            reward_type_filter(Some("coupon")).ok(),
            Some(Some(RewardType::Coupon)),
            "coupon"
        );
        assert_eq!(reward_type_filter(None).ok(), Some(None), "all");
        assert!(reward_type_filter(Some("stock")).is_err(), "unknown kind");
        assert_eq!(coupons_expired(None).ok(), Some(false), "default is valid");
        assert_eq!(
            coupons_expired(Some("Expired")).ok(),
            Some(true),
            "case-insensitive"
        );
        assert!(coupons_expired(Some("used")).is_err(), "unsupported filter");
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
    fn coupon_orders_keep_order_fields() {
        let locked: Vec<CouponOrder> = serde_json::from_value(json!([{"order_id": "1",
            "state": "Filled", "state_val": "FilledStatus", "executed_price": "10",
            "rebate": "1.5", "state_color": "#fff", "rebate_dt": ""}]))
        .expect("orders decode");
        let out = shape_coupon_orders("bd6b40914cd411f08becae6ae6fae931", &locked, &[]);
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
