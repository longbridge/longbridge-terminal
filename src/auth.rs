use anyhow::{anyhow, Context, Result};
use oauth2::basic::BasicClient;
use oauth2::reqwest::async_http_client;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, CsrfToken, RedirectUrl, RefreshToken, RevocationUrl,
    Scope, TokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::timeout;

const AUTH_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes

const OAUTH_BASE_URL: &str = "https://openapi.longportapp.com";
pub const OAUTH_CLIENT_ID: &str = "fd52fbc5-02a9-47f5-ad30-0842c841aae9";

fn session_file_path() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow!("Failed to get home directory"))?
        .join(".longbridge/terminal/.openapi-session"))
}

pub struct FileStorage;

impl FileStorage {
    pub fn save(data: &str) -> Result<()> {
        let path = session_file_path()?;

        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("Failed to create config directory")?;
        }

        fs::write(&path, data).context("Failed to write session file")?;

        tracing::debug!("Session saved to {}", path.display());
        Ok(())
    }

    pub fn load() -> Result<Option<String>> {
        let path = session_file_path()?;

        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&path).context("Failed to read session file")?;

        Ok(Some(content))
    }

    pub fn delete() -> Result<()> {
        let path = session_file_path()?;

        if path.exists() {
            fs::remove_file(&path).context("Failed to delete session file")?;
            tracing::debug!("Session file deleted: {}", path.display());
        }

        Ok(())
    }
}

fn create_oauth_client(redirect_uri: &str) -> BasicClient {
    BasicClient::new(
        ClientId::new(OAUTH_CLIENT_ID.to_string()),
        None, // No client secret for public clients
        AuthUrl::new(format!("{OAUTH_BASE_URL}/oauth2/authorize")).unwrap(),
        Some(TokenUrl::new(format!("{OAUTH_BASE_URL}/oauth2/token")).unwrap()),
    )
    .set_redirect_uri(RedirectUrl::new(redirect_uri.to_string()).unwrap())
    .set_revocation_uri(RevocationUrl::new(format!("{OAUTH_BASE_URL}/oauth2/revoke")).unwrap())
}

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
        let json = serde_json::to_string_pretty(self)?;
        FileStorage::save(&json)?;
        tracing::debug!("OAuth token saved to file");
        Ok(())
    }

    pub fn load() -> Result<Option<Self>> {
        if let Some(json) = FileStorage::load()? {
            let token: Self = serde_json::from_str(&json)?;
            Ok(Some(token))
        } else {
            Ok(None)
        }
    }

    async fn refresh_internal(&self) -> Result<Self> {
        let refresh_token = self
            .refresh_token
            .as_ref()
            .ok_or_else(|| anyhow!("No refresh token available"))?;

        tracing::debug!("Refreshing OAuth token using oauth2 library");

        // For refresh token flow, redirect_uri is not used, but we still need to provide one
        // Use a placeholder that matches one of the registered redirect URIs
        let client = create_oauth_client("http://localhost:60355/callback");
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

async fn start_authorization_flow() -> Result<OAuthToken> {
    // Bind callback server first to get the actual port
    let listener = bind_callback_server()?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://localhost:{port}/callback");

    tracing::debug!("Callback server listening on port {port}");
    tracing::debug!("Redirect URI: {redirect_uri}");

    let client = create_oauth_client(&redirect_uri);

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
    let (code, state) = wait_for_callback(listener).await?;

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

    // Debug: log the token response details
    tracing::debug!("Access token: {}", token_response.access_token().secret());
    tracing::debug!("Refresh token: {:?}", token_response.refresh_token().map(|t| t.secret()));
    tracing::debug!("Token type: {:?}", token_response.token_type());
    tracing::debug!("Expires in: {:?}", token_response.expires_in());
    tracing::debug!("Scopes: {:?}", token_response.scopes());

    Ok(OAuthToken::from_oauth2_response(&token_response))
}

#[allow(clippy::items_after_statements)]
async fn wait_for_callback(listener: TcpListener) -> Result<(String, String)> {
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

                    const STYLE: &str = "<style>html { \
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
    TcpListener::bind("127.0.0.1:60355")
        .with_context(|| "Failed to bind callback server on 127.0.0.1:60355")
}

pub async fn authorize() -> Result<OAuthToken> {
    let token = start_authorization_flow().await?;
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
            let new_token = token.refresh_internal().await?;
            new_token.save()?;
        }
    }
    Ok(())
}

pub fn clear_token() -> Result<()> {
    tracing::debug!("Clearing OAuth token from file");
    FileStorage::delete()?;
    tracing::debug!("OAuth token deleted");
    Ok(())
}
