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
    eprintln!("Configure your MCP client with: longbridge mcp serve");
    eprintln!("Requires prior authentication: longbridge auth login");

    server::Server::new().run(quote_stream).await
}
