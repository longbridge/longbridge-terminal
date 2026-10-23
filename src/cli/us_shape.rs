//! AP-shaped `--format json` output for US data-center accounts.
//!
//! The US endpoints (`/v1/us/assets/*`, `/v1/us/orders/*`) answer with their
//! own field names. These mappers rename the fields that mean the same thing
//! as an AP field to the AP name, AP value format and AP top-level shape
//! (array vs object) the same command prints for an AP account. Fields with
//! no AP counterpart stay as extra keys (or under `us` where the AP shape has
//! no room); AP fields with no US source are omitted, never invented.

use std::str::FromStr;

use anyhow::Result;
use longbridge::trade::{
    CashFlowDirection, OrderStatus, USAssetOverview, USCashAndBuyPower, USCashDetail,
    USExchangeRate, USExchangeRates, USTransaction,
};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};

use crate::cli::output::{fmt_decimal, header_to_json_key, strip_counter_ids};
use crate::cli::trade::normalize_us_overview;
use crate::cli::us_asset::margin_status_is_empty;
use crate::utils::counter::counter_id_to_symbol;
use crate::utils::datetime::{fmt_rfc3339, format_timestamp};

/// Table rows for the pretty view of an array of mapped rows: one cell per
/// header, read from the snake-case key of that header (`-` when absent).
pub fn table_rows(data: &Value, headers: &[&str]) -> Vec<Vec<String>> {
    data.as_array()
        .map(|list| {
            list.iter()
                .map(|row| {
                    headers
                        .iter()
                        .map(|h| {
                            let v = text(row.get(header_to_json_key(h)));
                            if v.is_empty() {
                                "-".to_string()
                            } else {
                                v
                            }
                        })
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Insert `key` only when `value` is non-empty.
fn put(map: &mut Map<String, Value>, key: &str, value: &str) {
    if !value.is_empty() {
        map.insert(key.to_string(), Value::String(value.to_string()));
    }
}

/// A raw JSON value as text: strings verbatim, numbers printed, else empty.
fn text(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// A US price string in the AP `--format json` price format (3 decimals, `-`
/// when absent). Crypto prices with more significant decimals keep them
/// rather than being cut to 3.
fn price(s: &str) -> String {
    let d = Decimal::from_str(s.trim()).ok().map(|d| d.normalize());
    match d {
        Some(d) if d.scale() > 3 => d.to_string(),
        _ => fmt_decimal(&d),
    }
}

/// A US unix-seconds string as RFC 3339, the AP time format. `None` when it
/// is absent or zero.
fn unix_to_rfc3339(s: &str) -> Option<String> {
    s.trim()
        .parse::<i64>()
        .ok()
        .filter(|&n| n > 0)
        .map(format_timestamp)
}

/// Sign of a decimal string: `Some(true)` positive, `Some(false)` negative,
/// `None` zero or unparsable.
fn sign(s: &str) -> Option<bool> {
    let d = Decimal::from_str(s.trim()).ok()?;
    if d.is_zero() {
        None
    } else {
        Some(d.is_sign_positive())
    }
}

/// The AP `direction` of a US transaction: the sign of `amount`, or of
/// `quantity` when the amount is zero (crypto transfers and fees carry
/// `amount` `"0.00"` and a signed quantity).
pub fn cash_flow_direction(amount: &str, quantity: &str) -> String {
    let dir = match sign(amount).or_else(|| sign(quantity)) {
        Some(true) => CashFlowDirection::In,
        Some(false) => CashFlowDirection::Out,
        None => CashFlowDirection::Unknown,
    };
    format!("{dir:?}")
}

/// The AP order status name (`Filled`, `Canceled`, ...) of a US status code
/// (`FilledStatus`, `CanceledStatus`, ...).
pub fn order_status(s: &str) -> String {
    OrderStatus::from_str(s).map_or_else(
        |_| s.strip_suffix("Status").unwrap_or(s).to_string(),
        |st| format!("{st:?}"),
    )
}

/// The AP order side of a US `action` code.
fn order_side(action: Option<&Value>) -> &'static str {
    match action.and_then(Value::as_i64) {
        Some(1) => "Buy",
        Some(2) => "Sell",
        _ => "Unknown",
    }
}

/// `symbol_key` of a US order when the server sent it (the gateway renames
/// `counter_id` → `symbol` once its symbol conversion is deployed), else
/// derived from `counter_id_key` (`ST/US/INTC` → `INTC.US`).
fn symbol_or_derived(raw: &Map<String, Value>, symbol_key: &str, counter_id_key: &str) -> String {
    let symbol = text(raw.get(symbol_key));
    if !symbol.is_empty() {
        return symbol;
    }
    let counter_id = text(raw.get(counter_id_key));
    if counter_id.is_empty() {
        String::new()
    } else {
        counter_id_to_symbol(&counter_id)
    }
}

/// The user-facing symbol of a US order: its `symbol` when present, else
/// derived from `counter_id`.
pub fn order_symbol(raw: &Map<String, Value>) -> String {
    symbol_or_derived(raw, "symbol", "counter_id")
}

/// US-only order fields kept as extras (raw values), when present.
const ORDER_EXTRAS: &[&str] = &[
    "currency",
    "msg",
    "trigger_price",
    "last_done",
    "executed_amount",
    "gtd",
    "operate_direction",
    "security_type",
];

fn put_order_extras(m: &mut Map<String, Value>, raw: &Map<String, Value>) {
    put(m, "stock_name", &text(raw.get("name")));
    put(
        m,
        "underlying_symbol",
        &symbol_or_derived(raw, "underlying_symbol", "underlying_counter_id"),
    );
    for key in ORDER_EXTRAS {
        put(m, key, &text(raw.get(*key)));
    }
    // Raw codes whose meaning differs from (or is unverified against) the AP
    // enums of the same name: kept under US-specific names.
    for (from, to) in [("time_in_force", "us_time_in_force"), ("tag", "us_tag")] {
        if let Some(v) = raw.get(from).filter(|v| !v.is_null()) {
            m.insert(to.to_string(), v.clone());
        }
    }
}

/// One `order` / `order --history` row for a US order, keyed like the AP
/// row (`order_id`, `symbol`, `side`, `order_type`, `status`, `quantity`,
/// `price`, `executed_quantity`, `executed_price`, `created_at`) plus extras.
pub fn order_item(raw: &Value) -> Value {
    let Some(raw) = raw.as_object() else {
        return raw.clone();
    };
    let mut m = Map::new();
    m.insert("order_id".into(), text(raw.get("id")).into());
    m.insert("symbol".into(), order_symbol(raw).into());
    m.insert("side".into(), order_side(raw.get("action")).into());
    m.insert("order_type".into(), text(raw.get("order_type")).into());
    m.insert(
        "status".into(),
        order_status(&text(raw.get("status"))).into(),
    );
    m.insert("quantity".into(), text(raw.get("quantity")).into());
    m.insert("price".into(), price(&text(raw.get("price"))).into());
    m.insert(
        "executed_quantity".into(),
        text(raw.get("executed_qty")).into(),
    );
    m.insert(
        "executed_price".into(),
        price(&text(raw.get("executed_price"))).into(),
    );
    m.insert(
        "created_at".into(),
        unix_to_rfc3339(&text(raw.get("submitted_at")))
            .unwrap_or_default()
            .into(),
    );
    if let Some(t) = unix_to_rfc3339(&text(raw.get("updated_at"))) {
        m.insert("updated_at".into(), t.into());
    }
    put_order_extras(&mut m, raw);
    Value::Object(m)
}

/// `order` / `order --history` for US accounts: an array, as on AP.
pub fn orders_json(orders: &[Value]) -> Value {
    Value::Array(orders.iter().map(order_item).collect())
}

/// `order <id>` for US accounts, keyed like the AP order detail (`order_id`,
/// `stock_name`, `symbol`, `side`, `order_type`, `status`, `quantity`,
/// `price`, `executed_quantity`, `executed_price`, `executed_amount`,
/// `currency`, `submitted_at`, `updated_at`, `remark`, `history`). AP's
/// `time_in_force` and `outside_rth` have no verified US source and are
/// omitted. `raw` is the `/v1/us/orders/{id}` body (`{order: {...}}`) or the
/// order object itself.
pub fn order_detail_json(raw: &Value) -> Value {
    let order = raw.get("order").filter(|o| o.is_object()).unwrap_or(raw);
    let Some(o) = order.as_object() else {
        return order.clone();
    };
    let mut m = Map::new();
    m.insert("order_id".into(), text(o.get("id")).into());
    m.insert("symbol".into(), order_symbol(o).into());
    m.insert("side".into(), order_side(o.get("action")).into());
    m.insert("order_type".into(), text(o.get("order_type")).into());
    m.insert("status".into(), order_status(&text(o.get("status"))).into());
    m.insert("quantity".into(), text(o.get("quantity")).into());
    m.insert("price".into(), price(&text(o.get("price"))).into());
    m.insert(
        "executed_quantity".into(),
        text(o.get("executed_qty")).into(),
    );
    m.insert(
        "executed_price".into(),
        price(&text(o.get("executed_price"))).into(),
    );
    m.insert(
        "submitted_at".into(),
        unix_to_rfc3339(&text(o.get("submitted_at")))
            .unwrap_or_default()
            .into(),
    );
    // The detail endpoint reports the last transition as `done_at`.
    let updated = unix_to_rfc3339(&text(o.get("updated_at")))
        .or_else(|| unix_to_rfc3339(&text(o.get("done_at"))));
    m.insert("updated_at".into(), updated.unwrap_or_default().into());
    m.insert("remark".into(), text(o.get("msg")).into());
    let history: Vec<Value> = o
        .get("order_histories")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|h| {
                    json!({
                        "status": order_status(&text(h.get("status"))),
                        "time": unix_to_rfc3339(&text(h.get("time"))).unwrap_or_default(),
                        "price": text(h.get("price")),
                        "quantity": text(h.get("qty")),
                        "msg": text(h.get("msg")),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    m.insert("history".into(), Value::Array(history));
    put_order_extras(&mut m, o);
    m.remove("msg");
    if let Some(attached) = o
        .get("attached_orders")
        .filter(|v| v.as_array().is_some_and(|a| !a.is_empty()))
    {
        let mut attached = attached.clone();
        strip_counter_ids(&mut attached);
        m.insert("attached_orders".into(), attached);
    }
    Value::Object(m)
}

/// One `cash-flow` row for a US transaction, keyed like the AP row
/// (`flow_name`, `symbol`, `balance`, `currency`, `time`, `description`)
/// plus `direction` and the US extras. AP's `business_type` has no US source
/// and is omitted; `biz_code` / `biz_code_name` classify the row instead.
pub fn cash_flow_item(t: &USTransaction) -> Value {
    let balance = Decimal::from_str(t.amount.trim()).map_or_else(
        |_| t.amount.trim_start_matches('-').to_string(),
        |d| d.abs().to_string(),
    );
    let mut m = Map::new();
    m.insert("flow_name".into(), t.title.clone().into());
    m.insert("symbol".into(), t.symbol.clone().into());
    m.insert(
        "direction".into(),
        cash_flow_direction(&t.amount, &t.quantity).into(),
    );
    m.insert("balance".into(), balance.into());
    m.insert("currency".into(), t.currency.clone().into());
    m.insert(
        "time".into(),
        t.done_at
            .or(t.create_at)
            .map(fmt_rfc3339)
            .unwrap_or_default()
            .into(),
    );
    m.insert("description".into(), t.remark.clone().into());
    put(&mut m, "biz_code", &t.biz_code);
    put(&mut m, "biz_code_name", &t.biz_code_name);
    put(&mut m, "quantity", &t.quantity);
    put(&mut m, "quantity_description", &t.quantity_description);
    put(&mut m, "ref_no", &t.ref_no);
    put(&mut m, "equity_type", &t.equity_type);
    Value::Object(m)
}

/// `cash-flow` for US accounts: an array, as on AP.
pub fn cash_flow_json(items: &[USTransaction]) -> Value {
    Value::Array(items.iter().map(cash_flow_item).collect())
}

/// A US rate string as a JSON number, the AP rate type.
fn rate_number(s: &str) -> Value {
    if s.is_empty() {
        return Value::Null;
    }
    serde_json::from_str::<serde_json::Number>(s.trim())
        .map_or_else(|_| Value::String(s.to_string()), Value::Number)
}

/// Preference among the US rate types for one currency pair.
fn rate_type_rank(spec_key: &str) -> u8 {
    match spec_key {
        "reference" => 0,
        "am" => 1,
        _ => 2,
    }
}

/// `exchange-rate` for US accounts: `{exchanges: [...]}` with one row per
/// currency pair, numeric rates as on AP. The US table lists each pair once
/// per rate type; the `reference` rate wins, then `am`, and the chosen type
/// stays visible as `spec_key`.
pub fn exchange_rates_json(rates: &USExchangeRates) -> Value {
    let mut picked: Vec<&USExchangeRate> = Vec::new();
    for e in &rates.exchanges {
        let same_pair = |p: &&USExchangeRate| {
            p.base_currency == e.base_currency && p.other_currency == e.other_currency
        };
        match picked.iter_mut().find(|p| same_pair(p)) {
            Some(p) if rate_type_rank(&e.spec_key) < rate_type_rank(&p.spec_key) => *p = e,
            Some(_) => {}
            None => picked.push(e),
        }
    }
    let exchanges: Vec<Value> = picked
        .iter()
        .map(|e| {
            json!({
                "base_currency": e.base_currency,
                "other_currency": e.other_currency,
                "bid_rate": rate_number(&e.bid_rate),
                "offer_rate": rate_number(&e.offer_rate),
                "average_rate": rate_number(&e.average_rate),
                "spec_key": e.spec_key,
            })
        })
        .collect();
    json!({ "exchanges": exchanges })
}

/// The user-facing symbol of a US holding (`MULL.US`, `BTCUSD.BKKT`).
fn holding_symbol(raw: &Map<String, Value>) -> String {
    let full = text(raw.get("full_symbol"));
    if !full.is_empty() {
        return full;
    }
    let counter_id = text(raw.get("counter_id"));
    if !counter_id.is_empty() {
        return counter_id_to_symbol(&counter_id);
    }
    let symbol = text(raw.get("symbol"));
    let market = text(raw.get("market"));
    if symbol.contains('.') || symbol.is_empty() || market.is_empty() {
        symbol
    } else {
        format!("{symbol}.{market}")
    }
}

/// One holding keyed like the AP `positions` row (`symbol`, `name`,
/// `quantity`, `cost_price`, `currency`, `market`), followed by the
/// remaining non-empty US fields. AP's `available` has no source in the
/// overview and is omitted.
fn holding_item(raw: Value) -> Value {
    let Value::Object(mut raw) = raw else {
        return raw;
    };
    let mut m = Map::new();
    m.insert("symbol".into(), holding_symbol(&raw).into());
    for (to, from) in [
        ("name", "name"),
        ("quantity", "quantity"),
        ("cost_price", "average_cost"),
        ("currency", "currency"),
        ("market", "market"),
    ] {
        m.insert(to.into(), text(raw.get(from)).into());
        raw.remove(from);
    }
    if let Some(u) = raw.remove("underlying_counter_id") {
        put(
            &mut m,
            "underlying_symbol",
            &counter_id_to_symbol(&text(Some(&u))),
        );
    }
    for key in ["symbol", "full_symbol", "counter_id", "industry_counter_id"] {
        raw.remove(key);
    }
    for (k, v) in raw {
        let empty = v.is_null() || v.as_str().is_some_and(str::is_empty);
        if !empty {
            m.entry(k).or_insert(v);
        }
    }
    Value::Object(m)
}

/// Stock, option and crypto holdings in one AP-shaped list.
fn holdings(overview: &USAssetOverview) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for s in &overview.stock_list {
        out.push(holding_item(serde_json::to_value(s)?));
    }
    for o in &overview.option_list {
        out.push(holding_item(o.clone()));
    }
    for c in &overview.crypto_list {
        out.push(holding_item(serde_json::to_value(c)?));
    }
    Ok(out)
}

/// `positions` for US accounts: an array, as on AP. Stock, option and crypto
/// holdings share the list; `asset_type` tells them apart.
pub fn positions_json(overview: &USAssetOverview) -> Result<Value> {
    Ok(Value::Array(holdings(overview)?))
}

/// The account currency of a US cash / buying-power answer.
fn account_currency(data: &USCashAndBuyPower) -> String {
    [
        data.buy_limit_info.currency.as_str(),
        data.margin_status.currency.as_str(),
    ]
    .into_iter()
    .chain(data.cash_list.iter().map(|c| c.currency.as_str()))
    .find(|c| !c.is_empty())
    .unwrap_or_default()
    .to_string()
}

fn total_cash_in(cash_list: &[USCashDetail], currency: &str) -> String {
    cash_list
        .iter()
        .find(|c| c.currency == currency)
        .map(|c| c.total_amount.clone())
        .unwrap_or_default()
}

fn margin_status_json(data: &USCashAndBuyPower) -> Result<Value> {
    if margin_status_is_empty(&data.margin_status) {
        Ok(Value::Null)
    } else {
        Ok(serde_json::to_value(&data.margin_status)?)
    }
}

/// `assets` for US accounts: a one-element array (the account currency),
/// keyed like the AP balance. Mapped: `currency`, `net_assets` (margin
/// `nlv`), `total_cash`, `buy_power` (`cash_buy_power`), `maintenance_margin`
/// (`mmr`), `cash_infos`. AP-only fields with no US source are omitted;
/// buying-power and margin details are under `us`.
pub fn assets_json(data: &USCashAndBuyPower) -> Result<Value> {
    let currency = account_currency(data);
    let mut m = Map::new();
    m.insert("currency".into(), currency.clone().into());
    put(&mut m, "net_assets", &data.margin_status.nlv);
    put(
        &mut m,
        "total_cash",
        &total_cash_in(&data.cash_list, &currency),
    );
    put(&mut m, "buy_power", &data.buy_limit_info.cash_buy_power);
    put(&mut m, "maintenance_margin", &data.margin_status.mmr);
    let cash_infos: Vec<Value> = data
        .cash_list
        .iter()
        .map(|c| {
            let mut ci = Map::new();
            ci.insert("currency".into(), c.currency.clone().into());
            put(&mut ci, "available_cash", &c.available_cash);
            put(&mut ci, "frozen_cash", &c.frozen_cash);
            put(&mut ci, "withdraw_cash", &c.withdraw_cash);
            put(&mut ci, "total_amount", &c.total_amount);
            put(&mut ci, "settled_cash", &c.settled_cash);
            put(&mut ci, "outstanding", &c.outstanding);
            put(&mut ci, "external_amount", &c.external_amount);
            Value::Object(ci)
        })
        .collect();
    m.insert("cash_infos".into(), Value::Array(cash_infos));
    m.insert(
        "us".into(),
        json!({
            "buy_limit_info": serde_json::to_value(&data.buy_limit_info)?,
            "margin_status": margin_status_json(data)?,
        }),
    );
    Ok(Value::Array(vec![Value::Object(m)]))
}

/// `portfolio` for US accounts, keyed like the AP portfolio: `overview`
/// (`currency`, `total_asset` = margin `nlv`, `total_cash`), `holdings` (as
/// `positions` prints them) and `cash_balances`. AP's `market_accounts` and
/// the overview P/L, margin and risk fields have no US source and are
/// omitted; account type, buying power and margin status are under `us`.
pub fn portfolio_json(overview: &USAssetOverview, cash: &USCashAndBuyPower) -> Result<Value> {
    let norm = normalize_us_overview(overview)?;
    let currency = if overview.currency.is_empty() {
        account_currency(cash)
    } else {
        overview.currency.clone()
    };
    let mut ov = Map::new();
    ov.insert("currency".into(), currency.clone().into());
    put(&mut ov, "total_asset", &cash.margin_status.nlv);
    put(
        &mut ov,
        "total_cash",
        &total_cash_in(&cash.cash_list, &currency),
    );
    let cash_balances: Vec<Value> = cash
        .cash_list
        .iter()
        .map(|c| {
            let mut cb = Map::new();
            cb.insert("currency".into(), c.currency.clone().into());
            put(&mut cb, "total_amount", &c.total_amount);
            put(&mut cb, "balance", &c.available_cash);
            put(&mut cb, "frozen_cash", &c.frozen_cash);
            put(&mut cb, "withdraw_cash", &c.withdraw_cash);
            put(&mut cb, "settled_cash", &c.settled_cash);
            put(&mut cb, "outstanding", &c.outstanding);
            Value::Object(cb)
        })
        .collect();
    let mut us = Map::new();
    for key in [
        "account_type",
        "asset_timestamp",
        "cash_buy_power",
        "overnight_buy_power",
        "multi_leg",
    ] {
        if let Some(v) = norm.get(key) {
            us.insert(key.into(), v.clone());
        }
    }
    us.insert(
        "buy_limit_info".into(),
        serde_json::to_value(&cash.buy_limit_info)?,
    );
    us.insert("margin_status".into(), margin_status_json(cash)?);
    Ok(json!({
        "overview": Value::Object(ov),
        "holdings": holdings(overview)?,
        "cash_balances": cash_balances,
        "us": Value::Object(us),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use longbridge::trade::{USBuyLimitInfo, USMarginStatus, USTransactionHistory};

    /// Sanitized fixture shaped like a prod `/v1/us/orders/query` item.
    fn raw_order() -> Value {
        json!({
            "id": "900000000000000001", "status": "FilledStatus", "name": "Example Corp",
            "price": "20.5", "quantity": "2", "executed_price": "20.48", "executed_qty": "2",
            "submitted_at": "1790000000", "updated_at": "1790000060", "action": 2,
            "counter_id": "ST/US/EXMP", "code": "EXMP", "order_type": "LO", "msg": "",
            "tag": 1, "currency": "USD", "time_in_force": 0, "gtd": "2026-10-02",
            "trigger_price": "", "last_done": "", "security_type": "CS",
            "operate_direction": "CLOSE", "executed_amount": "40.96"
        })
    }

    #[test]
    fn counter_id_derives_the_order_symbol() {
        for (cid, want) in [
            ("ST/US/INTC", "INTC.US"),
            ("ETF/US/MULL", "MULL.US"),
            ("VA/BKKT/BTCUSD", "BTCUSD.BKKT"),
        ] {
            let raw = json!({ "counter_id": cid });
            assert_eq!(order_symbol(raw.as_object().unwrap()), want);
        }
        // Once the gateway converts, `symbol` is sent (and `counter_id` may
        // be gone); it wins over any `counter_id`.
        let raw = json!({ "symbol": "AAPL.US", "counter_id": "ST/US/OTHER" });
        assert_eq!(order_symbol(raw.as_object().unwrap()), "AAPL.US");
        let raw = json!({ "symbol": "AAPL.US" });
        assert_eq!(order_symbol(raw.as_object().unwrap()), "AAPL.US");
        let raw = json!({ "symbol": "", "counter_id": "ST/US/INTC" });
        assert_eq!(order_symbol(raw.as_object().unwrap()), "INTC.US");
        assert_eq!(order_symbol(json!({}).as_object().unwrap()), "");
    }

    #[test]
    fn order_underlying_symbol_prefers_the_server_value() {
        let converted = json!({
            "id": "1", "symbol": "AAPL261120C345000.US", "underlying_symbol": "AAPL.US",
            "action": 1, "status": "NewStatus"
        });
        let item = order_item(&converted);
        assert_eq!(item["symbol"], "AAPL261120C345000.US");
        assert_eq!(item["underlying_symbol"], "AAPL.US");

        let raw = json!({
            "id": "1", "counter_id": "OP/US/AAPL261120C345000",
            "underlying_counter_id": "ST/US/AAPL", "action": 1, "status": "NewStatus"
        });
        let item = order_item(&raw);
        assert_eq!(item["symbol"], "AAPL261120C345000.US");
        assert_eq!(item["underlying_symbol"], "AAPL.US");
        assert!(order_item(&raw_order()).get("underlying_symbol").is_none());
    }

    #[test]
    fn us_order_detail_uses_a_converted_symbol() {
        let mut order = raw_order();
        let m = order.as_object_mut().unwrap();
        m.remove("counter_id");
        m.insert("symbol".into(), json!("EXMP.US"));
        let got = order_detail_json(&json!({ "order": order }));
        assert_eq!(got["symbol"], "EXMP.US");
        let got = order_detail_json(&json!({ "order": raw_order() }));
        assert_eq!(got["symbol"], "EXMP.US");
    }

    #[test]
    fn prices_use_the_ap_format_without_cutting_crypto_decimals() {
        assert_eq!(price("119.00"), "119.000");
        assert_eq!(price("0"), "0.000");
        assert_eq!(price(""), "-");
        assert_eq!(price("0.9998"), "0.9998");
        assert_eq!(price("2690.3410"), "2690.341");
    }

    #[test]
    fn order_status_uses_the_ap_names() {
        assert_eq!(order_status("FilledStatus"), "Filled");
        assert_eq!(order_status("RejectedStatus"), "Rejected");
        assert_eq!(order_status("CanceledStatus"), "Canceled");
        assert_eq!(order_status("PartialFilledStatus"), "PartialFilled");
        assert_eq!(order_status("SomethingNewStatus"), "SomethingNew");
    }

    #[test]
    fn us_orders_golden() {
        let market = json!({
            "id": "900000000000000002", "status": "RejectedStatus", "name": "Bitcoin",
            "price": "", "quantity": "0.001", "executed_price": "0", "executed_qty": "0",
            "submitted_at": "1790000100", "action": 1, "counter_id": "VA/BKKT/BTCUSD",
            "order_type": "MO", "msg": "insufficient", "tag": 2, "currency": "USD",
            "time_in_force": 1
        });
        let got = orders_json(&[raw_order(), market]);
        assert_eq!(
            got,
            json!([
                {
                    "order_id": "900000000000000001", "symbol": "EXMP.US", "side": "Sell",
                    "order_type": "LO", "status": "Filled", "quantity": "2",
                    "price": "20.500", "executed_quantity": "2", "executed_price": "20.480",
                    "created_at": "2026-09-21T14:13:20Z", "updated_at": "2026-09-21T14:14:20Z",
                    "stock_name": "Example Corp", "currency": "USD", "executed_amount": "40.96",
                    "gtd": "2026-10-02", "operate_direction": "CLOSE", "security_type": "CS",
                    "us_time_in_force": 0, "us_tag": 1
                },
                {
                    "order_id": "900000000000000002", "symbol": "BTCUSD.BKKT", "side": "Buy",
                    "order_type": "MO", "status": "Rejected", "quantity": "0.001",
                    "price": "-", "executed_quantity": "0", "executed_price": "0.000",
                    "created_at": "2026-09-21T14:15:00Z", "stock_name": "Bitcoin",
                    "currency": "USD", "msg": "insufficient", "us_time_in_force": 1, "us_tag": 2
                }
            ])
        );
    }

    #[test]
    fn us_order_detail_golden() {
        let mut order = raw_order();
        order["done_at"] = json!("1790000060");
        order.as_object_mut().unwrap().remove("updated_at");
        order["msg"] = json!("done");
        order["order_histories"] = json!([
            {"price": "20.4800", "qty": "2", "status": "FilledStatus", "msg": "", "time": "1790000060"},
            {"price": "20.5000", "qty": "2", "status": "NewStatus", "msg": "", "time": "1790000000"}
        ]);
        order["attached_orders"] = json!([]);
        let got = order_detail_json(&json!({ "order": order, "current_millisecond": "1" }));
        assert_eq!(
            got,
            json!({
                "order_id": "900000000000000001", "stock_name": "Example Corp",
                "symbol": "EXMP.US", "side": "Sell", "order_type": "LO", "status": "Filled",
                "quantity": "2", "price": "20.500", "executed_quantity": "2",
                "executed_price": "20.480", "executed_amount": "40.96", "currency": "USD",
                "submitted_at": "2026-09-21T14:13:20Z", "updated_at": "2026-09-21T14:14:20Z",
                "remark": "done",
                "history": [
                    {"status": "Filled", "time": "2026-09-21T14:14:20Z", "price": "20.4800", "quantity": "2", "msg": ""},
                    {"status": "New", "time": "2026-09-21T14:13:20Z", "price": "20.5000", "quantity": "2", "msg": ""}
                ],
                "gtd": "2026-10-02", "operate_direction": "CLOSE", "security_type": "CS",
                "us_time_in_force": 0, "us_tag": 1
            })
        );
    }

    #[test]
    fn direction_follows_amount_then_quantity() {
        assert_eq!(cash_flow_direction("59.99", "-60"), "In");
        assert_eq!(cash_flow_direction("-60.02", "60"), "Out");
        assert_eq!(cash_flow_direction("0.00", "0.00012736"), "In");
        assert_eq!(cash_flow_direction("0.00", "-0.5"), "Out");
        assert_eq!(cash_flow_direction("0", ""), "Unknown");
    }

    #[test]
    fn us_cash_flow_golden() {
        // Sanitized copies of prod shapes: a cash withdrawal and a crypto
        // transfer-in (amount 0, signed quantity, no currency).
        let page: USTransactionHistory = serde_json::from_value(json!({
            "current_page": "1",
            "items": [
                {"title": "Withdrawal", "biz_code": "LCWD", "biz_code_name": "Cash withdrawal",
                 "ref_no": "R1", "remark": "", "create_at": "1790000000", "done_at": "1790000060",
                 "currency": "USD", "amount": "-100.00", "counter_id": "", "symbol": "",
                 "market": "", "equity_type": "", "position_side": "", "quantity": "",
                 "quantity_description": ""},
                {"title": "Crypto in: BTCUSD", "biz_code": "LVAVNSCR", "biz_code_name": "Crypto in",
                 "ref_no": "R2", "remark": "", "create_at": "1790000100", "done_at": "",
                 "currency": "", "amount": "0.00", "counter_id": "VA/BKKT/BTCUSD",
                 "symbol": "BTCUSD.BKKT", "market": "BKKT", "equity_type": "CRYPTO",
                 "position_side": "LONG", "quantity": "0.0001", "quantity_description": "+0.0001 BTC"}
            ]
        }))
        .unwrap();
        assert_eq!(
            cash_flow_json(&page.items),
            json!([
                {"flow_name": "Withdrawal", "symbol": "", "direction": "Out", "balance": "100.00",
                 "currency": "USD", "time": "2026-09-21T14:14:20Z", "description": "",
                 "biz_code": "LCWD", "biz_code_name": "Cash withdrawal", "ref_no": "R1"},
                {"flow_name": "Crypto in: BTCUSD", "symbol": "BTCUSD.BKKT", "direction": "In",
                 "balance": "0.00", "currency": "", "time": "2026-09-21T14:15:00Z",
                 "description": "", "biz_code": "LVAVNSCR", "biz_code_name": "Crypto in",
                 "quantity": "0.0001", "quantity_description": "+0.0001 BTC", "ref_no": "R2",
                 "equity_type": "CRYPTO"}
            ])
        );
    }

    #[test]
    fn exchange_rate_prefers_reference_then_am() {
        let rate = |spec: &str, base: &str, other: &str, avg: &str| USExchangeRate {
            universal_key: "acct".to_string(),
            spec_key: spec.to_string(),
            base_currency: base.to_string(),
            other_currency: other.to_string(),
            bid_rate: avg.to_string(),
            offer_rate: avg.to_string(),
            average_rate: avg.to_string(),
        };
        let rates = USExchangeRates {
            exchanges: vec![
                rate("am", "USD", "USD", "1"),
                rate("reference", "USD", "USD", "1"),
                rate("am", "USD", "HKD", "0.1282"),
            ],
        };
        assert_eq!(
            exchange_rates_json(&rates),
            json!({"exchanges": [
                {"base_currency": "USD", "other_currency": "USD", "bid_rate": 1,
                 "offer_rate": 1, "average_rate": 1, "spec_key": "reference"},
                {"base_currency": "USD", "other_currency": "HKD", "bid_rate": 0.1282,
                 "offer_rate": 0.1282, "average_rate": 0.1282, "spec_key": "am"}
            ]})
        );
    }

    fn overview_fixture() -> USAssetOverview {
        serde_json::from_value(json!({
            "account_type": "1",
            "currency": "USD",
            "cash_buy_power": "500",
            "cash_list": [{"currency": "USD", "total_cash": "500"}],
            "stock_list": [{
                "symbol": "EXMP", "full_symbol": "EXMP.US", "counter_id": "ETF/US/EXMP",
                "asset_type": "STOCK", "quantity": "5", "currency": "USD",
                "average_cost": "26.4", "market": "US", "last_done": "15.9",
                "market_price": "15.5", "prev_close": "18.2", "today_pl": "0.000",
                "position_side": "LONG", "name": "Example ETF", "industry_name": ""
            }],
            "crypto_list": [{
                "symbol": "BTCUSD", "asset_type": "CRYPTO", "quantity": "0.0001",
                "currency": "USD", "average_cost": "80000", "market": "BKKT",
                "last_done": "81000", "name": "Bitcoin", "position_side": "LONG"
            }]
        }))
        .unwrap()
    }

    #[test]
    fn us_positions_golden() {
        assert_eq!(
            positions_json(&overview_fixture()).unwrap(),
            json!([
                {"symbol": "EXMP.US", "name": "Example ETF", "quantity": "5", "cost_price": "26.4",
                 "currency": "USD", "market": "US", "asset_type": "STOCK", "last_done": "15.9",
                 "market_price": "15.5", "prev_close": "18.2", "today_pl": "0.000",
                 "position_side": "LONG"},
                {"symbol": "BTCUSD.BKKT", "name": "Bitcoin", "quantity": "0.0001",
                 "cost_price": "80000", "currency": "USD", "market": "BKKT",
                 "asset_type": "CRYPTO", "last_done": "81000", "position_side": "LONG"}
            ])
        );
    }

    fn cash_fixture() -> USCashAndBuyPower {
        USCashAndBuyPower {
            buy_limit_info: USBuyLimitInfo {
                currency: "USD".to_string(),
                cash_buy_power: "500".to_string(),
                ..Default::default()
            },
            cash_list: vec![USCashDetail {
                currency: "USD".to_string(),
                total_amount: "500".to_string(),
                available_cash: "450".to_string(),
                withdraw_cash: "440".to_string(),
                frozen_cash: "0".to_string(),
                settled_cash: "500".to_string(),
                outstanding: "0".to_string(),
                external_amount: String::new(),
            }],
            margin_status: USMarginStatus {
                currency: "USD".to_string(),
                nlv: "580".to_string(),
                mmr: "40".to_string(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn us_assets_golden() {
        let got = assets_json(&cash_fixture()).unwrap();
        assert_eq!(
            got,
            json!([{
                "currency": "USD", "net_assets": "580", "total_cash": "500", "buy_power": "500",
                "maintenance_margin": "40",
                "cash_infos": [{"currency": "USD", "available_cash": "450", "frozen_cash": "0",
                                "withdraw_cash": "440", "total_amount": "500",
                                "settled_cash": "500", "outstanding": "0"}],
                "us": {
                    "buy_limit_info": serde_json::to_value(&cash_fixture().buy_limit_info).unwrap(),
                    "margin_status": serde_json::to_value(&cash_fixture().margin_status).unwrap()
                }
            }])
        );
        let mut cash_account = cash_fixture();
        cash_account.margin_status = USMarginStatus {
            currency: "USD".to_string(),
            ..Default::default()
        };
        let got = assets_json(&cash_account).unwrap();
        assert!(got[0].get("net_assets").is_none());
        assert!(got[0]["us"]["margin_status"].is_null());
    }

    #[test]
    fn us_portfolio_golden() {
        let got = portfolio_json(&overview_fixture(), &cash_fixture()).unwrap();
        assert_eq!(
            got["overview"],
            json!({"currency": "USD", "total_asset": "580", "total_cash": "500"})
        );
        assert_eq!(
            got["holdings"],
            positions_json(&overview_fixture()).unwrap()
        );
        assert_eq!(
            got["cash_balances"],
            json!([{"currency": "USD", "total_amount": "500", "balance": "450",
                    "frozen_cash": "0", "withdraw_cash": "440", "settled_cash": "500",
                    "outstanding": "0"}])
        );
        assert_eq!(got["us"]["account_type"], "Cash");
        assert_eq!(got["us"]["cash_buy_power"], "500");
        assert_eq!(got["us"]["margin_status"]["nlv"], "580");
        assert!(got.get("market_accounts").is_none());
    }
}
