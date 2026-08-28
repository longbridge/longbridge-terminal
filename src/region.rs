/// Region detection for China Mainland auto-routing.
///
/// On each startup:
/// 1. `refresh_region_cache()` re-probes geotest.lbkrs.com if the cached
///    verdict is older than `CACHE_TTL_SECS`, and persists the result.
/// 2. `spawn_latency_repin()` measures both access points' quote hosts in the
///    background and repins to the better one, so a wrong verdict repairs
///    itself.
/// 3. `is_cn_cached()` reads that verdict from disk for use in the Config
///    builder.
///
/// Geolocation only approximates "which access point serves you better"; the
/// latency probe measures it. That is why step 2 exists and can overrule step 1.
use std::{
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const GEOTEST_URL: &str = "https://geotest.lbkrs.com";
const GEOTEST_TIMEOUT_SECS: u64 = 2;

/// Timeout for one access-point probe request.
pub const CONNECT_TIMEOUT_SECS: u64 = 5;

/// How long the background probe holds off before its first request, so the
/// command's own startup has the network and the runtime to itself.
const PROBE_START_DELAY: Duration = Duration::from_secs(3);

/// Probe requests per access point, averaged after dropping the fastest and
/// slowest. Enough samples that one stalled request cannot decide a repin.
pub const PROBE_COUNT: usize = 10;

/// How much faster the other access point must measure before its latency
/// overrides the geo verdict — both an absolute and a relative margin.
///
/// Geolocation is only a proxy for "which access point serves you better"; the
/// probe measures that directly. But a single sample is noisy, and a
/// split-tunnel proxy can route geotest and the API over entirely different
/// paths, so only a wide, unambiguous gap should repin. Sized from observed
/// data: a same-continent 41ms / 20% edge is noise; a split-tunnel 135ms / 42%
/// gap is not.
const REPIN_MIN_DELTA_MS: u64 = 50;

/// One access point's measured reachability and warm-connection latency.
pub struct ProbeStats {
    pub ok: bool,
    pub ms: u64,
}

/// Whether measured latency should override the geo verdict, and which way.
///
/// `None` leaves the verdict alone.
fn repin_from_latency(is_cn: bool, global: &ProbeStats, cn: &ProbeStats) -> Option<bool> {
    let (active, other) = if is_cn { (cn, global) } else { (global, cn) };

    // An unreachable access point is never the right one — switch whenever the
    // alternative works. This is the case that stranded overseas clients.
    if !active.ok {
        return other.ok.then_some(!is_cn);
    }
    if !other.ok {
        return None;
    }

    let faster_by = active.ms.checked_sub(other.ms)?;
    // `other.ms * 4 <= active.ms * 3` is "at least 25% faster" in integers.
    let decisive = faster_by >= REPIN_MIN_DELTA_MS && other.ms * 4 <= active.ms * 3;
    decisive.then_some(!is_cn)
}

/// Measures HTTPS warm-connection latency with `PROBE_COUNT` requests.
/// Sends one warm-up request first to establish the connection, then
/// drops the fastest and slowest sample from the measured runs and averages the rest.
///
/// A reply — any reply the server itself wrote — is what proves the access point
/// reachable and times the path to it. The quote host answers a plain GET with a
/// 400 and a WebSocket handshake complaint, and that is a healthy answer from
/// it; only a transport failure or a timeout means unreachable.
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
                // Drain the body so the timing covers the whole exchange and the
                // connection stays reusable for the next sample.
                let _ = resp.bytes().await;
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

/// Measure both access points at once. The pair is what a repin decision needs:
/// either one alone says nothing about which is better.
///
/// The quote host is what gets measured, not `openapi.<tld>/health`, because it
/// is the host the reader waits on: every price, chart and trade list crosses
/// it, one request at a time. The two are not interchangeable, and on a
/// connection where they disagree the health endpoint is the one that is wrong —
/// measured from Shanghai, `openapi.longbridge.cn/health` answers in 58ms
/// against 120ms for `.com` while the quote host it is standing in for is the
/// other way round, 370ms against 215ms, and a `kline` command takes 2.5s on the
/// access point the health check preferred against 0.85s on the other.
pub async fn probe_access_points() -> (ProbeStats, ProbeStats) {
    let (global, cn) = (probe_url(QUOTE_WS_URL_GLOBAL), probe_url(QUOTE_WS_URL_CN));
    tokio::join!(probe(&global), probe(&cn))
}

/// The quote endpoint as something `reqwest` can GET: same host, same port,
/// plain HTTP scheme.
fn probe_url(ws_url: &str) -> String {
    match ws_url.split_once("://") {
        Some(("wss", rest)) => format!("https://{rest}"),
        Some(("ws", rest)) => format!("http://{rest}"),
        _ => ws_url.to_string(),
    }
}

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
    /// Unix seconds of the last access-point latency measurement, which paces
    /// itself separately from the geotest verdict above: it is far more
    /// expensive, and it is the one that can overrule that verdict.
    latency_checked_at: Option<u64>,
}

