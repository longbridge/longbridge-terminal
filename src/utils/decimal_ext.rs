use crate::data::Counter;
use rust_decimal::Decimal;

/// Decimal extension trait
pub trait DecimalExt {
    fn format_quote_by_counter(&self, counter: &Counter) -> String;
    fn format_percent(&self) -> String;
}

impl DecimalExt for Decimal {
    fn format_quote_by_counter(&self, _counter: &Counter) -> String {
        // Simplified implementation: choose precision based on value size
        if self.abs() < Decimal::from(10) {
            format!("{self:.3}")
        } else {
            format!("{self:.2}")
        }
    }

    fn format_percent(&self) -> String {
        format!("{:.2}%", self * Decimal::from(100))
    }
}
