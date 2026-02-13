use anyhow::{anyhow, Context, Result};
use oauth2::basic::BasicClient;
use oauth2::reqwest::async_http_client;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, CsrfToken, RedirectUrl, RefreshToken, RevocationUrl,
    Scope, TokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::timeout;

const OAUTH_BASE_URL: &str = "https://openapi.longbridge.xyz";
const KEYCHAIN_SERVICE: &str = "com.longbridge.terminal";
const REDIRECT_URI: &str = "http://localhost:8877/callback";
const AUTH_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes

// ============================================================================
// Keychain Storage - Business Logic
// ============================================================================

pub struct KeychainStorage;

impl KeychainStorage {
    pub fn save(key: &str, value: &str) -> Result<()> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, key)
            .context("Failed to create keychain entry")?;
        entry
            .set_password(value)
            .context("Failed to save to keychain")?;
        Ok(())
    }

    pub fn load(key: &str) -> Result<Option<String>> {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, key)
            .context("Failed to create keychain entry")?;
        match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(anyhow!("Failed to load from keychain: {e}")),
        }
    }
}

// ============================================================================
// OAuth Client Registration - Longbridge-specific Business Logic
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClientCredentials {
    client_id: String,
}

#[derive(Deserialize)]
struct RegisterResponse {
    client_id: String,
}

impl ClientCredentials {
    fn save(&self) -> Result<()> {
        KeychainStorage::save("oauth_client_id", &self.client_id)?;
        tracing::debug!("OAuth client saved to keychain");
        Ok(())
    }

    fn load() -> Result<Option<Self>> {
        let client_id = KeychainStorage::load("oauth_client_id")?;

        match client_id {
            Some(id) => Ok(Some(Self { client_id: id })),
            _ => Ok(None),
        }
    }

    async fn register() -> Result<Self> {
        tracing::debug!("Registering new OAuth client with Longbridge");

        let client = reqwest::Client::new();
        let response = client
            .post(format!("{OAUTH_BASE_URL}/oauth2/register"))
            .json(&serde_json::json!({
                "redirect_uris": [REDIRECT_URI],
                "client_name": "Longbridge Terminal",
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "client_secret_basic"
            }))
            .send()
            .await
            .context("Failed to send registration request")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "OAuth registration failed with status {status}: {error_text}"
            ));
        }

        let reg_response: RegisterResponse = response
            .json()
            .await
            .context("Failed to parse registration response")?;

        Ok(Self {
            client_id: reg_response.client_id,
        })
    }

    async fn get_or_register() -> Result<Self> {
        if let Some(creds) = Self::load()? {
            tracing::debug!("Loaded existing OAuth client from keychain");
            Ok(creds)
        } else {
            let creds = Self::register().await?;
            creds.save()?;
            Ok(creds)
        }
    }

    fn create_oauth_client(&self) -> BasicClient {
        BasicClient::new(
            ClientId::new(self.client_id.clone()),
            None, // No client secret for public clients
            AuthUrl::new(format!("{OAUTH_BASE_URL}/oauth2/authorize")).unwrap(),
            Some(TokenUrl::new(format!("{OAUTH_BASE_URL}/oauth2/token")).unwrap()),
        )
        .set_redirect_uri(RedirectUrl::new(REDIRECT_URI.to_string()).unwrap())
        .set_revocation_uri(RevocationUrl::new(format!("{OAUTH_BASE_URL}/oauth2/revoke")).unwrap())
    }
}

// ============================================================================
// Token Management - Business Logic with Keychain Storage
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: u64, // Unix timestamp
}

