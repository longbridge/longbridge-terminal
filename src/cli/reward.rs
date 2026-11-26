//! Reward-center commands: activity reward records, coupon cards, and the
//! orders a coupon was applied to.
//!
//! The data comes from the SDK's `AssetContext` reward APIs; JSON output
//! matches the Longbridge MCP server's reward tools, and the shaping lives in
//! [`super::reward_shape`].

use anyhow::Result;
use longbridge::asset::{CouponOrderType, GetCouponsOptions, GetRewardRecordsOptions};
use serde_json::Value;

use super::atm::{ap_call, ap_call_paced, cell, label_cell, print_shaped_json, time_cell};
use super::atm_shape::{clamp_page_size, page_number, parse_hong_kong_end, parse_hong_kong_start};
use super::output::print_table;
use super::reward_shape::{
    check_coupon_id, coupons_expired, reward_type_filter, shape_coupon_orders, shape_coupons,
    shape_reward_page, DatedRewards, DATED_PAGE_SIZE, MAX_REWARD_PAGES,
};
use super::OutputFormat;

/// Table cell for an item's `amount` followed by its `currency`, `-` when
/// there is no amount.
fn amount_cell(item: &Value) -> String {
    match (item.get("amount"), item.get("currency")) {
        (Some(a), Some(c)) => format!("{} {}", cell(a), cell(c)),
        (Some(a), None) => cell(a),
        _ => "-".to_string(),
    }
}

/// List the current account's activity reward records, newest first.
///
/// With `start` / `end`, pages through up to [`MAX_REWARD_PAGES`] pages of the
/// history (sequentially, retrying rate limits) and reports `searched_all`
/// instead of `total`.
pub async fn cmd_reward_list(
    kind: Option<&str>,
    start: Option<&str>,
    end: Option<&str>,
    page: u32,
    size: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let reward_type = reward_type_filter(kind)?;
    let start = start.map(parse_hong_kong_start).transpose()?;
    let end = end.map(parse_hong_kong_end).transpose()?;
    let ctx = crate::openapi::statement();
    let path = "/v1/account/rewards";
    let options = |page: i32, size: i32| {
        let opts = GetRewardRecordsOptions::new().page(page).page_size(size);
        match reward_type {
            Some(t) => opts.reward_type(t),
            None => opts,
        }
    };
    let value = if start.is_some() || end.is_some() {
        let mut dated = DatedRewards::new(start, end);
        let mut searched_all = true;
        let mut page: u32 = 1;
        loop {
            let opts = options(page.cast_signed(), DATED_PAGE_SIZE.cast_signed());
            let resp = match ap_call_paced("reward list", path, verbose, || {
                ctx.reward_records(opts.clone())
            })
            .await
            {
                Ok(resp) => resp,
                // A later page failed (e.g. still rate-limited): keep what was
                // read and say the search is incomplete.
                Err(_) if page > 1 => {
                    searched_all = false;
                    break;
                }
                Err(err) => return Err(err),
            };
            if dated.add_page(&resp) {
                break;
            }
            if page >= MAX_REWARD_PAGES {
                searched_all = false;
                break;
            }
            page += 1;
        }
        dated.output(searched_all)
    } else {
        let opts = options(page_number(page), clamp_page_size(size).cast_signed());
        let resp = ap_call("reward list", path, verbose, ctx.reward_records(opts)).await?;
        shape_reward_page(&resp)
    };
    match format {
        OutputFormat::Json => print_shaped_json(&value),
        OutputFormat::Pretty => {
            let rewards = value["rewards"].as_array().cloned().unwrap_or_default();
            if rewards.is_empty() {
                println!("No reward records.");
            } else {
                let headers = ["sent", "type", "activity", "reward", "amount", "status"];
                let rows = rewards
                    .iter()
                    .map(|r| {
                        let amount = amount_cell(r);
                        let status = match r.get("deliver_status_label") {
                            Some(d) => {
                                format!("{} ({})", label_cell(r, "status_label", "status"), cell(d))
                            }
                            None => label_cell(r, "status_label", "status"),
                        };
                        vec![
                            time_cell(&r["send_time"]),
                            label_cell(r, "reward_type_label", "reward_type"),
                            cell(&r["activity_title"]),
                            cell(&r["title"]),
                            amount,
                            status,
                        ]
                    })
                    .collect();
                print_table(&headers, rows, &OutputFormat::Pretty);
            }
            if value["searched_all"] == Value::Bool(false) {
                println!(
                    "\nOnly the newest {} records were searched; an older reward may be missing.",
                    DATED_PAGE_SIZE * MAX_REWARD_PAGES
                );
            } else if let Some(total) = value.get("total") {
                println!("\nTotal: {}", cell(total));
            }
        }
    }
    Ok(())
}

