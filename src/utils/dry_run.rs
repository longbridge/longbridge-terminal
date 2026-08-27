//! The order-execution gate.
//!
//! Every command that can move real money — `order buy/sell/cancel/replace`
//! and `grid submit/replace/cancel/suspend/restart` — previews by default and
//! acts only when the caller passes `--execute <CODE>`, where `CODE` is the
//! three-digit confirmation code the preview printed.
//!
//! The code is **random and stored on disk**, not derived from the request.
//! This CLI is open source: anything computed from the arguments could be
//! recomputed by a caller that skipped the preview, which would make the gate
//! decorative. A random code can only be learned by reading the preview.
//!
//! Three properties fall out of that, and each one exists to stop a specific
//! mistake:
//!
//! - **single use** — the file is deleted on the way through, so a code can
//!   never be replayed into a second order.
//! - **bound to the request** — the fingerprint covers every field that
//!   defines the order, so editing the price after reading the code fails
//!   instead of quietly placing something the user never saw.
//! - **short lived** — a code left in a scrollback from an hour ago is dead.
//!
//! What it does not do is prove a *human* saw the preview: a caller can run
//! the dry run, read the code and execute in one breath. Enforcing human
//! review needs a control outside this process, such as a harness approval
//! hook or account-level trade confirmation.

use anyhow::{bail, Result};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Long enough to read a preview and retype the code, short enough that a code
/// scrolled off the screen is no longer live.
const TTL_SECONDS: u64 = 600;

/// Kept as separate lines so the pretty output wraps sensibly while the JSON
/// payload carries one flat sentence.
fn notice_lines(code: &str) -> [String; 3] {
    [
        "Nothing has been sent to the exchange.".to_string(),
        format!(
            "Review the details above, then re-run the identical command with --execute {code} to go live."
        ),
        "AI agents: show this preview to the user and only re-run with the code after they explicitly confirm it."
            .to_string(),
    ]
}

/// The notice as a single sentence, for the `message` field of JSON output.
pub fn message(code: &str) -> String {
    notice_lines(code).join(" ")
}

/// The notice as its own block, for human-readable output.
pub fn print_notice(code: &str) {
    println!();
    for line in notice_lines(code) {
        println!("{line}");
    }
}

