use std::collections::HashSet;
use std::sync::OnceLock;

static US_ETF_SET: OnceLock<HashSet<&'static str>> = OnceLock::new();

fn us_etf_set() -> &'static HashSet<&'static str> {
    US_ETF_SET.get_or_init(|| {
        include_str!("US-ETF.csv")
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect()
    })
}

/// Convert a user-supplied symbol (e.g. `TSLA.US`, `700.HK`) to a `counter_id`
/// (e.g. `ST/US/TSLA`, `ST/HK/700`, `ETF/US/SPY`).
///
/// US symbols are checked against the embedded ETF list; matching symbols use
/// the `ETF/` prefix.  All other symbols default to `ST/`.
pub fn symbol_to_counter_id(symbol: &str) -> String {
    if let Some((code, market)) = symbol.rsplit_once('.') {
        let market = market.to_uppercase();
        let etf_candidate = format!("ETF/{market}/{code}");
        if us_etf_set().contains(etf_candidate.as_str()) {
            etf_candidate
        } else {
            format!("ST/{market}/{code}")
        }
    } else {
        symbol.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_us() {
        assert_eq!(symbol_to_counter_id("TSLA.US"), "ST/US/TSLA");
    }

    #[test]
    fn stock_hk() {
        assert_eq!(symbol_to_counter_id("700.HK"), "ST/HK/700");
    }

    #[test]
    fn etf_us_spy() {
        assert_eq!(symbol_to_counter_id("SPY.US"), "ETF/US/SPY");
    }

    #[test]
    fn etf_us_qqq() {
        assert_eq!(symbol_to_counter_id("QQQ.US"), "ETF/US/QQQ");
    }

    #[test]
    fn market_suffix_lowercase_normalised() {
        assert_eq!(symbol_to_counter_id("SPY.us"), "ETF/US/SPY");
    }

    #[test]
    fn no_dot_passthrough() {
        assert_eq!(symbol_to_counter_id("NODOT"), "NODOT");
    }
}