/// List the current account's coupon cards.
pub async fn cmd_coupon_list(
    status: Option<&str>,
    card_type: Option<&str>,
    with_terms: bool,
    page: u32,
    size: u32,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let expired = coupons_expired(status)?;
    let page = page.max(1);
    let mut opts = GetCouponsOptions::new()
        .page(page_number(page))
        .page_size(clamp_page_size(size).cast_signed())
        .expired(expired);
    if let Some(t) = card_type.map(str::trim).filter(|t| !t.is_empty()) {
        opts = opts.card_type(t);
    }
    let ctx = crate::openapi::statement();
    let resp = ap_call(
        "coupon list",
        "/v1/account/cards",
        verbose,
        ctx.coupons(opts),
    )
    .await?;
    let value = shape_coupons(&resp, page, with_terms);
    match format {
        OutputFormat::Json => print_shaped_json(&value),
        OutputFormat::Pretty => {
            let cards = value["cards"].as_array().cloned().unwrap_or_default();
            if cards.is_empty() {
                println!("No coupon cards.");
                return Ok(());
            }
            let headers = [
                "card id",
                "type",
                "title",
                "amount",
                "status",
                "valid from",
                "valid until",
            ];
            let rows = cards
                .iter()
                .map(|c| {
                    let until = if c["no_expiry"] == Value::Bool(true) {
                        "no expiry".to_string()
                    } else {
                        time_cell(&c["end_time"])
                    };
                    vec![
                        cell(&c["card_id"]),
                        cell(&c["card_type"]),
                        cell(&c["title"]),
                        amount_cell(c),
                        label_cell(c, "card_status_label", "card_status"),
                        time_cell(&c["start_time"]),
                        until,
                    ]
                })
                .collect();
            print_table(&headers, rows, &OutputFormat::Pretty);
            if let Some(total) = value.get("total") {
                print!("\nTotal: {}", cell(total));
                if let Some(expired) = value.get("expired_count") {
                    print!(", expired: {}", cell(expired));
                }
                println!();
            }
        }
    }
    Ok(())
}

/// List the orders a coupon card was applied to: `locked` (rebate not
/// settled yet) and `settled`.
pub async fn cmd_coupon_orders(card_id: &str, format: &OutputFormat, verbose: bool) -> Result<()> {
    let card_id = card_id.trim();
    check_coupon_id(card_id)?;
    let path = "/v1/account/card-orders";
    let ctx = crate::openapi::statement();
    // Sequential on purpose: the gateway rate-limits this route per member
    // tightly, and two concurrent calls draw 429s.
    let locked = ap_call_paced("coupon orders", path, verbose, || {
        ctx.coupon_orders(card_id, CouponOrderType::Locked)
    })
    .await?;
    let settled = ap_call_paced("coupon orders", path, verbose, || {
        ctx.coupon_orders(card_id, CouponOrderType::Settled)
    })
    .await?;
    let value = shape_coupon_orders(card_id, &locked, &settled);
    match format {
        OutputFormat::Json => print_shaped_json(&value),
        OutputFormat::Pretty => {
            let mut rows = Vec::new();
            for section in ["locked", "settled"] {
                for order in value[section].as_array().into_iter().flatten() {
                    rows.push(vec![
                        section.to_string(),
                        cell(&order["order_id"]),
                        cell(&order["state"]),
                        cell(&order["executed_price"]),
                        cell(&order["executed_quantity"]),
                        cell(&order["currency"]),
                        cell(&order["rebate"]),
                        cell(&order["rebate_dt"]),
                    ]);
                }
            }
            if rows.is_empty() {
                println!("No orders used this coupon.");
                return Ok(());
            }
            let headers = [
                "section",
                "order id",
                "state",
                "price",
                "quantity",
                "currency",
                "rebate",
                "rebate date",
            ];
            print_table(&headers, rows, &OutputFormat::Pretty);
        }
    }
    Ok(())
}

