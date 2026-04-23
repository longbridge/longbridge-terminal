use anyhow::Result;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use crate::mcp::protocol::{
    Capabilities, InitializeResult, Notification, Request, Response, ServerInfo,
    ERR_INVALID_REQUEST, ERR_METHOD_NOT_FOUND, ERR_NOT_INITIALIZED, ERR_PARSE, PROTOCOL_VERSION,
};
use crate::mcp::tools::ToolRegistry;

pub struct Server {
    registry: ToolRegistry,
    initialized: bool,
    stdout: Arc<Mutex<tokio::io::Stdout>>,
}

impl Server {
    pub fn new() -> Self {
        Self {
            registry: ToolRegistry::new(),
            initialized: false,
            stdout: Arc::new(Mutex::new(tokio::io::stdout())),
        }
    }

    pub async fn run(
        mut self,
        mut quote_stream: impl tokio_stream::Stream<Item = longbridge::quote::PushEvent>
            + Send
            + Unpin
            + 'static,
    ) -> Result<()> {
        let stdout = Arc::clone(&self.stdout);

        tokio::spawn(async move {
            use tokio_stream::StreamExt;
            while let Some(event) = quote_stream.next().await {
                if let Some(n) = push_event_to_notification(event) {
                    if let Ok(json) = serde_json::to_string(&n) {
                        let mut out = stdout.lock().await;
                        let _ = out.write_all(json.as_bytes()).await;
                        let _ = out.write_all(b"\n").await;
                        let _ = out.flush().await;
                    }
                }
            }
        });

        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();

        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Some(resp) = self.handle_line(trimmed).await {
                let json = serde_json::to_string(&resp)?;
                let mut out = self.stdout.lock().await;
                out.write_all(json.as_bytes()).await?;
                out.write_all(b"\n").await?;
                out.flush().await?;
            }
        }

        Ok(())
    }

    async fn handle_line(&mut self, line: &str) -> Option<Response> {
        let req: Request = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                return Some(Response::err(
                    Value::Null,
                    ERR_PARSE,
                    format!("parse error: {e}"),
                ));
            }
        };

        let id = req.id.clone().unwrap_or(Value::Null);

        if req.jsonrpc != "2.0" {
            return Some(Response::err(
                id,
                ERR_INVALID_REQUEST,
                "jsonrpc must be '2.0'".into(),
            ));
        }

        // Notifications have no id — no response needed
        if req.id.is_none() {
            return None;
        }

        match req.method.as_str() {
            "initialize" => Some(self.handle_initialize(id, req.params)),
            "ping" => Some(Response::ok(id, json!({}))),
            method if !self.initialized => Some(Response::err(
                id,
                ERR_NOT_INITIALIZED,
                format!("server not initialized, received: {method}"),
            )),
            "tools/list" => Some(self.handle_tools_list(id)),
            "tools/call" => self.handle_tools_call(id, req.params).await,
            method => Some(Response::err(
                id,
                ERR_METHOD_NOT_FOUND,
                format!("method not found: {method}"),
            )),
        }
    }

    fn handle_initialize(&mut self, id: Value, _params: Option<Value>) -> Response {
        self.initialized = true;
        let result = InitializeResult {
            protocol_version: PROTOCOL_VERSION,
            capabilities: Capabilities {
                tools: json!({}),
                logging: json!({}),
            },
            server_info: ServerInfo {
                name: "longbridge-mcp",
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
        };
        Response::ok(id, serde_json::to_value(result).unwrap_or(json!({})))
    }

    fn handle_tools_list(&self, id: Value) -> Response {
        let tools: Vec<Value> = self
            .registry
            .list()
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": t.input_schema,
                })
            })
            .collect();
        Response::ok(id, json!({ "tools": tools }))
    }

    async fn handle_tools_call(&self, id: Value, params: Option<Value>) -> Option<Response> {
        let params = params.unwrap_or(Value::Null);
        let name = match params.get("name").and_then(Value::as_str) {
            Some(n) => n.to_owned(),
            None => {
                return Some(Response::err(
                    id,
                    crate::mcp::protocol::ERR_INVALID_PARAMS,
                    "missing 'name' in tools/call params".into(),
                ));
            }
        };
        let args = params.get("arguments").cloned();

        match self.registry.call(&name, args).await {
            Ok(result) => Some(Response::ok(
                id,
                json!({
                    "content": [{"type": "text", "text": result.to_string()}],
                    "isError": false,
                }),
            )),
            Err((code, message)) => Some(Response::ok(
                id,
                json!({
                    "content": [{"type": "text", "text": message}],
                    "isError": true,
                    "_errorCode": code,
                }),
            )),
        }
    }
}

fn push_event_to_notification(event: longbridge::quote::PushEvent) -> Option<Notification> {
    use longbridge::quote::PushEventDetail;

    let data = match event.detail {
        PushEventDetail::Quote(q) => json!({
            "type": "quote",
            "symbol": event.symbol,
            "last_done": q.last_done.to_string(),
            "open": q.open.to_string(),
            "high": q.high.to_string(),
            "low": q.low.to_string(),
            "timestamp": q.timestamp.to_string(),
            "volume": q.volume,
            "turnover": q.turnover.to_string(),
        }),
        PushEventDetail::Depth(d) => json!({
            "type": "depth",
            "symbol": event.symbol,
            "asks": d.asks.iter().map(|x| json!({
                "price": x.price.map(|p| p.to_string()).unwrap_or_default(),
                "volume": x.volume,
            })).collect::<Vec<_>>(),
            "bids": d.bids.iter().map(|x| json!({
                "price": x.price.map(|p| p.to_string()).unwrap_or_default(),
                "volume": x.volume,
            })).collect::<Vec<_>>(),
        }),
        _ => return None,
    };

    Some(Notification::message(data))
}
