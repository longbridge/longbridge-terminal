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
