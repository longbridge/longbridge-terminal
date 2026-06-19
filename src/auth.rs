//! Auth utilities for `LongPort` `OpenAPI`.

use anyhow::{Context, Result};
use longport::oauth::TokenStorage as _;
use std::fs;
use std::path::PathBuf;

pub const CALLBACK_PORT: u16 = 60355;

/// Return the effective client ID used to build the runtime OAuth context and
/// refresh tokens.
///
/// Every login flow persists the `client_id` it authenticated with into the
/// local registration file: the browser / device flows store the id they
/// dynamically registered (RFC 7591), and the paste-code (`--auth-code <CODE>`)
/// flow stores the id carried inside the authorization code (the Connect AI
/// page registers it per Agent Name). Returns an empty string only when no
/// login has happened yet, in which case callers surface a re-auth prompt.
pub fn effective_client_id() -> String {
    load_registration().map(|r| r.client_id).unwrap_or_default()
}

/// OAuth client registration persisted locally so refresh can reuse the same
/// `client_id` the session authenticated with.
///
/// The browser / device flows register the client themselves (RFC 7591) and
/// fill in `registration_access_token` / `registration_client_uri`, which allow
/// the registration to be revoked on logout (RFC 7592). The paste-code flow
/// records only the `client_id` carried in the authorization code — the client
/// was registered server-side by the Connect AI page, so the CLI has no
/// management credentials and those fields stay `None`.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
struct ClientRegistration {
    client_id: String,
    #[serde(default)]
    registration_access_token: Option<String>,
    #[serde(default)]
    registration_client_uri: Option<String>,
}

fn previous_domain() -> String {
    format!("{}{}", "long", "bridge")
}

fn normalize_longport_url(url: &str) -> String {
    let previous = previous_domain();
    url.replace(
        &format!("openapi.{previous}.com"),
        "openapi.longportapp.com",
    )
    .replace(&format!("openapi.{previous}.cn"), "openapi.longportapp.cn")
    .replace(
        &format!("openapi.{previous}.xyz"),
        "openapi.longportapp.xyz",
    )
    .replace(&format!("open.{previous}.com"), "open.longportapp.com")
    .replace(&format!("open.{previous}.cn"), "open.longportapp.cn")
}

fn normalize_registration_urls(reg: &mut ClientRegistration) -> bool {
    let Some(uri) = &reg.registration_client_uri else {
        return false;
    };
    let normalized = normalize_longport_url(uri);
    if normalized == *uri {
        false
    } else {
        reg.registration_client_uri = Some(normalized);
        true
    }
}

fn registration_file_path() -> Result<PathBuf> {
    // Keep staging and production registrations separate: a `client_id`
    // registered against one environment is not valid on the other, so they
    // must not share a file.
    let filename = if crate::region::is_test_env() {
        "cli-registration-staging"
    } else {
        "cli-registration"
    };
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Failed to get home directory"))?
        .join(".longport")
        .join("openapi")
        .join(filename))
}

fn load_registration() -> Option<ClientRegistration> {
    let path = registration_file_path().ok()?;
    let content = fs::read_to_string(path).ok()?;
    let mut reg: ClientRegistration = serde_json::from_str(&content).ok()?;
    if normalize_registration_urls(&mut reg) {
        let _ = save_registration(&reg);
    }
    Some(reg)
}

fn save_registration(reg: &ClientRegistration) -> Result<()> {
    let mut reg = reg.clone();
    normalize_registration_urls(&mut reg);

    let path = registration_file_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("Failed to create config directory")?;
    }
    let json = serde_json::to_string_pretty(&reg).context("Failed to serialize registration")?;
    fs::write(&path, json).context("Failed to write registration file")?;
    crate::secure_storage::harden_file_permissions(&path);
    Ok(())
}

/// Suffix appended to every registered client name so the entry is identifiable
/// as a `LongPort` CLI login in the user's authorized-apps list.
const CLIENT_NAME_SUFFIX: &str = " (LongPort CLI)";

