use serde_json::{json, Value};

use super::{api_err, require_string};
use crate::mcp::protocol::{Tool, ERR_INVALID_PARAMS};

pub fn tool_definitions() -> Vec<Tool> {
    vec![
        Tool {
            name: "submit_order",
            description: "CAUTION: This tool submits a real order to the exchange. \
                Confirm with the user before calling. \
                Supported order types: LO (limit), MO (market), ELO (enhanced limit).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "symbol": {"type": "string", "description": "Symbol, e.g. \"700.HK\""},
                    "side": {"type": "string", "enum": ["buy", "sell"]},
                    "order_type": {
                        "type": "string",
                        "description": "LO (limit order), MO (market order), ELO (enhanced limit)",
                        "default": "LO"
                    },
                    "quantity": {"type": "string", "description": "Number of shares as a string, e.g. \"100\""},
                    "price": {"type": "string", "description": "Limit price as a string (required for LO/ELO)"},
                    "time_in_force": {
                        "type": "string",
                        "description": "Day (day order) or GTC (good till cancelled)",
                        "default": "Day"
                    },
                    "remark": {"type": "string", "description": "Optional order note (max 64 chars)"}
                },
                "required": ["symbol", "side", "quantity"]
            }),
        },
        Tool {
            name: "cancel_order",
            description:
                "CAUTION: This tool cancels a real order. Confirm with the user before calling.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "order_id": {"type": "string", "description": "Order ID to cancel"}
                },
                "required": ["order_id"]
            }),
        },
    ]
}

pub async fn handle_submit_order(args: Value) -> Result<Value, (i64, String)> {
    use longbridge::trade::{OrderSide, OrderType, SubmitOrderOptions, TimeInForceType};
    use rust_decimal::Decimal;
    use std::str::FromStr;

    let symbol = require_string(&args, "symbol")?;
    let side_str = require_string(&args, "side")?;
    let qty_str = require_string(&args, "quantity")?;

    let side = match side_str.to_lowercase().as_str() {
        "buy" => OrderSide::Buy,
        "sell" => OrderSide::Sell,
        _ => return Err((ERR_INVALID_PARAMS, "side must be 'buy' or 'sell'".into())),
    };

    let quantity =
        Decimal::from_str(&qty_str).map_err(|_| (ERR_INVALID_PARAMS, "invalid quantity".into()))?;

    let order_type_str = args
        .get("order_type")
        .and_then(Value::as_str)
        .unwrap_or("LO");
    let order_type = match order_type_str.to_uppercase().as_str() {
        "MO" => OrderType::MO,
        "ELO" => OrderType::ELO,
        _ => OrderType::LO,
    };

    let tif_str = args
        .get("time_in_force")
        .and_then(Value::as_str)
        .unwrap_or("Day");
    let tif = match tif_str {
        "GTC" => TimeInForceType::GoodTilCanceled,
        _ => TimeInForceType::Day,
    };

    let mut opts = SubmitOrderOptions::new(symbol, order_type, side, quantity, tif);

    if let Some(price_str) = args.get("price").and_then(Value::as_str) {
        let price = Decimal::from_str(price_str)
            .map_err(|_| (ERR_INVALID_PARAMS, "invalid price".into()))?;
        opts = opts.submitted_price(price);
    }

    if let Some(remark) = args.get("remark").and_then(Value::as_str) {
        opts = opts.remark(remark.to_owned());
    }

    let ctx = crate::openapi::trade();
    let resp = ctx.submit_order(opts).await.map_err(api_err)?;

    Ok(json!({ "order_id": resp.order_id }))
}

pub async fn handle_cancel_order(args: Value) -> Result<Value, (i64, String)> {
    let order_id = require_string(&args, "order_id")?;
    let ctx = crate::openapi::trade();
    ctx.cancel_order(order_id.clone()).await.map_err(api_err)?;
    Ok(json!({ "cancelled": true, "order_id": order_id }))
}
