//! Auth utilities for Longbridge `OpenAPI`.

use anyhow::{Context, Result};
use longbridge::oauth::TokenStorage as _;
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

/// Token file path: `~/.longbridge/cli/auth-token`
pub fn token_file_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Failed to get home directory"))?
        .join(".longbridge")
        .join("openapi")
        .join("cli-auth"))
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
pub fn account_channel() -> Option<String> {
    let full = crate::secure_storage::EncryptedFileTokenStorage::load_full(client_id())?;
    let claims = decode_jwt_payload(full["access_token"].as_str()?)?;
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

/// Device Authorization Flow (RFC 8628).
pub async fn device_login(verbose: bool) -> Result<()> {
    use std::time::{Duration, Instant};

    let oauth_base = oauth_base_url();
    let client_id = client_id();
    let http_client = reqwest::Client::new();
    let invite_code = read_invite_code();

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

            let access_token = token_resp["access_token"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("No access_token in token response"))?;
            let expires_in = token_resp["expires_in"].as_u64().unwrap_or(3600);
            let refresh_token = token_resp["refresh_token"].as_str().map(str::to_owned);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();

            let token = longbridge::oauth::StoredToken {
                client_id: client_id.to_string(),
                access_token: access_token.to_string(),
                refresh_token,
                expires_at: now + expires_in,
            };
            crate::secure_storage::EncryptedFileTokenStorage
                .save(&token)
                .map_err(|e| anyhow::anyhow!("Failed to save token: {e}"))?;

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
/// Not expired → returns immediately. Expired → refreshes via HTTP and saves.
/// This runs before `OAuthBuilder::build()` to avoid that SDK's 5-minute
/// browser-callback timeout when it encounters an expired token.
pub async fn refresh_if_expired() -> Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let storage = crate::secure_storage::EncryptedFileTokenStorage;
    let Some(full) = crate::secure_storage::EncryptedFileTokenStorage::load_full(client_id())
    else {
        return Ok(());
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let expires_at = full["expires_at"].as_u64().unwrap_or(0);
    if expires_at == 0 {
        return Ok(());
    }
    if expires_at > now {
        return Ok(());
    }

    let Some(refresh_token) = full["refresh_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
    else {
        return Err(anyhow::anyhow!(
            "No refresh token found. Please run 'longbridge auth login' to re-authenticate."
        ));
    };

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
            ("client_id", client_id()),
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

        if token_resp["refresh_token"].is_null() || token_resp["refresh_token"].as_str().is_none() {
            token_resp["refresh_token"] = serde_json::Value::String(refresh_token);
        }

        let access_token = token_resp["access_token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No access_token in refresh response"))?;
        let expires_in = token_resp["expires_in"].as_u64().unwrap_or(3600);
        let new_refresh = token_resp["refresh_token"].as_str().map(str::to_owned);

        let token = longbridge::oauth::StoredToken {
            client_id: client_id().to_string(),
            access_token: access_token.to_string(),
            refresh_token: new_refresh,
            expires_at: now + expires_in,
        };
        storage
            .save(&token)
            .map_err(|e| anyhow::anyhow!("Failed to save refreshed token: {e}"))?;

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

    Err(anyhow::anyhow!(
        "Token refresh failed ({status}): {error} — please retry"
    ))
}

/// Authorization Code Flow: opens a browser on this machine and listens on
/// `localhost:CALLBACK_PORT` for the OAuth callback.
pub async fn auth_code_login() -> Result<()> {
    println!("Opening browser for authorization...");
    println!("Listening on localhost:{CALLBACK_PORT} for the OAuth callback.");
    let invite_code = read_invite_code();

    let oauth_result = longbridge::oauth::OAuthBuilder::new(client_id())
        .callback_port(CALLBACK_PORT)
        .token_storage(crate::secure_storage::EncryptedFileTokenStorage)
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
            println!("Successfully authenticated.");
            Ok(())
        }
        Err(e) => Err(anyhow::anyhow!("OAuth authorization failed: {e}")),
    }
}

/// Clear the stored OAuth token (logout).
pub fn clear_token() -> Result<()> {
    crate::secure_storage::try_delete(client_id());

    let path = token_file_path()?;
    if path.exists() {
        fs::remove_file(&path).context("Failed to delete token file")?;
        tracing::debug!("OAuth token deleted: {}", path.display());
    }

    Ok(())
}