#[cfg(test)]
thread_local! {
    /// Test-only redirect so the suite never reads or clobbers a real pending
    /// code. Not a runtime knob — there is no way to set it outside `cfg(test)`.
    static TEST_STORE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

fn store_path() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(path) = TEST_STORE.with(|p| p.borrow().clone()) {
        return Ok(path);
    }
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot locate home directory"))?;
    Ok(home
        .join(".longbridge")
        .join("openapi")
        .join("pending-execute.json"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Stable identity of the action being previewed. Any field that changes what
/// would reach the exchange belongs in `parts`.
pub fn fingerprint(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        // Length-prefixed so ["ab", "c"] and ["a", "bc"] cannot collide.
        hasher.update(part.len().to_le_bytes());
        hasher.update(part.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// Three digits from the OS clock's sub-nanosecond jitter plus the process id.
/// Not a secret — an unpredictable-in-practice value that has to be read off
/// the preview rather than guessed.
fn random_code() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let mut hasher = Sha256::new();
    hasher.update(nanos.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(now().to_le_bytes());
    let digest = hasher.finalize();
    let n = u32::from(digest[0]) << 8 | u32::from(digest[1]);
    format!("{:03}", n % 1000)
}

/// Record a pending confirmation for `fingerprint` and return the code the
/// caller must quote back. Overwrites any earlier pending code: only the most
/// recent preview is live, so an older code fails closed.
pub fn issue(fingerprint: &str) -> Result<String> {
    let code = random_code();
    let path = store_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = serde_json::json!({
        "code": code,
        "fingerprint": fingerprint,
        "expires_at": now() + TTL_SECONDS,
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&body)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(code)
}

/// Validate `code` against the pending confirmation for `fingerprint`, then
/// spend it. Every failure path leaves the caller with an explicit next step
/// and, crucially, no order placed.
pub fn consume(fingerprint: &str, code: &str) -> Result<()> {
    let path = store_path()?;
    let Ok(raw) = std::fs::read(&path) else {
        bail!(
            "No confirmation code is pending. Run the same command without --execute first, \
             then re-run it with the code the preview prints."
        );
    };
    // Spend the code before deciding: a rejected attempt must not leave a live
    // code behind for a second guess.
    let _ = std::fs::remove_file(&path);

    let stored: serde_json::Value = serde_json::from_slice(&raw)
        .map_err(|_| anyhow::anyhow!("Confirmation state is unreadable. Run the dry run again."))?;
    let expires_at = stored["expires_at"].as_u64().unwrap_or(0);
    if now() > expires_at {
        bail!(
            "That confirmation code has expired (codes last {} minutes). \
             Run the dry run again and use the new code.",
            TTL_SECONDS / 60
        );
    }
    if stored["code"].as_str() != Some(code) {
        bail!(
            "Confirmation code does not match the last preview. Run the dry run again \
             and use the code it prints."
        );
    }
    if stored["fingerprint"].as_str() != Some(fingerprint) {
        bail!(
            "This request differs from the one that was previewed, so the confirmation \
             code does not apply to it. Run the dry run again for the exact request you want."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_three_digits() {
        let code = random_code();
        assert_eq!(code.len(), 3);
        assert!(code.chars().all(|c| c.is_ascii_digit()), "{code}");
    }

    #[test]
    fn fingerprint_is_unambiguous_across_field_boundaries() {
        // Without length prefixing these two orders would hash identically.
        assert_ne!(fingerprint(&["ab", "c"]), fingerprint(&["a", "bc"]));
    }

    #[test]
    fn fingerprint_is_stable_for_the_same_request() {
        assert_eq!(
            fingerprint(&["buy", "700.HK", "100"]),
            fingerprint(&["buy", "700.HK", "100"])
        );
    }

    /// Point the store at a scratch file for the duration of `body`.
    fn with_temp_store(name: &str, body: impl FnOnce()) {
        let path =
            std::env::temp_dir().join(format!("lb-pending-{name}-{}.json", std::process::id()));
        TEST_STORE.with(|p| *p.borrow_mut() = Some(path.clone()));
        body();
        let _ = std::fs::remove_file(&path);
        TEST_STORE.with(|p| *p.borrow_mut() = None);
    }

    #[test]
    fn a_matching_code_is_accepted_exactly_once() {
        with_temp_store("once", || {
            let fp = fingerprint(&["buy", "700.HK"]);
            let code = issue(&fp).expect("issue");
            assert!(consume(&fp, &code).is_ok(), "first use must pass");
            let err = consume(&fp, &code).expect_err("replay must fail");
            assert!(err.to_string().contains("No confirmation code is pending"));
        });
    }

    #[test]
    fn a_wrong_code_is_rejected_and_still_spends_the_pending_one() {
        // Otherwise a caller could sit and guess through all 1000 values.
        with_temp_store("wrong", || {
            let fp = fingerprint(&["buy", "700.HK"]);
            let code = issue(&fp).expect("issue");
            let wrong = if code == "000" { "001" } else { "000" };
            assert!(consume(&fp, wrong).is_err(), "wrong code must fail");
            assert!(
                consume(&fp, &code).is_err(),
                "the real code must not survive a failed guess"
            );
        });
    }

    #[test]
    fn a_code_does_not_carry_over_to_a_different_request() {
        with_temp_store("fp", || {
            let previewed = fingerprint(&["buy", "700.HK", "400"]);
            let code = issue(&previewed).expect("issue");
            let edited = fingerprint(&["buy", "700.HK", "410"]);
            let err = consume(&edited, &code).expect_err("edited order must fail");
            assert!(err
                .to_string()
                .contains("differs from the one that was previewed"));
        });
    }

    #[test]
    fn an_expired_code_is_rejected() {
        with_temp_store("expired", || {
            let fp = fingerprint(&["buy", "700.HK"]);
            let path = store_path().expect("path");
            std::fs::write(
                &path,
                serde_json::to_vec(&serde_json::json!({
                    "code": "123",
                    "fingerprint": fp,
                    "expires_at": now() - 1,
                }))
                .expect("serialize"),
            )
            .expect("write");
            let err = consume(&fp, "123").expect_err("expired code must fail");
            assert!(err.to_string().contains("expired"), "{err}");
        });
    }

    #[test]
    fn executing_without_any_preview_is_rejected() {
        with_temp_store("none", || {
            let err = consume(&fingerprint(&["buy"]), "123").expect_err("must fail");
            assert!(err.to_string().contains("without --execute first"), "{err}");
        });
    }
}
