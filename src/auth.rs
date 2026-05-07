//! Auth utilities for Longbridge `OpenAPI`.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

const OAUTH_CLIENT_ID: &str = "fd52fbc5-02a9-47f5-ad30-0842c841aae9";
const OAUTH_PATH: &str = "/oauth2";

const OAUTH_TEST_CLIENT_ID: &str = "37435cdf-c7e4-4de9-8715-b20d33416196";

pub const CALLBACK_PORT: u16 = 60355;

/// Whether the staging environment is active (`LONGBRIDGE_ENV=staging`).
pub fn is_test_env() -> bool {
    std::env::var("LONGBRIDGE_ENV").is_ok_and(|v| v == "staging")
}

/// Return the OAuth client ID for the current environment.
pub fn client_id() -> &'static str {
    if is_test_env() {
        OAUTH_TEST_CLIENT_ID
    } else {
        OAUTH_CLIENT_ID
    }
}

/// Return the OAuth base URL for the current environment and region.
fn oauth_base_url() -> String {
    let host = if is_test_env() {
        crate::region::HTTP_URL_TEST
    } else if crate::region::is_cn_cached() {
        crate::region::HTTP_URL_CN
    } else {
        crate::region::HTTP_URL_GLOBAL
    };
    format!("{host}{OAUTH_PATH}")
}

/// Token file path: `~/.longbridge/openapi/tokens/<client_id>`
///
/// Must stay in sync with `longbridge-oauth` crate internals (`token_path_for_client_id`).
pub fn token_file_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Failed to get home directory"))?
        .join(".longbridge")
        .join("openapi")
        .join("tokens")
        .join(client_id()))
}

/// Invite code file path: `~/.longbridge/openapi/invite-code`
fn invite_code_file_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Failed to get home directory"))?
        .join(".longbridge")
        .join("openapi")
        .join("invite-code"))
}

/// Default `account_channel` used when no token is present or the JWT
/// cannot be decoded. Matches the historical hardcoded value across the CLI/TUI.
pub const DEFAULT_ACCOUNT_CHANNEL: &str = "lb";

/// Decode a JWT payload (no signature verification) as a JSON value.
fn decode_jwt_payload(token: &str) -> Option<serde_json::Value> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

/// Read the logged-in user's `account_channel` from the local access token.
///
/// Longbridge tokens carry `sub` as a JSON-encoded string with fields like
/// `client_id`, `member_id`, and `account_channel` (`"lb"`,
/// `"lb_papertrading"`, etc.). Several APIs require this to match the
/// token's own channel — e.g. `/v1/quote/my-quotes` returns 401004 when
/// the request `account_channel` does not match.
///
/// Returns `None` if the token file is missing/unparseable or the JWT
/// lacks the field. Use [`account_channel_or_default`] to get a usable
/// string with a `"lb"` fallback.
pub fn account_channel() -> Option<String> {
    let path = token_file_path().ok()?;
    let contents = fs::read_to_string(path).ok()?;
    let data: serde_json::Value = serde_json::from_str(&contents).ok()?;
    let token = data["access_token"].as_str()?;
    let claims = decode_jwt_payload(token)?;
    let sub_str = claims["sub"].as_str()?;
    let sub: serde_json::Value = serde_json::from_str(sub_str).ok()?;
    sub["account_channel"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// [`account_channel`] with a fallback to [`DEFAULT_ACCOUNT_CHANNEL`].
pub fn account_channel_or_default() -> String {
    account_channel().unwrap_or_else(|| DEFAULT_ACCOUNT_CHANNEL.to_owned())
}

/// Persist the invite code to disk.
pub fn save_invite_code(invite_code: &str) -> Result<()> {
    let path = invite_code_file_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("Failed to create config directory")?;
    }
    fs::write(&path, invite_code).context("Failed to write invite code file")?;
    Ok(())
}

fn read_non_empty_file(path: PathBuf) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Read the stored invite code. Returns `None` if not set.
pub fn read_invite_code() -> Option<String> {
    invite_code_file_path().ok().and_then(read_non_empty_file)
}

fn append_query_param(url: &str, key: &str, value: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!(
        "{url}{sep}{key}={}",
        percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC)
    )
}

/// Try to open a URL in the system browser. Returns `true` if launched successfully.
pub fn open_browser(url: &str) -> bool {
    open::that(url).is_ok()
}

