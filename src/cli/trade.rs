use anyhow::{bail, Result};
use longbridge::trade::{
    EstimateMaxPurchaseQuantityOptions, GetCashFlowOptions, GetHistoryExecutionsOptions,
    GetHistoryOrdersOptions, GetTodayExecutionsOptions, GetTodayOrdersOptions, OrderSide,
    OrderType, OutsideRTH, ReplaceOrderOptions, SubmitOrderOptions, TimeInForceType,
};
use rust_decimal::Decimal;
use std::fmt::Write as _;
use std::str::FromStr;

use super::{
    api::TradeApi,
    output::{fmt_datetime, fmt_decimal, parse_datetime_end, parse_datetime_start, print_table},
    OutputFormat,
};

fn risk_level_name(level: i32) -> &'static str {
    match level {
        0 => "Safe",
        1 => "Medium Risk",
        2 => "Early Warning",
        3 => "Danger",
        _ => "Unknown",
    }
}

fn parse_order_type(s: &str) -> Result<OrderType> {
    match s.to_uppercase().as_str() {
        "LO" => Ok(OrderType::LO),
        "MO" => Ok(OrderType::MO),
        "ELO" => Ok(OrderType::ELO),
        "AO" => Ok(OrderType::AO),
        "ALO" => Ok(OrderType::ALO),
        "ODD" => Ok(OrderType::ODD),
        "SLO" => Ok(OrderType::SLO),
        "LIT" => Ok(OrderType::LIT),
        "MIT" => Ok(OrderType::MIT),
        "TSLPAMT" => Ok(OrderType::TSLPAMT),
        "TSLPPCT" => Ok(OrderType::TSLPPCT),
        _ => {
            bail!("Unknown order type '{s}'. Use: LO MO ELO AO ALO ODD SLO LIT MIT TSLPAMT TSLPPCT")
        }
    }
}

fn parse_tif(s: &str) -> Result<TimeInForceType> {
    match s.to_lowercase().as_str() {
        "day" => Ok(TimeInForceType::Day),
        "gtc" | "goodtilcanceled" => Ok(TimeInForceType::GoodTilCanceled),
        "gtd" | "goodtildate" => Ok(TimeInForceType::GoodTilDate),
        _ => bail!("Unknown time in force '{s}'. Use: day gtc gtd"),
    }
}

pub async fn cmd_orders(
    history: bool,
    start: Option<String>,
    end: Option<String>,
    symbol: Option<String>,
    format: &OutputFormat,
) -> Result<()> {
    let ctx = crate::openapi::trade();

    let orders = if history {
        let mut opts = GetHistoryOrdersOptions::new();
        if let Some(s) = symbol {
            opts = opts.symbol(s);
        }
        if let Some(s) = start {
            opts = opts.start_at(parse_datetime_start(&s)?);
        }
        if let Some(e) = end {
            opts = opts.end_at(parse_datetime_end(&e)?);
        }
        ctx.history_orders(opts).await?
    } else {
        let opts = longbridge::trade::GetTodayOrdersOptions::new();
        let opts = if let Some(s) = symbol {
            opts.symbol(s)
        } else {
            opts
        };
        ctx.today_orders(opts).await?
    };

    let headers = &[
        "Order ID",
        "Symbol",
        "Side",
        "Order Type",
        "Status",
        "Quantity",
        "Price",
        "Executed Quantity",
        "Executed Price",
        "Created At",
    ];
    let rows = orders
        .iter()
        .map(|o| {
            vec![
                o.order_id.clone(),
                o.symbol.clone(),
                format!("{:?}", o.side),
                format!("{:?}", o.order_type),
                format!("{:?}", o.status),
                o.quantity.to_string(),
                fmt_decimal(&o.price),
                o.executed_quantity.to_string(),
                fmt_decimal(&o.executed_price),
                fmt_datetime(o.submitted_at),
            ]
        })
        .collect();

    print_table(headers, rows, format);
    Ok(())
}

