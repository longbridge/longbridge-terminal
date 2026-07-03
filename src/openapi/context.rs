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

/// Global `FundamentalContext` for fundamental data (ratings, dividends, ETF allocation, etc.)
pub static FUNDAMENTAL_CTX: OnceLock<longbridge::FundamentalContext> = OnceLock::new();

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

fn ascii_args(args: Vec<String>) -> String {
    args.into_iter()
        .filter(|a| a.is_ascii())
        .collect::<Vec<_>>()
        .join(" ")
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
        // If the token file exists but cannot be decrypted (e.g. machine ID
        // changed), fail fast rather than hanging in the OAuth browser flow.
        if crate::secure_storage::EncryptedFileTokenStorage::load_full(
            &crate::auth::effective_client_id(),
        )
        .is_none()
        {
            return Err(anyhow::anyhow!(
                "Failed to decrypt auth token. Please run 'longbridge auth login' to \
                 re-authenticate."
            ));
        }

        // Refresh the access token ourselves if it has expired, before handing
        // off to the SDK.  This avoids a 5-minute browser-callback timeout that
        // the SDK would trigger when its own refresh fallback fires.
        crate::auth::refresh_if_expired().await?;

        let oauth_result = longbridge::oauth::OAuthBuilder::new(crate::auth::effective_client_id())
            .callback_port(crate::auth::CALLBACK_PORT)
            .token_storage(crate::secure_storage::EncryptedFileTokenStorage)
            .build(|_url| {
                tracing::warn!("OAuth browser flow triggered unexpectedly");
            })
            .await;

        let oauth = match oauth_result {
            Ok(o) => o,
            Err(e) => {
                return Err(anyhow::anyhow!("OAuth initialization failed: {e}"));
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

    // Enable the US overnight market so `quote` returns `overnight_quote`.
    // Pre/post-market quotes are returned without this flag, but the overnight
    // session is gated behind it (matches the longbridge-mcp server).
    config_builder = config_builder.enable_overnight();

    // If LONGBRIDGE_ENV=staging, override all endpoints to test environment.
    // This takes highest priority over region detection.
    let effective_http_url;
    if crate::region::is_test_env() {
        tracing::info!("Using TEST environment endpoints (openapi.longbridge.xyz)");
        config_builder = config_builder
            .http_url(crate::region::HTTP_URL_TEST)
            .quote_ws_url(crate::region::QUOTE_WS_URL_TEST)
            .trade_ws_url(crate::region::TRADE_WS_URL_TEST);
        http_client_config = http_client_config.http_url(crate::region::HTTP_URL_TEST);
        effective_http_url = crate::region::HTTP_URL_TEST;
    } else if crate::region::is_cn_cached()
        && (cfg!(not(debug_assertions)) || std::env::var("LONGBRIDGE_HTTP_URL").is_err())
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
        effective_http_url = crate::region::HTTP_URL_GLOBAL;
    }

    // Extract x-cli-cmd and x-cli-args from process arguments.
    // x-cli-cmd: the first positional (subcommand) arg.
    // x-cli-args: remaining args after the subcommand, non-ASCII tokens excluded.
    let (cli_cmd, cli_args) = {
        let mut iter = std::env::args().skip(1);
        let mut cmd = String::new();
        let mut args: Vec<String> = Vec::new();
        for arg in iter.by_ref() {
            if cmd.is_empty() && !arg.starts_with('-') {
                cmd = arg;
            } else if !arg.is_empty() {
                args.push(arg);
            }
        }
        let cli_args = ascii_args(args);
        (if cmd.is_ascii() { cmd } else { String::new() }, cli_args)
    };

    let user_agent = concat!("longbridge-cli/", env!("CARGO_PKG_VERSION"));

    // Inject into Config so headers appear in WebSocket upgrade requests too.
    config_builder = config_builder.header("user-agent", user_agent);
    if !cli_cmd.is_empty() {
        config_builder = config_builder.header("x-cli-cmd", &cli_cmd);
    }
    if !cli_args.is_empty() {
        config_builder = config_builder.header("x-cli-args", &cli_args);
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

    let fundamental_ctx = longbridge::FundamentalContext::new(Arc::clone(&config));
    FUNDAMENTAL_CTX
        .set(fundamental_ctx)
        .map_err(|_| anyhow::anyhow!("FundamentalContext already initialized"))?;

    // Also inject into the standalone HttpClient used for direct REST calls.
    let mut http_client = longbridge::httpclient::HttpClient::new(http_client_config);
    http_client = http_client.header("user-agent", user_agent);
    if !cli_cmd.is_empty() {
        http_client = http_client.header("x-cli-cmd", cli_cmd.as_str());
    }
    if !cli_args.is_empty() {
        http_client = http_client.header("x-cli-args", cli_args.as_str());
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

/// Server-side beacon endpoint. Quote operations flow over the WebSocket quote
/// channel and never reach the HTTP access log; a request to this fake path lets
/// the server record (and count) that a WS-backed quote command ran. The path
/// only needs to exist server-side to be logged.
pub(crate) const QUOTE_CMD_PATH: &str = "/v1/quote/cmd";

/// Send the tracking beacon over `client`. The (empty) body and any transport
/// error are ignored — the server only needs the access-log entry. Extracted as
/// its own awaitable function so the integration test can drive it against a
/// local server deterministically.
pub(crate) async fn send_quote_cmd(client: &longbridge::httpclient::HttpClient) {
    let _ = client
        .request(reqwest::Method::GET, QUOTE_CMD_PATH)
        .response::<String>()
        .send()
        .await;
}

/// Fire a best-effort `GET /v1/quote/cmd` so the server records a log entry for a
/// WS-backed quote operation. It reuses the global `HttpClient`, which already
/// carries the tracking headers (`user-agent`, `x-cli-cmd`, `x-cli-args`) and the
/// OAuth token, so no extra payload is needed. Fire-and-forget: spawned on the
/// runtime with its result and errors ignored, never blocking or delaying the
/// real quote call. Call this directly at CLI quote entry points that reach
/// `QuoteContext` only through shared helpers (e.g. portfolio via `account`).
pub fn track_quote_cmd() {
    let Some(client) = HTTP_CLIENT.get() else {
        return;
    };
    tokio::spawn(send_quote_cmd(client));
}

/// Get the global `QuoteContext` and record the WS quote operation server-side.
/// Use this at every CLI quote command entry point instead of [`quote`] so the
/// otherwise-unlogged WebSocket request is counted. See [`track_quote_cmd`].
pub fn quote_cmd() -> &'static longbridge::quote::QuoteContext {
    track_quote_cmd();
    quote()
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

/// Get global `FundamentalContext` for fundamental data
pub fn fundamental() -> &'static longbridge::FundamentalContext {
    FUNDAMENTAL_CTX
        .get()
        .expect("FundamentalContext not initialized, please call init_contexts() first")
}

/// Get the global authenticated `HttpClient` for direct `OpenAPI` requests
pub fn http_client() -> &'static longbridge::httpclient::HttpClient {
    HTTP_CLIENT
        .get()
        .expect("HttpClient not initialized, please call init_contexts() first")
}

/// Returns `true` when the current session is a US data-center account
/// (`token.ac` starts with `us_lb`). Used to route commands to US-specific
/// endpoints transparently without requiring a `--market` flag from the user.
pub async fn is_us_account() -> bool {
    http_client().dc_region().await == longbridge::DcRegion::Us
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

#[cfg(test)]
mod quote_cmd_tests {
    use super::{send_quote_cmd, QUOTE_CMD_PATH};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Start a throwaway HTTP server on an ephemeral port that captures the raw
    /// bytes of the first request, replies `200`, and hands the request back over
    /// a oneshot channel. A real socket — no HTTP mocking — so the test exercises
    /// the actual SDK `HttpClient` send path and survives future refactors.
    async fn spawn_capture_server() -> (u16, tokio::sync::oneshot::Receiver<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 4096];
            let mut data = Vec::new();
            // Read until the end of the request headers.
            while !data.windows(4).any(|w| w == b"\r\n\r\n") {
                match socket.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => data.extend_from_slice(&buf[..n]),
                }
            }
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .await;
            let _ = socket.flush().await;
            let _ = tx.send(String::from_utf8_lossy(&data).into_owned());
        });
        (port, rx)
    }

    /// `send_quote_cmd` must issue `GET /v1/quote/cmd` and carry whatever tracking
    /// headers the client was built with, so the server can attribute the
    /// otherwise-invisible WS quote operation.
    #[tokio::test]
    async fn send_quote_cmd_hits_endpoint_with_tracking_headers() {
        let (port, rx) = spawn_capture_server().await;

        // Build a client the same way production does (token + tracking headers),
        // but pointed at the local capture server.
        let oauth = longbridge::oauth::OAuth::from_token("test-token");
        let config = longbridge::httpclient::HttpClientConfig::from_oauth(oauth)
            .http_url(format!("http://127.0.0.1:{port}"));
        let client = longbridge::httpclient::HttpClient::new(config)
            .header("user-agent", "longbridge-cli/test")
            .header("x-cli-cmd", "quote");

        send_quote_cmd(&client).await;

        let request = tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .expect("capture server did not receive a request in time")
            .expect("capture server dropped the request");

        let request_line = request.lines().next().unwrap_or_default();
        assert!(
            request_line.starts_with(&format!("GET {QUOTE_CMD_PATH}")),
            "expected `GET {QUOTE_CMD_PATH}`, got request line: {request_line}"
        );

        let lower = request.to_lowercase();
        assert!(
            lower.contains("user-agent: longbridge-cli/test"),
            "tracking user-agent header missing; request was:\n{request}"
        );
        assert!(
            lower.contains("x-cli-cmd: quote"),
            "x-cli-cmd tracking header missing; request was:\n{request}"
        );
    }

    /// Recursively collect `.rs` files under `dir`.
    fn rs_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    out.extend(rs_files(&path));
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        out
    }

    /// Guard: every `QuoteContext` access inside `src/cli/` must go through the
    /// tracking accessor `quote_cmd()` (which fires the `/v1/quote/cmd` beacon),
    /// never the raw `quote()`. This turns "did we remember to track every
    /// command" from manual review into an enforced invariant — a new CLI
    /// command that reaches for `openapi::quote()` directly fails this test.
    ///
    /// Blind spot: commands that touch `QuoteContext` only through shared helpers
    /// in `src/openapi/` (e.g. portfolio via `account`) are not visible here;
    /// those fire the beacon explicitly at their CLI entry via `track_quote_cmd`.
    #[test]
    fn cli_uses_only_tracking_quote_accessor() {
        let cli_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cli");
        let mut offenders = Vec::new();
        for file in rs_files(&cli_dir) {
            let src = std::fs::read_to_string(&file).unwrap();
            for (i, line) in src.lines().enumerate() {
                if line.contains("openapi::quote()") {
                    offenders.push(format!("{}:{}", file.display(), i + 1));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "CLI must use `openapi::quote_cmd()` (fires the /v1/quote/cmd beacon), \
             not raw `openapi::quote()`. Untracked QuoteContext access at:\n{}",
            offenders.join("\n")
        );
    }
}

#[cfg(test)]
mod cli_header_tests {
    use super::ascii_args;

    #[test]
    fn all_ascii_pass_through() {
        let args = ["--format", "json", "--verbose"].map(String::from).to_vec();
        assert_eq!(ascii_args(args), "--format json --verbose");
    }

    #[test]
    fn non_ascii_value_is_excluded() {
        // The flag token itself is ASCII and kept; the CJK value is dropped.
        let args = ["--name", "我的组"].map(String::from).to_vec();
        assert_eq!(ascii_args(args), "--name");
    }

    #[test]
    fn mixed_args_keep_ascii_only() {
        let args = ["--format", "json", "--name", "我的组", "--verbose"]
            .map(String::from)
            .to_vec();
        assert_eq!(ascii_args(args), "--format json --name --verbose");
    }

    #[test]
    fn all_non_ascii_yields_empty() {
        let args = ["你好", "世界"].map(String::from).to_vec();
        assert_eq!(ascii_args(args), "");
    }

    #[test]
    fn empty_input_yields_empty() {
        assert_eq!(ascii_args(vec![]), "");
    }

    #[test]
    fn topic_body_non_ascii_excluded() {
        let args = ["--body", "这是话题内容"].map(String::from).to_vec();
        assert_eq!(ascii_args(args), "--body");
    }
}
