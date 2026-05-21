//! Encrypted file token storage for Longbridge OAuth tokens.
//!
//! Tokens are AES-256-GCM encrypted using a key derived from a machine-specific
//! identifier via HKDF-SHA256. This prevents token files from being used on a
//! different machine if stolen, while remaining fully transparent to callers.
//!
//! File format: MAGIC[3] || NONCE[12] || CIPHERTEXT+TAG
//!   MAGIC = [b'L', b'B', 0x01]
//!
//! Legacy plaintext JSON files are detected by the absence of the magic header
//! and migrated to the encrypted format on the next write.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use hkdf::Hkdf;
use longbridge::oauth::{OAuthError, OAuthResult, StoredToken, TokenStorage};
use serde::{Deserialize, Serialize};

const MAGIC: &[u8; 3] = b"LB\x01";
const HKDF_INFO: &[u8] = b"longbridge-token-v1";

static MACHINE_ID: OnceLock<String> = OnceLock::new();

/// Full token with extra metadata not present in `StoredToken`.
pub struct FullToken {
    pub client_id: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: u64,
    pub logged_in_at: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
struct EncryptedPayload {
    client_id: String,
    access_token: String,
    refresh_token: Option<String>,
    expires_at: u64,
    #[serde(default)]
    logged_in_at: Option<u64>,
}

impl From<EncryptedPayload> for StoredToken {
    fn from(p: EncryptedPayload) -> Self {
        Self {
            client_id: p.client_id,
            access_token: p.access_token,
            refresh_token: p.refresh_token,
            expires_at: p.expires_at,
        }
    }
}

impl From<EncryptedPayload> for FullToken {
    fn from(p: EncryptedPayload) -> Self {
        Self {
            client_id: p.client_id,
            access_token: p.access_token,
            refresh_token: p.refresh_token,
            expires_at: p.expires_at,
            logged_in_at: p.logged_in_at,
        }
    }
}

/// Token storage that encrypts files using a machine-derived AES-256-GCM key.
pub struct EncryptedFileTokenStorage;

fn load_payload_with_migration(client_id: &str) -> Option<EncryptedPayload> {
    let path = token_path(client_id).ok()?;
    let (payload, needs_migration) = read_payload(&path)?;
    if needs_migration {
        let _ = EncryptedFileTokenStorage.save(&StoredToken::from(payload.clone()));
    }
    Some(payload)
}

impl EncryptedFileTokenStorage {
    /// Load the full token (including `logged_in_at`) for the given client.
    pub fn load_full(client_id: &str) -> Option<FullToken> {
        Some(load_payload_with_migration(client_id)?.into())
    }
}

impl TokenStorage for EncryptedFileTokenStorage {
    fn load(&self, client_id: &str) -> Option<StoredToken> {
        Some(load_payload_with_migration(client_id)?.into())
    }

    fn save(&self, token: &StoredToken) -> OAuthResult<()> {
        let path = token_path(&token.client_id).map_err(|e| OAuthError::Other(e.to_string()))?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Preserve logged_in_at across refreshes; set to now on initial login.
        let logged_in_at = read_payload(&path)
            .and_then(|(p, _)| p.logged_in_at)
            .unwrap_or(now);

        let payload = EncryptedPayload {
            client_id: token.client_id.clone(),
            access_token: token.access_token.clone(),
            refresh_token: token.refresh_token.clone(),
            expires_at: token.expires_at,
            logged_in_at: Some(logged_in_at),
        };

        let json =
            serde_json::to_vec(&payload).map_err(|e| OAuthError::SerializeToken { source: e })?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| OAuthError::CreateDirFailed {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }

        let encrypted = encrypt(&json)
            .map_err(|e| OAuthError::Other(format!("Token encryption failed: {e}")))?;

        std::fs::write(&path, &encrypted).map_err(|e| OAuthError::TokenFileWrite {
            path: path.clone(),
            source: e,
        })?;

        harden_file_permissions(&path);
        Ok(())
    }
}

/// Migrate a legacy plaintext token file to the new encrypted location.
///
/// Called once at startup. If the new file already exists, this is a no-op.
/// Reads the old `~/.longbridge/openapi/tokens/<client_id>` file, re-saves it
/// via `EncryptedFileTokenStorage` (which writes to `~/.longbridge/cli/auth-token`
/// and deletes the legacy file).
pub fn migrate_legacy_token(client_id: &str) {
    let Ok(new_path) = token_path(client_id) else {
        return;
    };
    if new_path.exists() {
        return;
    }

    let Some(home) = dirs::home_dir() else {
        return;
    };
    let legacy = home
        .join(".longbridge")
        .join("openapi")
        .join("tokens")
        .join(client_id);

    let Ok(bytes) = std::fs::read(&legacy) else {
        return;
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return;
    };

    let Some(access_token) = json["access_token"].as_str() else {
        return;
    };
    let token = longbridge::oauth::StoredToken {
        client_id: client_id.to_string(),
        access_token: access_token.to_string(),
        refresh_token: json["refresh_token"].as_str().map(str::to_owned),
        expires_at: json["expires_at"].as_u64().unwrap_or(0),
    };

    if EncryptedFileTokenStorage.save(&token).is_ok() {
        let _ = std::fs::remove_file(&legacy);
        tracing::debug!("Migrated legacy token to encrypted storage");
    }
}

/// Delete the token file for `client_id`.
pub fn try_delete(client_id: &str) {
    if let Ok(path) = token_path(client_id) {
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Set file permissions to 0600 on Unix.
pub fn harden_file_permissions(path: &Path) {
    #[cfg(target_family = "unix")]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            if perms.mode() & 0o077 != 0 {
                perms.set_mode(0o600);
                if let Err(e) = std::fs::set_permissions(path, perms) {
                    tracing::debug!("Could not harden file permissions: {e}");
                }
            }
        }
    }
}