impl OAuthToken {
    fn from_oauth2_response<TT, T>(token_response: &T) -> Self
    where
        TT: oauth2::TokenType,
        T: TokenResponse<TT>,
    {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let expires_in = token_response.expires_in().map_or(3600, |d| d.as_secs());

        Self {
            access_token: token_response.access_token().secret().clone(),
            refresh_token: token_response.refresh_token().map(|t| t.secret().clone()),
            expires_at: now + expires_in,
        }
    }

    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        now >= self.expires_at
    }

    pub fn save(&self) -> Result<()> {
        let json = serde_json::to_string(self)?;
        KeychainStorage::save("oauth_token", &json)?;
        tracing::debug!("OAuth token saved to keychain");
        Ok(())
    }

    pub fn load() -> Result<Option<Self>> {
        if let Some(json) = KeychainStorage::load("oauth_token")? {
            let token: Self = serde_json::from_str(&json)?;
            Ok(Some(token))
        } else {
            Ok(None)
        }
    }

    async fn refresh_internal(&self, creds: &ClientCredentials) -> Result<Self> {
        let refresh_token = self
            .refresh_token
            .as_ref()
            .ok_or_else(|| anyhow!("No refresh token available"))?;

        tracing::debug!("Refreshing OAuth token using oauth2 library");

        let client = creds.create_oauth_client();
        let token_response = client
            .exchange_refresh_token(&RefreshToken::new(refresh_token.clone()))
            .request_async(async_http_client)
            .await
            .context("Failed to refresh token")?;

        let mut new_token = Self::from_oauth2_response(&token_response);

        // Preserve refresh token if not returned
        if new_token.refresh_token.is_none() {
            new_token.refresh_token.clone_from(&self.refresh_token);
        }

        Ok(new_token)
    }
}

// ============================================================================
// Authorization Flow - Using oauth2 library with local callback server
// ============================================================================

async fn start_authorization_flow(creds: ClientCredentials) -> Result<OAuthToken> {
    let client = creds.create_oauth_client();

    // Generate authorization URL using oauth2 library
    let (auth_url, csrf_token) = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new(String::new()))
        .url();

    tracing::debug!("Starting OAuth authorization flow");
    println!("Opening browser for Longbridge OpenAPI authorization...");
    println!("If the browser doesn't open, please visit:");
    println!("{auth_url}");

    // Try to open browser
    if let Err(e) = open::that(auth_url.as_str()) {
        tracing::warn!("Failed to open browser: {e}");
    }

    // Start local callback server and wait for authorization code
    let (code, state) = wait_for_callback().await?;

    // Verify CSRF token
    if state != *csrf_token.secret() {
        return Err(anyhow!("CSRF token mismatch"));
    }

    // Exchange code for token using oauth2 library
    tracing::debug!("Exchanging authorization code for token using oauth2 library");
    let token_response = client
        .exchange_code(AuthorizationCode::new(code))
        .request_async(oauth2::reqwest::async_http_client)
        .await
        .context("Failed to exchange code for token")?;

    Ok(OAuthToken::from_oauth2_response(&token_response))
}

