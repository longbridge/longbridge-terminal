/// Background version update checker.
///
/// On each startup:
/// 1. Read cached latest version from disk → compare with current → print notification.
/// 2. Spawn background task: fetch latest GitHub release tag → update cache (non-blocking).
///
/// Uses the GitHub releases redirect (no API key, avoids rate limits):
/// `GET https://github.com/.../releases/latest` → 302 to `.../releases/tag/vX.Y.Z`
use std::{path::PathBuf, time::Duration};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES_LATEST_URL: &str =
    "https://github.com/longbridge/longbridge-terminal/releases/latest";
const CHECK_INTERVAL_SECS: u64 = 86400; // 24 hours
const FETCH_TIMEOUT_SECS: u64 = 5;

fn cache_file_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".longbridge").join(".terminal-latest-version"))
}

fn read_cached_version() -> Option<String> {
    let path = cache_file_path()?;
    let s = std::fs::read_to_string(&path).ok()?;
    let v = s.trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

fn write_cached_version(version: &str) {
    let Some(path) = cache_file_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, version);
}

/// Returns true if the cache file was written within the last 24 hours.
fn cache_is_fresh() -> bool {
    let Some(path) = cache_file_path() else {
        return false;
    };
    let Ok(meta) = std::fs::metadata(&path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    modified
        .elapsed()
        .map(|d| d.as_secs() < CHECK_INTERVAL_SECS)
        .unwrap_or(false)
}

/// Compare two version strings (e.g., "0.9.0" vs "0.10.0").
/// Returns true if `other` is strictly newer than `current`.
fn is_newer(current: &str, other: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim_start_matches('v')
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect()
    };
    parse(other) > parse(current)
}

/// Read the cached latest version and print a notification to stderr if it is
/// newer than the running binary.  Fast (disk-only, no network).
pub fn notify_if_update_available() {
    let Some(latest) = read_cached_version() else {
        return;
    };
    if is_newer(CURRENT_VERSION, &latest) {
        eprintln!();
        eprintln!("  A new version of longbridge is available: {CURRENT_VERSION} → {latest}");
        eprintln!("  Upgrade:");
        eprintln!("    brew upgrade --cask longbridge/tap/longbridge-terminal");
        eprintln!("    # or: curl -sSL https://github.com/longbridge/longbridge-terminal/raw/main/install | sh");
        eprintln!();
    }
}

/// Spawn a background task that fetches the latest GitHub release tag and
/// updates the on-disk cache.  Skipped if the cache is less than 24 hours old.
pub fn spawn_version_check() {
    if cache_is_fresh() {
        return;
    }
    tokio::spawn(async move {
        if let Some(version) = fetch_latest_version().await {
            tracing::debug!("Latest release from GitHub: {version}");
            write_cached_version(&version);
        } else {
            tracing::debug!("Version check: could not fetch latest release");
        }
    });
}

/// Fetch the latest release version by following the GitHub releases/latest
/// redirect without calling the GitHub API.
async fn fetch_latest_version() -> Option<String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .build()
        .ok()?;

    let resp = client.get(RELEASES_LATEST_URL).send().await.ok()?;

    if !resp.status().is_redirection() {
        return None;
    }

    // Location: /longbridge/longbridge-terminal/releases/tag/v0.9.1
    let location = resp.headers().get("location")?.to_str().ok()?;
    let tag = location.rsplit('/').next()?;
    let version = tag.trim_start_matches('v');

    if version.is_empty() || !version.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }

    Some(version.to_string())
}

const INSTALL_SCRIPT_URL: &str =
    "https://github.com/longbridge/longbridge-terminal/raw/main/install";

/// Download the official install script with reqwest and pipe it to `sh`.
pub async fn cmd_update() -> anyhow::Result<()> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    let script = client
        .get(INSTALL_SCRIPT_URL)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    let mut child = Command::new("sh")
        .stdin(Stdio::piped())
        .spawn()?;

    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(script.as_bytes())?;
    }

    let status = child.wait()?;

    if status.success() {
        // Clear the cached version so the next run re-checks and won't show
        // a stale "update available" notification.
        if let Some(path) = cache_file_path() {
            let _ = std::fs::remove_file(path);
        }
    } else {
        anyhow::bail!(
            "update failed (exit code: {})",
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn test_is_newer() {
        assert!(is_newer("0.9.0", "0.10.0"));
        assert!(is_newer("0.9.0", "0.9.1"));
        assert!(!is_newer("0.9.0", "0.9.0"));
        assert!(!is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("0.9.0", "v0.9.1"));
    }
}
