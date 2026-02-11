use anyhow::Result;
use std::sync::{Arc, OnceLock};

use super::wrapper::{RateLimitedQuoteContext, RateLimitedTradeContext};

/// Global `QuoteContext`
pub static QUOTE_CTX: OnceLock<longport::quote::QuoteContext> = OnceLock::new();

/// Global `TradeContext`
pub static TRADE_CTX: OnceLock<longport::trade::TradeContext> = OnceLock::new();

/// Global rate-limited `QuoteContext` wrapper
pub static RATE_LIMITED_QUOTE_CTX: OnceLock<RateLimitedQuoteContext> = OnceLock::new();

/// Global rate-limited `TradeContext` wrapper
pub static RATE_LIMITED_TRADE_CTX: OnceLock<RateLimitedTradeContext> = OnceLock::new();

/// Get API language based on current UI locale
/// Maps UI locale to API-supported languages: en, zh-CN, zh-HK
/// Defaults to "en" if locale is not supported
fn get_api_language() -> &'static str {
    match rust_i18n::locale().as_str() {
        "zh-CN" => "zh-CN",
        "zh-HK" => "zh-HK",
        _ => "en", // Default to English
    }
}

/// Initialize contexts (should be called once at app startup)
/// Returns quote receiver for caller to handle WebSocket events
pub async fn init_contexts(
) -> Result<impl tokio_stream::Stream<Item = longport::quote::PushEvent> + Send + Unpin> {
    // Set language based on current UI locale
    std::env::set_var("LONGPORT_LANGUAGE", get_api_language());

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

    // Initialize rate-limited wrappers
    let quote_ref = QUOTE_CTX.get().expect("QuoteContext just initialized");
    let trade_ref = TRADE_CTX.get().expect("TradeContext just initialized");

    RATE_LIMITED_QUOTE_CTX
        .set(RateLimitedQuoteContext::new(quote_ref))
        .map_err(|_| anyhow::anyhow!("RateLimitedQuoteContext already initialized"))?;
    RATE_LIMITED_TRADE_CTX
        .set(RateLimitedTradeContext::new(trade_ref))
        .map_err(|_| anyhow::anyhow!("RateLimitedTradeContext already initialized"))?;

    tracing::info!("Rate limiter initialized: 10 requests/second, burst capacity: 20");

    // Wrap as Stream
    Ok(tokio_stream::wrappers::UnboundedReceiverStream::new(
        quote_receiver,
    ))
}

/// Get global `QuoteContext`
pub fn quote() -> &'static longport::quote::QuoteContext {
    QUOTE_CTX
        .get()
        .expect("QuoteContext not initialized, please call init_contexts() first")
}

/// Get global `TradeContext`
pub fn trade() -> &'static longport::trade::TradeContext {
    TRADE_CTX
        .get()
        .expect("TradeContext not initialized, please call init_contexts() first")
}

/// Get rate-limited `QuoteContext` (recommended for all API calls)
pub fn quote_limited() -> &'static RateLimitedQuoteContext {
    RATE_LIMITED_QUOTE_CTX
        .get()
        .expect("RateLimitedQuoteContext not initialized, please call init_contexts() first")
}

/// Get rate-limited `TradeContext` (recommended for all API calls)
pub fn trade_limited() -> &'static RateLimitedTradeContext {
    RATE_LIMITED_TRADE_CTX
        .get()
        .expect("RateLimitedTradeContext not initialized, please call init_contexts() first")
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
