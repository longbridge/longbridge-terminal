pub mod protocol;
pub mod server;
pub mod tools;

use anyhow::Result;

pub async fn serve(
    quote_stream: impl tokio_stream::Stream<Item = longbridge::quote::PushEvent>
        + Send
        + Unpin
        + 'static,
) -> Result<()> {
    eprintln!("Longbridge MCP server starting (stdio transport)");
    server::Server::new().run(quote_stream).await
}

pub fn print_guide() {
    println!(
        r#"Longbridge MCP Server Setup Guide
==================================

The Longbridge MCP server runs locally over stdio and connects your AI client
(Claude Desktop, Cursor, Continue, etc.) to live market data and trading APIs.

Prerequisites
-------------
1. Authenticate first (if you haven't already):
   longbridge auth login

2. Add the following to your MCP client configuration:

Claude Desktop (~/.claude/claude_desktop_config.json):
  {{
    "mcpServers": {{
      "longbridge": {{
        "command": "longbridge",
        "args": ["mcp", "serve"]
      }}
    }}
  }}

Cursor / Continue:
  Add an MCP server with command: longbridge mcp serve

Available Tools
---------------
  quote             Real-time quote for one or more symbols
  depth             Level 2 order book (bid/ask depth)
  trades            Recent trade ticks
  intraday          Intraday price history
  kline             OHLCV candlestick data (1m/5m/15m/30m/60m/day/week/month)
  static_info       Static instrument metadata
  positions         Current portfolio positions
  account_balance   Cash balance by currency
  orders            Today's orders
  submit_order      Place a new order (requires confirmation)
  cancel_order      Cancel an existing order (requires confirmation)
  subscribe_quote   Subscribe to real-time quote push events
  unsubscribe_quote Stop receiving quote push events

For more information: https://open.longbridge.com
"#
    );
}
