use anyhow::Result;
use std::sync::{Arc, OnceLock};

/// Global QuoteContext
pub static QUOTE_CTX: OnceLock<longport::quote::QuoteContext> = OnceLock::new();

/// Global TradeContext
pub static TRADE_CTX: OnceLock<longport::trade::TradeContext> = OnceLock::new();

/// Initialize contexts (should be called once at app startup)
/// Returns quote receiver for caller to handle WebSocket events
pub async fn init_contexts(
) -> Result<impl tokio_stream::Stream<Item = longport::quote::PushEvent> + Send + Unpin> {
    // Load config from environment variables
    let config = Arc::new(longport::Config::from_env()?);

    // Create QuoteContext and TradeContext
    let (quote_ctx, quote_receiver) =
        longport::quote::QuoteContext::try_new(Arc::clone(&config)).await?;
    let (trade_ctx, _trade_receiver) =
        longport::trade::TradeContext::try_new(Arc::clone(&config)).await?;

    // Store in global variables
    QUOTE_CTX
        .set(quote_ctx)
        .map_err(|_| anyhow::anyhow!("QuoteContext already initialized"))?;
    TRADE_CTX
        .set(trade_ctx)
        .map_err(|_| anyhow::anyhow!("TradeContext already initialized"))?;

    // Wrap as Stream
    Ok(tokio_stream::wrappers::UnboundedReceiverStream::new(
        quote_receiver,
    ))
}

/// Get global QuoteContext
pub fn quote() -> &'static longport::quote::QuoteContext {
    QUOTE_CTX
        .get()
        .expect("QuoteContext not initialized, please call init_contexts() first")
}

/// Get global TradeContext
pub fn trade() -> &'static longport::trade::TradeContext {
    TRADE_CTX
        .get()
        .expect("TradeContext not initialized, please call init_contexts() first")
}

/// Display config guide (when config loading fails)
pub fn print_config_guide() {
    eprintln!("Configuration Error: Missing required environment variables");
    eprintln!();
    eprintln!("Please configure the following environment variables:");
    eprintln!("  LONGPORT_APP_KEY=<your_app_key>");
    eprintln!("  LONGPORT_APP_SECRET=<your_app_secret>");
    eprintln!("  LONGPORT_ACCESS_TOKEN=<your_access_token>");
    eprintln!();
    eprintln!("You can also specify custom server addresses via LONGPORT_HTTP_URL and LONGPORT_QUOTE_WS_URL");
    eprintln!();
    eprintln!("Get Token: https://open.longbridge.com");
    eprintln!();
    eprintln!("Tip: You can create a .env file to configure these variables");
}
