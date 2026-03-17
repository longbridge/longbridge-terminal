//! Auth utilities for Longbridge `OpenAPI`.
//!
//! OAuth and token refresh are handled by the longbridge SDK (`OAuthBuilder`).
//! This module only provides clearing the stored token (logout).

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// OAuth client ID for the terminal (registered with Longbridge).
pub const OAUTH_CLIENT_ID: &str = "fd52fbc5-02a9-47f5-ad30-0842c841aae9";

/// Token file path used by longbridge SDK: `~/.longbridge-openapi/tokens/<client_id>`
fn token_file_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Failed to get home directory"))?
        .join(".longbridge-openapi")
        .join("tokens")
        .join(OAUTH_CLIENT_ID))
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
