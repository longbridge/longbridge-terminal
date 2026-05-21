//! OS-native credential store integration for encrypted token storage.
//!
//! Provides best-effort keychain storage on macOS (Keychain) and Windows
//! (Credential Manager). On other platforms the functions are no-ops; storage
//! falls back to the plaintext file required by the longbridge SDK.
//!
//! The plaintext file at `~/.longbridge/openapi/tokens/<client_id>` is always
//! kept in sync because the SDK's `OAuthBuilder` reads that path directly.
//! The credential store provides an additional encrypted copy that:
//!   - is protected by OS-level encryption (Keychain / DPAPI)
//!   - uses `kSecAttrAccessibleWhenUnlocked` on macOS (accessible while the screen
//!     is unlocked; items sync to iCloud Keychain if iCloud Keychain is enabled)
//!   - allows recovering the file if it is accidentally deleted without re-login

#[cfg(any(target_os = "macos", target_os = "windows"))]
const SERVICE: &str = "longbridge";

/// Store `token_json` in the OS credential store under the key `client_id`.
///
/// Silently ignores errors — if the credential store is unavailable, the
/// token continues to live only in the plaintext file.
pub fn try_store(client_id: &str, token_json: &str) {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    store_native(client_id, token_json);

    // On other platforms: suppress unused-variable warnings for function params.
    let _ = (client_id, token_json);
}

/// Load a token JSON string from the OS credential store.
///
/// Returns `None` if the entry does not exist, the credential store is
/// unavailable, or the stored bytes cannot be decoded as UTF-8.
pub fn try_load(client_id: &str) -> Option<String> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return load_native(client_id);

    let _ = client_id;
    None
}

/// Remove the credential store entry for `client_id`.
///
/// Silently succeeds if the entry does not exist or the store is unavailable.
pub fn try_delete(client_id: &str) {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    delete_native(client_id);

    let _ = client_id;
}

/// Harden `path` to user-only read/write permissions (Unix mode 0600).
///
/// No-op on Windows and when the permissions are already correct. Errors are
/// logged at DEBUG level and do not propagate — the file remains accessible.
pub fn harden_file_permissions(path: &std::path::Path) {
    #[cfg(target_family = "unix")]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            // Skip the syscall if permissions are already correct.
            if perms.mode() & 0o077 != 0 {
                perms.set_mode(0o600);
                if let Err(e) = std::fs::set_permissions(path, perms) {
                    tracing::debug!("Could not harden token file permissions: {e}");
                }
            }
        }
    }
}

// ── macOS / Windows implementations ──────────────────────────────────────────

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn store_native(client_id: &str, token_json: &str) {
    match keyring::Entry::new(SERVICE, client_id) {
        Ok(entry) => {
            if let Err(e) = entry.set_password(token_json) {
                tracing::debug!("Credential store write failed: {e}");
            }
        }
        Err(e) => tracing::debug!("Credential store entry creation failed: {e}"),
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn load_native(client_id: &str) -> Option<String> {
    let entry = keyring::Entry::new(SERVICE, client_id).ok()?;
    match entry.get_password() {
        Ok(s) => Some(s),
        Err(keyring::Error::NoEntry) => None,
        Err(e) => {
            tracing::debug!("Credential store read failed: {e}");
            None
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn delete_native(client_id: &str) {
    if let Ok(entry) = keyring::Entry::new(SERVICE, client_id) {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => tracing::debug!("Credential store delete failed: {e}"),
        }
    }
}
