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

/// Convert a `counter_id` (e.g. `ST/US/TSLA`, `ETF/US/SPY`, `IX/US/DJI`, `ST/HK/700`) back to
/// a display symbol (e.g. `TSLA.US`, `SPY.US`, `.DJI.US`, `700.HK`).
///
/// US index `counter_ids` (`IX/US/...`) map to leading-dot symbols (e.g. `.DJI.US`).
pub fn counter_id_to_symbol(counter_id: &str) -> String {
    let parts: Vec<&str> = counter_id.splitn(3, '/').collect();
    if parts.len() == 3 {
        let (prefix, market, code) = (parts[0], parts[1], parts[2]);
        if prefix == "IX" && market == "US" {
            format!(".{code}.{market}")
        } else {
            format!("{code}.{market}")
        }
    } else {
        counter_id.to_string()
    }
}

/// Convert a user-supplied symbol (e.g. `TSLA.US`, `700.HK`, `.DJI.US`) to a `counter_id`
/// (e.g. `ST/US/TSLA`, `ST/HK/700`, `ETF/US/SPY`, `IX/US/DJI`).
///
/// Leading-dot symbols (e.g. `.DJI.US`, `.VIX.US`) are US market indexes and map to
/// the `IX/` prefix.  US symbols are checked against the embedded ETF list; matching
/// symbols use the `ETF/` prefix.  All other symbols default to `ST/`.
pub fn symbol_to_counter_id(symbol: &str) -> String {
    if let Some((code, market)) = symbol.rsplit_once('.') {
        let market = market.to_uppercase();
        // Leading-dot symbols are US market indexes (e.g. `.DJI.US` → `IX/US/DJI`)
        if let Some(ix_code) = code.strip_prefix('.') {
            return format!("IX/{market}/{ix_code}");
        }
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

    #[test]
    fn ix_us_dji() {
        assert_eq!(symbol_to_counter_id(".DJI.US"), "IX/US/DJI");
    }

    #[test]
    fn ix_us_vix() {
        assert_eq!(symbol_to_counter_id(".VIX.US"), "IX/US/VIX");
    }

    #[test]
    fn ix_us_ixic() {
        assert_eq!(symbol_to_counter_id(".IXIC.US"), "IX/US/IXIC");
    }

    #[test]
    fn counter_id_ix_us_to_symbol() {
        assert_eq!(counter_id_to_symbol("IX/US/DJI"), ".DJI.US");
    }

    #[test]
    fn counter_id_ix_hk_to_symbol() {
        assert_eq!(counter_id_to_symbol("IX/HK/HSI"), "HSI.HK");
    }

    #[test]
    fn counter_id_st_to_symbol() {
        assert_eq!(counter_id_to_symbol("ST/US/TSLA"), "TSLA.US");
    }
}
