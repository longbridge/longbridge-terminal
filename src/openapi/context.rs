use anyhow::Result;
use std::sync::{Arc, OnceLock};

use super::wrapper::{RateLimitedQuoteContext, RateLimitedTradeContext};

/// Global `QuoteContext`
pub static QUOTE_CTX: OnceLock<longbridge::quote::QuoteContext> = OnceLock::new();

/// Global `AssetContext`
pub static STATEMENT_CTX: OnceLock<longbridge::AssetContext> = OnceLock::new();

/// Global `TradeContext`
pub static TRADE_CTX: OnceLock<longbridge::trade::TradeContext> = OnceLock::new();

/// Global `ContentContext` for news and topics
pub static CONTENT_CTX: OnceLock<longbridge::ContentContext> = OnceLock::new();

/// Global `HttpClient` for making authenticated requests to the Longbridge `OpenAPI`
pub static HTTP_CLIENT: OnceLock<longbridge::httpclient::HttpClient> = OnceLock::new();

/// Global rate-limited `QuoteContext` wrapper
pub static RATE_LIMITED_QUOTE_CTX: OnceLock<RateLimitedQuoteContext> = OnceLock::new();

/// Global rate-limited `TradeContext` wrapper
pub static RATE_LIMITED_TRADE_CTX: OnceLock<RateLimitedTradeContext> = OnceLock::new();

/// Map the effective content language to the SDK Language enum.
fn get_api_language() -> longbridge::Language {
    match crate::locale::get() {
        "zh-CN" => longbridge::Language::ZH_CN,
        "zh-HK" => longbridge::Language::ZH_HK,
        _ => longbridge::Language::EN,
    }
}

