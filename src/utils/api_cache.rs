use std::path::PathBuf;

#[derive(serde::Serialize, serde::Deserialize)]
struct CacheFile<T> {
    data: T,
    /// Unix timestamp (seconds) of when the entry was written.
    cached_at: i64,
}

/// A simple file-based cache stored under `~/.longbridge/terminal/.cache/<key>`.
///
/// Construct a static instance with [`ApiCache::new`], then call [`load`] /
/// [`save`] on it.  Both the stored type and the TTL are supplied at the call
/// site, so one `ApiCache` instance can serve a single logical endpoint.
///
/// [`load`]: ApiCache::load
/// [`save`]: ApiCache::save
pub struct ApiCache {
    key: &'static str,
    ttl_secs: i64,
}

impl ApiCache {
    pub const fn new(key: &'static str, ttl_secs: i64) -> Self {
        Self { key, ttl_secs }
    }

    fn path(&self) -> Option<PathBuf> {
        dirs::home_dir().map(|h| {
            h.join(".longbridge")
                .join("terminal")
                .join(".cache")
                .join(self.key)
        })
    }

    /// Load a cached value.  Returns `(value, is_fresh)`, or `None` if no
    /// valid cache file exists.
    pub fn load<T: serde::de::DeserializeOwned>(&self) -> Option<(T, bool)> {
        let bytes = std::fs::read(self.path()?).ok()?;
        let entry: CacheFile<T> = serde_json::from_slice(&bytes).ok()?;
        let age = time::OffsetDateTime::now_utc().unix_timestamp() - entry.cached_at;
        Some((entry.data, age < self.ttl_secs))
    }

    /// Persist a value to the cache file, updating the timestamp.
    pub fn save<T: serde::Serialize>(&self, value: &T) {
        let Some(path) = self.path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let entry = CacheFile {
            data: value,
            cached_at: time::OffsetDateTime::now_utc().unix_timestamp(),
        };
        if let Ok(json) = serde_json::to_string(&entry) {
            let _ = std::fs::write(path, json);
        }
    }
}
