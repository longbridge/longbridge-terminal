use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde_json::json;

use super::OutputFormat;
use crate::region;

const CONNECT_TIMEOUT_SECS: u64 = 5;
const PROBE_COUNT: usize = 10;

// ANSI colors
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

struct ProbeStats {
    ok: bool,
    ms: u64,
}

/// Measures HTTPS warm-connection latency with `PROBE_COUNT` requests.
/// Sends one warm-up request first to establish the connection, then
/// drops the fastest and slowest sample from the measured runs and averages the rest.
async fn probe(url: &str) -> ProbeStats {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .build()
    else {
        return ProbeStats { ok: false, ms: 0 };
    };
    // Warm-up: establish connection, result not counted
    if client.get(url).send().await.is_err() {
        return ProbeStats { ok: false, ms: 0 };
    }
    let mut samples = Vec::with_capacity(PROBE_COUNT);
    for _ in 0..PROBE_COUNT {
        let start = Instant::now();
        match client.get(url).send().await {
            Ok(resp) => {
                let body = resp.text().await.unwrap_or_default();
                if body.trim() != "success" {
                    return ProbeStats { ok: false, ms: 0 };
                }
            }
            Err(_) => return ProbeStats { ok: false, ms: 0 },
        }
        samples.push(start.elapsed().as_millis() as u64);
    }
    samples.sort_unstable();
    let trimmed = &samples[1..samples.len() - 1];
    let ms = trimmed.iter().sum::<u64>() / trimmed.len() as u64;
    ProbeStats { ok: true, ms }
}

fn latency_colored(ms: u64) -> String {
    let color = if ms < 100 {
        GREEN
    } else if ms < 500 {
        YELLOW
    } else {
        RED
    };
    format!("{color}{ms}ms{RESET}")
}

fn probe_line(label: &str, r: &ProbeStats, url: &str) -> String {
    let (icon, status) = if r.ok {
        (format!("{GREEN}OK{RESET}"), latency_colored(r.ms))
    } else {
        (
            format!("{RED}FAIL{RESET}"),
            format!("{RED}timeout (>{CONNECT_TIMEOUT_SECS}s){RESET}"),
        )
    };
    format!("  {label:<8} {icon}  {status:<10}  {DIM}{url}{RESET}")
}

/// Inner logic for expiry formatting, takes `now` as a parameter for testability.
fn format_expiry(expires_at: u64, now: u64) -> String {
    if expires_at == 0 {
        return String::new();
    }
    if expires_at <= now {
        format!("{RED}expired{RESET}")
    } else {
        let secs = expires_at - now;
        let days = secs / 86_400;
        if days > 0 {
            format!("{DIM}exp in {days}d{RESET}")
        } else {
            let hours = secs / 3_600;
            format!("{YELLOW}exp in {hours}h{RESET}")
        }
    }
}

/// Returns a colored human-readable expiry string, or an empty string if expiry is unknown.
fn token_expiry_str(expires_at: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format_expiry(expires_at, now)
}

