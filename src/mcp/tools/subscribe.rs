use serde_json::{json, Value};
use std::sync::Mutex;

use super::{api_err, require_strings};
use crate::mcp::protocol::Tool;

static SUBSCRIBED: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn tool_definitions() -> Vec<Tool> {
    vec![
        Tool {
            name: "subscribe_quote",
            description: "Subscribe to real-time quote updates for symbols. \
                The server will push MCP notifications/message events as prices change. \
                Use unsubscribe_quote to stop.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "symbols": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Symbols to subscribe, e.g. [\"700.HK\", \"TSLA.US\"]",
                        "minItems": 1
                    }
                },
                "required": ["symbols"]
            }),
        },
        Tool {
            name: "unsubscribe_quote",
            description: "Stop receiving real-time quote notifications for the given symbols.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "symbols": {
                        "type": "array",
                        "items": {"type": "string"},
                        "minItems": 1
                    }
                },
                "required": ["symbols"]
            }),
        },
    ]
}

pub async fn handle_subscribe(args: Value) -> Result<Value, (i64, String)> {
    use longbridge::quote::SubFlags;

    let symbols = require_strings(&args, "symbols")?;
    let ctx = crate::openapi::quote();

    ctx.subscribe(&symbols, SubFlags::QUOTE, true)
        .await
        .map_err(api_err)?;

    if let Ok(mut guard) = SUBSCRIBED.lock() {
        for s in &symbols {
            if !guard.contains(s) {
                guard.push(s.clone());
            }
        }
    }

    Ok(json!({ "subscribed": symbols }))
}

pub async fn handle_unsubscribe(args: Value) -> Result<Value, (i64, String)> {
    use longbridge::quote::SubFlags;

    let symbols = require_strings(&args, "symbols")?;
    let ctx = crate::openapi::quote();

    ctx.unsubscribe(&symbols, SubFlags::QUOTE)
        .await
        .map_err(api_err)?;

    if let Ok(mut guard) = SUBSCRIBED.lock() {
        guard.retain(|s| !symbols.contains(s));
    }

    Ok(json!({ "unsubscribed": symbols }))
}