/// Initialize contexts (should be called once at app startup).
/// If `LONGBRIDGE_APP_KEY`, `LONGBRIDGE_APP_SECRET`, and `LONGBRIDGE_ACCESS_TOKEN`
/// are all set, uses API key authentication (no browser needed).
/// Otherwise falls back to OAuth: loads token from disk or runs browser flow.
/// Returns `(quote_stream, using_api_key, http_url)` where `http_url` is the
/// effective base URL that was configured (useful for diagnostics/verbose output).
pub async fn init_contexts() -> Result<(
    impl tokio_stream::Stream<Item = longbridge::quote::PushEvent> + Send + Unpin,
    bool,
    &'static str,
)> {
    let (config_builder, http_client_config, using_api_key) = if let (Ok(config), Ok(http_config)) = (
        longbridge::Config::from_apikey_env(),
        longbridge::httpclient::HttpClientConfig::from_apikey_env(),
    ) {
        tracing::info!("Using API key authentication (env vars)");
        (
            config
                .language(get_api_language())
                .dont_print_quote_packages(),
            http_config,
            true,
        )
    } else {
        tracing::info!("No API key env vars found, using OAuth authentication");
        // If no token file exists, refuse to start a browser/callback-server flow.
        // CLI commands require a stored token; users must run `longbridge auth login` first.
        let token_path = crate::auth::token_file_path()?;
        if !token_path.exists() {
            return Err(anyhow::anyhow!(
                "Not authenticated. Please run 'longbridge auth login' first."
            ));
        }
        // Token file exists: load it via OAuthBuilder (SDK handles refresh automatically).
        let oauth_result = longbridge::oauth::OAuthBuilder::new(crate::auth::client_id())
            .callback_port(crate::auth::CALLBACK_PORT)
            .build(|_url| {
                // This callback is only invoked if the token file is missing or fully
                // expired and cannot be refreshed — in practice, we guard above, so
                // reaching here would be a race condition.  Just log; do not open browser.
                tracing::warn!("OAuth browser flow triggered unexpectedly");
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
                            "Stored token is invalid or expired. Please run 'longbridge auth login' to re-authenticate."
                        ));
                }
                return Err(anyhow::anyhow!("OAuth failed: {e}"));
            }
        };

        let config_builder = longbridge::Config::from_oauth(oauth.clone())
            .language(get_api_language())
            .dont_print_quote_packages();

        let http_client_config =
            longbridge::httpclient::HttpClientConfig::from_oauth(oauth.clone());
        (config_builder, http_client_config, false)
    };

    let mut config_builder = config_builder;
    let mut http_client_config = http_client_config;

    // If LONGBRIDGE_ENV=staging, override all endpoints to test environment.
    // This takes highest priority over region detection.
    let effective_http_url;
    if crate::auth::is_test_env() {
        tracing::info!("Using TEST environment endpoints (openapi.longbridge.xyz)");
        config_builder = config_builder
            .http_url(crate::region::HTTP_URL_TEST)
            .quote_ws_url(crate::region::QUOTE_WS_URL_TEST)
            .trade_ws_url(crate::region::TRADE_WS_URL_TEST);
        http_client_config = http_client_config.http_url(crate::region::HTTP_URL_TEST);
        effective_http_url = crate::region::HTTP_URL_TEST;
    } else if crate::region::is_cn_cached()
        && (cfg!(not(debug_assertions))
            || std::env::var("LONGBRIDGE_HTTP_URL")
                .or_else(|_| std::env::var("LONGPORT_HTTP_URL"))
                .is_err())
    {
        // If last geotest indicated China Mainland, use CN endpoints directly.
        // In debug builds, skip if LONGBRIDGE_HTTP_URL is set (allows local mock server testing).
        tracing::debug!("Using CN region endpoints (cached)");
        config_builder = config_builder
            .http_url(crate::region::HTTP_URL_CN)
            .quote_ws_url(crate::region::QUOTE_WS_URL_CN)
            .trade_ws_url(crate::region::TRADE_WS_URL_CN);
        http_client_config = http_client_config.http_url(crate::region::HTTP_URL_CN);
        effective_http_url = crate::region::HTTP_URL_CN;
    } else {
        effective_http_url = "https://openapi.longbridge.com";
    }

    // Extract x-cli-cmd: first positional (non-flag) arg; skip --format / --lang values.
    let cli_cmd = {
        let mut iter = std::env::args().skip(1);
        let mut found = String::new();
        while let Some(arg) = iter.next() {
            if arg == "--format" || arg == "--lang" {
                iter.next();
            } else if !arg.starts_with('-') {
                found = arg;
                break;
            }
        }
        found
    };

    let user_agent = concat!("longbridge-cli/", env!("CARGO_PKG_VERSION"));
    let channel = crate::auth::read_channel();

    // Inject into Config so headers appear in WebSocket upgrade requests too.
    config_builder = config_builder.header("user-agent", user_agent);
    if !cli_cmd.is_empty() {
        config_builder = config_builder.header("x-cli-cmd", &cli_cmd);
    }
    if let Some(ref ch) = channel {
        config_builder = config_builder.header("x-channel-id", ch.as_str());
    }

    let config = Arc::new(config_builder);

    let content_ctx = longbridge::ContentContext::new(Arc::clone(&config));
    CONTENT_CTX
        .set(content_ctx)
        .map_err(|_| anyhow::anyhow!("ContentContext already initialized"))?;

    let statement_ctx = longbridge::AssetContext::new(Arc::clone(&config));
    STATEMENT_CTX
        .set(statement_ctx)
        .map_err(|_| anyhow::anyhow!("AssetContext already initialized"))?;

    // Also inject into the standalone HttpClient used for direct REST calls.
    let mut http_client = longbridge::httpclient::HttpClient::new(http_client_config);
    http_client = http_client.header("user-agent", user_agent);
    if !cli_cmd.is_empty() {
        http_client = http_client.header("x-cli-cmd", cli_cmd.as_str());
    }
    if let Some(ref ch) = channel {
        http_client = http_client.header("x-channel-id", ch.as_str());
    }

    HTTP_CLIENT
        .set(http_client)
        .map_err(|_| anyhow::anyhow!("HttpClient already initialized"))?;

    // Create QuoteContext and TradeContext.
    // new() is synchronous and infallible in the new SDK; connection and auth errors
    // will surface naturally on the first real API call made by the caller.
    let (quote_ctx, quote_receiver) = longbridge::quote::QuoteContext::new(Arc::clone(&config));
    let (trade_ctx, _trade_receiver) = longbridge::trade::TradeContext::new(Arc::clone(&config));

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

    Ok((
        tokio_stream::wrappers::UnboundedReceiverStream::new(quote_receiver),
        using_api_key,
        effective_http_url,
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

/// Get global `ContentContext` for news and topics
pub fn content() -> &'static longbridge::ContentContext {
    CONTENT_CTX
        .get()
        .expect("ContentContext not initialized, please call init_contexts() first")
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
        .expect("TradeContext not initialized, please call init_contexts() first")
}

/// Get global `AssetContext`
pub fn statement() -> &'static longbridge::AssetContext {
    STATEMENT_CTX
        .get()
        .expect("AssetContext not initialized, please call init_contexts() first")
}
