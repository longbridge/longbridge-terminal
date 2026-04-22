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

/// Channel key file path: `~/.longbridge/openapi/channel`
fn channel_file_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Failed to get home directory"))?
        .join(".longbridge")
        .join("openapi")
        .join("channel"))
}

/// Persist the channel key to disk.
pub fn save_channel(channel_key: &str) -> Result<()> {
    let path = channel_file_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("Failed to create config directory")?;
    }
    fs::write(&path, channel_key).context("Failed to write channel file")?;
    Ok(())
}

/// Read the stored channel key. Returns `None` if not set.
pub fn read_channel() -> Option<String> {
    let path = channel_file_path().ok()?;
    fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Try to open a URL in the system browser. Returns `true` if the command was
/// launched successfully (the browser may still fail to load the page).
///
/// Checks the `BROWSER` environment variable first; falls back to the
/// platform default (`open` on macOS, `cmd /c start` on Windows, `xdg-open`
/// on Linux).
pub fn open_browser(url: &str) -> bool {
    // Honor the BROWSER environment variable if set.
    if let Ok(browser) = std::env::var("BROWSER") {
        if !browser.is_empty() {
            return std::process::Command::new(&browser)
                .arg(url)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .is_ok();
        }
    }

    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    {
        // `cmd /c start "" "URL"` — the empty string is the window title required when
        // the target starts with a quote; without it, cmd.exe misparses the argument.
        // Quoting the URL prevents `&` in query strings from being treated as a
        // command separator, which would silently drop everything after the first `&`.
        return std::process::Command::new("cmd")
            .args(["/c", "start", "", url])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = std::process::Command::new("xdg-open");

    #[cfg(not(target_os = "windows"))]
    {
        cmd.arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok()
    }
}

/// Write a token JSON blob to the SDK token file path.
fn save_token(client_id: &str, token_resp: &serde_json::Value) -> Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let access_token = token_resp["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No access_token in token response"))?;
    let expires_in = token_resp["expires_in"].as_u64().unwrap_or(3600);
    let refresh_token = token_resp["refresh_token"].as_str();
    let expires_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + expires_in;

    let token = serde_json::json!({
        "client_id": client_id,
        "access_token": access_token,
        "refresh_token": refresh_token,
        "expires_at": expires_at,
    });

    let token_path = token_file_path()?;
    if let Some(parent) = token_path.parent() {
        fs::create_dir_all(parent).context("Failed to create token directory")?;
    }
    fs::write(&token_path, serde_json::to_string_pretty(&token).unwrap())
        .context("Failed to write token file")?;
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
    let channel = read_channel();

    // Step 1: request device & user codes.
    let url = format!("{oauth_base}/device/authorize");
    if verbose {
        eprintln!("POST {url}");
    }
    let mut device_auth_form: Vec<(&str, &str)> = vec![("client_id", client_id)];
    if let Some(ref ch) = channel {
        device_auth_form.push(("channel", ch.as_str()));
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
    let verification_url = if let Some(ref ch) = channel {
        let sep = if verification_url_base.contains('?') {
            '&'
        } else {
            '?'
        };
        verification_url_owned = format!(
            "{verification_url_base}{sep}channel={}",
            percent_encoding::utf8_percent_encode(ch, percent_encoding::NON_ALPHANUMERIC)
        );
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
/// - Expired, server says invalid/expired refresh token → clears token file,
///   returns an error directing the user to re-authenticate.
/// - Expired, network/transient error → returns an error asking the user to
///   retry (token file is **not** cleared; the refresh token is still valid).
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
    if data["expires_at"].as_u64().unwrap_or(0) > now {
        return Ok(()); // still valid
    }

    let Some(refresh_token) = data["refresh_token"].as_str().filter(|s| !s.is_empty()) else {
        let _ = clear_token();
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
        let _ = clear_token();
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

    let oauth_result = longbridge::oauth::OAuthBuilder::new(client_id())
        .callback_port(CALLBACK_PORT)
        .build(|url| {
            println!();
            println!("Authorization URL: {url}");
            println!();
            if !open_browser(url) {
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

/// Clear the stored OAuth token (logout). Deletes the token file used by the longbridge SDK.
pub fn clear_token() -> Result<()> {
    let path = token_file_path()?;

    if path.exists() {
        fs::remove_file(&path).context("Failed to delete token file")?;
        tracing::debug!("OAuth token deleted: {}", path.display());
    }

    Ok(())
}
