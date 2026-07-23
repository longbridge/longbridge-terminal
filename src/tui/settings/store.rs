//! User-settings persistence.
//!
//! Only non-default values are stored (each field is `Option<T>` with
//! `skip_serializing_if`), written atomically (temp file + rename) as JSON to
//! `~/.longbridge/terminal.json`. JSON is used because `serde_json` is already
//! a dependency (no `toml` crate needed).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::data::StockColorMode;

/// On-disk user configuration. Fields are optional so unset settings are
/// omitted from the file rather than written as defaults.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stock_color_mode: Option<StockColorMode>,
}

fn config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".longbridge").join("terminal.json"))
}

/// Load the config from disk, falling back to defaults on any error.
#[must_use]
pub fn load() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}

/// Persist the config atomically (temp file + rename), creating the parent
/// directory if needed. Errors are swallowed — a settings write must never
/// take down the TUI.
pub fn save(config: &Config) {
    let Some(path) = config_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(json) = serde_json::to_string_pretty(config) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json.as_bytes()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_omits_unset_fields() {
        // "Only non-default values are stored": an unset config is an empty object.
        assert_eq!(serde_json::to_string(&Config::default()).unwrap(), "{}");
    }

    #[test]
    fn config_round_trips_through_json() {
        let config = Config {
            stock_color_mode: Some(StockColorMode::GreenUp),
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.stock_color_mode, Some(StockColorMode::GreenUp));
    }
}
