/// Region detection for China Mainland auto-routing.
///
/// On each startup:
/// 1. Read cached region from disk → `is_cn_cached()` for use in Config builder.
/// 2. Spawn background task: check geotest.lbkrs.com → update cache (non-blocking).
use std::{path::PathBuf, time::Duration};

const GEOTEST_URL: &str = "https://geotest.lbkrs.com";
const GEOTEST_TIMEOUT_SECS: u64 = 3;

// The `.com` and `.cn` hosts below are access points (CDN-style routing), not
// separate environments: identical data, identical auth, and a token issued by
// one is accepted by the other. A server response containing the other region's
// host is therefore valid and must not be rewritten client-side.
//
// They differ in one respect: `.com` reaches both data centers, while `.cn` has
// no path to US and can only authorize AP accounts. That restriction is enforced
// server-side (the `.cn` login page does not offer US accounts), so nothing here
// or in `auth` needs to account for it. Always logging in through `.com` is not
// an alternative — China Mainland networks may be unable to reach it, which is
// why `.cn` exists.
//
// Two separate concepts, easily confused:
//   - Access point (`.cn` / `.com`) — this module, network routing.
//   - Data center (`ap` / `us`)     — the `x-dc-region` header, selects the
//     account's data center and determines which US-only APIs are available.
//
// The two are not freely combinable. A credential's prefix (`us_…` / `ap_…`,
// see `longbridge::DcRegion::from_credential`) fixes which access points can
// serve it:
//
//   | Data center | `.com`                             | `.cn` |
//   |-------------|------------------------------------|-------|
//   | `us`        | yes — the only usable access point | no    |
//   | `ap`        | yes                                | yes   |
//
// `.cn` has no path to the US data center. This is a hard constraint, not a
// latency preference: a US token sent to `.cn` still authenticates, and basic
// calls such as `static_info` succeed, but every market-data request comes back
// `301604 no quote access` because `.cn` cannot source US-account quotes. The
// error reads like a missing permission and is not one.
//
// So a US-data-center token must be pinned to the global endpoints regardless
// of where the client sits — see the `token_dc_is_us` guard in
// `openapi::init_contexts`, which keeps US tokens off the CN branch even when
// the cached geotest says China Mainland. AP tokens take the nearer access
// point, since both serve them.

// Global endpoint URLs
pub const HTTP_URL_GLOBAL: &str = "https://openapi.longbridge.com";
pub const QUOTE_WS_URL_GLOBAL: &str = "wss://openapi-quote.longbridge.com/v2";
pub const TRADE_WS_URL_GLOBAL: &str = "wss://openapi-trade.longbridge.com/v2";
pub const OPEN_URL_GLOBAL: &str = "https://open.longbridge.com";

// CN endpoint URLs
pub const HTTP_URL_CN: &str = "https://openapi.longbridge.cn";
pub const QUOTE_WS_URL_CN: &str = "wss://openapi-quote.longbridge.cn/v2";
pub const TRADE_WS_URL_CN: &str = "wss://openapi-trade.longbridge.cn/v2";
pub const OPEN_URL_CN: &str = "https://open.longbridge.cn";

// Test environment URLs (openapi-global.longbridge.xyz). The HTTP host is the
// `-global` gateway, which performs `x-dc-region` data-center routing.
pub const HTTP_URL_TEST: &str = "https://openapi-global.longbridge.xyz";
pub const QUOTE_WS_URL_TEST: &str = "wss://openapi-global-quote.longbridge.xyz/v2";
pub const TRADE_WS_URL_TEST: &str = "wss://openapi-global-trade.longbridge.xyz/v2";

/// Whether the staging environment is active (`LONGBRIDGE_ENV=staging`).
pub fn is_test_env() -> bool {
    std::env::var("LONGBRIDGE_ENV").is_ok_and(|v| v == "staging")
}

/// `OpenAPI` HTTP base URL for the current environment and region.
pub fn http_url() -> &'static str {
    if is_test_env() {
        HTTP_URL_TEST
    } else if is_cn_cached() {
        HTTP_URL_CN
    } else {
        HTTP_URL_GLOBAL
    }
}

/// Developer portal host (`open.longbridge.*`) for the current region:
/// release CDN, docs, and the `/connect` reverse-authorization page.
pub fn open_url() -> &'static str {
    if is_cn_cached() {
        OPEN_URL_CN
    } else {
        OPEN_URL_GLOBAL
    }
}

fn cache_file_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".longbridge").join("openapi").join("region-cache"))
}

/// Returns `true` if the region is known to be CN.
///
/// Priority:
/// 1. `LONGBRIDGE_REGION` env var (explicit override)
/// 2. Cached result from the last background geotest probe
pub fn is_cn_cached() -> bool {
    if let Ok(region) = std::env::var("LONGBRIDGE_REGION") {
        return region.trim().eq_ignore_ascii_case("cn");
    }

    let Some(path) = cache_file_path() else {
        return false;
    };
    match std::fs::read_to_string(&path) {
        Ok(s) => s.trim().eq_ignore_ascii_case("cn"),
        Err(_) => false,
    }
}

fn write_cache(is_cn: bool) {
    let Some(path) = cache_file_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, if is_cn { "cn" } else { "global" });
}

/// Spawn a background task that checks geotest and updates the cache.
/// Does not block the caller. Safe to call after Tokio runtime is active.
pub fn spawn_region_update() {
    tokio::spawn(async move {
        let is_cn = check_geotest().await;
        tracing::debug!(
            "Region check: geotest={}",
            if is_cn { "CN" } else { "global" }
        );
        write_cache(is_cn);
    });
}

/// Returns `true` if geotest.lbkrs.com is reachable (implies China Mainland).
async fn check_geotest() -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(GEOTEST_TIMEOUT_SECS))
        .build()
    else {
        return false;
    };
    client.get(GEOTEST_URL).send().await.is_ok()
}
