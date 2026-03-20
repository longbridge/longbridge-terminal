//! Auth utilities for Longbridge `OpenAPI`.
//!
//! OAuth and token refresh are handled by the longbridge SDK (`OAuthBuilder`).
//! This module also provides a headless login flow for remote environments.

use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

/// OAuth client ID for the terminal (registered with Longbridge).
pub const OAUTH_CLIENT_ID: &str = "fd52fbc5-02a9-47f5-ad30-0842c841aae9";

const OAUTH_BASE_URL: &str = "https://openapi.longbridgeapp.com/oauth2";
const CALLBACK_REDIRECT_URI: &str = "http://localhost:60355/callback";

/// Token file path used by longbridge SDK: `~/.longbridge-openapi/tokens/<client_id>`
fn token_file_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Failed to get home directory"))?
        .join(".longbridge-openapi")
        .join("tokens")
        .join(OAUTH_CLIENT_ID))
}

/// Generate a random hex state string for CSRF protection.
fn random_state() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    let _ = fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Headless OAuth login for remote environments (SSH, cloud agents, etc.).
///
/// Prints the authorization URL. The user opens it in a local browser, completes
/// the flow, then pastes the redirect URL (which fails to load on localhost) back
/// into the terminal. The auth code is extracted, exchanged for a token, and saved
/// in the same location the SDK reads from, so subsequent commands work normally.
pub async fn headless_login() -> Result<()> {
    let state = random_state();
    let redirect_uri = urlencoding::encode(CALLBACK_REDIRECT_URI);
    let auth_url = format!(
        "{OAUTH_BASE_URL}/authorize?client_id={OAUTH_CLIENT_ID}&redirect_uri={redirect_uri}&response_type=code&state={state}"
    );

    println!("Open the following URL in your browser to authorize:");
    println!();
    println!("  {auth_url}");
    println!();
    println!("After authorizing, the browser will redirect to a localhost URL that will");
    println!("fail to load. Copy the full URL from the address bar and paste it here:");
    print!("> ");
    std::io::stdout().flush().context("Failed to flush stdout")?;

    let mut pasted = String::new();
    std::io::stdin()
        .read_line(&mut pasted)
        .context("Failed to read callback URL")?;
    let pasted = pasted.trim();

    let parsed = url::Url::parse(pasted).context("Invalid URL — paste the full redirect URL")?;
    let params: std::collections::HashMap<_, _> = parsed.query_pairs().collect();

    let code = params
        .get("code")
        .ok_or_else(|| anyhow::anyhow!("No 'code' parameter found in the URL"))?
        .to_string();
    let returned_state = params
        .get("state")
        .ok_or_else(|| anyhow::anyhow!("No 'state' parameter found in the URL"))?
        .to_string();

    anyhow::ensure!(
        returned_state == state,
        "CSRF state mismatch — the URL may have been tampered with"
    );

    let token = exchange_code(&code).await?;
    save_token_file(&token)?;

    println!("Successfully authenticated.");
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct TokenFile {
    client_id: String,
    access_token: String,
    refresh_token: Option<String>,
    expires_at: u64,
}

async fn exchange_code(code: &str) -> Result<TokenFile> {
    #[derive(serde::Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<u64>,
    }

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{OAUTH_BASE_URL}/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", CALLBACK_REDIRECT_URI),
            ("client_id", OAUTH_CLIENT_ID),
        ])
        .send()
        .await
        .context("Token exchange request failed")?
        .error_for_status()
        .context("Token endpoint returned an error")?
        .json::<TokenResponse>()
        .await
        .context("Failed to parse token response")?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    Ok(TokenFile {
        client_id: OAUTH_CLIENT_ID.to_string(),
        access_token: resp.access_token,
        refresh_token: resp.refresh_token,
        expires_at: now + resp.expires_in.unwrap_or(3600),
    })
}

fn save_token_file(token: &TokenFile) -> Result<()> {
    let path = token_file_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("Failed to create token directory")?;
    }
    let json = serde_json::to_string_pretty(token).context("Failed to serialize token")?;
    fs::write(&path, json).context("Failed to write token file")?;
    tracing::debug!("Token saved to {}", path.display());
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
