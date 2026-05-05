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
const CHECK_INTERVAL_SECS: u64 = 86400; // 24 hours
const FETCH_TIMEOUT_SECS: u64 = 5;
const DOWNLOAD_TIMEOUT_SECS: u64 = 300;

const HOST_GLOBAL: &str = "https://open.longbridge.com";
const HOST_CN: &str = "https://open.longbridge.cn";
const RELEASE_PATH: &str = "/github/release/longbridge-terminal";
const RELEASE_NOTES_PATH: &str = "/docs/cli/release-notes.md";

const RELEASES_LATEST_URL: &str =
    "https://github.com/longbridge/longbridge-terminal/releases/latest";
const PACKAGE_NAME: &str = "longbridge-terminal";

#[cfg(target_os = "macos")]
const PLATFORM: &str = "darwin";
#[cfg(target_os = "linux")]
const PLATFORM: &str = "linux";
#[cfg(target_os = "windows")]
const PLATFORM: &str = "windows";

#[cfg(target_os = "linux")]
const LIBC_SUFFIX: &str = "-musl";
#[cfg(not(target_os = "linux"))]
const LIBC_SUFFIX: &str = "";

#[cfg(target_arch = "x86_64")]
const ARCH: &str = "amd64";
#[cfg(target_arch = "aarch64")]
const ARCH: &str = "arm64";

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
        .is_ok_and(|d| d.as_secs() < CHECK_INTERVAL_SECS)
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
        let green = "\x1b[32m";
        let reset = "\x1b[0m";
        let url = release_notes_url();
        eprintln!(
            "\nNew version {latest} is available, run `{green}longbridge update{reset}` to update."
        );
        eprintln!("Release notes: {url}\n");
    }
}

