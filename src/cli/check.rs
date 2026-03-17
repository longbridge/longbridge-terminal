use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::json;

use super::OutputFormat;
use crate::region;

const CONNECT_TIMEOUT_SECS: u64 = 5;
const GLOBAL_HTTP_URL: &str = "https://openapi.longbridge.com";

// ANSI colors
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

struct ProbeResult {
    ok: bool,
    latency_ms: u64,
}

async fn probe(url: &str) -> ProbeResult {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .build()
        .expect("reqwest client");
    let start = Instant::now();
    let ok = client.get(url).send().await.is_ok();
    ProbeResult {
        ok,
        latency_ms: start.elapsed().as_millis() as u64,
    }
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

fn probe_line(label: &str, r: &ProbeResult, url: &str) -> String {
    let (icon, status) = if r.ok {
        (format!("{GREEN}OK{RESET}"), latency_colored(r.latency_ms))
    } else {
        (
            format!("{RED}FAIL{RESET}"),
            format!("{RED}timeout (>{}s){RESET}", CONNECT_TIMEOUT_SECS),
        )
    };
    format!("  {label:<8} {icon}  {status:<28}  {DIM}{url}{RESET}")
}

pub async fn cmd_check(format: &OutputFormat) -> Result<()> {
    // ── Region cache ─────────────────────────────────────────────────────────
    let region_cached = dirs::home_dir()
        .map(|h| h.join(".longbridge-openapi").join("region-cache"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_lowercase())
        .unwrap_or_else(|| "none".to_string());
    let is_cn = region_cached == "cn";

    // ── Token verification via market temperature API ─────────────────────────
    let token_ok: bool;
    let token_detail: String;

    match crate::openapi::init_contexts().await {
        Err(e) => {
            token_ok = false;
            token_detail = e.to_string();
        }
        Ok(_) => {
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
    }

    // ── Connectivity (concurrent) ─────────────────────────────────────────────
    let (global, cn) = tokio::join!(probe(GLOBAL_HTTP_URL), probe(region::HTTP_URL_CN),);

    match format {
        OutputFormat::Json => {
            let value = json!({
                "session": {
                    "token": if token_ok { "valid" } else { "invalid" },
                    "detail": token_detail,
                },
                "region": {
                    "cached": region_cached,
                    "active": if is_cn { "CN" } else { "Global" },
                },
                "connectivity": {
                    "global": { "url": GLOBAL_HTTP_URL, "ok": global.ok, "latency_ms": global.latency_ms },
                    "cn":     { "url": region::HTTP_URL_CN, "ok": cn.ok, "latency_ms": cn.latency_ms },
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
            println!(
                "  {:<8} {}  (active: {})",
                "region",
                region_cached,
                if is_cn { "CN" } else { "Global" }
            );

            println!();
            println!("Connectivity");
            println!("{}", probe_line("global", &global, GLOBAL_HTTP_URL));
            println!("{}", probe_line("cn", &cn, region::HTTP_URL_CN));
        }
    }

    Ok(())
}