fn token_path(_client_id: &str) -> anyhow::Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Failed to get home directory"))?
        .join(".longbridge")
        .join("cli")
        .join("auth-token"))
}

/// Read and decrypt the payload at `path`.
/// Returns `(payload, needs_migration)` where `needs_migration` is true for
/// legacy plaintext files that should be re-saved in encrypted format.
fn read_payload(path: &Path) -> Option<(EncryptedPayload, bool)> {
    let bytes = std::fs::read(path).ok()?;

    if bytes.starts_with(MAGIC) {
        let plaintext = decrypt(&bytes)
            .map_err(|e| {
                tracing::warn!("Token decryption failed (machine-id changed?): {e}");
                e
            })
            .ok()?;
        let payload = serde_json::from_slice(&plaintext).ok()?;
        Some((payload, false))
    } else {
        let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        let payload = EncryptedPayload {
            client_id: json["client_id"].as_str()?.to_owned(),
            access_token: json["access_token"].as_str()?.to_owned(),
            refresh_token: json["refresh_token"].as_str().map(str::to_owned),
            expires_at: json["expires_at"].as_u64().unwrap_or(0),
            logged_in_at: json["logged_in_at"].as_u64(),
        };
        Some((payload, true))
    }
}

fn encrypt(plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let key = machine_derived_key()?;
    let cipher = Aes256Gcm::new(&key);
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| format!("AES-GCM encrypt: {e}"))?;

    let mut out = Vec::with_capacity(MAGIC.len() + nonce.len() + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn decrypt(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < MAGIC.len() + 12 {
        return Err("data too short".into());
    }
    let nonce = Nonce::from_slice(&data[MAGIC.len()..MAGIC.len() + 12]);
    let key = machine_derived_key()?;
    let cipher = Aes256Gcm::new(&key);
    cipher
        .decrypt(nonce, &data[MAGIC.len() + 12..])
        .map_err(|e| format!("AES-GCM decrypt: {e}"))
}

fn machine_derived_key() -> Result<Key<Aes256Gcm>, String> {
    let id = machine_id()?;
    let hk = Hkdf::<sha2::Sha256>::new(None, id.as_bytes());
    let mut key_bytes = [0u8; 32];
    hk.expand(HKDF_INFO, &mut key_bytes)
        .map_err(|e| format!("HKDF expand: {e}"))?;
    Ok(*Key::<Aes256Gcm>::from_slice(&key_bytes))
}

/// Return a cached, stable machine-specific identifier.
fn machine_id() -> Result<&'static str, String> {
    if let Some(id) = MACHINE_ID.get() {
        return Ok(id.as_str());
    }
    let id = resolve_machine_id()?;
    Ok(MACHINE_ID.get_or_init(|| id).as_str())
}

fn resolve_machine_id() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("sysctl")
            .args(["-n", "kern.uuid"])
            .output()
        {
            let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !id.is_empty() {
                return Ok(id);
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        for path in &["/etc/machine-id", "/var/lib/dbus/machine-id"] {
            if let Ok(s) = std::fs::read_to_string(path) {
                let id = s.trim().to_string();
                if !id.is_empty() {
                    return Ok(id);
                }
            }
        }
    }

    Err("No machine identifier available on this platform".to_string())
}
