use anyhow::{bail, Result};
use longbridge::trade::{
    GetHistoryOrdersOptions, OrderSide, OrderType, ReplaceOrderOptions, SubmitOrderOptions,
    TimeInForceType,
};
use rust_decimal::Decimal;
use std::str::FromStr;

use super::{
    output::{fmt_datetime, fmt_decimal, parse_datetime_end, parse_datetime_start, print_table},
    OutputFormat,
};

fn parse_order_type(s: &str) -> Result<OrderType> {
    match s.to_uppercase().as_str() {
        "LO" => Ok(OrderType::LO),
        "MO" => Ok(OrderType::MO),
        "ELO" => Ok(OrderType::ELO),
        "ALO" => Ok(OrderType::ALO),
        "ODD" => Ok(OrderType::ODD),
        "SLO" => Ok(OrderType::SLO),
        "LIT" => Ok(OrderType::LIT),
        "MIT" => Ok(OrderType::MIT),
        _ => bail!(
            "Unknown order type '{}'. Use: LO MO ELO ALO ODD SLO LIT MIT",
            s
        ),
    }
}

fn parse_tif(s: &str) -> Result<TimeInForceType> {
    match s.to_lowercase().as_str() {
        "day" => Ok(TimeInForceType::Day),
        "gtc" | "goodtilcanceled" => Ok(TimeInForceType::GoodTilCanceled),
        "gtd" | "goodtildate" => Ok(TimeInForceType::GoodTilDate),
        _ => bail!(
            "Unknown time in force '{}'. Use: Day GoodTilCanceled GoodTilDate",
            s
        ),
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
        "Type",
        "Status",
        "Qty",
        "Price",
        "Executed Qty",
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
        OutputFormat::Table => {
            let headers = &["Field", "Value"];
            let rows = vec![
                vec!["Order ID".to_string(), detail.order_id.clone()],
                vec!["Symbol".to_string(), detail.symbol.clone()],
                vec!["Side".to_string(), format!("{:?}", detail.side)],
                vec!["Type".to_string(), format!("{:?}", detail.order_type)],
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
            print_table(headers, rows, &OutputFormat::Table);
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

    let headers = &["Order ID", "Trade ID", "Symbol", "Price", "Quantity", "Time"];
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

pub async fn cmd_submit_order(
    symbol: String,
    quantity: u64,
    price: Option<String>,
    order_type: String,
    tif: String,
    side: OrderSide,
    format: &OutputFormat,
) -> Result<()> {
    let ot = parse_order_type(&order_type)?;
    let tif_val = parse_tif(&tif)?;
    let qty = Decimal::from(quantity);

    let mut opts = SubmitOrderOptions::new(symbol.clone(), ot, side, qty, tif_val);
    if let Some(ref p) = price {
        let price_dec =
            Decimal::from_str(p).map_err(|_| anyhow::anyhow!("Invalid price: {}", p))?;
        opts = opts.submitted_price(price_dec);
    }

    // Confirm before submitting
    println!(
        "Submitting {:?} order: {} {} @ {}",
        side,
        quantity,
        symbol,
        price.as_deref().unwrap_or("market")
    );
    print!("Confirm? [y/N] ");
    use std::io::Write;
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if input.trim().to_lowercase() != "y" {
        println!("Cancelled.");
        return Ok(());
    }

    let ctx = crate::openapi::trade();
    let resp = ctx.submit_order(opts).await?;

    match format {
        OutputFormat::Json => {
            let val = serde_json::json!({ "order_id": resp.order_id });
            println!("{}", serde_json::to_string_pretty(&val)?);
        }
        OutputFormat::Table => {
            println!("Order submitted successfully.");
            println!("Order ID: {}", resp.order_id);
        }
    }
    Ok(())
}

pub async fn cmd_cancel_order(order_id: String) -> Result<()> {
    print!("Cancel order {}? [y/N] ", order_id);
    use std::io::Write;
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if input.trim().to_lowercase() != "y" {
        println!("Cancelled.");
        return Ok(());
    }

    let ctx = crate::openapi::trade();
    ctx.cancel_order(order_id.clone()).await?;
    println!("Order {} cancelled.", order_id);
    Ok(())
}

pub async fn cmd_replace_order(
    order_id: String,
    qty: Option<u64>,
    price: Option<String>,
) -> Result<()> {
    let quantity = qty.ok_or_else(|| anyhow::anyhow!("--qty is required"))?;
    let qty_dec = Decimal::from(quantity);

    let mut opts = ReplaceOrderOptions::new(order_id.clone(), qty_dec);
    if let Some(p) = price {
        let price_dec =
            Decimal::from_str(&p).map_err(|_| anyhow::anyhow!("Invalid price: {}", p))?;
        opts = opts.price(price_dec);
    }

    print!("Modify order {}? [y/N] ", order_id);
    use std::io::Write;
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if input.trim().to_lowercase() != "y" {
        println!("Cancelled.");
        return Ok(());
    }

    let ctx = crate::openapi::trade();
    ctx.replace_order(opts).await?;
    println!("Order {} modified.", order_id);
    Ok(())
}

pub async fn cmd_balance(currency: Option<String>, format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::trade();
    let balances = ctx.account_balance(currency.as_deref()).await?;

    let headers = &[
        "Currency",
        "Total Cash",
        "Max Finance Amount",
        "Remaining Finance",
        "Risk Level",
        "Margin Call",
    ];
    let rows = balances
        .iter()
        .map(|b| {
            vec![
                b.currency.clone(),
                b.total_cash.to_string(),
                b.max_finance_amount.to_string(),
                b.remaining_finance_amount.to_string(),
                b.risk_level.to_string(),
                b.margin_call.to_string(),
            ]
        })
        .collect();

    print_table(headers, rows, format);
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
        _ => bail!("Unknown side '{}'. Use: Buy Sell", side),
    };
    let ot = parse_order_type(order_type)?;

    let price_dec = price
        .as_deref()
        .map(|p| Decimal::from_str(p).map_err(|_| anyhow::anyhow!("Invalid price: {}", p)))
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
        vec![
            "Cash Max Qty".to_string(),
            resp.cash_max_qty.to_string(),
        ],
        vec![
            "Margin Max Qty".to_string(),
            resp.margin_max_qty.to_string(),
        ],
    ];

    print_table(headers, rows, format);
    Ok(())
}
