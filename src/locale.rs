/// Global content language, initialized once at startup.
///
/// Priority: `--lang` CLI flag → system `LANG` env var → `"en"`.
/// Supported values: `"zh-CN"`, `"en"`.
use std::sync::OnceLock;

static EFFECTIVE_LANG: OnceLock<&'static str> = OnceLock::new();

/// Initialize the effective language (call once before any API or URL usage).
pub fn init(flag: Option<&str>) {
    EFFECTIVE_LANG.get_or_init(|| resolve(flag));
}

/// Get the effective language. Returns `"en"` if `init()` has not been called.
pub fn get() -> &'static str {
    EFFECTIVE_LANG.get().copied().unwrap_or("en")
}

/// Normalize a raw language tag to a canonical value.
///
/// - `zh-HK` / `zh_HK.*` → `"zh-HK"`
/// - other `zh-*` / `zh_*` → `"zh-CN"`
/// - anything else → `"en"`
fn normalize(s: &str) -> &'static str {
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("zh_hk") || lower.starts_with("zh-hk") {
        "zh-HK"
    } else if lower.starts_with("zh") {
        "zh-CN"
    } else {
        "en"
    }
}

/// Map a raw language string or env-var value to a canonical value.
fn resolve(flag: Option<&str>) -> &'static str {
    if let Some(s) = flag {
        return normalize(s);
    }
    let val = std::env::var("LANG").unwrap_or_default();
    normalize(&val)
}