/// Spawn a background task that fetches the latest GitHub release tag and
/// updates the on-disk cache.  Skipped if the cache is less than 24 hours old.
pub fn spawn_version_check() {
    if cache_is_fresh() {
        return;
    }
    // Write a placeholder synchronously before spawning so the mtime is
    // updated immediately.  Fast CLI commands often exit before the async
    // task completes; without this the file would never be written and every
    // startup would trigger a new GitHub request.
    write_cached_version(CURRENT_VERSION);
    tokio::spawn(async move {
        if let Some(version) = fetch_latest_version().await {
            tracing::debug!("Latest release from GitHub: {version}");
            write_cached_version(&version);
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

fn get_host() -> &'static str {
    if crate::region::is_cn_cached() {
        HOST_CN
    } else {
        HOST_GLOBAL
    }
}

async fn fetch_latest_version_for_update() -> anyhow::Result<String> {
    let host = get_host();

    // Both Global and CN CDN: GET {host}{RELEASE_PATH}/latest returns plain text like "v0.15.0"
    let url = format!("{host}{RELEASE_PATH}/latest");
    let resp = reqwest::Client::builder()
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .build()?
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let version = resp.trim().trim_start_matches('v').to_string();

    if version.is_empty() || !version.starts_with(|c: char| c.is_ascii_digit()) {
        anyhow::bail!("Invalid version string: {version}");
    }

    Ok(version)
}

fn build_download_url(base: &str, version: &str) -> String {
    #[cfg(not(target_os = "windows"))]
    let ext = "tar.gz";
    #[cfg(target_os = "windows")]
    let ext = "zip";

    let asset = format!("{PACKAGE_NAME}-{PLATFORM}{LIBC_SUFFIX}-{ARCH}.{ext}");

    // Both Global and CN CDN use the same path structure
    format!("{base}/v{version}/{asset}")
}

async fn download_to_file(url: &str, dest: &std::path::Path) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
        .build()?;

    eprint!("Downloading...");
    let bytes = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    #[allow(clippy::cast_precision_loss)]
    {
        eprintln!(" {:.1}MB", bytes.len() as f64 / 1_048_576.0);
    }

    std::fs::write(dest, &bytes)?;
    Ok(())
}

#[cfg(unix)]
fn sudo_mv(src: &std::path::Path, dest: &std::path::Path) -> anyhow::Result<()> {
    eprintln!("Permission denied — retrying with sudo...");
    let status = std::process::Command::new("sudo")
        .arg("mv")
        .arg(src)
        .arg(dest)
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to run sudo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!(
            "sudo mv failed (exit code: {}).\nYou can also run: sudo longbridge update",
            status.code().unwrap_or(-1)
        )
    }
}

#[cfg(unix)]
fn extract_and_replace(
    archive_path: &std::path::Path,
    target_exe: &std::path::Path,
) -> anyhow::Result<()> {
    use flate2::read::GzDecoder;
    use std::os::unix::fs::PermissionsExt as _;

    let file = std::fs::File::open(archive_path)?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    let tmp_dir = tempfile::tempdir()?;
    archive.unpack(tmp_dir.path())?;

    // The archive contains a single binary named "longbridge"
    let extracted = tmp_dir.path().join("longbridge");
    if !extracted.exists() {
        anyhow::bail!("Binary 'longbridge' not found in archive");
    }

    // Set executable permission
    let perms = std::fs::Permissions::from_mode(0o755);
    std::fs::set_permissions(&extracted, perms)?;

    let target_dir = target_exe.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot determine parent directory of {}",
            target_exe.display()
        )
    })?;

    // Try atomic rename first (works if same filesystem)
    if std::fs::rename(&extracted, target_exe).is_ok() {
        return Ok(());
    }

    // Cross-device fallback:
    // 1) copy the new binary into the target directory under a temporary name
    // 2) atomically rename it over the running executable
    // This avoids writing directly to a running binary (ETXTBUSY).
    let staged_result = tempfile::Builder::new()
        .prefix(".longbridge-update-")
        .tempfile_in(target_dir);

    let staged_path = match staged_result {
        Ok(f) => f.into_temp_path(),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            return sudo_mv(&extracted, target_exe);
        }
        Err(e) => return Err(e.into()),
    };

    if let Err(e) = std::fs::copy(&extracted, &staged_path) {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            drop(staged_path);
            return sudo_mv(&extracted, target_exe);
        }
        return Err(e.into());
    }
    std::fs::set_permissions(&staged_path, std::fs::Permissions::from_mode(0o755))?;

    if let Err(e) = std::fs::rename(&staged_path, target_exe) {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            drop(staged_path);
            return sudo_mv(&extracted, target_exe);
        }
        return Err(e.into());
    }

    Ok(())
}

#[cfg(windows)]
fn extract_and_replace(
    archive_path: &std::path::Path,
    target_exe: &std::path::Path,
) -> anyhow::Result<()> {
    use std::process::Command;

    let tmp_dir = tempfile::tempdir()?;

    // Use PowerShell to extract zip
    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                archive_path.display(),
                tmp_dir.path().display()
            ),
        ])
        .status()?;

    if !status.success() {
        anyhow::bail!(
            "PowerShell Expand-Archive failed (exit code: {})",
            status.code().unwrap_or(-1)
        );
    }

    let extracted = tmp_dir.path().join("longbridge.exe");
    if !extracted.exists() {
        anyhow::bail!("Binary 'longbridge.exe' not found in archive");
    }

    // Rename current running exe to .old (Windows allows renaming a running exe)
    let old_exe = target_exe.with_extension("exe.old");
    let _ = std::fs::remove_file(&old_exe);
    std::fs::rename(target_exe, &old_exe)?;

    // Move new exe into place (copy + remove as fallback for cross-drive)
    if std::fs::rename(&extracted, target_exe).is_err() {
        if let Err(e) = std::fs::copy(&extracted, target_exe) {
            // Attempt to restore the old binary if copy fails
            let _ = std::fs::rename(&old_exe, target_exe);
            return Err(e.into());
        }
        let _ = std::fs::remove_file(&extracted);
    }

    Ok(())
}

