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
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/c", "start"]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = std::process::Command::new("xdg-open");

    cmd.arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
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

    // Step 1: request device & user codes.
    let url = format!("{oauth_base}/device/authorize");
    if verbose {
        eprintln!("POST {url}");
    }
    let raw = http_client
        .post(&url)
        .form(&[("client_id", client_id)])
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
    let verification_url = device_resp["verification_uri_complete"]
        .as_str()
        .unwrap_or_else(|| device_resp["verification_uri"].as_str().unwrap_or(""));
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

/// Clear the stored OAuth token (logout). Deletes the token file used by the longbridge SDK.
pub fn clear_token() -> Result<()> {
    let path = token_file_path()?;

    if path.exists() {
        fs::remove_file(&path).context("Failed to delete token file")?;
        tracing::debug!("OAuth token deleted: {}", path.display());
    }

    Ok(())
}
