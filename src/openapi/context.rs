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

/// The HTTP base URL chosen at context init (test env / CN / global).
static EFFECTIVE_HTTP_URL: OnceLock<String> = OnceLock::new();

/// Whether the session authenticated with API-key env vars instead of OAuth.
static USING_API_KEY: OnceLock<bool> = OnceLock::new();

/// `true` when the current process authenticated through
/// `LONGBRIDGE_APP_KEY` / `LONGBRIDGE_APP_SECRET` / `LONGBRIDGE_ACCESS_TOKEN`
/// (see [`init_contexts`]) rather than OAuth. Callers that bypass the SDK
/// `HttpClient` — notably the SSE transport in `cli::agent::client`, which
/// signs nothing and sends a plain OAuth bearer token — must check this so
/// they fail with an actionable message instead of authenticating as a
/// different principal (or not at all).
///
/// Defaults to `false` before contexts are initialized.
pub fn using_api_key() -> bool {
    USING_API_KEY.get().copied().unwrap_or(false)
}

/// `LONGBRIDGE_HTTP_URL` as it was at process start, captured once.
static CAPTURED_HTTP_URL_OVERRIDE: OnceLock<Option<String>> = OnceLock::new();

/// The `LONGBRIDGE_HTTP_URL` override, read from the environment exactly once
/// and cached for the life of the process.
///
/// Security-relevant: the SDK constructors load a `.env` file from the current
/// working directory during [`init_contexts`], which means a checked-in `.env`
/// in whatever repository the user happens to `cd` into can inject environment
/// variables *after* startup. If the agent SSE transport re-read the variable
/// at request time, such a file could redirect a request carrying the OAuth
/// bearer token to an attacker-controlled host. Capturing the value before any
/// of that runs (see the first statement of `main`) removes that window; every
/// consumer must use this accessor and never `std::env::var` directly.
pub fn captured_http_url_override() -> Option<&'static str> {
    CAPTURED_HTTP_URL_OVERRIDE
        .get_or_init(|| {
            std::env::var("LONGBRIDGE_HTTP_URL")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .as_deref()
}

/// The host override, but only when it is actually allowed to take effect.
///
/// Invariant — *one resolution, both transports*: the SDK-backed REST/WS
/// clients and the hand-rolled agent SSE transport must always talk to the
/// same host. [`init_contexts`] only honors the override in debug builds (it
/// exists for pointing a local build at a mock server), so the same gate is
/// applied here; otherwise a release build would send REST to the region host
/// and SSE somewhere else.
fn allowed_http_url_override() -> Option<String> {
    if cfg!(debug_assertions) {
        captured_http_url_override().map(ToString::to_string)
    } else {
        None
    }
}

/// Resolve the effective HTTP base URL from its three sources, in priority
/// order: the URL published by [`init_contexts`], the allowed
/// `LONGBRIDGE_HTTP_URL` override, and finally the region-derived default.
/// Split out from [`effective_http_url`] so the precedence is unit-testable
/// without touching process-global state.
///
/// Invariant — *host resolution happens once, both transports read that one
/// value*: `initialized` is whatever [`init_contexts`] pinned the SDK clients
/// to, so once it exists it is authoritative and nothing may override it. The
/// captured override is a pre-init fallback only; consulting it first would
/// let the SSE transport disagree with the SDK whenever `init_contexts` chose
/// a different host (e.g. `LONGBRIDGE_ENV=staging` wins over the override).
fn resolve_http_url(initialized: Option<String>, env_override: Option<String>) -> String {
    initialized
        .or_else(|| env_override.filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| crate::region::http_url().to_string())
}

/// The HTTP base URL chosen at context init (override / test env / CN /
/// global). This is what the agent SSE transport builds its request URL from.
///
/// Once [`init_contexts`] has run, `EFFECTIVE_HTTP_URL` holds the host the SDK
/// clients were pinned to — including the override when it applied — and that
/// value wins outright, so both transports agree by construction. The override
/// is only consulted for callers that run before contexts exist.
pub fn effective_http_url() -> String {
    resolve_http_url(
        EFFECTIVE_HTTP_URL.get().cloned(),
        allowed_http_url_override(),
    )
}

/// The endpoint triple the SDK clients get pinned to, and whose `http` field is
/// published through `EFFECTIVE_HTTP_URL` for the SSE transport to reuse.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Endpoints {
    http: String,
    quote_ws: String,
    trade_ws: String,
}