pub async fn cmd_check(format: &OutputFormat) -> Result<()> {
    // ── Token expiry (from local store, before init) ──────────────────────────
    let client_id = crate::auth::effective_client_id();
    let expires_at = crate::secure_storage::EncryptedFileTokenStorage::load_full(&client_id)
        .and_then(|v| v["expires_at"].as_u64())
        .unwrap_or(0);

    // ── Region cache ─────────────────────────────────────────────────────────
    let region_cached = dirs::home_dir()
        .map(|h| h.join(".longbridge").join("openapi").join("region-cache"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map_or_else(|| "none".to_string(), |s| s.trim().to_lowercase());
    let is_cn = region::is_cn_cached();

    // ── Token verification via market temperature API ─────────────────────────
    let token_ok: bool;
    let token_detail: String;

    if let Err(e) = crate::openapi::init_contexts().await {
        token_ok = false;
        token_detail = e.to_string();
    } else {
        let ctx = crate::openapi::quote_cmd();
        match ctx.market_temperature(longbridge::Market::HK).await {
            Ok(temp) => {
                token_ok = true;
                token_detail = format!(
                    "market temp HK: {} ({})",
                    temp.temperature, temp.description
                );
            }
            Err(e) => {
                token_ok = true;
                token_detail = format!("api error: {e}");
            }
        }
    }

    // ── Connectivity (concurrent) ─────────────────────────────────────────────
    let global_probe_url = format!("{}/health", region::HTTP_URL_GLOBAL);
    let cn_probe_url = format!("{}/health", region::HTTP_URL_CN);
    let (global, cn) = tokio::join!(probe(&global_probe_url), probe(&cn_probe_url),);

    match format {
        OutputFormat::Json => {
            let connectivity_ok = global.ok && cn.ok;
            let status = if token_ok && connectivity_ok {
                "ok"
            } else if token_ok {
                "warn"
            } else {
                "fail"
            };
            let value = json!({
                "status": status,
                "session": {
                    "token": if token_ok { "valid" } else { "invalid" },
                    "detail": token_detail,
                    "expires_at": expires_at,
                },
                "region": {
                    "cached": region_cached,
                    "active": if is_cn { "CN" } else { "Global" },
                },
                "connectivity": {
                    "global": { "url": region::HTTP_URL_GLOBAL, "ok": global.ok, "ms": global.ms },
                    "cn":     { "url": region::HTTP_URL_CN, "ok": cn.ok, "ms": cn.ms },
                },
            });
            println!("{}", serde_json::to_string_pretty(&value)?);
        }

        OutputFormat::Pretty => {
            let token_icon = if token_ok {
                format!("{GREEN}OK{RESET}")
            } else {
                format!("{RED}FAIL{RESET}")
            };
            let token_label = if token_ok {
                format!("{GREEN}valid{RESET}")
            } else {
                format!("{RED}invalid{RESET}")
            };
            let expiry = token_expiry_str(expires_at);

            println!("Session");
            if expiry.is_empty() {
                println!(
                    "  {:<8} {}  {}  {DIM}{}{RESET}",
                    "token", token_icon, token_label, token_detail
                );
            } else {
                println!(
                    "  {:<8} {}  {}  {}  {DIM}{}{RESET}",
                    "token", token_icon, token_label, expiry, token_detail
                );
            }
            println!(
                "  {:<8} {}  (active: {})",
                "region",
                region_cached,
                if is_cn { "CN" } else { "Global" }
            );

            println!();
            println!("Connectivity {DIM}(avg of {PROBE_COUNT}){RESET}");
            println!("{}", probe_line("global", &global, region::HTTP_URL_GLOBAL));
            println!("{}", probe_line("cn", &cn, region::HTTP_URL_CN));

            // Summary line
            let passed = [token_ok, global.ok, cn.ok]
                .into_iter()
                .filter(|&b| b)
                .count();
            let total = 3_usize;
            let summary_color = if passed == total {
                GREEN
            } else if passed == 0 {
                RED
            } else {
                YELLOW
            };
            println!();
            if passed == total {
                println!("{summary_color}All {total} checks passed{RESET}");
            } else {
                println!("{summary_color}{passed}/{total} checks passed{RESET}");
            }
        }
    }

    Ok(())
}

pub(crate) fn schema_for_path(path: &[String]) -> Option<super::schema::ResponseSchema> {
    use super::schema::{field, ResponseSchema, RootKind};

    (path == ["check"]).then(|| ResponseSchema {
        summary: "Check token validity, and API connectivity".to_string(),
        root: RootKind::Object,
        fields: vec![
            field("status", "string", "Overall status: ok | warn | fail"),
            field("session", "object", "Token validity and expiry details"),
            field("region", "object", "Cached and active region details"),
            field(
                "connectivity",
                "object",
                "Global/CN connectivity probe results",
            ),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_zero_returns_empty() {
        assert!(format_expiry(0, 1_000_000).is_empty());
    }

    #[test]
    fn expiry_in_past_returns_expired() {
        let result = format_expiry(1_000, 2_000);
        assert!(result.contains("expired"));
    }

    #[test]
    fn expiry_at_exactly_now_returns_expired() {
        let now = 1_000_000_u64;
        let result = format_expiry(now, now);
        assert!(result.contains("expired"));
    }

    #[test]
    fn expiry_in_hours_shows_hours() {
        let now = 1_000_000_u64;
        let expires_at = now + 3 * 3_600; // 3 hours from now
        let result = format_expiry(expires_at, now);
        assert!(result.contains("exp in 3h"));
    }

    #[test]
    fn expiry_in_days_shows_days() {
        let now = 1_000_000_u64;
        let expires_at = now + 42 * 86_400; // 42 days from now
        let result = format_expiry(expires_at, now);
        assert!(result.contains("exp in 42d"));
    }

    #[test]
    fn expiry_one_second_away_shows_zero_hours() {
        let now = 1_000_000_u64;
        let result = format_expiry(now + 1, now);
        assert!(result.contains("exp in 0h"));
    }
}
