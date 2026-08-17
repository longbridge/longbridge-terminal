//! JSON-RPC 2.0 message types for `longbridge serve`.
//!
//! The wire format is newline-delimited JSON (NDJSON): exactly one compact
//! JSON object per line, UTF-8, no framing headers. This is the same base
//! protocol LSP, MCP and ACP build on, so a client needs nothing beyond a
//! JSON parser and a line splitter.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Protocol revision reported by `initialize`. Bump on a breaking change to
/// method names, parameters, or result shapes.
pub const PROTOCOL_VERSION: &str = "1";

/// Standard JSON-RPC 2.0 error codes.
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;

/// Application error: the request was well-formed but the upstream Longbridge
/// call failed (network, permission, unknown symbol, ...).
pub const API_ERROR: i32 = -32000;

/// An incoming JSON-RPC request.
///
/// `id` is absent for notifications, which take no response. Both are parsed
/// by the same type because the only difference is whether a reply is owed.
#[derive(Debug, Deserialize)]
pub struct Request {
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct ErrorObject {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// A response or a server-initiated notification.
///
/// JSON-RPC distinguishes them by the presence of `id`, so one struct with
/// skipped-when-absent fields covers both and keeps the writer path single.
#[derive(Debug, Serialize)]
pub struct Message {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorObject>,
}

impl Message {
    fn base() -> Self {
        Self {
            jsonrpc: "2.0",
            id: None,
            method: None,
            params: None,
            result: None,
            error: None,
        }
    }

    /// A successful response. `result` is always present — JSON-RPC requires
    /// exactly one of `result`/`error`, and `null` is a valid result.
    pub fn result(id: Value, result: Value) -> Self {
        Self {
            id: Some(id),
            result: Some(result),
            ..Self::base()
        }
    }

    pub fn error(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            id,
            error: Some(ErrorObject {
                code,
                message: message.into(),
                data: None,
            }),
            ..Self::base()
        }
    }

    /// A server-initiated notification (no `id`, so the client owes no reply).
    pub fn notification(method: &'static str, params: Value) -> Self {
        Self {
            method: Some(method),
            params: Some(params),
            ..Self::base()
        }
    }

    /// Serialize to a single NDJSON line. Compact on purpose: a pretty-printed
    /// object would span lines and break the framing.
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|e| {
            // Only reachable if a handler produced a non-serializable value.
            // Degrade to a valid frame rather than desynchronizing the stream.
            format!(
                r#"{{"jsonrpc":"2.0","error":{{"code":{API_ERROR},"message":"failed to serialize response: {}"}}}}"#,
                e.to_string().replace('"', "'")
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_and_error_are_mutually_exclusive_on_the_wire() {
        let ok = Message::result(Value::from(1), serde_json::json!({"a": 1})).to_line();
        assert_eq!(ok, r#"{"jsonrpc":"2.0","id":1,"result":{"a":1}}"#);

        let err = Message::error(Some(Value::from(2)), METHOD_NOT_FOUND, "nope").to_line();
        assert_eq!(
            err,
            r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"nope"}}"#
        );
    }

    #[test]
    fn notifications_carry_no_id() {
        let line = Message::notification("quote.updated", serde_json::json!({"symbol": "700.HK"}))
            .to_line();
        assert_eq!(
            line,
            r#"{"jsonrpc":"2.0","method":"quote.updated","params":{"symbol":"700.HK"}}"#
        );
    }

    #[test]
    fn a_request_without_id_parses_as_a_notification() {
        let req: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"shutdown"}"#).unwrap();
        assert!(req.id.is_none());
        assert_eq!(req.method, "shutdown");
    }

    #[test]
    fn every_frame_is_a_single_line() {
        let line = Message::result(
            Value::from(1),
            serde_json::json!({"nested": {"deep": [1, 2, 3]}}),
        )
        .to_line();
        assert!(!line.contains('\n'));
    }
}