/// On Windows, remove the `.old` binary left behind by a previous update.
/// On Unix this is a no-op.
pub fn cleanup_old_binary() {
    #[cfg(windows)]
    {
        let Ok(exe) = std::env::current_exe().and_then(|p| p.canonicalize()) else {
            return;
        };
        let old = exe.with_extension("exe.old");
        if old.exists() {
            let _ = std::fs::remove_file(old);
        }
    }
}

/// Download the latest release and replace the current binary in place.
pub async fn cmd_update(verbose: bool, force: bool) -> anyhow::Result<()> {
    // 1. Resolve current binary path
    let current_exe = std::env::current_exe()?.canonicalize()?;

    if verbose {
        eprintln!("* Binary: {}", current_exe.display());
    }

    // 2. Fetch latest version
    let latest = fetch_latest_version_for_update().await?;

    // 3. Check if already up to date
    if !force && !is_newer(CURRENT_VERSION, &latest) {
        eprintln!("Already up to date (v{CURRENT_VERSION}).");
        return Ok(());
    }

    eprintln!("Updating v{CURRENT_VERSION} → v{latest} ...");

    // 4. Build download URL
    let host = get_host();
    let base = format!("{host}{RELEASE_PATH}");
    let url = build_download_url(&base, &latest);

    if verbose {
        eprintln!("* Download: {url}");
    }

    // 5. Download to temp file
    // Use into_temp_path() to close the file handle — on Windows,
    // NamedTempFile holds a lock that prevents PowerShell from reading it.
    let mut tmp_builder = tempfile::Builder::new();
    tmp_builder.prefix("longbridge-update-");
    #[cfg(windows)]
    tmp_builder.suffix(".zip");
    let tmp_path = tmp_builder.tempfile()?.into_temp_path();
    download_to_file(&url, &tmp_path).await?;

    // 6. Extract and replace binary (platform-specific)
    extract_and_replace(&tmp_path, &current_exe)?;

    // 7. Clear version cache
    if let Some(path) = cache_file_path() {
        let _ = std::fs::remove_file(path);
    }

    eprintln!("Updated to v{latest} successfully.\n");

    // 8. Show release notes and update the last-run marker so the next
    //    startup won't show them again.
    write_last_run_version();
    match fetch_release_notes().await {
        Ok(md) => render_release_notes(&md),
        Err(e) => tracing::debug!("Failed to fetch release notes: {e}"),
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Release notes
// ---------------------------------------------------------------------------

fn last_run_version_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".longbridge").join(".terminal-last-run-version"))
}

fn read_last_run_version() -> Option<String> {
    let path = last_run_version_path()?;
    let s = std::fs::read_to_string(&path).ok()?;
    let v = s.trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

fn write_last_run_version() {
    let Some(path) = last_run_version_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, CURRENT_VERSION);
}

fn release_notes_url() -> String {
    format!("{}{RELEASE_NOTES_PATH}", get_host())
}

/// Strip YAML front matter (leading `---` … `---` block) from markdown.
fn strip_frontmatter(s: &str) -> &str {
    let trimmed = s.trim_start();
    if let Some(rest) = trimmed.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            return rest[end + 4..].trim_start_matches('\n');
        }
    }
    s
}

async fn fetch_release_notes() -> anyhow::Result<String> {
    let url = release_notes_url();
    let body = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(strip_frontmatter(&body).to_string())
}