#[allow(clippy::items_after_statements)]
async fn wait_for_callback() -> Result<(String, String)> {
    let listener = bind_callback_server()?;
    let addr = listener.local_addr()?;
    tracing::debug!("Callback server listening on {addr}");

    let code = Arc::new(Mutex::new(None));
    let state = Arc::new(Mutex::new(None));
    let error = Arc::new(Mutex::new(None));

    let code_clone = Arc::clone(&code);
    let state_clone = Arc::clone(&state);
    let error_clone = Arc::clone(&error);

    let server_task = tokio::task::spawn_blocking(move || {
        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Failed to accept connection: {e}");
                    continue;
                }
            };

            let mut reader = BufReader::new(&stream);
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }

            tracing::debug!("Received callback: {request_line}");

            // Parse callback URL parameters
            if let Some(url_part) = request_line.split_whitespace().nth(1) {
                if let Ok(url) = url::Url::parse(&format!("http://localhost{url_part}")) {
                    let mut received_code = None;
                    let mut received_state = None;
                    let mut received_error = None;

                    for (key, value) in url.query_pairs() {
                        match key.as_ref() {
                            "code" => received_code = Some(value.to_string()),
                            "state" => received_state = Some(value.to_string()),
                            "error" => received_error = Some(value.to_string()),
                            _ => {}
                        }
                    }

                    const STYLE: &str =
                        "<style>html { \
                        font-family: system-ui, -apple-system, BlinkMacSystemFont, \
                        sans-serif; font-size: 16px; color: #e0e0e0; background: #202020; \
                        padding: 2rem; text-align: center; } </style>";

                    // Send HTML response to browser
                    let response = if let Some(err) = &received_error {
                        format!(
                            "HTTP/1.1 400 Bad Request\r\n\
                             Content-Type: text/html; charset=utf-8\r\n\
                             \r\n\
                             <html><body>{STYLE}<h1>Authorization Failed</h1>\
                             <p>Error: {err}</p></body></html>"
                        )
                    } else if received_code.is_some() && received_state.is_some() {
                        *code_clone.lock().unwrap() = received_code;
                        *state_clone.lock().unwrap() = received_state;
                        format!("HTTP/1.1 200 OK\r\n\
                         Content-Type: text/html; charset=utf-8\r\n\
                         \r\n\
                         <html><body>{STYLE}<h1>✓ Authorization Successful!</h1>\
                         <p>You can close this window and return to the terminal.</p> </body> </html>")
                    } else {
                        format!(
                            "HTTP/1.1 400 Bad Request\r\n\
                         Content-Type: text/html; charset=utf-8\r\n\
                         \r\n\
                         <html><body>{STYLE}<h1>Missing Parameters</h1>\
                         <p>Authorization code or state not received</p></body></html>"
                        )
                    };

                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();

                    if received_error.is_some() {
                        *error_clone.lock().unwrap() = received_error;
                    }

                    // Exit after first valid request
                    break;
                }
            }
        }
    });

    // Wait for callback with timeout
    match timeout(AUTH_TIMEOUT, server_task).await {
        Ok(Ok(())) => {
            if let Some(err) = error.lock().unwrap().take() {
                Err(anyhow!("OAuth authorization failed: {err}"))
            } else if let (Some(code_str), Some(state_str)) =
                (code.lock().unwrap().take(), state.lock().unwrap().take())
            {
                Ok((code_str, state_str))
            } else {
                Err(anyhow!("No authorization code received"))
            }
        }
        Ok(Err(e)) => Err(anyhow!("Callback server error: {e}")),
        Err(_) => Err(anyhow!(
            "Authorization timeout - no response received within 5 minutes"
        )),
    }
}

fn bind_callback_server() -> Result<TcpListener> {
    // Try to bind to port 8877, then fallback to 8878-8880
    for port in 8877..=8880 {
        match TcpListener::bind(format!("127.0.0.1:{port}")) {
            Ok(listener) => {
                tracing::debug!("Bound callback server to port {port}");
                return Ok(listener);
            }
            Err(e) => {
                tracing::warn!("Failed to bind to port {port}: {e}");
            }
        }
    }
    Err(anyhow!(
        "Failed to bind callback server to any port (8877-8880)"
    ))
}

// ============================================================================
// Public API
// ============================================================================

pub async fn authorize() -> Result<OAuthToken> {
    let creds = ClientCredentials::get_or_register().await?;
    let token = start_authorization_flow(creds).await?;
    token.save()?;
    Ok(token)
}

pub fn load_token() -> Result<Option<OAuthToken>> {
    OAuthToken::load()
}

pub async fn refresh_token_if_needed() -> Result<()> {
    if let Some(token) = OAuthToken::load()? {
        // Refresh if token expires within 1 hour
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        if token.expires_at.saturating_sub(now) < 3600 {
            tracing::debug!("Token expires soon, refreshing...");
            let creds = ClientCredentials::get_or_register().await?;
            let new_token = token.refresh_internal(&creds).await?;
            new_token.save()?;
        }
    }
    Ok(())
}

pub fn clear_token() -> Result<()> {
    tracing::debug!("Clearing OAuth token and client credentials from keychain");

    // Try to delete token
    if let Ok(Some(_)) = KeychainStorage::load("oauth_token") {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, "oauth_token")
            .context("Failed to create keychain entry for token")?;
        entry
            .delete_password()
            .context("Failed to delete token from keychain")?;
        tracing::debug!("OAuth token deleted from keychain");
    }

    // Try to delete client credentials
    if let Ok(Some(_)) = KeychainStorage::load("oauth_client_id") {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, "oauth_client_id")
            .context("Failed to create keychain entry for client_id")?;
        entry
            .delete_password()
            .context("Failed to delete client_id from keychain")?;
        tracing::debug!("OAuth client_id deleted from keychain");
    }

    Ok(())
}
