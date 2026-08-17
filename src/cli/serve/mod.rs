//! `longbridge serve` — a long-lived JSON-RPC 2.0 endpoint for third-party
//! clients (desktop widgets, bar plugins, scripts).
//!
//! # Why this exists
//!
//! Without it, a client polls by spawning `longbridge <cmd> --format json` on a
//! timer. Every spawn re-does region detection, token load, WebSocket connect,
//! one request, exit — so the cost dominates, and the client is stuck at
//! poll-interval latency with no way to see a tick.
//!
//! `serve` pays that cost once and keeps the connection. Quotes then arrive as
//! server-initiated notifications at WebSocket speed.
//!
//! # Wire format
//!
//! Newline-delimited JSON-RPC 2.0 over stdio: one compact JSON object per
//! line. This is the base protocol LSP, MCP and ACP share, so a client needs
//! only a JSON parser and a line splitter — no protocol library.
//!
//! Requests are handled concurrently: a slow `trade.stock_positions` must not
//! stall the quote feed, so each one runs in its own task and every writer
//! funnels through a single channel that owns stdout. Responses may therefore
//! arrive out of order — correlate them by `id`, as JSON-RPC intends.
//! [`MAX_CONCURRENT_REQUESTS`] bounds how much of that reaches Longbridge.
//!
//! ```text
//! → {"jsonrpc":"2.0","id":1,"method":"quote.quote","params":{"symbols":["700.HK"]}}
//! ← {"jsonrpc":"2.0","id":1,"result":[{"symbol":"700.HK","last_done":"320.000",...}]}
//! → {"jsonrpc":"2.0","id":2,"method":"quote.subscribe","params":{"symbols":["700.HK"]}}
//! ← {"jsonrpc":"2.0","id":2,"result":{"subscribed":[{"symbol":"700.HK","fields":["quote"]}]}}
//! ← {"jsonrpc":"2.0","method":"quote.updated","params":{"symbol":"700.HK","last_done":"320.200",...}}
//! ```
//!
//! See [`methods`] for where the method surface comes from and why its payloads
//! are the raw upstream shapes rather than the CLI's `--format json` output.

pub mod methods;
pub mod params;
pub mod protocol;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Semaphore};
use tokio_stream::{Stream, StreamExt};

use params::ParamError;
use protocol::{
    Message, API_ERROR, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR,
};

/// How many requests may be in flight upstream at once.
///
/// Longbridge allows on the order of 10 requests a second. A one-shot CLI
/// invocation cannot get near that, but this process lives long enough for a
/// client to queue lines faster than the network answers them, and every
/// request runs in its own task — without a cap, one burst becomes one burst
/// upstream and the whole session starts getting throttled.
const MAX_CONCURRENT_REQUESTS: usize = 8;

