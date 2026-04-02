use std::cmp::Ordering;

pub trait Sign {
    fn positive(&self) -> bool;
    fn negative(&self) -> bool;
    fn zero(&self) -> bool;
    fn sign(&self) -> Ordering;
}

impl Sign for str {
    fn positive(&self) -> bool {
        !(self.negative() || self.zero())
    }

    fn negative(&self) -> bool {
        self.starts_with('-')
    }

    fn zero(&self) -> bool {
        self.chars().all(|c| matches!(c, '0' | '.' | '+' | '-'))
    }

    fn sign(&self) -> Ordering {
        if self.negative() {
            Ordering::Less
        } else if self.zero() {
            Ordering::Equal
        } else {
            Ordering::Greater
        }
    }
}

impl Sign for rust_decimal::Decimal {
    fn positive(&self) -> bool {
        self.is_sign_positive() && !self.is_zero()
    }

    fn negative(&self) -> bool {
        self.is_sign_negative()
    }

    fn zero(&self) -> bool {
        self.is_zero()
    }

    fn sign(&self) -> Ordering {
        if self.is_sign_negative() {
            Ordering::Less
        } else if self.is_zero() {
            Ordering::Equal
        } else {
            Ordering::Greater
        }
    }
}

/// Format a raw numeric string for display.
/// Large absolute values are scaled to B/M/K; percent values get a `%` suffix.
pub fn format_financial_value(raw: &str, is_percent: bool) -> String {
    let Ok(n) = raw.parse::<f64>() else {
        return raw.to_owned();
    };
    if is_percent {
        return format!("{n:.2}%");
    }
    let abs = n.abs();
    if abs >= 1_000_000_000.0 {
        format!("{:.2}B", n / 1_000_000_000.0)
    } else if abs >= 1_000_000.0 {
        format!("{:.2}M", n / 1_000_000.0)
    } else if abs >= 1_000.0 {
        format!("{:.2}K", n / 1_000.0)
    } else {
        format!("{n:.4}")
    }
}

/// Format volume to short format
/// Example: 1234567 → 1.23M
pub fn format_volume(volume: u64) -> String {
    if volume == 0 {
        return "--".to_string();
    }

    #[allow(clippy::cast_precision_loss)]
    let volume_f = volume as f64;

    if volume >= 1_000_000_000 {
        format!("{:.2}B", volume_f / 1_000_000_000.0)
    } else if volume >= 1_000_000 {
        format!("{:.2}M", volume_f / 1_000_000.0)
    } else if volume >= 1_000 {
        format!("{:.2}K", volume_f / 1_000.0)
    } else {
        volume.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // format_financial_value

    #[test]
    fn financial_non_numeric_passthrough() {
        assert_eq!(format_financial_value("-", false), "-");
        assert_eq!(format_financial_value("N/A", false), "N/A");
    }

    #[test]
    fn financial_percent() {
        assert_eq!(format_financial_value("12.5", true), "12.50%");
        assert_eq!(format_financial_value("-3.1", true), "-3.10%");
    }

    #[test]
    fn financial_billions() {
        assert_eq!(format_financial_value("2500000000", false), "2.50B");
    }

    #[test]
    fn financial_millions() {
        assert_eq!(format_financial_value("1500000", false), "1.50M");
    }

    #[test]
    fn financial_thousands() {
        assert_eq!(format_financial_value("2500", false), "2.50K");
    }

    #[test]
    fn financial_small() {
        assert_eq!(format_financial_value("3.14", false), "3.1400");
    }

    #[test]
    fn financial_negative_millions() {
        assert_eq!(format_financial_value("-1500000", false), "-1.50M");
    }

    // format_volume

    #[test]
    fn volume_zero() {
        assert_eq!(format_volume(0), "--");
    }

    #[test]
    fn volume_billions() {
        assert_eq!(format_volume(2_500_000_000), "2.50B");
    }

    #[test]
    fn volume_millions() {
        assert_eq!(format_volume(1_500_000), "1.50M");
    }

    #[test]
    fn volume_thousands() {
        assert_eq!(format_volume(2_500), "2.50K");
    }

    #[test]
    fn volume_small() {
        assert_eq!(format_volume(42), "42");
    }
}
