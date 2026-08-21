use anyhow::Result;
use serde_json::json;

use super::OutputFormat;
use crate::region::{self, ProbeStats, CONNECT_TIMEOUT_SECS, PROBE_COUNT};

// ANSI colors
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

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
    // ── Region ───────────────────────────────────────────────────────────────
    // Detect rather than read the cache: reporting a stale verdict would defeat
    // the point of a diagnostic, and detecting repairs the cache along the way.
    let geotest_country = region::redetect_region().await;
    let region_override = region::region_override();
    let mut is_cn = region::is_cn_cached();

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
    let (global, cn) = region::probe_access_points().await;

    // ── Repin from measurement ───────────────────────────────────────────────
    // The latency probe measures the thing geolocation only approximates, so a
    // decisive gap wins. Persisted, so subsequent commands follow it too — and
    // recorded either way, so the background probe on the next startup does not
    // immediately repeat what was just measured here.
    let repinned = region::record_measurement(is_cn, &global, &cn);
    if let Some(measured_is_cn) = repinned {
        is_cn = measured_is_cn;
    }

    let region_cached = region::cached_verdict().unwrap_or("none");
    let geotest_label = match (region_override, geotest_country.as_deref()) {
        // Not probed: the override decides regardless of location.
        (Some(_), _) => None,
        (None, Some(country)) => Some(country),
        (None, None) => Some("unreachable"),
    };

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
                    "override": region_override,
                    "geotest": geotest_label,
                    "repinned_by_latency": repinned.is_some(),
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

            println!("Session");
            println!(
                "  {:<8} {}  {}  {DIM}{}{RESET}",
                "token", token_icon, token_label, token_detail
            );
            let region_source = match (region_override, geotest_label) {
                (Some(value), _) => {
                    format!("  {DIM}(pinned by LONGBRIDGE_REGION={value}){RESET}")
                }
                (None, Some(country)) => format!("  {DIM}(geotest: {country}){RESET}"),
                (None, None) => String::new(),
            };
            println!(
                "  {:<8} {}  (active: {}){}",
                "region",
                region_cached,
                if is_cn { "CN" } else { "Global" },
                region_source
            );

            println!();
            println!("Connectivity {DIM}(avg of {PROBE_COUNT}){RESET}");
            println!("{}", probe_line("global", &global, region::HTTP_URL_GLOBAL));
            println!("{}", probe_line("cn", &cn, region::HTTP_URL_CN));
            if let Some(measured_is_cn) = repinned {
                let (winner, loser) = if measured_is_cn {
                    ("cn", "global")
                } else {
                    ("global", "cn")
                };
                println!(
                    "  {YELLOW}→{RESET} repinned to {winner}: measurably better than {loser}, \
                     overriding geotest"
                );
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
            field("session", "object", "Token validity details"),
            field(
                "region",
                "object",
                "Active access point, the geotest country behind it, any \
                 LONGBRIDGE_REGION override, and whether latency repinned it",
            ),
            field(
                "connectivity",
                "object",
                "Global/CN connectivity probe results",
            ),
            field("status", "string", "Compatibility status summary"),
        ],
    })
}
