/// Region detection for China Mainland auto-routing.
///
/// On each startup:
/// 1. Read cached region from disk → `is_cn_cached()` for use in Config builder.
/// 2. Spawn background task: check geotest.lbkrs.com → update cache (non-blocking).
use std::{path::PathBuf, time::Duration};

const GEOTEST_URL: &str = "https://geotest.lbkrs.com";
const GEOTEST_TIMEOUT_SECS: u64 = 3;

// CN endpoint URLs
pub const HTTP_URL_CN: &str = "https://openapi.longportapp.cn";
pub const QUOTE_WS_URL_CN: &str = "wss://openapi-quote.longportapp.cn/v2";
pub const TRADE_WS_URL_CN: &str = "wss://openapi-trade.longportapp.cn/v2";

fn cache_file_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".longbridge-openapi").join("region-cache"))
}

/// Returns `true` if the cached region from the last geotest check was CN.
pub fn is_cn_cached() -> bool {
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
