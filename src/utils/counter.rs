#[allow(clippy::unreadable_literal)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/special_counter_ids.rs"));
}
use generated::SPECIAL_COUNTER_IDS;

/// Convert a `counter_id` (e.g. `ST/US/TSLA`, `ETF/US/SPY`, `IX/US/.DJI`, `ST/HK/700`) back to
/// a display symbol (e.g. `TSLA.US`, `SPY.US`, `.DJI.US`, `700.HK`).
///
/// US index `counter_ids` (`IX/US/...`) preserve the leading dot in the code part
/// (e.g. `IX/US/.DJI` → `.DJI.US`).
pub fn counter_id_to_symbol(counter_id: &str) -> String {
    let parts: Vec<&str> = counter_id.splitn(3, '/').collect();
    if parts.len() == 3 {
        let (_prefix, market, code) = (parts[0], parts[1], parts[2]);
        format!("{code}.{market}")
    } else {
        counter_id.to_string()
    }
}

/// Convert a user-supplied symbol (e.g. `TSLA.US`, `700.HK`, `.DJI.US`, `HSI.HK`) to a
/// `counter_id` (e.g. `ST/US/TSLA`, `ST/HK/700`, `IX/US/DJI`, `IX/HK/HSI`).
///
/// Leading-dot symbols (e.g. `.DJI.US`) are US market indexes and always map to `IX/`.
/// All other symbols are checked against the embedded ETF + index set; a matching entry
/// is returned as-is.  Unmatched symbols default to `ST/`.
pub fn symbol_to_counter_id(symbol: &str) -> String {
    if let Some((code, market)) = symbol.rsplit_once('.') {
        let market = market.to_uppercase();
        // Leading-dot symbols are US market indexes; the dot is part of the counter_id
        // (e.g. `.DJI.US` → `IX/US/.DJI`)
        if code.starts_with('.') {
            return format!("IX/{market}/{code}");
        }
        // Strip leading zeros from numeric codes (e.g. `00700` → `700`)
        let code = if code.chars().all(|c| c.is_ascii_digit()) {
            code.trim_start_matches('0')
        } else {
            code
        };
        // Check special counter_ids set (ETF, IX, and WT entries)
        for prefix in &["ETF", "IX", "WT"] {
            let candidate = format!("{prefix}/{market}/{code}");
            if SPECIAL_COUNTER_IDS.contains(candidate.as_str()) {
                return candidate;
            }
        }
        format!("ST/{market}/{code}")
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
    fn stock_hk_leading_zeros() {
        assert_eq!(symbol_to_counter_id("00700.HK"), "ST/HK/700");
    }

    #[test]
    fn stock_hk_leading_zeros_short() {
        assert_eq!(symbol_to_counter_id("09988.HK"), "ST/HK/9988");
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
        assert_eq!(symbol_to_counter_id(".DJI.US"), "IX/US/.DJI");
    }

    #[test]
    fn ix_us_vix() {
        assert_eq!(symbol_to_counter_id(".VIX.US"), "IX/US/.VIX");
    }

    #[test]
    fn ix_us_ixic() {
        assert_eq!(symbol_to_counter_id(".IXIC.US"), "IX/US/.IXIC");
    }

    #[test]
    fn ix_us_spx() {
        assert_eq!(symbol_to_counter_id(".SPX.US"), "IX/US/.SPX");
    }

    #[test]
    fn ix_hk_hsi_via_set() {
        assert_eq!(symbol_to_counter_id("HSI.HK"), "IX/HK/HSI");
    }

    #[test]
    fn wt_hk_via_set() {
        assert_eq!(symbol_to_counter_id("10005.HK"), "WT/HK/10005");
    }

    #[test]
    fn counter_id_ix_us_to_symbol() {
        assert_eq!(counter_id_to_symbol("IX/US/.DJI"), ".DJI.US");
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
