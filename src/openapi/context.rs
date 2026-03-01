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
    // Try to load existing token or start OAuth flow
    let token = match crate::auth::load_token()? {
        Some(t) if !t.is_expired() => {
            tracing::debug!("Using existing OAuth token from keychain");
            t.access_token
        }
        Some(_) => {
            tracing::debug!("Token expired, starting OAuth flow");
            let token = crate::auth::authorize().await?;
            token.access_token
        }
        None => {
            tracing::debug!("No token found, starting OAuth flow");
            let token = crate::auth::authorize().await?;
            token.access_token
        }
    };

    init_contexts_with_token(token).await
}

/// Initialize contexts with OAuth token
async fn init_contexts_with_token(
    access_token: String,
) -> Result<impl tokio_stream::Stream<Item = longport::quote::PushEvent> + Send + Unpin> {
    // Set language based on current UI locale
    std::env::set_var("LONGPORT_LANGUAGE", get_api_language());
    std::env::set_var("LONGPORT_PRINT_QUOTE_PACKAGES", "false");

    // For OAuth 2.0, the client_id acts as app_key
    // The access_token should be prefixed with "Bearer "
    let bearer_token = format!("Bearer {}", access_token);

    let config = Arc::new(longport::Config::new(
        "fd52fbc5-02a9-47f5-ad30-0842c841aae9", // OAuth client_id as app_key
        "", // app_secret not needed for OAuth Bearer tokens
        bearer_token,
    ));

    // Create QuoteContext and TradeContext
    let (quote_ctx, quote_receiver) =
        match longport::quote::QuoteContext::try_new(Arc::clone(&config)).await {
            Ok(ctx) => ctx,
            Err(e) => {
                // Check if error is authentication-related
                let error_msg = e.to_string();
                if error_msg.contains("401")
                    || error_msg.contains("Unauthorized")
                    || error_msg.contains("authentication")
                {
                    tracing::error!("Token validation failed, clearing stored credentials");
                    if let Err(clear_err) = crate::auth::clear_token() {
                        tracing::error!("Failed to clear token: {clear_err}");
                    }
                    return Err(anyhow::anyhow!(
                    "Token validation failed. Please restart the application to re-authenticate."
                ));
                }
                return Err(e.into());
            }
        };
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
