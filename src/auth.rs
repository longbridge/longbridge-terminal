//! Auth utilities for Longbridge `OpenAPI`.
//!
//! Standard login uses the longbridge SDK's `OAuthBuilder` (browser flow with local callback server).
//! Headless login is a manual OAuth 2.0 authorization code flow for remote environments where
//! the browser cannot redirect to localhost — the user copies the redirect URL from their browser
//! and pastes it into the terminal.

use anyhow::{Context, Result};
use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
use std::fs;
use std::path::PathBuf;

/// OAuth client ID for the terminal (registered with Longbridge).
pub const OAUTH_CLIENT_ID: &str = "fd52fbc5-02a9-47f5-ad30-0842c841aae9";

const OAUTH_BASE_URL: &str = "https://openapi.longbridge.com/oauth2";
const CALLBACK_PORT: u16 = 60355;

/// Token file path: `~/.longbridge/openapi/tokens/<client_id>`
///
/// Must stay in sync with `longbridge-oauth` crate internals (`token_path_for_client_id`).
fn token_file_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Failed to get home directory"))?
        .join(".longbridge")
        .join("openapi")
        .join("tokens")
        .join(OAUTH_CLIENT_ID))
}

/// Headless OAuth login for remote environments (SSH, cloud agents, etc.).
///
/// Prints the authorization URL. The user opens it in a local browser, completes
/// authorization, then pastes the redirect URL (from the browser address bar) back
/// into the terminal. The authorization code is extracted and exchanged for a token.
pub async fn headless_login() -> Result<()> {
    use std::io::{BufRead, Write};
    use std::time::{SystemTime, UNIX_EPOCH};

    let redirect_uri = format!("http://localhost:{CALLBACK_PORT}/callback");

    // Pseudo-random state for CSRF protection (time + pid, sufficient for CLI use).
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let state = format!("{:x}{:x}", nanos, std::process::id());

    // Percent-encode the redirect URI for inclusion in the query string.
    let redirect_uri_enc = utf8_percent_encode(&redirect_uri, NON_ALPHANUMERIC).to_string();
    let auth_url = format!(
        "{OAUTH_BASE_URL}/authorize?client_id={OAUTH_CLIENT_ID}&redirect_uri={redirect_uri_enc}&response_type=code&state={state}"
    );

    println!("Open the following URL in your browser to authorize:");
    println!();
    println!("  {auth_url}");
    println!();
    println!("After authorizing, the browser will try to redirect to localhost.");
    println!("The page will likely fail to load — that is expected.");
    println!("Copy the full URL from your browser's address bar and paste it here:");
    print!("> ");
    std::io::stdout().flush()?;

    let mut pasted = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut pasted)
        .context("Failed to read redirect URL")?;
    let pasted = pasted.trim();

    let (code, returned_state) = parse_callback_url(pasted)?;
    if returned_state != state {
        anyhow::bail!("State mismatch — possible CSRF attack or wrong URL pasted.");
    }

    // Exchange authorization code for access token.
    let raw = reqwest::Client::new()
        .post(format!("{OAUTH_BASE_URL}/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", OAUTH_CLIENT_ID),
        ])
        .send()
        .await
        .context("Token exchange request failed")?;

    if !raw.status().is_success() {
        let status = raw.status();
        let body = raw.text().await.unwrap_or_default();
        anyhow::bail!("Token exchange failed ({status}): {body}");
    }

    let resp = raw
        .json::<serde_json::Value>()
        .await
        .context("Failed to parse token response")?;

    let access_token = resp["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No access_token in token response"))?;
    let expires_in = resp["expires_in"].as_u64().unwrap_or(3600);
    let refresh_token = resp["refresh_token"].as_str();
    let expires_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + expires_in;

    // Write token in the format expected by the SDK.
    let token = serde_json::json!({
        "client_id": OAUTH_CLIENT_ID,
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

    println!("Successfully authenticated.");
    Ok(())
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

/// Extract `code` and `state` query parameters from an OAuth redirect URL.
fn parse_callback_url(url: &str) -> Result<(String, String)> {
    let query = url
        .split('?')
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("Invalid URL: no query string found"))?;

    let code = find_query_param(query, "code")
        .ok_or_else(|| anyhow::anyhow!("No 'code' parameter in URL"))?;
    let state = find_query_param(query, "state")
        .ok_or_else(|| anyhow::anyhow!("No 'state' parameter in URL"))?;

    Ok((code, state))
}

fn find_query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        if k == key {
            Some(percent_decode_str(v).decode_utf8_lossy().into_owned())
        } else {
            None
        }
    })
}