/// Write a token JSON blob to the SDK token file path.
///
/// Preserves the existing `logged_in_at` field on refresh; sets it to now on initial login.
fn save_token(client_id: &str, token_resp: &serde_json::Value) -> Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let access_token = token_resp["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No access_token in token response"))?;
    let expires_in = token_resp["expires_in"].as_u64().unwrap_or(3600);
    let refresh_token = token_resp["refresh_token"].as_str();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let expires_at = now + expires_in;

    let token_path = token_file_path()?;
    let logged_in_at = fs::read_to_string(&token_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["logged_in_at"].as_u64())
        .unwrap_or(now);

    let token = serde_json::json!({
        "client_id": client_id,
        "access_token": access_token,
        "refresh_token": refresh_token,
        "expires_at": expires_at,
        "logged_in_at": logged_in_at,
    });

    if let Some(parent) = token_path.parent() {
        fs::create_dir_all(parent).context("Failed to create token directory")?;
    }
    fs::write(&token_path, serde_json::to_string_pretty(&token).unwrap())
        .context("Failed to write token file")?;
    Ok(())
}

/// Patch the token file written by the SDK's `OAuthBuilder` to add `logged_in_at` if missing.
fn patch_token_logged_in_at() -> Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let token_path = token_file_path()?;
    let contents = fs::read_to_string(&token_path)?;
    let mut data: serde_json::Value = serde_json::from_str(&contents)?;

    if data["logged_in_at"].is_null() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        data["logged_in_at"] = serde_json::Value::from(now);
        fs::write(&token_path, serde_json::to_string_pretty(&data).unwrap())
            .context("Failed to patch token file")?;
    }
    Ok(())
}

/// Device Authorization Flow (RFC 8628).
///
/// Displays a URL for the user to open in any browser (no localhost redirect needed).
/// Polls for the token until the user completes authorization or the code expires.
/// Works on any machine including SSH sessions, cloud agents, and headless servers.
pub async fn device_login(verbose: bool) -> Result<()> {
    use std::time::{Duration, Instant};

    let oauth_base = oauth_base_url();
    let client_id = client_id();
    let http_client = reqwest::Client::new();
    let invite_code = read_invite_code();

    // Step 1: request device & user codes.
    let url = format!("{oauth_base}/device/authorize");
    if verbose {
        eprintln!("POST {url}");
    }
    let mut device_auth_form: Vec<(&str, &str)> = vec![("client_id", client_id)];
    if let Some(ref invite_code) = invite_code {
        device_auth_form.push(("invite-code", invite_code.as_str()));
    }
    let raw = http_client
        .post(&url)
        .form(&device_auth_form)
        .send()
        .await
        .context("Device authorization request failed")?;

    let status = raw.status();
    if !status.is_success() {
        let body = raw.text().await.unwrap_or_default();
        anyhow::bail!("Device authorization failed ({status}): {body}");
    }

    let device_resp = raw
        .json::<serde_json::Value>()
        .await
        .context("Failed to parse device authorization response")?;

    let device_code = device_resp["device_code"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No device_code in response"))?
        .to_owned();
    let verification_url_base = device_resp["verification_uri_complete"]
        .as_str()
        .unwrap_or_else(|| device_resp["verification_uri"].as_str().unwrap_or(""));
    let verification_url_owned;
    let verification_url = if let Some(ref invite_code) = invite_code {
        verification_url_owned =
            append_query_param(verification_url_base, "invite-code", invite_code);
        verification_url_owned.as_str()
    } else {
        verification_url_base
    };
    let expires_in = device_resp["expires_in"].as_u64().unwrap_or(300);
    let interval = device_resp["interval"].as_u64().unwrap_or(5);

    // Try to open the browser automatically; silently fall back to manual if unavailable.
    let opened = open_browser(verification_url);

    println!("Open the following URL in your browser to authorize:");
    println!();
    println!("{verification_url}");
    println!();
    if opened {
        println!("Browser opened. Waiting for authorization...");
    } else {
        println!("Waiting for authorization...");
    }

    // Step 2: poll until authorized or expired.
    let deadline = Instant::now() + Duration::from_secs(expires_in);
    let poll_interval = Duration::from_secs(interval);

    loop {
        tokio::time::sleep(poll_interval).await;

        if Instant::now() >= deadline {
            anyhow::bail!("Device authorization timed out — please try again.");
        }

        let url = format!("{oauth_base}/token");
        if verbose {
            eprintln!("POST {url}  grant_type=device_code");
        }
        let raw = http_client
            .post(&url)
            .form(&[
                ("client_id", client_id),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code.as_str()),
            ])
            .send()
            .await
            .context("Token poll request failed")?;

        let status = raw.status();
        if status.is_success() {
            let token_resp = raw
                .json::<serde_json::Value>()
                .await
                .context("Failed to parse token response")?;
            save_token(client_id, &token_resp)?;
            println!("Successfully authenticated.");
            return Ok(());
        }

        let err_resp = raw.json::<serde_json::Value>().await.unwrap_or_default();
        match err_resp["error"].as_str() {
            Some("authorization_pending" | "slow_down") => {}
            Some(other) => anyhow::bail!("Authorization failed: {other}"),
            None => anyhow::bail!("Unexpected token poll response"),
        }
    }
}

