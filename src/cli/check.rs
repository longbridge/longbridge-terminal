use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::json;

use super::OutputFormat;
use crate::region;

const CONNECT_TIMEOUT_SECS: u64 = 5;
const PROBE_COUNT: usize = 10;
const GLOBAL_HTTP_URL: &str = "https://openapi.longbridge.com";
const GLOBAL_PROBE_URL: &str = "https://openapi.longbridge.com/health";
const CN_PROBE_URL: &str = "https://openapi.longbridge.cn/health";
const TEST_HTTP_URL: &str = "https://openapi.longbridge.xyz";
const TEST_PROBE_URL: &str = "https://openapi.longbridge.xyz/health";

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

pub async fn cmd_check(format: &OutputFormat) -> Result<()> {
    // ── Region cache ─────────────────────────────────────────────────────────
    let region_cached = dirs::home_dir()
        .map(|h| h.join(".longbridge").join("openapi").join("region-cache"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map_or_else(|| "none".to_string(), |s| s.trim().to_lowercase());
    let is_cn = region_cached == "cn";
    let is_test_env = std::env::var("LONGBRIDGE_TEST_ENV").is_ok();

    // ── Token verification via market temperature API ─────────────────────────
    let token_ok: bool;
    let token_detail: String;

    if let Err(e) = crate::openapi::init_contexts().await {
        token_ok = false;
        token_detail = e.to_string();
    } else {
        let ctx = crate::openapi::quote();
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
    let (global, cn, test) = tokio::join!(
        probe(GLOBAL_PROBE_URL),
        probe(CN_PROBE_URL),
        probe(TEST_PROBE_URL),
    );

    match format {
        OutputFormat::Json => {
            let value = json!({
                "session": {
                    "token": if token_ok { "valid" } else { "invalid" },
                    "detail": token_detail,
                },
                "region": {
                    "cached": region_cached,
                    "active": if is_test_env { "Test" } else if is_cn { "CN" } else { "Global" },
                },
                "connectivity": {
                    "global": { "url": GLOBAL_HTTP_URL, "ok": global.ok, "ms": global.ms },
                    "cn":     { "url": region::HTTP_URL_CN, "ok": cn.ok, "ms": cn.ms },
                    "test":   { "url": TEST_HTTP_URL, "ok": test.ok, "ms": test.ms },
                },
            });
            println!("{}", serde_json::to_string_pretty(&value)?);
        }

        OutputFormat::Table => {
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

            println!("Session");
            println!(
                "  {:<8} {}  {}  {DIM}{}{RESET}",
                "token", token_icon, token_label, token_detail
            );
            let active_env = if is_test_env {
                "Test"
            } else if is_cn {
                "CN"
            } else {
                "Global"
            };
            println!(
                "  {:<8} {}  (active: {})",
                "region", region_cached, active_env
            );

            println!();
            println!("Connectivity {DIM}(avg of {PROBE_COUNT}){RESET}");
            println!("{}", probe_line("global", &global, GLOBAL_HTTP_URL));
            println!("{}", probe_line("cn", &cn, region::HTTP_URL_CN));
            println!("{}", probe_line("test", &test, TEST_HTTP_URL));
        }
    }

    Ok(())
}
