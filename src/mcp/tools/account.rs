use serde_json::{json, Value};

use super::{api_err, opt_string};
use crate::mcp::protocol::Tool;

pub fn tool_definitions() -> Vec<Tool> {
    vec![
        Tool {
            name: "positions",
            description: "Get current stock positions in the account.",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        Tool {
            name: "account_balance",
            description: "Get account balance and cash information.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "currency": {
                        "type": "string",
                        "description": "Filter by currency, e.g. \"HKD\", \"USD\". Omit for all."
                    }
                }
            }),
        },
        Tool {
            name: "orders",
            description: "Get today's order list. Use status to filter by order state.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "symbol": {
                        "type": "string",
                        "description": "Filter by symbol, e.g. \"700.HK\""
                    }
                }
            }),
        },
    ]
}

pub async fn handle_positions(_args: Value) -> Result<Value, (i64, String)> {
    let ctx = crate::openapi::trade();
    let resp = ctx.stock_positions(None).await.map_err(api_err)?;

    let positions: Vec<Value> = resp
        .channels
        .iter()
        .flat_map(|ch| {
            ch.positions.iter().map(|p| {
                json!({
                    "symbol": p.symbol,
                    "symbol_name": p.symbol_name,
                    "quantity": p.quantity.to_string(),
                    "available_quantity": p.available_quantity.to_string(),
                    "currency": p.currency,
                })
            })
        })
        .collect();

    Ok(json!({ "positions": positions }))
}

pub async fn handle_account_balance(args: Value) -> Result<Value, (i64, String)> {
    let currency = opt_string(&args, "currency");
    let ctx = crate::openapi::trade();
    let balances = ctx.account_balance(currency).await.map_err(api_err)?;

    let result: Vec<Value> = balances
        .iter()
        .map(|b| {
            json!({
                "currency": b.currency,
                "total_cash": b.total_cash.to_string(),
                "max_finance_amount": b.max_finance_amount.to_string(),
                "remaining_finance_amount": b.remaining_finance_amount.to_string(),
                "risk_level": b.risk_level,
                "margin_call": b.margin_call.to_string(),
                "net_assets": b.net_assets.to_string(),
                "init_margin": b.init_margin.to_string(),
                "maintenance_margin": b.maintenance_margin.to_string(),
            })
        })
        .collect();

    Ok(json!({ "balances": result }))
}

pub async fn handle_orders(args: Value) -> Result<Value, (i64, String)> {
    use longbridge::trade::GetTodayOrdersOptions;

    let mut opts = GetTodayOrdersOptions::new();
    if let Some(s) = opt_string(&args, "symbol") {
        opts = opts.symbol(s.to_owned());
    }

    let ctx = crate::openapi::trade();
    let orders = ctx.today_orders(opts).await.map_err(api_err)?;

    let result: Vec<Value> = orders
        .iter()
        .map(|o| {
            json!({
                "order_id": o.order_id,
                "symbol": o.symbol,
                "side": format!("{:?}", o.side),
                "order_type": format!("{:?}", o.order_type),
                "quantity": o.quantity.to_string(),
                "executed_quantity": o.executed_quantity.to_string(),
                "price": o.price.map(|p| p.to_string()),
                "executed_price": o.executed_price.map(|p| p.to_string()),
                "status": format!("{:?}", o.status),
                "submitted_at": o.submitted_at.to_string(),
                "updated_at": o.updated_at.map(|t| t.to_string()),
                "currency": o.currency,
            })
        })
        .collect();

    Ok(json!({ "orders": result }))
}
