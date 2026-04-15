use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde_json::json;

use super::OutputFormat;

const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

/// Format a duration in seconds as a human-readable string (e.g. "2h 14m", "45m", "30s").
fn format_duration(secs: u64) -> String {
    if secs >= 3600 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{h}h")
        } else {
            format!("{h}h {m}m")
        }
    } else if secs >= 60 {
        let m = secs / 60;
        let s = secs % 60;
        if s == 0 {
            format!("{m}m")
        } else {
            format!("{m}m {s}s")
        }
    } else {
        format!("{secs}s")
    }
}

pub fn cmd_auth_status(format: &OutputFormat) -> Result<()> {
    let token_path = crate::auth::token_file_path()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    enum TokenState {
        NotFound,
        Valid { expires_at: u64, secs_left: u64 },
        Expired { expires_at: u64, secs_ago: u64 },
        Present { expires_at: u64 }, // file exists but expires_at is 0
    }

    let state = if token_path.exists() {
        let contents = std::fs::read_to_string(&token_path)?;
        let data: serde_json::Value = serde_json::from_str(&contents)?;
        let expires_at = data["expires_at"].as_u64().unwrap_or(0);
        if expires_at == 0 {
            TokenState::Present { expires_at }
        } else if expires_at > now {
            TokenState::Valid {
                expires_at,
                secs_left: expires_at - now,
            }
        } else {
            TokenState::Expired {
                expires_at,
                secs_ago: now - expires_at,
            }
        }
    } else {
        TokenState::NotFound
    };

    let path_str = token_path.display().to_string();

    match format {
        OutputFormat::Json => {
            let value = match &state {
                TokenState::NotFound => json!({
                    "status": "not_found",
                    "path": path_str,
                }),
                TokenState::Valid { expires_at, secs_left } => json!({
                    "status": "valid",
                    "expires_at": expires_at,
                    "expires_in_secs": secs_left,
                    "path": path_str,
                }),
                TokenState::Expired { expires_at, secs_ago } => json!({
                    "status": "expired",
                    "expires_at": expires_at,
                    "expired_secs_ago": secs_ago,
                    "path": path_str,
                }),
                TokenState::Present { expires_at } => json!({
                    "status": "present",
                    "expires_at": expires_at,
                    "path": path_str,
                }),
            };
            println!("{}", serde_json::to_string_pretty(&value)?);
        }

        OutputFormat::Pretty => {
            println!("Token");
            let (icon, label, detail) = match &state {
                TokenState::NotFound => (
                    format!("{RED}✗{RESET}"),
                    format!("{RED}not found{RESET}"),
                    format!("run {DIM}longbridge auth login{RESET} to authenticate"),
                ),
                TokenState::Valid { secs_left, .. } => (
                    format!("{GREEN}✓{RESET}"),
                    format!("{GREEN}valid{RESET}"),
                    format!("expires in {}", format_duration(*secs_left)),
                ),
                TokenState::Expired { secs_ago, .. } => (
                    format!("{YELLOW}!{RESET}"),
                    format!("{YELLOW}expired{RESET}"),
                    format!(
                        "{} ago — run {DIM}longbridge auth login{RESET} to re-authenticate",
                        format_duration(*secs_ago)
                    ),
                ),
                TokenState::Present { .. } => (
                    format!("{GREEN}✓{RESET}"),
                    format!("{GREEN}present{RESET}"),
                    String::new(),
                ),
            };
            println!("  {icon}  {label}  {detail}");
            println!("  {DIM}{path_str}{RESET}");
        }
    }

    Ok(())
}