/// Normalize a user-supplied client name (from `--client-name`) by appending the
/// [`CLIENT_NAME_SUFFIX`], e.g. `Claude Code` becomes `Claude Code (LongPort CLI)`.
/// The suffix is not duplicated if the input already ends with it.
fn apply_client_name_suffix(name: String) -> String {
    let name = name.trim();
    if name.ends_with(CLIENT_NAME_SUFFIX) {
        name.to_owned()
    } else {
        format!("{name}{CLIENT_NAME_SUFFIX}")
    }
}

/// Build the OAuth client name used for dynamic registration, identifying the device
/// in the user's authorized-apps list.
///
/// Format is `<user>@<machine> (LongPort CLI)`, e.g. `jason@huacnlee-macbook
/// (LongPort CLI)`. The login user name comes first, followed by the host name (which
/// usually encodes the device type). When the host name is unavailable it falls back to
/// the OS label so a generic server login still gets a device hint, e.g. `ubuntu@Linux
/// (LongPort CLI)`. When the user name is unavailable the machine part is used alone.
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
        Some(user) => format!("{user}@{machine}{CLIENT_NAME_SUFFIX}"),
        None => format!("{machine}{CLIENT_NAME_SUFFIX}"),
    }
}

/// Build a reqwest HTTP client with the `LongPort` terminal User-Agent.
fn build_http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("longport-terminal/", env!("CARGO_PKG_VERSION")))
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
        "client_name": name_override.map_or_else(client_name, apply_client_name_suffix),
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
        registration_access_token: Some(
            data["registration_access_token"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("No registration_access_token in response"))?
                .to_owned(),
        ),
        registration_client_uri: Some(
            data["registration_client_uri"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("No registration_client_uri in response"))?
                .to_owned(),
        ),
    })
}

/// Return the locally persisted client registration if present, otherwise register a
/// new one (RFC 7591). Reusing the stored registration across logins avoids creating a
/// duplicate `client_id` on the server every time the CLI authenticates.
///
/// An explicit `name_override` (from `--client-name`) forces a fresh registration so the
/// new name takes effect: any prior registration is revoked (RFC 7592) and replaced,
/// rather than silently reused.
async fn get_or_register_client(
    http_client: &reqwest::Client,
    verbose: bool,
    name_override: Option<String>,
) -> Result<ClientRegistration> {
    if name_override.is_some() {
        // Explicit name requested: drop the old client so the new name replaces it
        // instead of being ignored in favor of the cached registration.
        revoke_client_registration(http_client).await;
    } else if let Some(reg) = load_registration() {
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

    // Only CLI-registered clients (browser / device flows) carry management
    // credentials; the paste-code flow stores just a client_id with no way to
    // revoke server-side, so skip the DELETE for those and only drop the file.
    if let (Some(uri), Some(token)) = (&reg.registration_client_uri, &reg.registration_access_token)
    {
        if let Ok(resp) = http_client.delete(uri).bearer_auth(token).send().await {
            tracing::debug!("Client registration revocation: HTTP {}", resp.status());
        }
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

/// Redirect URI bound to the authorization code generated by the Connect AI
/// page. Must exactly match the `redirect_uri` the code was issued against.
fn agent_redirect_uri() -> String {
    format!("{}/connect/done", crate::region::open_url())
}

pub fn token_file_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Failed to get home directory"))?
        .join(".longport")
        .join("openapi")
        .join("cli-auth"))
}

/// Invite code file path: `~/.longport/openapi/invite-code`
fn invite_code_file_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Failed to get home directory"))?
        .join(".longport")
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
    let full = crate::secure_storage::EncryptedFileTokenStorage::load_full(&effective_client_id())?;
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

fn encode_query_value(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

fn authorization_url(
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    invite_code: Option<&str>,
) -> String {
    let mut url = format!(
        "{}/authorize?response_type=code&client_id={}&redirect_uri={}&state={}&scope=",
        oauth_base_url(),
        encode_query_value(client_id),
        encode_query_value(redirect_uri),
        encode_query_value(state)
    );
    if let Some(invite_code) = invite_code {
        url = append_query_param(&url, "invite-code", invite_code);
    }
    url
}

fn random_oauth_state() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).context("Failed to generate OAuth state")?;
    Ok(bs58::encode(bytes).into_string())
}