impl CachedRegion {
    fn is_fresh(&self) -> bool {
        fresh(self.checked_at)
    }
}

/// Whether a stamp is recent enough to trust. A missing stamp is stale, so a
/// cache from a version that did not write one re-probes once.
fn fresh(checked_at: Option<u64>) -> bool {
    let Some(checked_at) = checked_at else {
        return false;
    };
    now_unix().is_some_and(|now| now.saturating_sub(checked_at) < CACHE_TTL_SECS)
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
    let mut stamp = || lines.next().and_then(|ts| ts.trim().parse::<u64>().ok());
    CachedRegion {
        is_cn,
        checked_at: stamp(),
        // Absent in caches written before the latency probe existed, which
        // reads as "never measured" — exactly right for those.
        latency_checked_at: stamp(),
    }
}

fn read_cache() -> Option<CachedRegion> {
    let path = cache_file_path()?;
    let raw = std::fs::read_to_string(path).ok()?;
    Some(parse_cache(&raw))
}

/// The latency stamp on disk, or `None` when the access points have never been
/// measured.
fn cached_latency_checked_at() -> Option<u64> {
    read_cache().and_then(|c| c.latency_checked_at)
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

/// Persist a verdict plus the latency stamp to carry forward.
///
/// The stamp is passed in rather than read here: a geotest refresh must keep
/// whatever the last measurement wrote, while a measurement replaces it.
fn write_cache(is_cn: bool, latency_checked_at: Option<u64>) {
    let Some(path) = cache_file_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let verdict = if is_cn { "cn" } else { "global" };
    // The latency stamp is the third line, so it can only be written when the
    // second one is there to hold its place.
    let contents = match (now_unix(), latency_checked_at) {
        (Some(ts), Some(latency)) => format!("{verdict}\n{ts}\n{latency}\n"),
        (Some(ts), None) => format!("{verdict}\n{ts}\n"),
        (None, _) => format!("{verdict}\n"),
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

    let latency_checked_at = cached.as_ref().and_then(|c| c.latency_checked_at);
    write_cache(probe_or_keep(cached).await, latency_checked_at);
}

/// Measure both access points in the background and repin to the better one.
///
/// Geolocation is a guess about which access point serves this client best, and
/// on a split-tunnel proxy — or for someone who has simply moved — it is the
/// wrong guess. `check` has always measured and corrected it, but only when a
/// user thought to run it, which is not something a user has any reason to do
/// until they are already suffering. Measuring on startup makes the wrong guess
/// repair itself instead.
///
/// Backgrounded and never awaited, because the measurement costs an order of
/// magnitude more than the geotest probe and nothing in this run depends on it:
/// the verdict is read from disk at the *next* startup, when swapping the access
/// point costs nothing. Awaiting it would trade an invisible fix for a visible
/// stall.
///
/// The stamp is written before the probe starts, following the same reasoning as
/// the version check: a short command usually exits before the task finishes, and
/// without the stamp every run would start another probe that never lands.
///
/// The probe waits [`PROBE_START_DELAY`] before its first request, so it cannot
/// compete with the command's own startup. Building two TLS clients and opening
/// two connections is not free, and doing it while the process is still coming up
/// delayed the work the user actually asked for — enough to push an ACP
/// `initialize` response past the editor's timeout. Waiting also means a short
/// command exits before the probe costs it anything at all, and the runs that do
/// pay for it are the long-lived ones that were going to outlive it anyway.
pub fn spawn_latency_repin() {
    // With an override in force the verdict is not consulted, so measuring it
    // would be work whose result nothing can read.
    if std::env::var("LONGBRIDGE_REGION").is_ok() {
        return;
    }
    if fresh(cached_latency_checked_at()) {
        return;
    }
    let is_cn = is_cn_cached();
    write_cache(is_cn, now_unix());
    tokio::spawn(async move {
        tokio::time::sleep(PROBE_START_DELAY).await;
        let (global, cn) = probe_access_points().await;
        if let Some(measured_is_cn) = record_measurement(is_cn, &global, &cn) {
            tracing::debug!(
                "Access point repinned to {} by latency (global {}ms ok={}, cn {}ms ok={})",
                if measured_is_cn { "CN" } else { "global" },
                global.ms,
                global.ok,
                cn.ms,
                cn.ok,
            );
        }
    });
}

/// Persist what a measurement found: repin when the gap is decisive, and stamp
/// the probe either way so the background one paces itself.
///
/// Returns the new verdict when it changed, for `check` to report.
pub fn record_measurement(
    active_is_cn: bool,
    global: &ProbeStats,
    cn: &ProbeStats,
) -> Option<bool> {
    if std::env::var("LONGBRIDGE_REGION").is_ok() {
        return None;
    }
    let repinned = repin_from_latency(active_is_cn, global, cn);
    write_cache(repinned.unwrap_or(active_is_cn), now_unix());
    repinned
}

/// Probe geotest, falling back to the previous verdict when it cannot answer.
async fn probe_or_keep(cached: Option<CachedRegion>) -> bool {
    if let Some(country) = probe_country().await {
        let is_cn = country.eq_ignore_ascii_case("CN");
        tracing::debug!(
            "Region check: geotest={country} → {}",
            if is_cn { "CN" } else { "global" }
        );
        return is_cn;
    }
    // Inconclusive (unreachable, non-2xx, or unparsable body). Keep the
    // previous verdict but let the caller refresh the timestamp, so a broken
    // network does not make every command pay the probe timeout.
    let fallback = cached.is_some_and(|c| c.is_cn);
    tracing::debug!(
        "Region check: geotest inconclusive, keeping {}",
        if fallback { "CN" } else { "global" }
    );
    fallback
}

/// Detect the region again and persist it, ignoring the cached verdict's TTL.
/// Returns the country code geotest reported, or `None` if it could not answer.
///
/// `check` uses this instead of reading the cache: a diagnostic that reports a
/// possibly stale verdict is answering the wrong question, since a stale
/// verdict is exactly what the user is likely running it to investigate.
/// Detecting here also repairs the cache as a side effect.
pub async fn redetect_region() -> Option<String> {
    // With an override in force nothing reads the cache, so leave it as it is.
    if std::env::var("LONGBRIDGE_REGION").is_ok() {
        return None;
    }

    let country = probe_country().await;
    let cached = read_cache();
    let is_cn = match country.as_deref() {
        Some(code) => code.eq_ignore_ascii_case("CN"),
        None => cached.as_ref().is_some_and(|c| c.is_cn),
    };
    write_cache(is_cn, cached.and_then(|c| c.latency_checked_at));
    country
}

/// Persist a verdict reached some other way than the geotest probe — `check`
/// repinning from measured endpoint latency. No-op while an override is set,
/// since nothing would read the result.
pub fn record_region(is_cn: bool) {
    if std::env::var("LONGBRIDGE_REGION").is_ok() {
        return;
    }
    write_cache(is_cn, cached_latency_checked_at());
}

/// The region forced by `LONGBRIDGE_REGION`, if that variable is set.
///
/// Normalised the same way [`is_cn_cached`] reads it: anything other than `cn`
/// pins the global access point.
pub fn region_override() -> Option<&'static str> {
    let raw = std::env::var("LONGBRIDGE_REGION").ok()?;
    Some(if raw.trim().eq_ignore_ascii_case("cn") {
        "cn"
    } else {
        "global"
    })
}

/// Probe geotest for the caller's country code. `None` when it cannot answer.
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
async fn probe_country() -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(GEOTEST_TIMEOUT_SECS))
        .build()
        .ok()?;
    let resp = client.get(GEOTEST_URL).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.text().await.ok()?;
    parse_geotest_country(&body).map(str::to_ascii_uppercase)
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

    /// The probe has to reach the same host the quotes come from, so it takes
    /// the endpoint the SDK uses and only swaps the scheme.
    #[test]
    fn the_probe_url_keeps_the_quote_host_and_path() {
        assert_eq!(
            probe_url("wss://openapi-quote.longbridge.com/v2"),
            "https://openapi-quote.longbridge.com/v2"
        );
        assert_eq!(
            probe_url("ws://127.0.0.1:8080/v2"),
            "http://127.0.0.1:8080/v2"
        );
        // Already plain, or an unrecognised scheme: left alone rather than mangled.
        assert_eq!(
            probe_url("https://example.test/v2"),
            "https://example.test/v2"
        );
    }

    /// Both constants must stay reachable by the probe; a scheme change that
    /// slipped past `probe_url` would make both access points look unreachable
    /// and freeze the verdict wherever it happened to be.
    #[test]
    fn both_access_points_produce_http_probe_urls() {
        for url in [QUOTE_WS_URL_GLOBAL, QUOTE_WS_URL_CN] {
            let probed = probe_url(url);
            assert!(
                probed.starts_with("http://") || probed.starts_with("https://"),
                "{url} probes as {probed}"
            );
        }
    }

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

    /// The latency stamp is a third line. A cache written before the background
    /// probe existed has none, which has to read as "never measured" so the
    /// first run after upgrading measures once.
    #[test]
    fn parses_the_latency_stamp_and_treats_its_absence_as_never() {
        let measured = parse_cache("cn\n1750000000\n1750000900\n");
        assert_eq!(measured.checked_at, Some(1_750_000_000));
        assert_eq!(measured.latency_checked_at, Some(1_750_000_900));

        let never = parse_cache("cn\n1750000000\n");
        assert_eq!(never.latency_checked_at, None);
        assert!(!fresh(never.latency_checked_at));
    }

    /// A stamp from the last few minutes is trusted; one older than the TTL is
    /// not, which is what paces the background probe.
    #[test]
    fn a_stamp_expires_with_the_ttl() {
        let now = now_unix().expect("a clock");
        assert!(fresh(Some(now)));
        assert!(!fresh(Some(now - CACHE_TTL_SECS - 1)));
        assert!(!fresh(None));
    }

    #[test]
    fn repins_when_the_other_access_point_is_decisively_faster() {
        // Measured on a split-tunnel proxy: geotest said CN, but the global
        // endpoint was 135ms (42%) ahead of the CN one.
        assert_eq!(repin_from_latency(true, &ok(321), &ok(456)), Some(false));
    }

    #[test]
    fn ignores_a_narrow_lead() {
        // Measured on a US CI runner: cn led global by 41ms (20%). Too close to
        // act on — repinning here would undo correct geo detection.
        assert_eq!(repin_from_latency(false, &ok(244), &ok(203)), None);
    }

    #[test]
    fn leaves_an_already_optimal_verdict_alone() {
        // CN proxy exit: cn is active and far ahead, nothing to do.
        assert_eq!(repin_from_latency(true, &ok(228), &ok(46)), None);
    }

    #[test]
    fn always_leaves_an_unreachable_access_point() {
        // The case that stranded overseas clients on longbridge.cn.
        assert_eq!(repin_from_latency(true, &ok(300), &down()), Some(false));
        assert_eq!(repin_from_latency(false, &down(), &ok(300)), Some(true));
    }

    #[test]
    fn stays_put_when_the_alternative_is_also_down() {
        assert_eq!(repin_from_latency(true, &down(), &down()), None);
        // A working active endpoint is kept even if the other one is dead.
        assert_eq!(repin_from_latency(true, &down(), &ok(50)), None);
    }

    fn ok(ms: u64) -> ProbeStats {
        ProbeStats { ok: true, ms }
    }
    fn down() -> ProbeStats {
        ProbeStats { ok: false, ms: 0 }
    }
}
