//! Auth utilities for Longbridge `OpenAPI`.

use anyhow::{Context, Result};
use longbridge::oauth::TokenStorage as _;
use std::fs;
use std::path::PathBuf;

pub const CALLBACK_PORT: u16 = 60355;

/// OAuth client registration info persisted after dynamic client registration (RFC 7591).
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
struct ClientRegistration {
    client_id: String,
    registration_access_token: String,
    registration_client_uri: String,
}

fn registration_file_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Failed to get home directory"))?
        .join(".longbridge")
        .join("openapi")
        .join("cli-registration"))
}

fn load_registration() -> Option<ClientRegistration> {
    let path = registration_file_path().ok()?;
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_registration(reg: &ClientRegistration) -> Result<()> {
    let path = registration_file_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("Failed to create config directory")?;
    }
    let json = serde_json::to_string_pretty(reg).context("Failed to serialize registration")?;
    fs::write(&path, json).context("Failed to write registration file")?;
    crate::secure_storage::harden_file_permissions(&path);
    Ok(())
}

/// Build the OAuth client name used for dynamic registration, identifying the device
/// in the user's authorized-apps list.
///
/// Format is `<user>@<machine> (Longbridge CLI)`, e.g. `jason@huacnlee-macbook
/// (Longbridge CLI)`. The login user name comes first, followed by the host name (which
/// usually encodes the device type). When the host name is unavailable it falls back to
/// the OS label so a generic server login still gets a device hint, e.g. `ubuntu@Linux
/// (Longbridge CLI)`. When the user name is unavailable the machine part is used alone.
fn client_name() -> String {
    let os = match std::env::consts::OS {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        "ios" => "iOS",
        "android" => "Android",
        "freebsd" => "FreeBSD",
        other => other,
    };

    let machine = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .and_then(|h| h.split('.').next().map(str::to_owned))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| os.to_owned());

    match whoami::fallible::username().ok().filter(|s| !s.is_empty()) {
        Some(user) => format!("{user}@{machine} (Longbridge CLI)"),
        None => format!("{machine} (Longbridge CLI)"),
    }
}

/// Build a reqwest HTTP client with the Longbridge terminal User-Agent.
fn build_http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("longbridge-terminal/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("Failed to build HTTP client")
}

/// Register a new OAuth client via RFC 7591 dynamic client registration.
///
/// `name_override` lets the caller declare an explicit client name (e.g. via
/// `--client-name`); when `None` the auto-derived [`client_name`] is used.
async fn register_new_client(
    http_client: &reqwest::Client,
    verbose: bool,
    name_override: Option<String>,
) -> Result<ClientRegistration> {
    let url = format!("{}/register", oauth_base_url());
    if verbose {
        eprintln!("POST {url}  (dynamic client registration)");
    }

    let body = serde_json::json!({
        "client_name": name_override.unwrap_or_else(client_name),
        "redirect_uris": [format!("http://localhost:{CALLBACK_PORT}/callback")],
    });

    let resp = http_client
        .post(&url)
        .json(&body)
        .send()
        .await
        .context("Client registration request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Client registration failed ({status}): {text}");
    }

    let data: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse registration response")?;

    Ok(ClientRegistration {
        client_id: data["client_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No client_id in registration response"))?
            .to_owned(),
        registration_access_token: data["registration_access_token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No registration_access_token in response"))?
            .to_owned(),
        registration_client_uri: data["registration_client_uri"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("No registration_client_uri in response"))?
            .to_owned(),
    })
}

/// Return the locally persisted client registration if present, otherwise register a
/// new one (RFC 7591). Reusing the stored registration across logins avoids creating a
/// duplicate `client_id` on the server every time the CLI authenticates.
async fn get_or_register_client(
    http_client: &reqwest::Client,
    verbose: bool,
    name_override: Option<String>,
) -> Result<ClientRegistration> {
    if let Some(reg) = load_registration() {
        if verbose {
            eprintln!("Reusing existing client registration ({})", reg.client_id);
        }
        return Ok(reg);
    }
    // Persist immediately, before the (interactive, possibly-abandoned) login
    // proceeds. If we waited until login succeeded, an aborted login — timeout,
    // Ctrl+C, or an unauthorized browser — would leave a server-side client_id
    // with no local record, and the next attempt would register a duplicate.
    // Saving here guarantees one machine keeps exactly one client_id.
    let reg = register_new_client(http_client, verbose, name_override).await?;
    save_registration(&reg)?;
    Ok(reg)
}