/// How long to wait for in-flight requests after `shutdown` or stdin EOF.
///
/// The writer only stops once every handler has dropped its sender, so a
/// single request stuck upstream would otherwise hold the process open after
/// its client is gone — the exact orphan the EOF handling exists to prevent.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// The method list appended to `serve -h`.
///
/// Generated from the [`methods`] routing tables, not written out a second
/// time, so the help cannot advertise a surface the dispatcher does not have.
/// Only names: parameters follow the Longbridge `OpenAPI` request for the same
/// call, and a second hand-maintained copy of them here would be one more
/// thing to drift.
pub fn method_reference() -> String {
    use std::fmt::Write;

    let mut out = String::from(
        "PROTOCOL\n\
         \x20 Newline-delimited JSON-RPC 2.0 on stdin/stdout: one compact JSON object per\n\
         \x20 line, UTF-8. One request per line — batches (a JSON array) are not accepted,\n\
         \x20 as in LSP and MCP.\n\n\
         \x20 request       {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"<method>\",\"params\":{…}}\n\
         \x20 response      {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":…}\n\
         \x20 error         {\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32602,\"message\":\"…\"}}\n\
         \x20 notification  {\"jsonrpc\":\"2.0\",\"method\":\"quote.updated\",\"params\":{…}}\n\n\
         \x20 Requests are answered concurrently, so responses may arrive out of order —\n\
         \x20 correlate them by id. Up to 8 run upstream at once; the rest queue, so a\n\
         \x20 burst is never dropped, only paced. Codes follow JSON-RPC: -32700 parse,\n\
         \x20 -32600 invalid request, -32601 unknown method, -32602 bad params, -32000\n\
         \x20 upstream failure. A -32602 message names the offending field and means\n\
         \x20 retrying as-is will not help; -32000 may. The process exits on stdin EOF.\n\n\
         PARAMS AND RESULTS\n\
         \x20 Both are the raw Longbridge OpenAPI shapes for the same call, not the\n\
         \x20 reshaped JSON the equivalent CLI command prints. Look a method up under its\n\
         \x20 own name at https://open.longbridge.com/docs for its fields; quote.* and\n\
         \x20 trade.* are named after the SDK calls they forward to. api.get/api.post\n\
         \x20 return the response body as received, less the account-identifying fields\n\
         \x20 the CLI also strips (aaid, account_channel).\n\n\
         NOTIFICATIONS\n\
         \x20 quote.updated, quote.depth, quote.brokers, quote.trades — delivered for the\n\
         \x20 securities and fields named in quote.subscribe, each carrying its symbol.\n\
         \x20 quote.updated is a tick, not a full quote: it carries last_done, open, high,\n\
         \x20 low, volume, turnover, current_volume, current_turnover, trade_status,\n\
         \x20 trade_session and timestamp. quote.subscribe returns a `quotes` snapshot to\n\
         \x20 start from (omitted if that one call failed; fall back to quote.quote). A\n\
         \x20 push may still be older than the snapshot, so keep whichever timestamp is\n\
         \x20 newer.\n\n\
         METHODS\n",
    );

    // Two per line: 50 names down a single column would bury the sections
    // above it in scrollback.
    let names = methods::all_methods();
    for pair in names.chunks(2) {
        let _ = match pair {
            [a, b] => writeln!(out, "  {a:<38}{b}"),
            [a] => writeln!(out, "  {a}"),
            _ => Ok(()),
        };
    }

    out.push_str(
        "\n\x20 `initialize` returns this same list, so a client can discover it in-band.\n\n\
         EXAMPLE\n\
         \x20 $ longbridge serve\n\
         \x20 → {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"quote.quote\",\"params\":{\"symbols\":[\"700.HK\"]}}\n\
         \x20 ← {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":[{\"symbol\":\"700.HK\",\"last_done\":\"445.600\",…}]}\n\
         \x20 → {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"quote.subscribe\",\"params\":{\"symbols\":[\"700.HK\"]}}\n\
         \x20 ← {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"subscribed\":[{\"symbol\":\"700.HK\",\"fields\":[\"quote\"]}]}}\n\
         \x20 ← {\"jsonrpc\":\"2.0\",\"method\":\"quote.updated\",\"params\":{\"symbol\":\"700.HK\",\"last_done\":\"446.000\",…}}\n",
    );
    out
}

/// Classify a handler error into a JSON-RPC code.
///
/// Parameter validation is the caller's fault (`INVALID_PARAMS`); anything else
/// came back from Longbridge and is ours or the network's (`API_ERROR`). The
/// distinction matters to a client deciding whether a retry can help, so it is
/// carried by [`ParamError`]'s type — an upstream message that happens to be
/// worded like a parameter complaint must not be blamed on the client.
fn error_code_for(err: &anyhow::Error) -> i32 {
    if err.downcast_ref::<ParamError>().is_some() {
        INVALID_PARAMS
    } else {
        API_ERROR
    }
}

/// Serve until stdin closes or the client calls `shutdown`.
///
/// `quote_stream` is the WebSocket push feed from `openapi::init_contexts`,
/// which every other CLI command discards.
pub async fn run(
    quote_stream: impl Stream<Item = longbridge::quote::PushEvent> + Send + Unpin + 'static,
) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS));

    // Sole owner of stdout. Serializing writes through one task is what keeps
    // concurrent handlers and the push feed from interleaving mid-line.
    let writer = tokio::spawn(async move {
        let mut out = tokio::io::stdout();
        while let Some(line) = rx.recv().await {
            if out.write_all(line.as_bytes()).await.is_err()
                || out.write_all(b"\n").await.is_err()
                || out.flush().await.is_err()
            {
                // The client is gone; further writes cannot succeed either.
                break;
            }
        }
    });

    let push_tx = tx.clone();
    let pusher = tokio::spawn(async move {
        let mut quote_stream = quote_stream;
        while let Some(event) = quote_stream.next().await {
            if let Some(message) = methods::push_to_notification(event) {
                if push_tx.send(message.to_line()).is_err() {
                    break;
                }
            }
        }
    });

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    // EOF on stdin means the client exited: shut down rather than linger as an
    // orphan holding a WebSocket open.
    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let request: protocol::Request = match serde_json::from_str(line) {
            Ok(request) => request,
            Err(e) => {
                let code = if serde_json::from_str::<serde_json::Value>(line).is_ok() {
                    // Valid JSON, wrong shape (e.g. no `method`).
                    INVALID_REQUEST
                } else {
                    PARSE_ERROR
                };
                let _ = tx.send(Message::error(None, code, e.to_string()).to_line());
                continue;
            }
        };

        if request.method == "shutdown" {
            if let Some(id) = request.id {
                let _ = tx.send(Message::result(id, serde_json::Value::Null).to_line());
            }
            break;
        }

        if !methods::is_known(&request.method) {
            // Unknown notifications are silently ignored, as JSON-RPC requires.
            if let Some(id) = request.id {
                let message = format!("unknown method `{}`", request.method);
                let _ = tx.send(Message::error(Some(id), METHOD_NOT_FOUND, message).to_line());
            }
            continue;
        }

        // One task per request so a slow call cannot block the read loop —
        // that is the whole point of holding the connection open. The permit
        // is what keeps "concurrent" from meaning "unbounded": it is taken
        // inside the task so the read loop keeps accepting lines while
        // requests queue for their turn upstream.
        let reply_tx = tx.clone();
        let permits = Arc::clone(&permits);
        tokio::spawn(async move {
            let _permit = permits.acquire().await;
            let outcome = methods::call(&request.method, request.params).await;
            // A notification (no `id`) still runs; it just owes no reply.
            let Some(id) = request.id else { return };
            let message = match outcome {
                Ok(result) => Message::result(id, result),
                Err(e) => Message::error(Some(id), error_code_for(&e), e.to_string()),
            };
            let _ = reply_tx.send(message.to_line());
        });
    }

    // Drop the last sender so the writer drains what is queued and stops once
    // the in-flight handlers drop theirs — bounded, so one wedged upstream
    // call cannot keep the process alive after its client is gone.
    drop(tx);
    pusher.abort();
    let _ = tokio::time::timeout(DRAIN_TIMEOUT, writer).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_mistakes_and_upstream_failures_get_different_codes() {
        assert_eq!(
            error_code_for(&params::param_err("missing required parameter `symbols`")),
            INVALID_PARAMS
        );
        assert_eq!(
            error_code_for(&anyhow::anyhow!("connection reset by peer")),
            API_ERROR
        );
    }

    /// The reason the classification is by type: an upstream failure worded
    /// like a parameter complaint used to be reported as the client's fault,
    /// telling it not to retry something a retry would have fixed.
    #[test]
    fn an_upstream_error_worded_like_a_parameter_complaint_is_still_upstream() {
        for msg in [
            "`700.HK` is not available in your region",
            "missing required parameter `symbols` (upstream said so)",
            "unknown field in response",
        ] {
            assert_eq!(
                error_code_for(&anyhow::anyhow!(msg)),
                API_ERROR,
                "misclassified: {msg}"
            );
        }
    }

    /// A `ParamError` keeps its meaning through the `?` chain that carries it
    /// out of a handler, which is where the classification actually happens.
    #[test]
    fn a_param_error_survives_being_wrapped_by_anyhow() {
        fn inner() -> Result<()> {
            Err(params::param_err("`side` must be buy or sell"))
        }
        fn outer() -> Result<()> {
            inner()?;
            Ok(())
        }
        assert_eq!(error_code_for(&outer().unwrap_err()), INVALID_PARAMS);
    }
}