/// Derive a WebSocket URL from an HTTP base URL, for the debug-only
/// `LONGBRIDGE_HTTP_URL` override. The other branches pair their HTTP host
/// with the matching WS constants; an override names a single host (typically
/// a local mock), so quote and trade both point at it rather than silently
/// falling back to the production global endpoints.
fn ws_url_from_http(http_url: &str) -> String {
    let trimmed = http_url.trim_end_matches('/');
    let base = if let Some(rest) = trimmed.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        // Already a ws/wss URL, or scheme-less: leave the authority as given.
        trimmed.to_string()
    };
    if base.ends_with("/v2") {
        base
    } else {
        format!("{base}/v2")
    }
}

/// The single place endpoints are chosen. Pure so the SSE resolver and the SDK
/// host can be asserted to agree in tests for every combination of inputs.
///
/// Priority: staging env > debug-only host override > CN access point > global.
fn resolve_endpoints(is_test_env: bool, override_url: Option<String>, use_cn: bool) -> Endpoints {
    if is_test_env {
        // `LONGBRIDGE_ENV=staging` outranks everything, including the override.
        Endpoints {
            http: crate::region::HTTP_URL_TEST.to_string(),
            quote_ws: crate::region::QUOTE_WS_URL_TEST.to_string(),
            trade_ws: crate::region::TRADE_WS_URL_TEST.to_string(),
        }
    } else if let Some(url) = override_url.filter(|s| !s.trim().is_empty()) {
        // Debug builds only: point the whole CLI at a local mock server. WS is
        // derived from the same host so REST, WS and SSE cannot diverge.
        let ws = ws_url_from_http(&url);
        Endpoints {
            http: url,
            quote_ws: ws.clone(),
            trade_ws: ws,
        }
    } else if use_cn {
        Endpoints {
            http: crate::region::HTTP_URL_CN.to_string(),
            quote_ws: crate::region::QUOTE_WS_URL_CN.to_string(),
            trade_ws: crate::region::TRADE_WS_URL_CN.to_string(),
        }
    } else {
        Endpoints {
            http: crate::region::HTTP_URL_GLOBAL.to_string(),
            quote_ws: crate::region::QUOTE_WS_URL_GLOBAL.to_string(),
            trade_ws: crate::region::TRADE_WS_URL_GLOBAL.to_string(),
        }
    }
}

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
    String,
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

    let _ = USING_API_KEY.set(using_api_key);

    let mut config_builder = config_builder;
    let mut http_client_config = http_client_config;

    // Enable the US overnight market so `quote` returns `overnight_quote`.
    // Pre/post-market quotes are returned without this flag, but the overnight
    // session is gated behind it (matches the longbridge-mcp server).
    config_builder = config_builder.enable_overnight();

    // Host resolution happens exactly once, here, and the result is published
    // through `EFFECTIVE_HTTP_URL`. Both transports read that one value: the
    // SDK clients because they are pinned below, and the agent SSE transport
    // through `effective_http_url()`. Nothing downstream may re-derive a host
    // (and nothing may read `LONGBRIDGE_HTTP_URL` from the live environment —
    // see `captured_http_url_override`).
    //
    // If LONGBRIDGE_ENV=staging, override all endpoints to test environment.
    // This takes highest priority over region detection and over the override.
    //
    // If last geotest indicated China Mainland, use CN endpoints directly.
    // Skip for US-DC tokens: US-specific APIs only exist on the global host.
    // Otherwise pin to the global host explicitly so the SDK does not re-run
    // geotest at request time (which would still resolve to CN on a China
    // Mainland network).
    let endpoints = resolve_endpoints(
        crate::region::is_test_env(),
        allowed_http_url_override(),
        crate::region::is_cn_cached() && !token_dc_is_us(&crate::auth::effective_client_id()),
    );
    tracing::info!("Using API endpoints: {}", endpoints.http);

    config_builder = config_builder
        .http_url(&endpoints.http)
        .quote_ws_url(&endpoints.quote_ws)
        .trade_ws_url(&endpoints.trade_ws);
    http_client_config = http_client_config.http_url(&endpoints.http);
    let effective_http_url = endpoints.http;

    let _ = EFFECTIVE_HTTP_URL.set(effective_http_url.clone());

    // Extract x-cli-cmd and x-cli-args from process arguments.
    // x-cli-cmd: the first positional (subcommand) arg.
    // x-cli-args: remaining args after the subcommand, non-ASCII tokens excluded.
    let (cli_cmd, cli_args) = {
        let mut iter = std::env::args().skip(1);
        let mut cmd = String::new();
        let mut args: Vec<String> = Vec::new();
        let mut prev_was_flag = false;
        for arg in iter.by_ref() {
            if cmd.is_empty() && !arg.starts_with('-') && !prev_was_flag {
                cmd.clone_from(&arg);
            } else if !arg.is_empty() {
                args.push(arg.clone());
            }
            // Only global value-taking flags (--format, --lang) consume the next arg as
            // their value. Boolean global flags do not, and subcommand-specific flags always
            // appear after cmd is already captured, so they cannot affect cmd extraction.
            prev_was_flag = matches!(arg.as_str(), "--format" | "--lang") && !arg.contains('=');
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

/// Returns `true` if the stored OAuth token carries a US data-center credential.
/// Used before `HttpClient` is initialized to choose the correct HTTP endpoint.
fn token_dc_is_us(client_id: &str) -> bool {
    crate::secure_storage::EncryptedFileTokenStorage::load_full(client_id)
        .and_then(|full| {
            full["access_token"]
                .as_str()
                .map(|t| longbridge::DcRegion::from_credential(t) == longbridge::DcRegion::Us)
        })
        .unwrap_or(false)
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
mod http_url_tests {
    use super::{
        allowed_http_url_override, captured_http_url_override, effective_http_url,
        resolve_endpoints, resolve_http_url, ws_url_from_http,
    };
    use serial_test::serial;

    /// The host `init_contexts` published is authoritative: once it exists,
    /// the pre-init override must not move the SSE transport off it.
    #[test]
    fn initialized_url_wins_over_env_override() {
        assert_eq!(
            resolve_http_url(
                Some("https://openapi-global.longbridge.xyz".to_string()),
                Some("http://127.0.0.1:8080".to_string()),
            ),
            "https://openapi-global.longbridge.xyz"
        );
    }

    /// Before `init_contexts` publishes anything, the override is the only
    /// signal available and is used.
    #[test]
    fn env_override_applies_before_initialization() {
        assert_eq!(
            resolve_http_url(None, Some("http://127.0.0.1:8080".to_string())),
            "http://127.0.0.1:8080"
        );
    }

    #[test]
    fn blank_env_override_is_ignored() {
        assert_eq!(
            resolve_http_url(None, Some("   ".to_string())),
            crate::region::http_url().to_string()
        );
        assert_eq!(
            resolve_http_url(Some("https://openapi.longbridge.cn".to_string()), None),
            "https://openapi.longbridge.cn"
        );
    }

    #[test]
    fn falls_back_to_region_url_without_cache() {
        assert_eq!(
            resolve_http_url(None, None),
            crate::region::http_url().to_string()
        );
    }

    /// Blocker 1 regression: with `LONGBRIDGE_ENV=staging` *and* a captured
    /// `LONGBRIDGE_HTTP_URL` override both present, `init_contexts` pins the
    /// SDK to the staging host — and the SSE resolver must land on that same
    /// host rather than preferring the override.
    #[test]
    fn sse_resolver_agrees_with_sdk_host_when_staging_and_override_collide() {
        let override_url = "http://127.0.0.1:8080".to_string();
        let endpoints = resolve_endpoints(true, Some(override_url.clone()), false);
        assert_eq!(endpoints.http, crate::region::HTTP_URL_TEST);

        // What `effective_http_url()` computes after `init_contexts` published
        // the SDK's choice, with the override still captured.
        let sse_url = resolve_http_url(Some(endpoints.http.clone()), Some(override_url));
        assert_eq!(
            sse_url, endpoints.http,
            "SSE transport and SDK must resolve the same host"
        );
    }

    /// The same agreement must hold for every branch `resolve_endpoints` can
    /// take, with and without an override captured.
    #[test]
    fn sse_resolver_agrees_with_sdk_host_for_every_branch() {
        let override_url = Some("http://127.0.0.1:8080".to_string());
        for is_test_env in [true, false] {
            for over in [None, override_url.clone()] {
                for use_cn in [true, false] {
                    let endpoints = resolve_endpoints(is_test_env, over.clone(), use_cn);
                    assert_eq!(
                        resolve_http_url(Some(endpoints.http.clone()), over.clone()),
                        endpoints.http,
                        "mismatch for staging={is_test_env} override={over:?} cn={use_cn}"
                    );
                }
            }
        }
    }

    #[test]
    fn staging_outranks_the_override() {
        let endpoints = resolve_endpoints(true, Some("http://127.0.0.1:8080".to_string()), true);
        assert_eq!(endpoints.http, crate::region::HTTP_URL_TEST);
        assert_eq!(endpoints.quote_ws, crate::region::QUOTE_WS_URL_TEST);
        assert_eq!(endpoints.trade_ws, crate::region::TRADE_WS_URL_TEST);
    }

    /// Blocker 2 regression: the override branch must derive its WS URLs from
    /// the override, not silently keep the production global endpoints.
    #[test]
    fn override_branch_derives_ws_urls_from_the_override() {
        let endpoints = resolve_endpoints(false, Some("http://127.0.0.1:8080".to_string()), false);
        assert_eq!(endpoints.http, "http://127.0.0.1:8080");
        assert_eq!(endpoints.quote_ws, "ws://127.0.0.1:8080/v2");
        assert_eq!(endpoints.trade_ws, "ws://127.0.0.1:8080/v2");
        assert_ne!(endpoints.quote_ws, crate::region::QUOTE_WS_URL_GLOBAL);
        assert_ne!(endpoints.trade_ws, crate::region::TRADE_WS_URL_GLOBAL);
    }

    #[test]
    fn ws_url_derivation_maps_schemes() {
        assert_eq!(
            ws_url_from_http("http://localhost:9000"),
            "ws://localhost:9000/v2"
        );
        assert_eq!(
            ws_url_from_http("https://mock.example/"),
            "wss://mock.example/v2"
        );
        assert_eq!(
            ws_url_from_http("wss://mock.example/v2"),
            "wss://mock.example/v2"
        );
    }

    #[test]
    fn blank_override_falls_through_to_region_endpoints() {
        let endpoints = resolve_endpoints(false, Some("   ".to_string()), true);
        assert_eq!(endpoints.http, crate::region::HTTP_URL_CN);
        assert_eq!(endpoints.quote_ws, crate::region::QUOTE_WS_URL_CN);
    }

    #[test]
    fn cn_and_global_branches_keep_their_paired_constants() {
        let cn = resolve_endpoints(false, None, true);
        assert_eq!(cn.http, crate::region::HTTP_URL_CN);
        assert_eq!(cn.quote_ws, crate::region::QUOTE_WS_URL_CN);
        assert_eq!(cn.trade_ws, crate::region::TRADE_WS_URL_CN);

        let global = resolve_endpoints(false, None, false);
        assert_eq!(global.http, crate::region::HTTP_URL_GLOBAL);
        assert_eq!(global.quote_ws, crate::region::QUOTE_WS_URL_GLOBAL);
        assert_eq!(global.trade_ws, crate::region::TRADE_WS_URL_GLOBAL);
    }

    /// Security property: the override is captured once, at process start.
    /// A `.env` file loaded later by the SDK (from whatever directory the user
    /// happens to be in) must not be able to move the host — that host
    /// receives the OAuth bearer token on the agent SSE path.
    #[test]
    #[serial]
    fn captured_override_ignores_later_env_mutations() {
        let first = captured_http_url_override().map(ToString::to_string);
        std::env::set_var("LONGBRIDGE_HTTP_URL", "http://attacker.example");
        let second = captured_http_url_override().map(ToString::to_string);
        let effective = effective_http_url();
        std::env::remove_var("LONGBRIDGE_HTTP_URL");

        assert_eq!(first, second, "captured override must not change");
        assert_ne!(second.as_deref(), Some("http://attacker.example"));
        assert!(
            !effective.contains("attacker.example"),
            "late env mutation reached the SSE host: {effective}"
        );
    }

    /// One resolution, both transports. Two halves:
    ///
    /// 1. The override may only be *considered* where `init_contexts` also
    ///    applies it to the SDK configs (debug builds).
    /// 2. Even when considered, it never outranks the host `init_contexts`
    ///    published — that value is authoritative for both transports. (The
    ///    earlier version of this test asserted the opposite precedence, which
    ///    is exactly what let staging + override split the two transports.)
    #[test]
    fn override_gate_matches_the_sdk_gate() {
        if cfg!(debug_assertions) {
            assert_eq!(
                allowed_http_url_override().as_deref(),
                captured_http_url_override()
            );
        } else {
            assert!(allowed_http_url_override().is_none());
        }

        let published = "https://openapi-global.longbridge.xyz".to_string();
        assert_eq!(
            resolve_http_url(Some(published.clone()), allowed_http_url_override()),
            published,
            "the initialized host must outrank the captured override"
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
        // The flag token itself is ASCII and kept; the non-ASCII value is dropped.
        let args = ["--name", "caf\u{00e9}"].map(String::from).to_vec();
        assert_eq!(ascii_args(args), "--name");
    }

    #[test]
    fn mixed_args_keep_ascii_only() {
        let args = ["--format", "json", "--name", "na\u{00ef}ve", "--verbose"]
            .map(String::from)
            .to_vec();
        assert_eq!(ascii_args(args), "--format json --name --verbose");
    }

    #[test]
    fn all_non_ascii_yields_empty() {
        let args = ["r\u{00e9}sum\u{00e9}", "na\u{00ef}ve"]
            .map(String::from)
            .to_vec();
        assert_eq!(ascii_args(args), "");
    }

    #[test]
    fn empty_input_yields_empty() {
        assert_eq!(ascii_args(vec![]), "");
    }

    #[test]
    fn topic_body_non_ascii_excluded() {
        let args = ["--body", "\u{8fd9}\u{662f}\u{8bdd}\u{9898}\u{5185}\u{5bb9}"]
            .map(String::from)
            .to_vec();
        assert_eq!(ascii_args(args), "--body");
    }
}