/// Revoke the stored client registration via RFC 7592 DELETE, then remove the local file.
/// Silently succeeds if no registration is stored or the server returns an error.
async fn revoke_client_registration(http_client: &reqwest::Client) {
    let Some(reg) = load_registration() else {
        return;
    };

    if let Ok(resp) = http_client
        .delete(&reg.registration_client_uri)
        .bearer_auth(&reg.registration_access_token)
        .send()
        .await
    {
        tracing::debug!("Client registration revocation: HTTP {}", resp.status());
    }

    if let Ok(path) = registration_file_path() {
        let _ = fs::remove_file(path);
    }
}

/// Return the OAuth base URL for the current environment and region.
fn oauth_base_url() -> String {
    format!("{}/oauth2", crate::region::http_url())
}

/// `/connect` reverse-authorization page URL for the current region.
fn connect_url() -> String {
    format!("{}/connect", crate::region::open_url())
}

/// Redirect URI registered for the "AI Agent" client. Must exactly match the
/// `redirect_uri` bound to the authorization code generated at [`connect_url`].
fn agent_redirect_uri() -> String {
    format!("{}/connect/done", crate::region::open_url())
}

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
    let full = crate::secure_storage::EncryptedFileTokenStorage::load_full()?;
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

/// Refresh the access token in-place if it has expired.
///
/// Not expired → returns immediately. Expired → refreshes via HTTP and saves.
/// This runs before `OAuthBuilder::build()` to avoid that SDK's 5-minute
/// browser-callback timeout when it encounters an expired token.
pub async fn refresh_if_expired() -> Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let storage = crate::secure_storage::EncryptedFileTokenStorage;
    let Some(full) = crate::secure_storage::EncryptedFileTokenStorage::load_full() else {
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
        .user_agent(concat!("longbridge-terminal/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .context("Failed to build HTTP client for token refresh")?;

    let url = format!("{}/token", oauth_base_url());
    tracing::debug!("Refreshing expired access token via {url}");

    // Refresh under the same client_id the token was issued with (stored in the
    // token file): a dynamically-registered id for the browser flow, the agent
    // client for the paste-code flow.
    let client_id = full["client_id"]
        .as_str()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Stored token has no client_id. Please run 'longbridge auth login' to \
                 re-authenticate."
            )
        })?
        .to_owned();
    let resp = http_client
        .post(&url)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("client_id", client_id.as_str()),
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
            client_id: client_id.clone(),
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
pub async fn auth_code_login(verbose: bool, client_name: Option<String>) -> Result<()> {
    println!("Opening browser for authorization...");
    println!("Listening on localhost:{CALLBACK_PORT} for the OAuth callback.");
    let invite_code = read_invite_code();

    let http_client = build_http_client()?;
    let reg = get_or_register_client(&http_client, verbose, client_name).await?;

    let oauth_result = longbridge::oauth::OAuthBuilder::new(&reg.client_id)
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
            // Registration was already persisted in get_or_register_client.
            println!("Successfully authenticated.");
            Ok(())
        }
        Err(e) => Err(anyhow::anyhow!("OAuth authorization failed: {e}")),
    }
}

/// Build the ordered exchange candidates for a user-pasted authorization
/// code. The connect page displays codes in a pure-alphanumeric base58 form
/// (raw standard-base64 codes get escaped or truncated by chat apps); legacy
/// pages used an unpadded base64url form. Candidate order: base58-decoded,
/// base64url-restored, the string as pasted. A rejected lookup does not
/// consume the one-time code, so later attempts are safe.
///
/// Mirrors the hosted MCP server's `authenticate` tool — keep the two in
/// sync (both repos pin the same shared test vector).
fn auth_code_candidates(input: &str) -> Vec<String> {
    use base64::Engine as _;
    let raw = input
        .trim()
        .trim_matches(|c| matches!(c, '"' | '\'' | '`'))
        .to_string();
    if raw.is_empty() {
        return Vec::new();
    }
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(bytes) = bs58::decode(&raw).into_vec() {
        candidates.push(base64::engine::general_purpose::STANDARD.encode(bytes));
    }
    if let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&raw) {
        let restored = base64::engine::general_purpose::STANDARD.encode(bytes);
        if !candidates.contains(&restored) {
            candidates.push(restored);
        }
    }
    if !candidates.contains(&raw) {
        candidates.push(raw);
    }
    candidates
}