fn render_release_notes(markdown: &str) {
    use crossterm::{
        event::{self, Event, KeyCode, KeyEventKind},
        terminal,
    };
    use std::io::Write;

    let (term_width, term_height) = terminal::size().unwrap_or((80, 24));
    let page_height = (term_height as usize).saturating_sub(1);

    let skin = termimad::MadSkin::default();
    let rendered = format!("{}", skin.text(markdown, Some(term_width as usize)));
    let lines: Vec<&str> = rendered.lines().collect();

    if lines.len() <= page_height {
        print!("{rendered}");
        return;
    }

    if terminal::enable_raw_mode().is_err() {
        print!("{rendered}");
        return;
    }

    let mut stdout = std::io::stdout();
    let mut pos = 0;

    'outer: loop {
        let end = (pos + page_height).min(lines.len());
        for line in &lines[pos..end] {
            let _ = write!(stdout, "{line}\r\n");
        }
        pos = end;

        if pos >= lines.len() {
            break;
        }

        let _ = write!(
            stdout,
            "\x1b[7m--More-- (Space/Enter: next page, q: quit)\x1b[0m"
        );
        let _ = stdout.flush();

        loop {
            match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        let _ = write!(stdout, "\r\x1b[2K");
                        break;
                    }
                    KeyCode::Char('q' | 'Q') | KeyCode::Esc => {
                        let _ = write!(stdout, "\r\x1b[2K\r\n");
                        break 'outer;
                    }
                    _ => {}
                },
                Err(_) => break 'outer,
                _ => {}
            }
        }
    }

    let _ = terminal::disable_raw_mode();
}

/// Show release notes for the `longbridge update --release-notes` command.
pub async fn cmd_release_notes() -> anyhow::Result<()> {
    let markdown = fetch_release_notes().await?;
    render_release_notes(&markdown);
    Ok(())
}

/// Check if the binary version changed since the last run. If so, print a
/// one-line notice with the release notes URL (no network request).
pub fn check_and_show_release_notes() {
    let last = read_last_run_version();

    // Always update the marker so we only show once.
    write_last_run_version();

    let Some(last) = last else {
        // First-ever run — no previous version recorded, skip.
        return;
    };

    if last == CURRENT_VERSION {
        return;
    }

    let green = "\x1b[32m";
    let reset = "\x1b[0m";
    let url = release_notes_url();
    eprintln!(
        "\n{green}Updated to v{CURRENT_VERSION} (was v{last}). Release notes: {url}{reset}\n"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer() {
        assert!(is_newer("0.9.0", "0.10.0"));
        assert!(is_newer("0.9.0", "0.9.1"));
        assert!(!is_newer("0.9.0", "0.9.0"));
        assert!(!is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("0.9.0", "v0.9.1"));
    }

    #[test]
    fn test_strip_frontmatter() {
        let with_fm = "---\ntitle: Release Notes\nsidebar: 100\n---\n# Heading\nbody";
        assert_eq!(strip_frontmatter(with_fm), "# Heading\nbody");

        let no_fm = "# Heading\nbody";
        assert_eq!(strip_frontmatter(no_fm), no_fm);

        let incomplete = "---\ntitle: x\nno closing";
        assert_eq!(strip_frontmatter(incomplete), incomplete);
    }

    #[test]
    fn test_release_notes_url_respects_region() {
        // Force global
        std::env::set_var("LONGBRIDGE_REGION", "global");
        let url_global = release_notes_url();
        assert_eq!(
            url_global,
            "https://open.longbridge.com/docs/cli/release-notes.md"
        );

        // Force CN
        std::env::set_var("LONGBRIDGE_REGION", "cn");
        let url_cn = release_notes_url();
        assert_eq!(
            url_cn,
            "https://open.longbridge.cn/docs/cli/release-notes.md"
        );

        std::env::remove_var("LONGBRIDGE_REGION");
    }

    #[test]
    fn test_last_run_version_roundtrip() {
        let path = last_run_version_path().expect("home dir should exist");
        let backup = std::fs::read_to_string(&path).ok();

        // Write and read back
        write_last_run_version();
        let v = read_last_run_version().expect("should read back");
        assert_eq!(v, CURRENT_VERSION);

        // Restore original state
        match backup {
            Some(original) => std::fs::write(&path, original).unwrap(),
            None => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}