fn query_param(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        let (raw_key, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
        let decoded_key = percent_encoding::percent_decode_str(raw_key)
            .decode_utf8()
            .ok()?;
        if decoded_key == key {
            return percent_encoding::percent_decode_str(raw_value)
                .decode_utf8()
                .ok()
                .map(std::borrow::Cow::into_owned);
        }
    }
    None
}

fn parse_callback_request_line(line: &str, expected_state: &str) -> Result<String> {
    let path = line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("OAuth callback request was malformed"))?;
    let query = path
        .split_once('?')
        .map(|(_, query)| query)
        .ok_or_else(|| anyhow::anyhow!("OAuth callback did not include a query string"))?;

    if let Some(error) = query_param(query, "error") {
        anyhow::bail!("OAuth authorization failed: {error}");
    }

    let state = query_param(query, "state")
        .ok_or_else(|| anyhow::anyhow!("OAuth callback did not include state"))?;
    if state != expected_state {
        anyhow::bail!("OAuth callback state mismatch");
    }

    query_param(query, "code").ok_or_else(|| anyhow::anyhow!("OAuth callback did not include code"))
}

fn wait_for_authorization_callback(
    listener: std::net::TcpListener,
    expected_state: String,
) -> Result<String> {
    use std::io::{Read, Write as _};
    use std::time::{Duration, Instant};

    listener
        .set_nonblocking(true)
        .context("Failed to set OAuth callback timeout")?;
    let deadline = Instant::now() + Duration::from_mins(5);
    let (mut stream, _) = loop {
        match listener.accept() {
            Ok(pair) => break pair,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    anyhow::bail!("Timed out waiting for OAuth callback");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e).context("Failed to accept OAuth callback"),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .context("Failed to set OAuth callback read timeout")?;

    let mut buf = [0u8; 8192];
    let n = stream
        .read(&mut buf)
        .context("Failed to read OAuth callback request")?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let request_line = req.lines().next().unwrap_or_default();
    let result = parse_callback_request_line(request_line, &expected_state);

    let (status, body) = if result.is_ok() {
        (
            "200 OK",
            "Authorization received. You can close this window.",
        )
    } else {
        (
            "400 Bad Request",
            "Authorization failed. Return to the terminal for details.",
        )
    };
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());

    result
}

async fn exchange_browser_authorization_code(
    http_client: &reqwest::Client,
    client_id: &str,
    code: &str,
    redirect_uri: &str,
) -> Result<()> {
    let url = format!("{}/token", oauth_base_url());
    let raw = http_client
        .post(&url)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
        ])
        .send()
        .await
        .context("Authorization token exchange failed")?;

    let status = raw.status();
    if !status.is_success() {
        let text = raw.text().await.unwrap_or_default();
        anyhow::bail!("Authorization token exchange failed ({status}): {text}");
    }

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

    let token = longport::oauth::StoredToken {
        client_id: client_id.to_string(),
        access_token: access_token.to_string(),
        refresh_token,
        expires_at: now + expires_in,
    };
    crate::secure_storage::EncryptedFileTokenStorage
        .save(&token)
        .map_err(|e| anyhow::anyhow!("Failed to save token: {e}"))?;

    Ok(())
}

/// Try to open a URL in the system browser. Returns `true` if launched successfully.
pub fn open_browser(url: &str) -> bool {
    open::that(url).is_ok()
}

fn device_verification_url(device_resp: &serde_json::Value, invite_code: Option<&str>) -> String {
    let verification_url_base = device_resp["verification_uri_complete"]
        .as_str()
        .unwrap_or_else(|| device_resp["verification_uri"].as_str().unwrap_or(""));
    let mut verification_url = normalize_longport_url(verification_url_base);
    if let Some(invite_code) = invite_code {
        verification_url = append_query_param(&verification_url, "invite-code", invite_code);
    }
    verification_url
}