/// Agent Auth Code reverse-authorization flow.
///
/// Exchanges a standard OAuth authorization code — generated by the user at
/// <https://open.longbridge.com/connect> (5-minute, single-use) — for an
/// access/refresh token in a single synchronous call. No browser, no polling,
/// no local callback server.
///
/// The pasted code is restored from its chat-safe display form via
/// [`auth_code_candidates`] (base58 / legacy base64url / raw, in order); a
/// rejected lookup does not consume the one-time code, so trying multiple
/// candidates is safe.
///
/// The resulting tokens are written to the same encrypted token cache used by
/// every other login path, so subsequent refresh logic is fully shared.
pub async fn auth_code_exchange_login(code: &str) -> Result<()> {
    let candidates = auth_code_candidates(code);
    if candidates.is_empty() {
        anyhow::bail!(
            "Empty authorization code. Generate one at {} and pass it as \
             `longbridge auth login --auth-code <CODE>`.",
            connect_url()
        );
    }

    let url = format!("{}/token", oauth_base_url());
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .context("Failed to build HTTP client for authorization code exchange")?;

    // Public client: token endpoint auth method `none` (no client_secret),
    // no PKCE (no code_verifier).
    let redirect_uri = agent_redirect_uri();
    let mut last_rejection: Option<(reqwest::StatusCode, serde_json::Value)> = None;
    for candidate in &candidates {
        tracing::debug!("Exchanging authorization code via {url}");
        let send_result = http_client
            .post(&url)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", candidate.as_str()),
                ("redirect_uri", redirect_uri.as_str()),
                ("client_id", crate::region::agent_client_id()),
            ])
            .send()
            .await;
        let resp = match send_result {
            Ok(resp) => resp,
            // Keep the first definitive rejection instead of failing on a
            // transport error from the fallback attempt.
            Err(_) if last_rejection.is_some() => break,
            Err(e) => {
                return Err(anyhow::Error::new(e).context(
                    "Authorization request failed — check your network connection and try again.",
                ));
            }
        };

        let status = resp.status();
        if !status.is_success() {
            let err_resp = resp.json::<serde_json::Value>().await.unwrap_or_default();
            last_rejection = Some((status, err_resp));
            continue;
        }

        let token_resp = resp
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

        // Store under the agent client_id used to exchange the code. The refresh
        // path posts the token's stored client_id, so this keeps refresh working.
        let token = longbridge::oauth::StoredToken {
            client_id: crate::region::agent_client_id().to_string(),
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

    // Surface a self-healing hint: an invalid/expired/used code means the agent
    // should ask the user to regenerate one.
    let (status, err_resp) = last_rejection.unwrap_or_default();
    let error = err_resp["error"].as_str().unwrap_or("unknown");
    let description = err_resp["error_description"].as_str().unwrap_or("");

    match error {
        "invalid_grant" | "invalid_request" | "expired_token" | "access_denied" => {
            anyhow::bail!(
                "Authorization code is invalid, expired, or already used. \
                 Please generate a new one at {} \
                 and run `longbridge auth login --auth-code <CODE>` again.",
                connect_url()
            )
        }
        _ => {
            let detail = if description.is_empty() {
                error.to_string()
            } else {
                format!("{error}: {description}")
            };
            anyhow::bail!("Authorization code exchange failed ({status}): {detail}")
        }
    }
}

/// Clear the stored OAuth token (logout).
///
/// Revokes the dynamic client registration on the server (RFC 7592) before
/// deleting the local token file, so the token cannot be reused on other machines.
pub async fn clear_token() -> Result<()> {
    if let Ok(http_client) = build_http_client() {
        revoke_client_registration(&http_client).await;
    }

    let path = token_file_path()?;
    if path.exists() {
        fs::remove_file(&path).context("Failed to delete token file")?;
        tracing::debug!("OAuth token deleted: {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod auth_code_tests {
    use super::*;

    // Shared vector with longbridge-mcp: the connect page displays
    // `RKBXES26iL0CdQL85vXz+tSNeoHeqyUuLEz3nVWgqVU=` as
    // `5ctXgj3mEtEHoUBRwJnyURf4EkfA1924fY3o9Njev2zp` (base58).
    const ORIGINAL: &str = "RKBXES26iL0CdQL85vXz+tSNeoHeqyUuLEz3nVWgqVU=";
    const BASE58_FORM: &str = "5ctXgj3mEtEHoUBRwJnyURf4EkfA1924fY3o9Njev2zp";
    const BASE64URL_FORM: &str = "RKBXES26iL0CdQL85vXz-tSNeoHeqyUuLEz3nVWgqVU";

    #[test]
    fn base58_display_form_decodes_to_original_code() {
        assert_eq!(auth_code_candidates(BASE58_FORM)[0], ORIGINAL);
    }

    #[test]
    fn legacy_base64url_form_is_restored() {
        assert!(auth_code_candidates(BASE64URL_FORM).contains(&ORIGINAL.to_string()));
    }

    #[test]
    fn original_base64_passes_through() {
        assert!(auth_code_candidates(ORIGINAL).contains(&ORIGINAL.to_string()));
    }

    #[test]
    fn chat_copy_junk_is_stripped() {
        assert_eq!(
            auth_code_candidates(&format!("  `{BASE58_FORM}`  "))[0],
            ORIGINAL
        );
    }

    #[test]
    fn empty_input_yields_no_candidates() {
        assert!(auth_code_candidates("   ").is_empty());
    }
}