/// Refresh the access token in-place if it has expired.
///
/// - Not expired → returns `Ok(())` immediately (no network call).
/// - Expired, refresh succeeds → writes new token to disk, returns `Ok(())`.
/// - Expired, refresh fails → returns an error; token file is **never** deleted
///   so `auth status` shows "expired" rather than "not found".
pub async fn refresh_if_expired() -> Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let token_path = token_file_path()?;
    let Ok(contents) = fs::read_to_string(&token_path) else {
        return Ok(()); // unreadable — let OAuthBuilder handle it
    };
    let Ok(data) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return Ok(()); // unparseable — let OAuthBuilder handle it
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let expires_at = data["expires_at"].as_u64().unwrap_or(0);
    if expires_at == 0 {
        return Ok(()); // no expiry info — let OAuthBuilder handle it
    }
    if expires_at > now {
        return Ok(()); // still valid
    }

    let Some(refresh_token) = data["refresh_token"].as_str().filter(|s| !s.is_empty()) else {
        return Err(anyhow::anyhow!(
            "No refresh token found. Please run 'longbridge auth login' to re-authenticate."
        ));
    };
    let refresh_token = refresh_token.to_string();

    let client_id = client_id();
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .context("Failed to build HTTP client for token refresh")?;

    let url = format!("{}/token", oauth_base_url());
    tracing::debug!("Refreshing expired access token via {url}");

    let resp = http_client
        .post(&url)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("client_id", client_id),
        ])
        .send()
        .await
        .context("Token refresh request failed — please retry")?;

    let status = resp.status();
    if status.is_success() {
        let mut token_resp = resp
            .json::<serde_json::Value>()
            .await
            .context("Failed to parse token refresh response")?;

        // Preserve the existing refresh token if the server did not rotate it.
        if token_resp["refresh_token"].is_null() || token_resp["refresh_token"].as_str().is_none() {
            token_resp["refresh_token"] = serde_json::Value::String(refresh_token);
        }

        save_token(client_id, &token_resp)?;
        tracing::debug!("Access token refreshed successfully");
        return Ok(());
    }

    let err_resp = resp.json::<serde_json::Value>().await.unwrap_or_default();
    let error = err_resp["error"].as_str().unwrap_or("unknown");

    if error == "invalid_grant" {
        return Err(anyhow::anyhow!(
            "Refresh token has expired. Please run 'longbridge auth login' to re-authenticate."
        ));
    }

    // Other server errors (5xx etc.) — keep token intact, let the user retry.
    Err(anyhow::anyhow!(
        "Token refresh failed ({status}): {error} — please retry"
    ))
}

/// Authorization Code Flow: opens a browser on this machine and listens on
/// `localhost:CALLBACK_PORT` for the OAuth callback.
///
/// Unlike `device_login`, this requires the browser to be on the same machine.
/// The SDK handles the local callback server and token persistence.
pub async fn auth_code_login() -> Result<()> {
    println!("Opening browser for authorization...");
    println!("Listening on localhost:{CALLBACK_PORT} for the OAuth callback.");
    let invite_code = read_invite_code();

    let oauth_result = longbridge::oauth::OAuthBuilder::new(client_id())
        .callback_port(CALLBACK_PORT)
        .build(|url| {
            let authorization_url = invite_code.as_deref().map_or_else(
                || url.to_string(),
                |invite_code| append_query_param(url, "invite-code", invite_code),
            );
            println!();
            println!("Authorization URL: {authorization_url}");
            println!();
            if !open_browser(&authorization_url) {
                println!("Could not open browser automatically. Please visit the URL above.");
            }
        })
        .await;

    match oauth_result {
        Ok(_) => {
            let _ = patch_token_logged_in_at();
            println!("Successfully authenticated.");
            Ok(())
        }
        Err(e) => Err(anyhow::anyhow!("OAuth authorization failed: {e}")),
    }
}

/// Clear the stored OAuth token (logout). Deletes the token file used by the longbridge SDK.
pub fn clear_token() -> Result<()> {
    let path = token_file_path()?;

    if path.exists() {
        fs::remove_file(&path).context("Failed to delete token file")?;
        tracing::debug!("OAuth token deleted: {}", path.display());
    }

    Ok(())
}
