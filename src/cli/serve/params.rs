//! Typed extraction of JSON-RPC `params` fields.
//!
//! Every accessor names the offending field in its error so a client gets a
//! message it can act on rather than a bare "invalid params". Errors start
//! with a backtick or a known prefix; `serve::error_code_for` keys off that to
//! return `INVALID_PARAMS` instead of `API_ERROR`.

use anyhow::{bail, Result};
use serde_json::Value;

/// Borrowed view over a request's `params` object.
#[derive(Clone, Copy)]
pub struct Params<'a>(pub Option<&'a Value>);

impl<'a> Params<'a> {
    fn get(self, key: &str) -> Option<&'a Value> {
        self.0.and_then(|p| p.get(key))
    }

    pub fn str(self, key: &str) -> Result<String> {
        match self.get(key) {
            Some(Value::String(s)) => Ok(s.clone()),
            Some(_) => bail!("`{key}` must be a string"),
            None => bail!("missing required parameter `{key}`"),
        }
    }

    pub fn str_opt(self, key: &str) -> Result<Option<String>> {
        match self.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => bail!("`{key}` must be a string"),
        }
    }

    pub fn strs(self, key: &str) -> Result<Vec<String>> {
        let Some(value) = self.get(key) else {
            bail!("missing required parameter `{key}`");
        };
        let Some(list) = value.as_array() else {
            bail!("`{key}` must be an array of strings");
        };
        let mut out = Vec::with_capacity(list.len());
        for item in list {
            let Some(s) = item.as_str() else {
                bail!("`{key}` must be an array of strings");
            };
            out.push(s.to_string());
        }
        if out.is_empty() {
            bail!("`{key}` must not be empty");
        }
        Ok(out)
    }

    /// Optional string array; an absent key yields an empty list rather than
    /// an error, for genuinely optional filters.
    pub fn strs_opt(self, key: &str) -> Result<Vec<String>> {
        match self.get(key) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(_) => self.strs(key),
        }
    }

    pub fn i64(self, key: &str) -> Result<i64> {
        match self.get(key) {
            Some(v) => v
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("`{key}` must be an integer")),
            None => bail!("missing required parameter `{key}`"),
        }
    }

    pub fn usize_or(self, key: &str, default: usize) -> Result<usize> {
        match self.get(key) {
            None | Some(Value::Null) => Ok(default),
            Some(v) => {
                let n = v
                    .as_u64()
                    .ok_or_else(|| anyhow::anyhow!("`{key}` must be a non-negative integer"))?;
                Ok(usize::try_from(n).unwrap_or(usize::MAX))
            }
        }
    }

    pub fn bool_or(self, key: &str, default: bool) -> Result<bool> {
        match self.get(key) {
            None | Some(Value::Null) => Ok(default),
            Some(Value::Bool(b)) => Ok(*b),
            Some(_) => bail!("`{key}` must be a boolean"),
        }
    }

    /// A `YYYY-MM-DD` date, parsed by the same helper the CLI flags use.
    pub fn date(self, key: &str) -> Result<time::Date> {
        let raw = self.str(key)?;
        crate::cli::output::parse_date(&raw)
            .map_err(|_| anyhow::anyhow!("`{key}` must be a date in YYYY-MM-DD form, got `{raw}`"))
    }

    pub fn date_opt(self, key: &str) -> Result<Option<time::Date>> {
        match self.str_opt(key)? {
            None => Ok(None),
            Some(raw) => crate::cli::output::parse_date(&raw).map(Some).map_err(|_| {
                anyhow::anyhow!("`{key}` must be a date in YYYY-MM-DD form, got `{raw}`")
            }),
        }
    }

    /// Market code, accepted in the same spellings as the CLI (`HK`, `US`,
    /// `CN`/`SH`/`SZ`, `SG`).
    pub fn market(self, key: &str) -> Result<longbridge::Market> {
        let raw = self.str(key)?;
        crate::cli::quote::parse_market(&raw).map_err(|e| anyhow::anyhow!("`{key}`: {e}"))
    }

    pub fn period(self, key: &str) -> Result<longbridge::quote::Period> {
        let raw = self.str(key)?;
        crate::cli::quote::parse_period(&raw).map_err(|e| anyhow::anyhow!("`{key}`: {e}"))
    }

    /// Adjustment type; defaults to `none`, matching `kline --adjust`.
    pub fn adjust(self, key: &str) -> Result<longbridge::quote::AdjustType> {
        match self.str_opt(key)? {
            None => Ok(longbridge::quote::AdjustType::NoAdjust),
            Some(raw) => {
                crate::cli::quote::parse_adjust(&raw).map_err(|e| anyhow::anyhow!("`{key}`: {e}"))
            }
        }
    }

    /// Raw query-string pairs for the REST passthrough. Scalars are stringified
    /// so a client can pass `{"count": 20}` as well as `{"count": "20"}`.
    pub fn query(self, key: &str) -> Result<Vec<(String, String)>> {
        let Some(value) = self.get(key) else {
            return Ok(Vec::new());
        };
        if value.is_null() {
            return Ok(Vec::new());
        }
        let Some(map) = value.as_object() else {
            bail!("`{key}` must be an object of string values");
        };
        map.iter()
            .map(|(k, v)| match v {
                Value::String(s) => Ok((k.clone(), s.clone())),
                Value::Bool(_) | Value::Number(_) => Ok((k.clone(), v.to_string())),
                _ => bail!("`{key}.{k}` must be a string, number or boolean"),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn p(v: &Value) -> Params<'_> {
        Params(Some(v))
    }

    #[test]
    fn a_missing_field_is_named_in_the_error() {
        let v = json!({});
        assert_eq!(
            p(&v).str("symbol").unwrap_err().to_string(),
            "missing required parameter `symbol`"
        );
        assert_eq!(
            Params(None).strs("symbols").unwrap_err().to_string(),
            "missing required parameter `symbols`"
        );
    }

    #[test]
    fn a_wrong_type_is_named_in_the_error() {
        let v = json!({"symbol": 7, "symbols": "700.HK", "flag": "yes"});
        assert_eq!(
            p(&v).str("symbol").unwrap_err().to_string(),
            "`symbol` must be a string"
        );
        assert_eq!(
            p(&v).strs("symbols").unwrap_err().to_string(),
            "`symbols` must be an array of strings"
        );
        assert_eq!(
            p(&v).bool_or("flag", false).unwrap_err().to_string(),
            "`flag` must be a boolean"
        );
    }

    #[test]
    fn optional_fields_fall_back_without_erroring() {
        let v = json!({});
        assert_eq!(p(&v).str_opt("x").unwrap(), None);
        assert_eq!(p(&v).strs_opt("x").unwrap(), Vec::<String>::new());
        assert_eq!(p(&v).usize_or("count", 20).unwrap(), 20);
        assert!(p(&v).bool_or("flag", true).unwrap());
        assert_eq!(p(&v).date_opt("start").unwrap(), None);
        assert_eq!(
            p(&v).adjust("adjust").unwrap(),
            longbridge::quote::AdjustType::NoAdjust
        );
        // An explicit null is the same as absent: clients serialize optionals
        // that way, and rejecting it would make every optional field awkward.
        let nulls = json!({"x": null, "count": null, "start": null});
        assert_eq!(p(&nulls).str_opt("x").unwrap(), None);
        assert_eq!(p(&nulls).usize_or("count", 5).unwrap(), 5);
        assert_eq!(p(&nulls).date_opt("start").unwrap(), None);
    }

    #[test]
    fn enums_accept_the_same_spellings_as_cli_flags() {
        let v = json!({"market": "hk", "period": "1d", "adjust": "forward"});
        assert_eq!(p(&v).market("market").unwrap(), longbridge::Market::HK);
        assert_eq!(
            p(&v).period("period").unwrap(),
            longbridge::quote::Period::Day
        );
        assert_eq!(
            p(&v).adjust("adjust").unwrap(),
            longbridge::quote::AdjustType::ForwardAdjust
        );
    }

    #[test]
    fn a_bad_enum_or_date_names_the_field_and_the_input() {
        let v = json!({"market": "MARS", "start": "07/01/2026"});
        let err = p(&v).market("market").unwrap_err().to_string();
        assert!(err.starts_with("`market`"), "{err}");
        assert!(err.contains("MARS"), "{err}");

        let err = p(&v).date("start").unwrap_err().to_string();
        assert!(err.starts_with("`start`"), "{err}");
        assert!(err.contains("07/01/2026"), "{err}");
    }

    #[test]
    fn query_stringifies_scalars() {
        let v = json!({"q": {"count": 20, "symbol": "700.HK", "all": true}});
        let mut got = p(&v).query("q").unwrap();
        got.sort();
        assert_eq!(
            got,
            vec![
                ("all".to_string(), "true".to_string()),
                ("count".to_string(), "20".to_string()),
                ("symbol".to_string(), "700.HK".to_string()),
            ]
        );
        assert!(p(&json!({})).query("q").unwrap().is_empty());
        assert!(p(&json!({"q": {"bad": []}})).query("q").is_err());
    }

    #[test]
    fn an_empty_required_array_is_rejected() {
        let v = json!({"symbols": []});
        assert_eq!(
            p(&v).strs("symbols").unwrap_err().to_string(),
            "`symbols` must not be empty"
        );
    }
}
