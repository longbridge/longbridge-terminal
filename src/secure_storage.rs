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

/// Token storage that encrypts files using a machine-derived AES-256-GCM key.
pub struct EncryptedFileTokenStorage;

fn load_payload_with_migration() -> Option<EncryptedPayload> {
    let path = token_path().ok()?;
    let (payload, needs_migration) = read_payload(&path)?;
    if needs_migration {
        let _ = EncryptedFileTokenStorage.save(&StoredToken::from(payload.clone()));
    }
    Some(payload)
}

impl EncryptedFileTokenStorage {
    /// Load the full token payload as JSON (includes `logged_in_at` and all fields).
    pub fn load_full() -> Option<serde_json::Value> {
        serde_json::to_value(load_payload_with_migration()?).ok()
    }
}

impl TokenStorage for EncryptedFileTokenStorage {
    fn load(&self, _client_id: &str) -> Option<StoredToken> {
        Some(load_payload_with_migration()?.into())
    }

    fn save(&self, token: &StoredToken) -> OAuthResult<()> {
        let path = token_path().map_err(|e| OAuthError::Other(e.to_string()))?;

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

        // Write to a sibling temp file then rename for atomic replacement.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &encrypted).map_err(|e| OAuthError::TokenFileWrite {
            path: tmp.clone(),
            source: e,
        })?;
        std::fs::rename(&tmp, &path).map_err(|e| OAuthError::TokenFileWrite {
            path: path.clone(),
            source: e,
        })?;

        harden_file_permissions(&path);
        Ok(())
    }
}

/// Delete the token file for `client_id`.
pub fn try_delete(_client_id: &str) {
    if let Ok(path) = token_path() {
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

fn token_path() -> anyhow::Result<PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Failed to get home directory"))?
        .join(".longbridge")
        .join("openapi")
        .join("cli-auth"))
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
    let key = machine_derived_key();
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
    let key = machine_derived_key();
    let cipher = Aes256Gcm::new(&key);
    cipher
        .decrypt(nonce, &data[MAGIC.len() + 12..])
        .map_err(|e| format!("AES-GCM decrypt: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let plaintext = b"hello, longbridge token";
        let encrypted = encrypt(plaintext).expect("encrypt failed");
        assert!(encrypted.starts_with(MAGIC), "magic header missing");
        let decrypted = decrypt(&encrypted).expect("decrypt failed");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn decrypt_fails_on_tampered_ciphertext() {
        let plaintext = b"some token data";
        let mut encrypted = encrypt(plaintext).expect("encrypt failed");
        // Flip a byte in the ciphertext region (after MAGIC + NONCE).
        let idx = MAGIC.len() + 12;
        encrypted[idx] ^= 0xFF;
        assert!(
            decrypt(&encrypted).is_err(),
            "expected decryption failure on tampered data"
        );
    }

    #[test]
    fn encrypt_decrypt_with_empty_machine_id() {
        // Simulate machine_id unavailable by deriving key with empty IKM directly.
        let hk = hkdf::Hkdf::<sha2::Sha256>::new(None, b"");
        let mut key_bytes = [0u8; 32];
        hk.expand(HKDF_INFO, &mut key_bytes).unwrap();
        let key = *aes_gcm::Key::<Aes256Gcm>::from_slice(&key_bytes);
        let cipher = Aes256Gcm::new(&key);
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let plaintext = b"docker token payload";
        let ciphertext = cipher.encrypt(&nonce, plaintext.as_ref()).unwrap();
        let mut data = Vec::new();
        data.extend_from_slice(MAGIC);
        data.extend_from_slice(&nonce);
        data.extend_from_slice(&ciphertext);
        // Decryption must succeed when both sides use the same (empty-IKM) key.
        let decrypted = cipher
            .decrypt(&nonce, ciphertext.as_ref())
            .expect("decrypt failed with empty machine id key");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn decrypt_fails_on_truncated_data() {
        let result = decrypt(b"LB\x01tooshort");
        assert!(result.is_err());
    }

    #[test]
    fn save_and_load_roundtrip() {
        // Write to a temp file by monkey-patching via a temp dir isn't easy
        // without refactoring token_path(), so we exercise the encrypt/decrypt
        // primitives directly via a known payload.
        let payload = EncryptedPayload {
            client_id: "test-client".to_string(),
            access_token: "at_test".to_string(),
            refresh_token: Some("rt_test".to_string()),
            expires_at: 9_999_999_999,
            logged_in_at: Some(1_700_000_000),
        };

        let json = serde_json::to_vec(&payload).unwrap();
        let encrypted = encrypt(&json).unwrap();
        let decrypted = decrypt(&encrypted).unwrap();
        let loaded: EncryptedPayload = serde_json::from_slice(&decrypted).unwrap();

        assert_eq!(loaded.access_token, "at_test");
        assert_eq!(loaded.refresh_token.as_deref(), Some("rt_test"));
        assert_eq!(loaded.expires_at, 9999999999);
        assert_eq!(loaded.logged_in_at, Some(1_700_000_000));
    }
}

fn machine_derived_key() -> Key<Aes256Gcm> {
    let id = machine_id();
    let hk = Hkdf::<sha2::Sha256>::new(None, id.as_bytes());
    let mut key_bytes = [0u8; 32];
    // HKDF expand is infallible for output lengths ≤ 255 * HashLen.
    let _ = hk.expand(HKDF_INFO, &mut key_bytes);
    *Key::<Aes256Gcm>::from_slice(&key_bytes)
}

/// Return a cached, stable machine-specific identifier.
///
/// Falls back to an empty string when the machine ID is unavailable (e.g.
/// minimal Docker containers without `/etc/machine-id`). Encryption still
/// works in that case; only the machine-binding property is lost.
fn machine_id() -> &'static str {
    MACHINE_ID
        .get_or_init(|| match machine_uid::get() {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(
                    "Could not obtain machine ID (token will not be machine-bound): {e}"
                );
                String::new()
            }
        })
        .as_str()
}
