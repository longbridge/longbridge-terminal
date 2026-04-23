use anyhow::Result;
use serde_json::Value;

use crate::mcp::protocol::{Tool, ERR_INVALID_PARAMS, ERR_METHOD_NOT_FOUND};

pub mod account;
pub mod quote;
pub mod subscribe;
pub mod trade;

pub struct ToolRegistry {
    tools: Vec<Tool>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        let mut tools = Vec::new();
        tools.extend(quote::tool_definitions());
        tools.extend(account::tool_definitions());
        tools.extend(trade::tool_definitions());
        tools.extend(subscribe::tool_definitions());
        Self { tools }
    }

    pub fn list(&self) -> &[Tool] {
        &self.tools
    }

    pub async fn call(&self, name: &str, params: Option<Value>) -> Result<Value, (i64, String)> {
        let args = params.unwrap_or(Value::Object(Default::default()));

        match name {
            // Quote tools
            "quote" => quote::handle_quote(args).await,
            "depth" => quote::handle_depth(args).await,
            "trades" => quote::handle_trades(args).await,
            "intraday" => quote::handle_intraday(args).await,
            "kline" => quote::handle_kline(args).await,
            "static_info" => quote::handle_static_info(args).await,
            // Account tools
            "positions" => account::handle_positions(args).await,
            "account_balance" => account::handle_account_balance(args).await,
            "orders" => account::handle_orders(args).await,
            // Trade tools
            "submit_order" => trade::handle_submit_order(args).await,
            "cancel_order" => trade::handle_cancel_order(args).await,
            // Subscribe tools
            "subscribe_quote" => subscribe::handle_subscribe(args).await,
            "unsubscribe_quote" => subscribe::handle_unsubscribe(args).await,
            _ => Err((ERR_METHOD_NOT_FOUND, format!("unknown tool: {name}"))),
        }
    }
}

// ── Param helpers ─────────────────────────────────────────────────────────────

pub fn require_string(args: &Value, key: &str) -> Result<String, (i64, String)> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| (ERR_INVALID_PARAMS, format!("missing required param: {key}")))
}

pub fn require_strings(args: &Value, key: &str) -> Result<Vec<String>, (i64, String)> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .filter(|v: &Vec<String>| !v.is_empty())
        .ok_or_else(|| (ERR_INVALID_PARAMS, format!("missing required param: {key}")))
}

pub fn opt_u32(args: &Value, key: &str, default: u32) -> u32 {
    args.get(key)
        .and_then(Value::as_u64)
        .map(|v| v as u32)
        .unwrap_or(default)
}

pub fn opt_string<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

pub fn api_err(e: impl std::fmt::Display) -> (i64, String) {
    (crate::mcp::protocol::ERR_API, e.to_string())
}
