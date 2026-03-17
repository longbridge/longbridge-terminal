use anyhow::Result;
use std::sync::{Arc, OnceLock};

use super::wrapper::{RateLimitedQuoteContext, RateLimitedTradeContext};

/// Global `QuoteContext`
pub static QUOTE_CTX: OnceLock<longbridge::quote::QuoteContext> = OnceLock::new();

/// Global `TradeContext`
pub static TRADE_CTX: OnceLock<longbridge::trade::TradeContext> = OnceLock::new();

/// Global `HttpClient` for making authenticated requests to the Longbridge `OpenAPI`
pub static HTTP_CLIENT: OnceLock<longbridge::httpclient::HttpClient> = OnceLock::new();

/// Global rate-limited `QuoteContext` wrapper
pub static RATE_LIMITED_QUOTE_CTX: OnceLock<RateLimitedQuoteContext> = OnceLock::new();

/// Global rate-limited `TradeContext` wrapper
pub static RATE_LIMITED_TRADE_CTX: OnceLock<RateLimitedTradeContext> = OnceLock::new();

/// Get API language based on current UI locale
/// Maps UI locale to API-supported languages: en, zh-CN, zh-HK
/// Defaults to "en" if locale is not supported
fn get_api_language() -> longbridge::Language {
    match rust_i18n::locale().as_str() {
        "zh-CN" => longbridge::Language::ZH_CN,
        "zh-HK" => longbridge::Language::ZH_HK,
        _ => longbridge::Language::EN,
    }
}

/// Initialize contexts (should be called once at app startup).
/// Uses longbridge SDK OAuth: loads token from disk or runs browser flow, auto-refreshes token.
/// Returns quote receiver for caller to handle WebSocket events.
pub async fn init_contexts(
) -> Result<impl tokio_stream::Stream<Item = longbridge::quote::PushEvent> + Send + Unpin> {
    // Build OAuth client: loads token from ~/.longbridge/terminal/.openapi-session
    // or starts browser authorization. Token refresh is automatic inside the SDK.
    let oauth_result = longbridge::oauth::OAuthBuilder::new(crate::auth::OAUTH_CLIENT_ID)
        .callback_port(60355)
        .build(|url| {
            println!("Opening browser for Longbridge OpenAPI authorization...");
            println!("If the browser doesn't open, please visit:\n{url}");
            if let Err(e) = open::that(url) {
                tracing::warn!("Failed to open browser: {e}");
            }
        })
        .await;

    let oauth = match oauth_result {
        Ok(o) => o,
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("refresh token") || msg.contains("parse server response") {
                tracing::warn!("Token refresh failed, clearing stale token: {msg}");
                let _ = crate::auth::clear_token();
                return Err(anyhow::anyhow!(
                    "Stored token is invalid or expired. Please run 'longbridge login' to re-authenticate."
                ));
            }
            return Err(anyhow::anyhow!("OAuth failed: {e}"));
        }
    };

    let http_client = longbridge::httpclient::HttpClient::new(
        longbridge::httpclient::HttpClientConfig::from_oauth(oauth.clone()),
    );
    HTTP_CLIENT
        .set(http_client)
        .map_err(|_| anyhow::anyhow!("HttpClient already initialized"))?;

    let mut config_builder = longbridge::Config::from_oauth(oauth)
        .language(get_api_language())
        .dont_print_quote_packages();

    // If last geotest indicated China Mainland, use CN endpoints directly.
    if crate::region::is_cn_cached() {
        tracing::debug!("Using CN region endpoints (cached)");
        config_builder = config_builder
            .http_url(crate::region::HTTP_URL_CN)
            .quote_ws_url(crate::region::QUOTE_WS_URL_CN)
            .trade_ws_url(crate::region::TRADE_WS_URL_CN);
    }

    let config = Arc::new(config_builder);

    // Create QuoteContext and TradeContext
    let quote_result = longbridge::quote::QuoteContext::try_new(Arc::clone(&config)).await;
    let (quote_ctx, quote_receiver) = match quote_result {
        Ok(ctx) => ctx,
        Err(e) => {
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
        longbridge::trade::TradeContext::try_new(Arc::clone(&config)).await?;

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

    Ok(tokio_stream::wrappers::UnboundedReceiverStream::new(
        quote_receiver,
    ))
}

/// Get global `QuoteContext`
pub fn quote() -> &'static longbridge::quote::QuoteContext {
    QUOTE_CTX
        .get()
        .expect("QuoteContext not initialized, please call init_contexts() first")
}

/// Get global `TradeContext`
pub fn trade() -> &'static longbridge::trade::TradeContext {
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

/// Get the global authenticated `HttpClient` for direct `OpenAPI` requests
pub fn http_client() -> &'static longbridge::httpclient::HttpClient {
    HTTP_CLIENT
        .get()
        .expect("HttpClient not initialized, please call init_contexts() first")
}

/// Get rate-limited `TradeContext` (recommended for all API calls)
pub fn trade_limited() -> &'static RateLimitedTradeContext {
    RATE_LIMITED_TRADE_CTX
        .get()
        .expect("RateLimitedTradeContext not initialized, please call init_contexts() first")
}