pub(crate) fn schema_for_path(path: &[String]) -> Option<super::schema::ResponseSchema> {
    use super::schema::{field, schema, RootKind};

    let command = path.join(" ");
    let schema = match command.as_str() {
        "reward list" => schema(
            "Activity reward records, newest first",
            RootKind::Object,
            vec![
                field(
                    "rewards",
                    "object[]",
                    "Records: {reward_type, reward_type_label (cash/gift/coupon/task), record_id, \
                     activity_title, title, description, card_type, card_status, prize_type, \
                     prize_type_label (physical: shipped goods), amount, currency, status, \
                     status_label (upcoming/cancelled/exited_activity/unclaimed/issuing/issued/\
                     failed/exchanged/expired/refunded), deliver_status, deliver_status_label \
                     (issued prizes only), send_time, start_time, end_time, effective_time, \
                     receive_deadline}. Times are Hong Kong time (+08:00)",
                ),
                field(
                    "total",
                    "number",
                    "All records of this type; only without --start/--end",
                ),
                field(
                    "searched_all",
                    "boolean",
                    "Only with --start/--end: false when older records were not read, so a \
                     missing reward may be among them",
                ),
            ],
        ),
        "coupon list" => schema(
            "Coupon cards",
            RootKind::Object,
            vec![
                field(
                    "cards",
                    "object[]",
                    "Cards: {card_id, card_type, title, title_sec, description, detail (with \
                     --with-terms), card_status, card_status_label (unused/used/expired/\
                     redeeming/in_use/processing), start_time, end_time (absent with no_expiry: \
                     true for a card that never expires), activate_time, amount, currency, \
                     available_trade_session, trade_sessions, subscription_id, and the terms of the \
                     card's own type (the other types are absent): commission{markets, targets \
                     (stocks/options/warrants/ETF/LOF), rebate_rate, bz_types (0 stocks, 1 options, \
                     2 warrants, 3 ETF, 4 LOF), commission_balance, commission_duration, unlimited \
                     (0 proportionate, 1 full, 2 lifetime, 6 by amount per, 7 by rate per), \
                     commission_expired_time, usage_count, lave_usage_count, settlement_model, \
                     amount_limit, superposition}, cash{receive_duration}, \
                     coffee_coupon{coupon_channel, coupon_id, coupon_no, is_available_all_store, \
                     period_type, settled_store_id, store_id_list, store_list[]{store_id, \
                     store_name}, threshold, use_limit}, ai_benefit{benefit_days, benefit_start, \
                     benefit_end, benefit_tag}, quote{…} and fundamental{…} (each \
                     {current_end_time, inapplicable, investor_type (0 any, 1 non-professional, 2 \
                     professional), manual_activate, markets, member_investor_type, pair_spu_no, \
                     quote_duration, quote_market, quote_tags, superposition}), stock{counter_id, \
                     counter_name, amount, redemption_type, send_way, \
                     optional_counters[]{counter_id, shares}, fallback{enabled, type, counter_id, \
                     shares}}, stock_fragment{counter_id, counter_name, market, markets, \
                     underlying_type (as bz_types), amounts}, ipo{subscription_id}, \
                     ipo_commission{bz_types (1 financing IPO, 2 cash IPO), counter_id, \
                     counter_name, financing_limit}, ipo_interest{discount_rate}, \
                     platform_fee_deduct{bz_types, counter_id, counter_name, markets, usage_count, \
                     lave_usage_count, superposition}, finance_interest{discount_mode, \
                     discount_rate, superposition}, fund_deduct{max_risk_level, min_order_amount, \
                     order_types, underlying_type, underlying_values, underlying_names}, \
                     crypto{fallback{enabled, counter_id}, optional_counters[]{counter_id}}}. Times \
                     are Hong Kong time (+08:00)",
                ),
                field("total", "number", "Total cards"),
                field("expired_count", "number", "Expired cards"),
            ],
        ),
        "coupon orders" => schema(
            "Orders a coupon card was applied to",
            RootKind::Object,
            vec![
                field("card_id", "string", "Coupon card id"),
                field(
                    "locked",
                    "object[]",
                    "Orders holding the card, rebate not settled yet: {order_id, state, \
                     state_val, executed_price, executed_quantity, currency, \
                     settlement_currency, rebate, rebate_dt}",
                ),
                field(
                    "settled",
                    "object[]",
                    "Orders whose rebate is settled (same fields)",
                ),
            ],
        ),
        _ => return None,
    };
    Some(schema)
}