/// Device Authorization Flow (RFC 8628).
pub async fn device_login(verbose: bool, client_name: Option<String>) -> Result<()> {
    use std::time::{Duration, Instant};

    let oauth_base = oauth_base_url();
    let http_client = build_http_client()?;
    let invite_code = read_invite_code();

    let reg = get_or_register_client(&http_client, verbose, client_name).await?;
    let client_id = reg.client_id.as_str();

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
    let verification_url = device_verification_url(&device_resp, invite_code.as_deref());
    let expires_in = device_resp["expires_in"].as_u64().unwrap_or(300);
    let interval = device_resp["interval"].as_u64().unwrap_or(5);

    let opened = open_browser(&verification_url);

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

            let token = longport::oauth::StoredToken {
                client_id: client_id.to_string(),
                access_token: access_token.to_string(),
                refresh_token,
                expires_at: now + expires_in,
            };
            crate::secure_storage::EncryptedFileTokenStorage
                .save(&token)
                .map_err(|e| anyhow::anyhow!("Failed to save token: {e}"))?;

            // Registration was already persisted in get_or_register_client, so
            // there is nothing more to store here.
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
    let Some(full) =
        crate::secure_storage::EncryptedFileTokenStorage::load_full(&effective_client_id())
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
            "No refresh token found. Please run 'longport auth login' to re-authenticate."
        ));
    };

    let http_client = reqwest::Client::builder()
        .user_agent(concat!("longport-terminal/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .context("Failed to build HTTP client for token refresh")?;

    let url = format!("{}/token", oauth_base_url());
    tracing::debug!("Refreshing expired access token via {url}");

    let dynamic_client_id = effective_client_id();
    let resp = http_client
        .post(&url)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.as_str()),
            ("client_id", dynamic_client_id.as_str()),
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

        let token = longport::oauth::StoredToken {
            client_id: dynamic_client_id.clone(),
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
            "Refresh token has expired. Please run 'longport auth login' to re-authenticate."
        ));
    }

    Err(anyhow::anyhow!(
        "Token refresh failed ({status}): {error} — please retry"
    ))
}

