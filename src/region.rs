/// Region detection for China Mainland auto-routing.
///
/// On each startup:
/// 1. `refresh_region_cache()` re-probes geotest.lbkrs.com if the cached
///    verdict is older than `CACHE_TTL_SECS`, and persists the result.
/// 2. `is_cn_cached()` reads that verdict from disk for use in the Config
///    builder.
use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const GEOTEST_URL: &str = "https://geotest.lbkrs.com";
const GEOTEST_TIMEOUT_SECS: u64 = 2;

/// How long a probed verdict is trusted before it is re-checked. Long enough
/// that the probe is invisible in day-to-day use, short enough that a laptop
/// carried across the border re-routes itself the same day.
const CACHE_TTL_SECS: u64 = 6 * 60 * 60;

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

/// A previously probed region verdict, as stored on disk.
struct CachedRegion {
    is_cn: bool,
    /// Unix seconds of the probe that produced `is_cn`. `None` for caches
    /// written by an older version, which had no timestamp; those are treated
    /// as stale so the first run after upgrading re-probes.
    checked_at: Option<u64>,
}

impl CachedRegion {
    fn is_fresh(&self) -> bool {
        let Some(checked_at) = self.checked_at else {
            return false;
        };
        now_unix().is_some_and(|now| now.saturating_sub(checked_at) < CACHE_TTL_SECS)
    }
}

fn now_unix() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// Parse the on-disk cache: a verdict line, optionally followed by a line
/// holding the Unix timestamp of the probe.
fn parse_cache(raw: &str) -> CachedRegion {
    let mut lines = raw.lines();
    let is_cn = lines
        .next()
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("cn"));
    let checked_at = lines.next().and_then(|ts| ts.trim().parse::<u64>().ok());
    CachedRegion { is_cn, checked_at }
}

fn read_cache() -> Option<CachedRegion> {
    let path = cache_file_path()?;
    let raw = std::fs::read_to_string(path).ok()?;
    Some(parse_cache(&raw))
}

/// The cached region verdict (`"cn"` / `"global"`), or `None` when no cache
/// file exists yet. Reports what is on disk, ignoring `LONGBRIDGE_REGION`.
pub fn cached_verdict() -> Option<&'static str> {
    read_cache().map(|c| if c.is_cn { "cn" } else { "global" })
}

/// Returns `true` if the region is known to be CN.
///
/// Priority:
/// 1. `LONGBRIDGE_REGION` env var (explicit override)
/// 2. Cached result from the last geotest probe
pub fn is_cn_cached() -> bool {
    if let Ok(region) = std::env::var("LONGBRIDGE_REGION") {
        return region.trim().eq_ignore_ascii_case("cn");
    }

    read_cache().is_some_and(|c| c.is_cn)
}

fn write_cache(is_cn: bool) {
    let Some(path) = cache_file_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let verdict = if is_cn { "cn" } else { "global" };
    let contents = match now_unix() {
        Some(ts) => format!("{verdict}\n{ts}\n"),
        None => format!("{verdict}\n"),
    };
    let _ = std::fs::write(&path, contents);
}

/// Re-probe the region when the cached verdict has gone stale, and persist the
/// result.
///
/// This awaits the probe rather than spawning it. A CLI command usually
/// finishes in a few hundred milliseconds, so a detached background task is
/// routinely dropped before the probe returns — which left a stale verdict on
/// disk indefinitely, e.g. keeping a user pinned to the China Mainland access
/// point long after they had left. Only the first command after the TTL
/// expires pays the probe latency.
pub async fn refresh_region_cache() {
    // An explicit override wins; no point spending a request on the probe.
    if std::env::var("LONGBRIDGE_REGION").is_ok() {
        return;
    }

    let cached = read_cache();
    if cached.as_ref().is_some_and(CachedRegion::is_fresh) {
        return;
    }

    let is_cn = if let Some(is_cn) = check_geotest().await {
        tracing::debug!(
            "Region check: geotest={}",
            if is_cn { "CN" } else { "global" }
        );
        is_cn
    } else {
        // Inconclusive (unreachable, non-2xx, or unparsable body). Keep the
        // previous verdict but still refresh the timestamp, so a broken
        // network does not make every command pay the probe timeout.
        let fallback = cached.is_some_and(|c| c.is_cn);
        tracing::debug!(
            "Region check: geotest inconclusive, keeping {}",
            if fallback { "CN" } else { "global" }
        );
        fallback
    };
    write_cache(is_cn);
}

/// Probe geotest for the caller's country. `None` when the answer is unknown.
///
/// `geotest.lbkrs.com` is served by a global CDN and echoes `<ip>,<country>`
/// (e.g. `1.2.3.4,CN`). It is reachable from outside China Mainland, so
/// reachability says nothing about location — the country code in the body is
/// the actual signal, and treating a successful response as "CN" misroutes
/// overseas users to the China Mainland access point.
///
/// The probe deliberately honours the ambient proxy environment: if all traffic
/// exits through a China Mainland proxy, then so will the API requests, and the
/// `.cn` access point is the right one for them.
async fn check_geotest() -> Option<bool> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(GEOTEST_TIMEOUT_SECS))
        .build()
        .ok()?;
    let resp = client.get(GEOTEST_URL).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.text().await.ok()?;
    Some(parse_geotest_country(&body)?.eq_ignore_ascii_case("CN"))
}

/// Extract the ISO country code from a geotest body of the form `<ip>,<country>`.
fn parse_geotest_country(body: &str) -> Option<&str> {
    let code = body.trim().rsplit(',').next()?.trim();
    let looks_like_country_code =
        (2..=3).contains(&code.len()) && code.chars().all(|c| c.is_ascii_alphabetic());
    looks_like_country_code.then_some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_country_code_from_geotest_body() {
        assert_eq!(parse_geotest_country("111.9.52.4,CN"), Some("CN"));
        assert_eq!(parse_geotest_country("1.2.3.4,US\n"), Some("US"));
        assert_eq!(parse_geotest_country("2001:db8::1,HK"), Some("HK"));
    }

    #[test]
    fn rejects_bodies_without_a_country_code() {
        // A CDN error page or a captive portal must not be read as a verdict.
        assert_eq!(parse_geotest_country("<html>403</html>"), None);
        assert_eq!(parse_geotest_country("111.9.52.4,"), None);
        assert_eq!(parse_geotest_country(""), None);
    }

    #[test]
    fn parses_cache_with_and_without_timestamp() {
        let with_ts = parse_cache("cn\n1750000000\n");
        assert!(with_ts.is_cn);
        assert_eq!(with_ts.checked_at, Some(1_750_000_000));

        // Legacy format, written before the cache carried a timestamp.
        let legacy = parse_cache("cn");
        assert!(legacy.is_cn);
        assert_eq!(legacy.checked_at, None);
        assert!(!legacy.is_fresh());

        assert!(!parse_cache("global\n1750000000\n").is_cn);
    }
}
