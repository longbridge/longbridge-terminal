//! Auth utilities for Longbridge `OpenAPI`.
//!
//! OAuth and token refresh are handled by the longbridge SDK (`OAuthBuilder`).
//! This module also provides a headless login flow for remote environments.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// OAuth client ID for the terminal (registered with Longbridge).
pub const OAUTH_CLIENT_ID: &str = "fd52fbc5-02a9-47f5-ad30-0842c841aae9";

/// Token file path used by longbridge SDK: `~/.longbridge/openapi/tokens/<client_id>`
///
/// Keep in sync with `longbridge-oauth` crate internals (`token_path_for_client_id`).
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
/// Prints the authorization URL instead of opening a browser. The SDK starts a
/// local callback server on port 60355 so the browser redirect completes without
/// a 404. Token is persisted by the SDK in the standard location.
pub async fn headless_login() -> Result<()> {
    longbridge::oauth::OAuthBuilder::new(OAUTH_CLIENT_ID)
        .build(|url| {
            println!("Open the following URL in your browser to authorize:");
            println!();
            println!("  {url}");
            println!();
            println!("Waiting for OAuth callback on localhost:60355 ...");
        })
        .await
        .context("OAuth authorization failed")?;

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