/// Authorization Code Flow: opens a browser on this machine and listens on
/// `localhost:CALLBACK_PORT` for the OAuth callback.
pub async fn auth_code_login(client_name: Option<String>) -> Result<()> {
    println!("Opening browser for authorization...");
    let listener =
        std::net::TcpListener::bind(("127.0.0.1", CALLBACK_PORT)).with_context(|| {
            format!("Failed to listen on localhost:{CALLBACK_PORT} for the OAuth callback")
        })?;
    println!("Listening on localhost:{CALLBACK_PORT} for the OAuth callback.");
    let invite_code = read_invite_code();

    let http_client = build_http_client()?;
    let reg = get_or_register_client(&http_client, false, client_name).await?;
    let redirect_uri = format!("http://localhost:{CALLBACK_PORT}/callback");
    let state = random_oauth_state()?;
    let authorization_url = authorization_url(
        &reg.client_id,
        &redirect_uri,
        &state,
        invite_code.as_deref(),
    );

    println!();
    println!("Authorization URL: {authorization_url}");
    println!();
    if !open_browser(&authorization_url) {
        println!("Could not open browser automatically. Please visit the URL above.");
    }

    let code =
        tokio::task::spawn_blocking(move || wait_for_authorization_callback(listener, state))
            .await
            .map_err(|e| anyhow::anyhow!("OAuth callback task failed: {e}"))??;

    exchange_browser_authorization_code(&http_client, &reg.client_id, &code, &redirect_uri).await?;

    // Registration was already persisted in get_or_register_client.
    println!("Successfully authenticated.");
    Ok(())
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

/// Extract the `client_id` claim from a restored authorization code.
///
/// The Connect AI page issues the code as `base64(JWT.utf8)`, where the JWT
/// payload carries the dynamically-registered `client_id` (one per Agent Name).
/// [`auth_code_candidates`] restores each candidate to that standard-base64
/// form; here we base64-decode it back to the JWT string and read the
/// `client_id` claim. Returns `None` for candidates that are not such a code.
fn client_id_from_code(candidate: &str) -> Option<String> {
    use base64::Engine as _;
    let jwt_bytes = base64::engine::general_purpose::STANDARD
        .decode(candidate)
        .ok()?;
    let jwt = std::str::from_utf8(&jwt_bytes).ok()?;
    decode_jwt_payload(jwt)?["client_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Unpack the base58 "packed agent code" issued by the Connect AI page.
///
/// Layout (pre-base58): `[0x01 version][client_id_len: 1 byte][client_id utf8][code utf8]`.
/// The trailing `code` is the backend's standard-base64 authorization code,
/// sent verbatim to `/token`; the `client_id` (one per Agent Name, registered
/// dynamically) is presented alongside it. Returns `(client_id, code)`, or
/// `None` if the input isn't this packed form.
///
/// Mirrors the web `packAgentAuthCode()` and the hosted MCP `authenticate`
/// tool — keep all three in sync (shared format + test vector).
fn unpack_agent_code(input: &str) -> Option<(String, String)> {
    let raw = input.trim().trim_matches(|c| matches!(c, '"' | '\'' | '`'));
    if raw.is_empty() {
        return None;
    }
    let bytes = bs58::decode(raw).into_vec().ok()?;
    if bytes.len() < 2 || bytes[0] != 0x01 {
        return None;
    }
    let cid_len = bytes[1] as usize;
    if bytes.len() < 2 + cid_len {
        return None;
    }
    let client_id = std::str::from_utf8(&bytes[2..2 + cid_len])
        .ok()?
        .to_string();
    let auth_code = std::str::from_utf8(&bytes[2 + cid_len..]).ok()?.to_string();
    if client_id.is_empty() || auth_code.is_empty() {
        return None;
    }
    Some((client_id, auth_code))
}

/// Agent Auth Code reverse-authorization flow.
///
/// Exchanges a standard OAuth authorization code — generated by the user at
/// <https://open.longportapp.com/connect> (5-minute, single-use) — for an
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
             `longport auth login --auth-code <CODE>`.",
            connect_url()
        );
    }

    // Build the ordered (code, client_id) exchange attempts:
    // 1) the base58 "packed agent code" — client_id + real code packed together
    //    (current Connect AI format); the trailing code is sent to /token.
    // 2) legacy base64(JWT) candidates that carry client_id as a JWT claim — the
    //    candidate itself is both the code and the client_id source.
    let mut attempts: Vec<(String, String)> = Vec::new();
    if let Some((client_id, real_code)) = unpack_agent_code(code) {
        attempts.push((real_code, client_id));
    }
    for candidate in &candidates {
        if let Some(client_id) = client_id_from_code(candidate) {
            attempts.push((candidate.clone(), client_id));
        }
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
    let saw_client_id = !attempts.is_empty();
    for (exchange_code, client_id) in &attempts {
        tracing::debug!("Exchanging authorization code via {url}");
        let send_result = http_client
            .post(&url)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", exchange_code.as_str()),
                ("redirect_uri", redirect_uri.as_str()),
                ("client_id", client_id.as_str()),
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

        // Persist under the client_id carried in the code. This flow does no
        // dynamic registration of its own, so record the client_id locally too
        // (no management credentials), letting refresh_if_expired's
        // effective_client_id() resolve it on later runs.
        let token = longport::oauth::StoredToken {
            client_id: client_id.clone(),
            access_token: access_token.to_string(),
            refresh_token,
            expires_at: now + expires_in,
        };
        crate::secure_storage::EncryptedFileTokenStorage
            .save(&token)
            .map_err(|e| anyhow::anyhow!("Failed to save token: {e}"))?;

        if let Err(e) = save_registration(&ClientRegistration {
            client_id: client_id.clone(),
            registration_access_token: None,
            registration_client_uri: None,
        }) {
            tracing::warn!("Failed to persist client registration for refresh: {e}");
        }

        println!("Successfully authenticated.");
        return Ok(());
    }

    // No candidate was a JWT carrying a client_id — the pasted string isn't a
    // Connect AI code, or predates the Agent Name step.
    if !saw_client_id && last_rejection.is_none() {
        anyhow::bail!(
            "Authorization code does not carry a client_id. Generate a fresh one at {} \
             (enter an Agent Name) and run `longport auth login --auth-code <CODE>` again.",
            connect_url()
        );
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
                 and run `longport auth login --auth-code <CODE>` again.",
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

    // Shared vector with longport-mcp: the connect page displays
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

    // Packed agent code: base58([0x01][cid_len][client_id][ORIGINAL]).
    // Shared format/vector with the web `packAgentAuthCode` and longport-mcp.
    const PACKED_CLIENT_ID: &str = "c91cd252-2f89-4024-9c5d-7b1340fc3bd1";
    const PACKED_DISPLAY: &str =
        "F4ep4yfKvDgpZnFUR6T8vm5bCjG65XZKgaTiNWbwTCVPqGw3HCrpDvYuxLUu6uNtn73ht5BKtKS7Fk9WG9MV9V2PYkwSGWoZfoEtFbfCL2f45c8";

    #[test]
    fn unpack_agent_code_extracts_client_id_and_code() {
        let (client_id, code) = unpack_agent_code(PACKED_DISPLAY).expect("should unpack");
        assert_eq!(client_id, PACKED_CLIENT_ID);
        assert_eq!(code, ORIGINAL);
    }

    #[test]
    fn unpack_agent_code_strips_chat_copy_junk() {
        let (client_id, code) =
            unpack_agent_code(&format!("  `{PACKED_DISPLAY}`  ")).expect("should unpack");
        assert_eq!(client_id, PACKED_CLIENT_ID);
        assert_eq!(code, ORIGINAL);
    }

    #[test]
    fn unpack_agent_code_rejects_non_packed() {
        // Legacy base58 display form (no version byte) and empty input → None.
        assert!(unpack_agent_code(BASE58_FORM).is_none());
        assert!(unpack_agent_code("").is_none());
    }

    #[test]
    fn oauth_local_files_use_longport_directory() {
        let openapi_dir = std::path::Path::new(".longport").join("openapi");

        assert!(token_file_path()
            .unwrap()
            .ends_with(openapi_dir.join("cli-auth")));
        assert!(registration_file_path().unwrap().ends_with(
            std::path::Path::new(".longport").join("openapi").join(
                if crate::region::is_test_env() {
                    "cli-registration-staging"
                } else {
                    "cli-registration"
                }
            )
        ));
        assert!(invite_code_file_path().unwrap().ends_with(
            std::path::Path::new(".longport")
                .join("openapi")
                .join("invite-code")
        ));
    }

    #[test]
    #[serial_test::serial]
    fn oauth_base_urls_use_longportapp_hosts() {
        std::env::set_var("LONGPORT_REGION", "global");
        assert_eq!(oauth_base_url(), "https://openapi.longportapp.com/oauth2");

        std::env::set_var("LONGPORT_REGION", "cn");
        assert_eq!(oauth_base_url(), "https://openapi.longportapp.cn/oauth2");

        std::env::remove_var("LONGPORT_REGION");
    }

    #[test]
    fn cli_registration_uri_is_normalized_to_longportapp() {
        let old_host = format!("openapi.{}.com", previous_domain());
        let mut reg = ClientRegistration {
            client_id: "client-id".to_string(),
            registration_access_token: Some("token".to_string()),
            registration_client_uri: Some(format!("https://{old_host}/oauth2/register/client-id")),
        };

        normalize_registration_urls(&mut reg);

        assert_eq!(
            reg.registration_client_uri.as_deref(),
            Some("https://openapi.longportapp.com/oauth2/register/client-id")
        );
    }

    #[test]
    #[serial_test::serial]
    fn browser_authorization_url_uses_longportapp_host() {
        std::env::set_var("LONGPORT_REGION", "global");
        let url = authorization_url(
            "client-id",
            "http://localhost:60355/callback",
            "state-value",
            Some("invite code"),
        );

        assert!(url.starts_with("https://openapi.longportapp.com/oauth2/authorize?"));
        assert!(url.contains("client_id=client%2Did"));
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A60355%2Fcallback"));
        assert!(url.contains("state=state%2Dvalue"));
        assert!(url.contains("invite-code=invite%20code"));

        std::env::remove_var("LONGPORT_REGION");
    }

    #[test]
    fn device_verification_url_is_normalized_to_longportapp() {
        let previous = previous_domain();
        let device_resp = serde_json::json!({
            "verification_uri_complete": format!(
                "https://open.{previous}.com/oauth2/device?user_code=ABCD-EFGH"
            ),
            "verification_uri": format!("https://open.{previous}.com/oauth2/device"),
        });

        let url = device_verification_url(&device_resp, Some("invite code"));

        assert_eq!(
            url,
            "https://open.longportapp.com/oauth2/device?user_code=ABCD-EFGH&invite-code=invite%20code"
        );
    }
}