pub async fn cmd_order_detail(order_id: String, format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::trade();
    let detail = ctx.order_detail(order_id).await?;

    match format {
        OutputFormat::Json => {
            let val = serde_json::json!({
                "order_id": detail.order_id,
                "symbol": detail.symbol,
                "side": format!("{:?}", detail.side),
                "order_type": format!("{:?}", detail.order_type),
                "status": format!("{:?}", detail.status),
                "quantity": detail.quantity.to_string(),
                "price": fmt_decimal(&detail.price),
                "executed_quantity": detail.executed_quantity.to_string(),
                "executed_price": fmt_decimal(&detail.executed_price),
                "submitted_at": fmt_datetime(detail.submitted_at),
                "updated_at": detail.updated_at.map(fmt_datetime).unwrap_or_default(),
                "remark": detail.msg,
            });
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Pretty => {
            let headers = &["Field", "Value"];
            let rows = vec![
                vec!["Order ID".to_string(), detail.order_id.clone()],
                vec!["Symbol".to_string(), detail.symbol.clone()],
                vec!["Side".to_string(), format!("{:?}", detail.side)],
                vec!["Order Type".to_string(), format!("{:?}", detail.order_type)],
                vec!["Status".to_string(), format!("{:?}", detail.status)],
                vec!["Quantity".to_string(), detail.quantity.to_string()],
                vec!["Price".to_string(), fmt_decimal(&detail.price)],
                vec![
                    "Executed Qty".to_string(),
                    detail.executed_quantity.to_string(),
                ],
                vec![
                    "Executed Price".to_string(),
                    fmt_decimal(&detail.executed_price),
                ],
                vec![
                    "Submitted At".to_string(),
                    fmt_datetime(detail.submitted_at),
                ],
                vec![
                    "Updated At".to_string(),
                    detail.updated_at.map(fmt_datetime).unwrap_or_default(),
                ],
                vec!["Remark".to_string(), detail.msg.clone()],
            ];
            print_table(headers, rows, &OutputFormat::Pretty);
        }
    }
    Ok(())
}

pub async fn cmd_executions(
    history: bool,
    start: Option<String>,
    end: Option<String>,
    symbol: Option<String>,
    format: &OutputFormat,
) -> Result<()> {
    let ctx = crate::openapi::trade();

    let executions = if history {
        let mut opts = longbridge::trade::GetHistoryExecutionsOptions::new();
        if let Some(s) = symbol {
            opts = opts.symbol(s);
        }
        if let Some(s) = start {
            opts = opts.start_at(parse_datetime_start(&s)?);
        }
        if let Some(e) = end {
            opts = opts.end_at(parse_datetime_end(&e)?);
        }
        ctx.history_executions(opts).await?
    } else {
        let mut opts = longbridge::trade::GetTodayExecutionsOptions::new();
        if let Some(s) = symbol {
            opts = opts.symbol(s);
        }
        ctx.today_executions(opts).await?
    };

    let headers = &[
        "Order ID", "Trade ID", "Symbol", "Price", "Quantity", "Time",
    ];
    let rows = executions
        .iter()
        .map(|e| {
            vec![
                e.order_id.clone(),
                e.trade_id.clone(),
                e.symbol.clone(),
                e.price.to_string(),
                e.quantity.to_string(),
                fmt_datetime(e.trade_done_at),
            ]
        })
        .collect();

    print_table(headers, rows, format);
    Ok(())
}

fn parse_outside_rth(s: &str) -> Result<OutsideRTH> {
    match s.to_uppercase().as_str() {
        "RTH_ONLY" => Ok(OutsideRTH::RTHOnly),
        "ANY_TIME" => Ok(OutsideRTH::AnyTime),
        "OVERNIGHT" => Ok(OutsideRTH::Overnight),
        _ => bail!("Unknown outside-rth '{s}'. Use: RTH_ONLY ANY_TIME OVERNIGHT"),
    }
}

pub async fn cmd_submit_order(
    symbol: String,
    quantity: u64,
    price: Option<String>,
    trigger_price: Option<String>,
    trailing_amount: Option<String>,
    trailing_percent: Option<String>,
    limit_offset: Option<String>,
    expire_date: Option<String>,
    outside_rth: Option<String>,
    remark: Option<String>,
    order_type: String,
    tif: String,
    side: OrderSide,
    yes: bool,
    format: &OutputFormat,
) -> Result<()> {
    use std::io::Write;
    let ot = parse_order_type(&order_type)?;
    let tif_val = parse_tif(&tif)?;
    let qty = Decimal::from(quantity);

    let mut opts = SubmitOrderOptions::new(symbol.clone(), ot, side, qty, tif_val);
    if let Some(ref p) = price {
        let price_dec = Decimal::from_str(p).map_err(|_| anyhow::anyhow!("Invalid price: {p}"))?;
        opts = opts.submitted_price(price_dec);
    }
    if let Some(ref tp) = trigger_price {
        let tp_dec =
            Decimal::from_str(tp).map_err(|_| anyhow::anyhow!("Invalid trigger price: {tp}"))?;
        opts = opts.trigger_price(tp_dec);
    }
    if let Some(ref ta) = trailing_amount {
        let ta_dec =
            Decimal::from_str(ta).map_err(|_| anyhow::anyhow!("Invalid trailing amount: {ta}"))?;
        opts = opts.trailing_amount(ta_dec);
    }
    if let Some(ref tp) = trailing_percent {
        let tp_dec =
            Decimal::from_str(tp).map_err(|_| anyhow::anyhow!("Invalid trailing percent: {tp}"))?;
        opts = opts.trailing_percent(tp_dec);
    }
    if let Some(ref lo) = limit_offset {
        let lo_dec =
            Decimal::from_str(lo).map_err(|_| anyhow::anyhow!("Invalid limit offset: {lo}"))?;
        opts = opts.limit_offset(lo_dec);
    }
    if let Some(ref ed) = expire_date {
        let date = time::Date::parse(
            ed,
            &time::format_description::parse("[year]-[month]-[day]")
                .map_err(|e| anyhow::anyhow!("Date format error: {e}"))?,
        )
        .map_err(|_| anyhow::anyhow!("Invalid expire date '{ed}'. Use YYYY-MM-DD"))?;
        opts = opts.expire_date(date);
    }
    if let Some(ref rth) = outside_rth {
        opts = opts.outside_rth(parse_outside_rth(rth)?);
    }
    if let Some(ref r) = remark {
        opts = opts.remark(r.clone());
    }

    // Confirm before submitting
    let mut price_display = match (price.as_deref(), trigger_price.as_deref()) {
        (Some(p), Some(tp)) => format!("{p} (trigger: {tp})"),
        (Some(p), None) => p.to_string(),
        (None, Some(tp)) => format!("market (trigger: {tp})"),
        (None, None) => "market".to_string(),
    };
    if let Some(ref ta) = trailing_amount {
        let _ = write!(price_display, " trailing-amount: {ta}");
    }
    if let Some(ref tp) = trailing_percent {
        let _ = write!(price_display, " trailing-percent: {tp}%");
    }
    if let Some(ref lo) = limit_offset {
        let _ = write!(price_display, " limit-offset: {lo}");
    }
    if let Some(ref ed) = expire_date {
        let _ = write!(price_display, " expire: {ed}");
    }
    if let Some(ref rth) = outside_rth {
        let _ = write!(price_display, " outside-rth: {rth}");
    }
    println!("Submitting {side:?} order: {quantity} {symbol} @ {price_display}");
    if !yes {
        print!("Confirm? [y/N] ");
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if input.trim().to_lowercase() != "y" {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let ctx = crate::openapi::trade();
    let resp = ctx.submit_order(opts).await?;

    match format {
        OutputFormat::Json => {
            let val = serde_json::json!({ "order_id": resp.order_id });
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Pretty => {
            println!("Order submitted successfully.");
            println!("Order ID: {}", resp.order_id);
        }
    }
    Ok(())
}

pub async fn cmd_cancel_order(order_id: String, yes: bool) -> Result<()> {
    use std::io::Write;
    if !yes {
        print!("Cancel order {order_id}? [y/N] ");
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if input.trim().to_lowercase() != "y" {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let ctx = crate::openapi::trade();
    ctx.cancel_order(order_id.clone()).await?;
    println!("Order {order_id} cancelled.");
    Ok(())
}

pub async fn cmd_replace_order(
    order_id: String,
    qty: Option<u64>,
    price: Option<String>,
    yes: bool,
) -> Result<()> {
    use std::io::Write;
    let quantity = qty.ok_or_else(|| anyhow::anyhow!("--qty is required"))?;
    let qty_dec = Decimal::from(quantity);

    let mut opts = ReplaceOrderOptions::new(order_id.clone(), qty_dec);
    if let Some(p) = price {
        let price_dec = Decimal::from_str(&p).map_err(|_| anyhow::anyhow!("Invalid price: {p}"))?;
        opts = opts.price(price_dec);
    }

    if !yes {
        print!("Modify order {order_id}? [y/N] ");
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if input.trim().to_lowercase() != "y" {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let ctx = crate::openapi::trade();
    ctx.replace_order(opts).await?;
    println!("Order {order_id} modified.");
    Ok(())
}

fn print_assets(balances: &[longbridge::trade::AccountBalance], format: &OutputFormat) {
    match format {
        OutputFormat::Json => {
            let records: Vec<serde_json::Value> = balances
                .iter()
                .map(|b| {
                    let cash_infos: Vec<serde_json::Value> = b
                        .cash_infos
                        .iter()
                        .map(|c| {
                            serde_json::json!({
                                "currency": c.currency,
                                "available_cash": c.available_cash.to_string(),
                                "frozen_cash": c.frozen_cash.to_string(),
                                "settling_cash": c.settling_cash.to_string(),
                                "withdraw_cash": c.withdraw_cash.to_string(),
                            })
                        })
                        .collect();
                    serde_json::json!({
                        "currency": b.currency,
                        "net_assets": b.net_assets.to_string(),
                        "total_cash": b.total_cash.to_string(),
                        "buy_power": b.buy_power.to_string(),
                        "max_finance_amount": b.max_finance_amount.to_string(),
                        "remaining_finance_amount": b.remaining_finance_amount.to_string(),
                        "init_margin": b.init_margin.to_string(),
                        "maintenance_margin": b.maintenance_margin.to_string(),
                        "margin_call": b.margin_call.to_string(),
                        "risk_level": risk_level_name(b.risk_level),
                        "cash_infos": cash_infos,
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&records).unwrap_or_default()
            );
        }
        OutputFormat::Pretty => {
            let headers = &[
                "Currency",
                "Net Assets",
                "Total Cash",
                "Buy Power",
                "Max Finance",
                "Remaining Finance",
                "Init Margin",
                "Maintenance Margin",
                "Margin Call",
                "Risk Level",
            ];
            let rows = balances
                .iter()
                .map(|b| {
                    vec![
                        b.currency.clone(),
                        b.net_assets.to_string(),
                        b.total_cash.to_string(),
                        b.buy_power.to_string(),
                        b.max_finance_amount.to_string(),
                        b.remaining_finance_amount.to_string(),
                        b.init_margin.to_string(),
                        b.maintenance_margin.to_string(),
                        b.margin_call.to_string(),
                        risk_level_name(b.risk_level).to_string(),
                    ]
                })
                .collect::<Vec<_>>();
            print_table(headers, rows, format);

            let cash_headers = &[
                "Currency",
                "Available Cash",
                "Frozen Cash",
                "Settling Cash",
                "Withdrawable",
            ];
            let mut cash_rows = vec![];
            for b in balances {
                for c in &b.cash_infos {
                    cash_rows.push(vec![
                        c.currency.clone(),
                        c.available_cash.to_string(),
                        c.frozen_cash.to_string(),
                        c.settling_cash.to_string(),
                        c.withdraw_cash.to_string(),
                    ]);
                }
            }
            if !cash_rows.is_empty() {
                println!();
                print_table(cash_headers, cash_rows, format);
            }
        }
    }
}

pub async fn cmd_assets(currency: Option<String>, format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::trade();
    let balances = ctx.account_balance(currency.as_deref()).await?;
    print_assets(&balances, format);
    Ok(())
}

pub async fn cmd_cash_flow(
    start: Option<String>,
    end: Option<String>,
    format: &OutputFormat,
) -> Result<()> {
    let ctx = crate::openapi::trade();

    let now = time::OffsetDateTime::now_utc();
    let start_at = start
        .as_deref()
        .map(parse_datetime_start)
        .transpose()?
        .unwrap_or_else(|| now - time::Duration::days(30));
    let end_at = end
        .as_deref()
        .map(parse_datetime_end)
        .transpose()?
        .unwrap_or(now);

    let opts = longbridge::trade::GetCashFlowOptions::new(start_at, end_at);
    let flows = ctx.cash_flow(opts).await?;

    let headers = &[
        "Flow Name",
        "Symbol",
        "Business Type",
        "Balance",
        "Currency",
        "Time",
        "Description",
    ];
    let rows = flows
        .iter()
        .map(|f| {
            vec![
                f.transaction_flow_name.clone(),
                f.symbol.clone().unwrap_or_default(),
                format!("{:?}", f.business_type),
                f.balance.to_string(),
                f.currency.clone(),
                fmt_datetime(f.business_time),
                f.description.clone(),
            ]
        })
        .collect();

    print_table(headers, rows, format);
    Ok(())
}

pub async fn cmd_positions(format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::trade();
    let resp = ctx.stock_positions(None).await?;

    let headers = &[
        "Symbol",
        "Name",
        "Quantity",
        "Available",
        "Cost Price",
        "Currency",
        "Market",
    ];
    let mut rows = vec![];
    for channel in &resp.channels {
        for pos in &channel.positions {
            rows.push(vec![
                pos.symbol.clone(),
                pos.symbol_name.clone(),
                pos.quantity.to_string(),
                pos.available_quantity.to_string(),
                pos.cost_price.to_string(),
                pos.currency.clone(),
                format!("{:?}", pos.market),
            ]);
        }
    }

    print_table(headers, rows, format);
    Ok(())
}

pub async fn cmd_fund_positions(format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::trade();
    let resp = ctx.fund_positions(None).await?;

    let headers = &[
        "Symbol",
        "Name",
        "Net Asset Value",
        "Cost Net Asset Value",
        "Currency",
        "Holding Units",
    ];
    let mut rows = vec![];
    for channel in &resp.channels {
        for pos in &channel.positions {
            rows.push(vec![
                pos.symbol.clone(),
                pos.symbol_name.clone(),
                pos.current_net_asset_value.to_string(),
                pos.cost_net_asset_value.to_string(),
                pos.currency.clone(),
                pos.holding_units.to_string(),
            ]);
        }
    }

    print_table(headers, rows, format);
    Ok(())
}

pub async fn cmd_margin_ratio(symbol: String, format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::trade();
    let ratio = ctx.margin_ratio(symbol.clone()).await?;

    let headers = &["Field", "Value"];
    let rows = vec![
        vec!["Symbol".to_string(), symbol],
        vec![
            "Initial Margin Ratio".to_string(),
            ratio.im_factor.to_string(),
        ],
        vec![
            "Maintenance Margin Ratio".to_string(),
            ratio.mm_factor.to_string(),
        ],
        vec![
            "Forced Liquidation Ratio".to_string(),
            ratio.fm_factor.to_string(),
        ],
    ];

    print_table(headers, rows, format);
    Ok(())
}

pub async fn cmd_max_qty(
    symbol: String,
    side: &str,
    price: Option<String>,
    order_type: &str,
    format: &OutputFormat,
) -> Result<()> {
    let ctx = crate::openapi::trade();
    let side_val = match side.to_lowercase().as_str() {
        "buy" => OrderSide::Buy,
        "sell" => OrderSide::Sell,
        _ => bail!("Unknown side '{side}'. Use: Buy Sell"),
    };
    let ot = parse_order_type(order_type)?;

    let price_dec = price
        .as_deref()
        .map(|p| Decimal::from_str(p).map_err(|_| anyhow::anyhow!("Invalid price: {p}")))
        .transpose()?;

    let opts =
        longbridge::trade::EstimateMaxPurchaseQuantityOptions::new(symbol.clone(), ot, side_val);
    let opts = if let Some(p) = price_dec {
        opts.price(p)
    } else {
        opts
    };

    let resp = ctx.estimate_max_purchase_quantity(opts).await?;

    let headers = &["Field", "Value"];
    let rows = vec![
        vec!["Symbol".to_string(), symbol],
        vec!["Cash Max Qty".to_string(), resp.cash_max_qty.to_string()],
        vec![
            "Margin Max Qty".to_string(),
            resp.margin_max_qty.to_string(),
        ],
    ];

    print_table(headers, rows, format);
    Ok(())
}

pub async fn cmd_portfolio(format: &OutputFormat) -> Result<()> {
    let portfolio = crate::openapi::account::fetch_portfolio().await?;

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&portfolio)?);
        }
        OutputFormat::Pretty => {
            let o = &portfolio.overview;

            // Overview section
            let risk_label = match o.risk_level {
                0 => "Safe",
                1 => "Middle",
                2 => "Warning",
                3 => "Danger",
                _ => "Unknown",
            };
            let overview_headers = &["Field", "Value"];
            let overview_rows = vec![
                vec!["Currency".to_string(), o.currency.clone()],
                vec!["Total Asset".to_string(), fmt_decimal(&Some(o.total_asset))],
                vec!["Market Cap".to_string(), fmt_decimal(&Some(o.market_cap))],
                vec!["Total Cash".to_string(), fmt_decimal(&Some(o.total_cash))],
                vec!["P/L".to_string(), format!("{:.2}", o.total_pl)],
                vec![
                    "Intraday P/L".to_string(),
                    format!("{:.2}", o.total_today_pl),
                ],
                vec!["Margin Call".to_string(), fmt_decimal(&Some(o.margin_call))],
                vec!["Risk Level".to_string(), risk_label.to_string()],
                vec![
                    "Credit Limit".to_string(),
                    fmt_decimal(&Some(o.credit_limit)),
                ],
                vec![
                    "Fund Market Value".to_string(),
                    fmt_decimal(&Some(o.fund_market_value)),
                ],
            ];
            print_table(overview_headers, overview_rows, format);

            // Asset distribution section
            {
                println!();
                let total = o.total_asset;
                // Aggregate USD market value per market (from symbol suffix)
                let mut market_values: std::collections::BTreeMap<String, rust_decimal::Decimal> =
                    std::collections::BTreeMap::new();
                for h in &portfolio.holdings {
                    let market_label = if let Some(dot_pos) = h.symbol.rfind('.') {
                        match &h.symbol[dot_pos + 1..] {
                            "US" => "US",
                            "SH" | "SZ" => "CN",
                            "SG" => "SG",
                            _ => "HK",
                        }
                    } else {
                        "HK"
                    };
                    *market_values.entry(market_label.to_string()).or_default() +=
                        h.market_value_usd;
                }
                // Add cash and fund
                market_values.insert("Cash".to_string(), o.total_cash);
                if o.fund_market_value > rust_decimal::Decimal::ZERO {
                    market_values.insert("Fund".to_string(), o.fund_market_value);
                }

                // Sort by value descending
                let mut dist: Vec<(String, rust_decimal::Decimal)> =
                    market_values.into_iter().collect();
                dist.sort_by(|a, b| b.1.cmp(&a.1));

                let dist_headers = &["Market", "Value (USD)", "%"];
                let dist_rows = dist
                    .iter()
                    .map(|(label, value)| {
                        let pct = if total > rust_decimal::Decimal::ZERO {
                            value / total * rust_decimal::Decimal::ONE_HUNDRED
                        } else {
                            rust_decimal::Decimal::ZERO
                        };
                        vec![
                            label.clone(),
                            format!("{:.2}", value),
                            format!("{:.2}%", pct),
                        ]
                    })
                    .collect::<Vec<_>>();
                print_table(dist_headers, dist_rows, format);
            }

            // Holdings section
            if portfolio.holdings.is_empty() {
                println!("\nNo holdings data");
            } else {
                println!();
                let holding_headers = &[
                    "Symbol",
                    "Name",
                    "Qty",
                    "Price",
                    "Cost",
                    "Mkt Value",
                    "P/L",
                    "P/L%",
                    "Intraday",
                    "Intraday%",
                    "Currency",
                ];
                let holding_rows = portfolio
                    .holdings
                    .iter()
                    .map(|h| {
                        let pl = h.cost_price.map_or(rust_decimal::Decimal::ZERO, |cost| {
                            (h.market_price - cost) * h.quantity
                        });
                        let pl_pct = h
                            .cost_price
                            .filter(|&c| c > rust_decimal::Decimal::ZERO)
                            .map_or(rust_decimal::Decimal::ZERO, |cost| {
                                (h.market_price - cost) / cost * rust_decimal::Decimal::ONE_HUNDRED
                            });
                        let today_pl = h
                            .prev_close
                            .filter(|&pc| pc > rust_decimal::Decimal::ZERO)
                            .map_or(rust_decimal::Decimal::ZERO, |pc| {
                                (h.market_price - pc) * h.quantity
                            });
                        let today_pl_pct = h
                            .prev_close
                            .filter(|&pc| pc > rust_decimal::Decimal::ZERO)
                            .map_or(rust_decimal::Decimal::ZERO, |pc| {
                                (h.market_price - pc) / pc * rust_decimal::Decimal::ONE_HUNDRED
                            });
                        vec![
                            h.symbol.clone(),
                            h.name.clone(),
                            h.quantity.to_string(),
                            fmt_decimal(&Some(h.market_price)),
                            h.cost_price
                                .map(|c| fmt_decimal(&Some(c)))
                                .unwrap_or_default(),
                            fmt_decimal(&Some(h.market_value)),
                            fmt_decimal(&Some(pl)),
                            format!("{:.2}%", pl_pct),
                            fmt_decimal(&Some(today_pl)),
                            format!("{:.2}%", today_pl_pct),
                            format!("{:?}", h.currency),
                        ]
                    })
                    .collect::<Vec<_>>();
                print_table(holding_headers, holding_rows, format);
            }

            // Cash balances section
            if !portfolio.cash_balances.is_empty() {
                println!();
                let cash_headers = &["Currency", "Total", "Available", "Frozen", "Withdrawable"];
                let cash_rows = portfolio
                    .cash_balances
                    .iter()
                    .map(|c| {
                        vec![
                            format!("{:?}", c.currency),
                            fmt_decimal(&Some(c.total_amount)),
                            fmt_decimal(&Some(c.balance)),
                            fmt_decimal(&Some(c.frozen_cash)),
                            fmt_decimal(&Some(c.withdraw_cash)),
                        ]
                    })
                    .collect::<Vec<_>>();
                print_table(cash_headers, cash_rows, format);
            }

            println!("\n{}", t!("Portfolio.QuoteDisclaimer"));
        }
    }

    Ok(())
}

// ─── Testable run_* functions ─────────────────────────────────────────────────

pub async fn run_today_orders(
    api: &dyn TradeApi,
    opts: GetTodayOrdersOptions,
    format: &OutputFormat,
) -> Result<()> {
    let orders = api.today_orders(opts).await?;
    let headers = &[
        "Order ID",
        "Symbol",
        "Side",
        "Order Type",
        "Status",
        "Quantity",
        "Price",
        "Created At",
    ];
    let rows = orders
        .iter()
        .map(|o| {
            vec![
                o.order_id.clone(),
                o.symbol.clone(),
                format!("{:?}", o.side),
                format!("{:?}", o.order_type),
                format!("{:?}", o.status),
                o.quantity.to_string(),
                fmt_decimal(&o.price),
                fmt_datetime(o.submitted_at),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_history_orders(
    api: &dyn TradeApi,
    opts: GetHistoryOrdersOptions,
    format: &OutputFormat,
) -> Result<()> {
    let orders = api.history_orders(opts).await?;
    let headers = &[
        "Order ID",
        "Symbol",
        "Side",
        "Order Type",
        "Status",
        "Quantity",
        "Price",
        "Created At",
    ];
    let rows = orders
        .iter()
        .map(|o| {
            vec![
                o.order_id.clone(),
                o.symbol.clone(),
                format!("{:?}", o.side),
                format!("{:?}", o.order_type),
                format!("{:?}", o.status),
                o.quantity.to_string(),
                fmt_decimal(&o.price),
                fmt_datetime(o.submitted_at),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_order_detail(
    api: &dyn TradeApi,
    order_id: String,
    format: &OutputFormat,
) -> Result<()> {
    let detail = api.order_detail(order_id).await?;
    match format {
        OutputFormat::Json => {
            let val = serde_json::json!({"order_id": detail.order_id, "symbol": detail.symbol, "side": format!("{:?}", detail.side), "status": format!("{:?}", detail.status)});
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Pretty => {
            let headers = &["Field", "Value"];
            let rows = vec![
                vec!["Order ID".to_string(), detail.order_id.clone()],
                vec!["Symbol".to_string(), detail.symbol.clone()],
                vec!["Side".to_string(), format!("{:?}", detail.side)],
                vec!["Status".to_string(), format!("{:?}", detail.status)],
            ];
            print_table(headers, rows, &OutputFormat::Pretty);
        }
    }
    Ok(())
}

pub async fn run_today_executions(
    api: &dyn TradeApi,
    opts: GetTodayExecutionsOptions,
    format: &OutputFormat,
) -> Result<()> {
    let executions = api.today_executions(opts).await?;
    let headers = &[
        "Order ID", "Trade ID", "Symbol", "Price", "Quantity", "Time",
    ];
    let rows = executions
        .iter()
        .map(|e| {
            vec![
                e.order_id.clone(),
                e.trade_id.clone(),
                e.symbol.clone(),
                e.price.to_string(),
                e.quantity.to_string(),
                fmt_datetime(e.trade_done_at),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_history_executions(
    api: &dyn TradeApi,
    opts: GetHistoryExecutionsOptions,
    format: &OutputFormat,
) -> Result<()> {
    let executions = api.history_executions(opts).await?;
    let headers = &[
        "Order ID", "Trade ID", "Symbol", "Price", "Quantity", "Time",
    ];
    let rows = executions
        .iter()
        .map(|e| {
            vec![
                e.order_id.clone(),
                e.trade_id.clone(),
                e.symbol.clone(),
                e.price.to_string(),
                e.quantity.to_string(),
                fmt_datetime(e.trade_done_at),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_submit_order(
    api: &dyn TradeApi,
    opts: SubmitOrderOptions,
    format: &OutputFormat,
) -> Result<()> {
    let resp = api.submit_order(opts).await?;
    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({"order_id": resp.order_id}))?
            );
        }
        OutputFormat::Pretty => println!("Order ID: {}", resp.order_id),
    }
    Ok(())
}

pub async fn run_cancel_order(api: &dyn TradeApi, order_id: String) -> Result<()> {
    api.cancel_order(order_id).await?;
    Ok(())
}

pub async fn run_replace_order(api: &dyn TradeApi, opts: ReplaceOrderOptions) -> Result<()> {
    api.replace_order(opts).await?;
    Ok(())
}

pub async fn run_assets(
    api: &dyn TradeApi,
    currency: Option<String>,
    format: &OutputFormat,
) -> Result<()> {
    let balances = api.account_balance(currency).await?;
    print_assets(&balances, format);
    Ok(())
}

pub async fn run_cash_flow(
    api: &dyn TradeApi,
    opts: GetCashFlowOptions,
    format: &OutputFormat,
) -> Result<()> {
    let flows = api.cash_flow(opts).await?;
    let headers = &[
        "Flow Name",
        "Symbol",
        "Business Type",
        "Balance",
        "Currency",
        "Time",
    ];
    let rows = flows
        .iter()
        .map(|f| {
            vec![
                f.transaction_flow_name.clone(),
                f.symbol.clone().unwrap_or_default(),
                format!("{:?}", f.business_type),
                f.balance.to_string(),
                f.currency.clone(),
                fmt_datetime(f.business_time),
            ]
        })
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_positions(api: &dyn TradeApi, format: &OutputFormat) -> Result<()> {
    let resp = api.stock_positions().await?;
    let headers = &[
        "Symbol",
        "Name",
        "Quantity",
        "Available",
        "Cost Price",
        "Currency",
        "Market",
    ];
    let mut rows = vec![];
    for channel in &resp.channels {
        for pos in &channel.positions {
            rows.push(vec![
                pos.symbol.clone(),
                pos.symbol_name.clone(),
                pos.quantity.to_string(),
                pos.available_quantity.to_string(),
                pos.cost_price.to_string(),
                pos.currency.clone(),
                format!("{:?}", pos.market),
            ]);
        }
    }
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_fund_positions(api: &dyn TradeApi, format: &OutputFormat) -> Result<()> {
    let resp = api.fund_positions().await?;
    let headers = &[
        "Symbol",
        "Name",
        "Net Asset Value",
        "Cost NAV",
        "Currency",
        "Holding Units",
    ];
    let mut rows = vec![];
    for channel in &resp.channels {
        for pos in &channel.positions {
            rows.push(vec![
                pos.symbol.clone(),
                pos.symbol_name.clone(),
                pos.current_net_asset_value.to_string(),
                pos.cost_net_asset_value.to_string(),
                pos.currency.clone(),
                pos.holding_units.to_string(),
            ]);
        }
    }
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_margin_ratio(
    api: &dyn TradeApi,
    symbol: String,
    format: &OutputFormat,
) -> Result<()> {
    let ratio = api.margin_ratio(symbol.clone()).await?;
    let headers = &["Field", "Value"];
    let rows = vec![
        vec!["Symbol".to_string(), symbol],
        vec![
            "Initial Margin Ratio".to_string(),
            ratio.im_factor.to_string(),
        ],
        vec![
            "Maintenance Margin Ratio".to_string(),
            ratio.mm_factor.to_string(),
        ],
        vec![
            "Forced Liquidation Ratio".to_string(),
            ratio.fm_factor.to_string(),
        ],
    ];
    print_table(headers, rows, format);
    Ok(())
}

pub async fn run_max_qty(
    api: &dyn TradeApi,
    opts: EstimateMaxPurchaseQuantityOptions,
    symbol: String,
    format: &OutputFormat,
) -> Result<()> {
    let resp = api.estimate_max_purchase_quantity(opts).await?;
    let headers = &["Field", "Value"];
    let rows = vec![
        vec!["Symbol".to_string(), symbol],
        vec!["Cash Max Qty".to_string(), resp.cash_max_qty.to_string()],
        vec![
            "Margin Max Qty".to_string(),
            resp.margin_max_qty.to_string(),
        ],
    ];
    print_table(headers, rows, format);
    Ok(())
}

// ── Pending commands ─────────────────────────────────────────────────────────

fn val_str(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "-".to_owned(),
        other => other.to_string(),
    }
}

fn print_json_value(data: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(data).unwrap_or_default());
}

pub async fn cmd_alert_list(
    symbol: Option<String>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let mut params: Vec<(&str, &str)> = vec![];
    let cid;
    if let Some(ref sym) = symbol {
        cid = crate::utils::counter::symbol_to_counter_id(sym);
        params.push(("counter_id", cid.as_str()));
    }
    let data = super::api::http_get("/v1/notify/reminders", &params, verbose).await?;
    match format {
        OutputFormat::Json => print_json_value(&data),
        OutputFormat::Pretty => {
            let stocks = match data
                .get("lists")
                .or_else(|| data.get("list"))
                .and_then(|v| v.as_array())
            {
                Some(a) if !a.is_empty() => a,
                _ => {
                    println!("No alerts found.");
                    return Ok(());
                }
            };
            let headers = ["id", "symbol", "price", "alert", "enabled", "frequency"];
            let mut rows: Vec<Vec<String>> = Vec::new();
            for stock in stocks {
                let sym =
                    crate::utils::counter::counter_id_to_symbol(&val_str(&stock["counter_id"]));
                let price = val_str(&stock["price"]);
                let Some(indicators) = stock.get("indicators").and_then(|v| v.as_array()) else {
                    continue;
                };
                for ind in indicators {
                    let enabled = if ind["enabled"].as_bool().unwrap_or(false) {
                        "\u{2713}" // ✓
                    } else {
                        ""
                    };
                    let freq = match ind["frequency"].as_i64() {
                        Some(1) => "daily",
                        Some(2) => "every",
                        Some(3) => "once",
                        _ => "-",
                    };
                    rows.push(vec![
                        val_str(&ind["id"]),
                        sym.clone(),
                        price.clone(),
                        val_str(&ind["text"]),
                        enabled.to_string(),
                        freq.to_string(),
                    ]);
                }
            }
            if rows.is_empty() {
                println!("No alerts found.");
            } else {
                print_table(&headers, rows, format);
            }
        }
    }
    Ok(())
}

pub async fn cmd_alert_add(
    symbol: String,
    price: &str,
    direction: &str,
    alert_type: &str,
    frequency: &str,
    _note: Option<String>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    let cid = crate::utils::counter::symbol_to_counter_id(&symbol);
    // indicator_id: 1=price_rise, 2=price_fall, 3=change%_rise, 4=change%_fall
    let indicator_id: i32 = match (alert_type, direction) {
        ("percent", "fall" | "down") => 4,
        ("percent", _) => 3,
        (_, "fall" | "down") => 2,
        _ => 1,
    };
    let freq: i32 = match frequency {
        "daily" => 1,
        "every" => 2,
        _ => 3, // once
    };
    let setting_key = match indicator_id {
        3 | 4 => "chg",
        _ => "price",
    };
    let body = serde_json::json!({
        "counter_id": cid,
        "indicator_id": indicator_id.to_string(),
        "value_map": { setting_key: price },
        "frequency": freq,
        "enabled": true,
        "scope": 0,
        "state": [1],
    });
    let data = super::api::http_post("/v1/notify/reminders", body, verbose).await?;
    match format {
        OutputFormat::Json => print_json_value(&data),
        OutputFormat::Pretty => println!("Alert added for {symbol} at {price} ({direction})"),
    }
    Ok(())
}

pub async fn cmd_alert_delete(id: String, format: &OutputFormat, verbose: bool) -> Result<()> {
    let id_num: i64 = id.parse().unwrap_or(0);
    let body = serde_json::json!({ "ids": [id_num] });
    let data = super::api::http_delete("/v1/notify/reminders", body, verbose).await?;
    match format {
        OutputFormat::Json => print_json_value(&data),
        OutputFormat::Pretty => println!("Alert {id} deleted"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::api::MockTradeApi;

    fn make_submit_opts() -> SubmitOrderOptions {
        SubmitOrderOptions::new(
            "TSLA.US",
            OrderType::LO,
            OrderSide::Buy,
            Decimal::from(100u64),
            TimeInForceType::Day,
        )
    }

    #[tokio::test]
    async fn test_run_today_orders_dispatches() {
        let mut mock = MockTradeApi::new();
        mock.expect_today_orders()
            .times(1)
            .returning(|_| Ok(vec![]));
        run_today_orders(&mock, GetTodayOrdersOptions::new(), &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_history_orders_dispatches() {
        let mut mock = MockTradeApi::new();
        mock.expect_history_orders()
            .times(1)
            .returning(|_| Ok(vec![]));
        run_history_orders(&mock, GetHistoryOrdersOptions::new(), &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_today_executions_dispatches() {
        let mut mock = MockTradeApi::new();
        mock.expect_today_executions()
            .times(1)
            .returning(|_| Ok(vec![]));
        run_today_executions(
            &mock,
            GetTodayExecutionsOptions::new(),
            &OutputFormat::Pretty,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_history_executions_dispatches() {
        let mut mock = MockTradeApi::new();
        mock.expect_history_executions()
            .times(1)
            .returning(|_| Ok(vec![]));
        run_history_executions(
            &mock,
            GetHistoryExecutionsOptions::new(),
            &OutputFormat::Pretty,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_submit_order_dispatches() {
        let mut mock = MockTradeApi::new();
        mock.expect_submit_order().times(1).returning(|_| {
            Ok(longbridge::trade::SubmitOrderResponse {
                order_id: "order-1".to_string(),
            })
        });
        run_submit_order(&mock, make_submit_opts(), &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_cancel_order_dispatches() {
        let mut mock = MockTradeApi::new();
        mock.expect_cancel_order()
            .with(mockall::predicate::eq("order-1".to_string()))
            .times(1)
            .returning(|_| Ok(()));
        run_cancel_order(&mock, "order-1".to_string())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_replace_order_dispatches() {
        let mut mock = MockTradeApi::new();
        mock.expect_replace_order().times(1).returning(|_| Ok(()));
        let opts = ReplaceOrderOptions::new("order-1", Decimal::from(200u64));
        run_replace_order(&mock, opts).await.unwrap();
    }

    #[tokio::test]
    async fn test_run_assets_dispatches() {
        let mut mock = MockTradeApi::new();
        mock.expect_account_balance()
            .with(mockall::predicate::eq(None::<String>))
            .times(1)
            .returning(|_| Ok(vec![]));
        run_assets(&mock, None, &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_positions_dispatches() {
        use longbridge::trade::StockPositionsResponse;
        let mut mock = MockTradeApi::new();
        mock.expect_stock_positions()
            .times(1)
            .returning(|| Ok(StockPositionsResponse { channels: vec![] }));
        run_positions(&mock, &OutputFormat::Pretty).await.unwrap();
    }

    #[tokio::test]
    async fn test_run_fund_positions_dispatches() {
        use longbridge::trade::FundPositionsResponse;
        let mut mock = MockTradeApi::new();
        mock.expect_fund_positions()
            .times(1)
            .returning(|| Ok(FundPositionsResponse { channels: vec![] }));
        run_fund_positions(&mock, &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_run_margin_ratio_dispatches() {
        use longbridge::trade::MarginRatio;
        let mut mock = MockTradeApi::new();
        mock.expect_margin_ratio()
            .with(mockall::predicate::eq("TSLA.US".to_string()))
            .times(1)
            .returning(|_| {
                Ok(MarginRatio {
                    im_factor: Decimal::ZERO,
                    mm_factor: Decimal::ZERO,
                    fm_factor: Decimal::ZERO,
                })
            });
        run_margin_ratio(&mock, "TSLA.US".to_string(), &OutputFormat::Pretty)
            .await
            .unwrap();
    }

    #[test]
    fn test_parse_order_type_valid() {
        assert!(parse_order_type("LO").is_ok());
        assert!(parse_order_type("lo").is_ok());
        assert!(parse_order_type("MO").is_ok());
        assert!(parse_order_type("ELO").is_ok());
    }

    #[test]
    fn test_parse_order_type_invalid() {
        assert!(parse_order_type("LIMIT").is_err());
        assert!(parse_order_type("").is_err());
    }

    #[test]
    fn test_parse_tif_valid() {
        assert!(parse_tif("day").is_ok());
        assert!(parse_tif("gtc").is_ok());
        assert!(parse_tif("goodtilcanceled").is_ok());
        assert!(parse_tif("gtd").is_ok());
    }

    #[test]
    fn test_parse_tif_invalid() {
        assert!(parse_tif("ioc").is_err());
    }
}
