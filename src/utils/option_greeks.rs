use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use statrs::distribution::{Continuous, ContinuousCDF, Normal};
use time::macros::offset;

trait Distribution {
    fn norm_pdf(self) -> Self;
    fn norm_cdf(self) -> Self;
}

impl Distribution for f64 {
    fn norm_pdf(self) -> Self {
        Normal::standard().pdf(self)
    }

    fn norm_cdf(self) -> Self {
        Normal::standard().cdf(self)
    }
}

/// Black-Scholes option greeks.
///
/// Matches the engine algorithm in `engine/src/util/quote/share_option.rs`.
/// All greek outputs use the same scaling as the engine:
///   - theta: per calendar day (divided by `365`)
///   - vega:  per 1% move in IV (divided by `100`)
///   - rho:   per 1% move in rate (divided by `100`)
pub struct OptionGreeks {
    /// Strike price
    k: f64,
    /// Underlying price minus dividends to expiry
    s0: f64,
    /// Implied volatility as a fraction (e.g., 1.1328 for 113.28%)
    o: f64,
    /// Risk-free rate as a fraction (e.g., 0.052237)
    r: f64,
    /// Time to expiry in years: `(remaining_days + 1) / 365`
    t: f64,
    /// Whether the option is a call (false = put)
    is_call: bool,
    /// Square root of t
    st: f64,
}

impl OptionGreeks {
    const YEAR_DAYS: f32 = 365.0;

    /// Construct greeks from option parameters.
    ///
    /// `implied_volatility` must be a decimal fraction, not a percentage
    /// (e.g., pass `1.1328`, not `113.28`).
    /// `dividend_to_expiry` should be `Decimal::ZERO` when not available.
    pub fn new(
        strike_price: Decimal,
        underlying_last_done: Decimal,
        dividend_to_expiry: Decimal,
        expiry_date: time::Date,
        implied_volatility: Decimal,
        interest_rate: Decimal,
        is_call: bool,
    ) -> Option<Self> {
        let s0 = underlying_last_done - dividend_to_expiry;
        let days = remaining_days(expiry_date);
        Self::with_days(strike_price, s0, days, implied_volatility, interest_rate, is_call)
    }

    /// Construct greeks given an explicit number of remaining calendar days.
    /// Useful for testing without depending on the current date.
    pub(crate) fn with_days(
        strike_price: Decimal,
        s0: Decimal,
        days: i32,
        implied_volatility: Decimal,
        interest_rate: Decimal,
        is_call: bool,
    ) -> Option<Self> {
        if implied_volatility <= Decimal::ZERO || days < 0 {
            return None;
        }

        #[allow(clippy::cast_precision_loss)]
        let t = (days + 1) as f32 / Self::YEAR_DAYS;
        let st = f64::from(t.sqrt());

        Some(OptionGreeks {
            k: strike_price.to_f64()?,
            s0: s0.to_f64()?,
            o: implied_volatility.to_f64()?,
            r: interest_rate.to_f64()?,
            t: f64::from(t),
            is_call,
            st,
        })
    }

    fn d1(&self) -> f64 {
        let lns0k = (self.s0 / self.k).ln();
        let rot = (self.r + self.o * self.o / 2.0) * self.t;
        let ot = self.o * self.st;
        (lns0k + rot) / ot
    }

    fn d2(&self) -> f64 {
        self.d1() - self.o * self.st
    }

    fn npd1(&self) -> f64 {
        self.d1().norm_pdf()
    }

    pub fn delta(&self) -> f64 {
        let d1 = self.d1();
        if self.is_call {
            d1.norm_cdf()
        } else {
            d1.norm_cdf() - 1.0
        }
    }

    pub fn gamma(&self) -> f64 {
        let n = self.s0 * self.o * self.st;
        self.npd1() / n
    }

    pub fn theta(&self) -> f64 {
        let rt = -self.r * self.t;
        let d2 = self.d2();
        let v1 = -(self.s0 * self.npd1() * self.o) / (2.0 * self.st);
        let v2 = if self.is_call {
            -self.r * self.k * rt.exp() * d2.norm_cdf()
        } else {
            self.r * self.k * rt.exp() * (-d2).norm_cdf()
        };
        (v1 + v2) / 365.0
    }

    pub fn vega(&self) -> f64 {
        self.s0 * self.st * self.npd1() / 100.0
    }

    pub fn rho(&self) -> f64 {
        let d2 = self.d2();
        let v1 = (-self.r * self.t).exp();
        if self.is_call {
            let v2 = d2.norm_cdf();
            self.k * self.t * v1 * v2 / 100.0
        } else {
            let v2 = (-d2).norm_cdf();
            (-self.k) * self.t * v1 * v2 / 100.0
        }
    }
}

/// Days remaining until expiry, measured in ET (UTC-5) calendar days.
///
/// Mirrors the engine's `OptionUtil::remaining_days`.
pub fn remaining_days(expiry_date: time::Date) -> i32 {
    let expire = expiry_date.midnight().assume_offset(offset!(-5));
    let now = time::OffsetDateTime::now_utc().to_offset(offset!(-5));
    expire.to_julian_day() - now.to_julian_day()
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    // Tolerance for floating-point comparisons.
    const EPS: f64 = 0.001;

    fn assert_near(label: &str, got: f64, expected: f64) {
        assert!(
            (got - expected).abs() < EPS,
            "{label}: got {got:.6}, expected {expected:.6} (diff {:.6})",
            (got - expected).abs()
        );
    }

    // Reference values computed from textbook Black-Scholes:
    //   S=100, K=100, r=5%, σ=20%, T=1yr (days=364 → t=365/365=1)
    //   d1 = (0 + 0.07) / 0.2 = 0.35,  d2 = 0.15
    //   N(0.35)≈0.6368, N(0.15)≈0.5596, N'(0.35)≈0.3752
    fn atm_call() -> OptionGreeks {
        OptionGreeks::with_days(
            dec!(100),
            dec!(100),
            364, // t = (364+1)/365 = 1.0
            dec!(0.2),
            dec!(0.05),
            true,
        )
        .expect("should construct")
    }

    fn atm_put() -> OptionGreeks {
        OptionGreeks::with_days(
            dec!(100),
            dec!(100),
            364,
            dec!(0.2),
            dec!(0.05),
            false,
        )
        .expect("should construct")
    }

    #[test]
    fn call_delta() {
        assert_near("call delta", atm_call().delta(), 0.6368);
    }

    #[test]
    fn put_delta() {
        // put-call parity: delta_put = delta_call - 1
        assert_near("put delta", atm_put().delta(), 0.6368 - 1.0);
    }

    #[test]
    fn gamma_call_put_equal() {
        // Gamma is the same for call and put with identical parameters.
        let g_call = atm_call().gamma();
        let g_put = atm_put().gamma();
        assert!((g_call - g_put).abs() < 1e-9, "gamma should be equal");
        assert_near("gamma", g_call, 0.01876);
    }

    #[test]
    fn call_theta_negative() {
        let theta = atm_call().theta();
        assert!(theta < 0.0, "call theta must be negative, got {theta}");
        // theta per calendar day ≈ -0.01757
        assert_near("call theta", theta, -0.01757);
    }

    #[test]
    fn put_theta_negative() {
        // Put theta can be positive for deep ITM, but ATM put theta is negative.
        let theta = atm_put().theta();
        assert!(theta < 0.0, "ATM put theta must be negative, got {theta}");
    }

    #[test]
    fn vega_call_put_equal() {
        let v_call = atm_call().vega();
        let v_put = atm_put().vega();
        assert!((v_call - v_put).abs() < 1e-9, "vega should be equal");
        // vega = S * √T * N'(d1) / 100 = 100 * 1 * 0.3752 / 100 ≈ 0.3752
        assert_near("vega", v_call, 0.3752);
    }

    #[test]
    fn call_rho_positive() {
        let rho = atm_call().rho();
        assert!(rho > 0.0, "call rho must be positive, got {rho}");
        // rho = K * T * e^(-rT) * N(d2) / 100 ≈ 0.5322
        assert_near("call rho", rho, 0.5322);
    }

    #[test]
    fn put_rho_negative() {
        let rho = atm_put().rho();
        assert!(rho < 0.0, "put rho must be negative, got {rho}");
    }

    #[test]
    fn zero_iv_returns_none() {
        let g = OptionGreeks::with_days(dec!(100), dec!(100), 30, dec!(0), dec!(0.05), true);
        assert!(g.is_none(), "zero IV should return None");
    }

    #[test]
    fn expired_returns_none() {
        let g = OptionGreeks::with_days(dec!(100), dec!(100), -1, dec!(0.2), dec!(0.05), true);
        assert!(g.is_none(), "expired option should return None");
    }

    #[test]
    fn put_call_parity_price() {
        // C - P = S - K*e^(-rT)  →  verify via delta: delta_call - delta_put = 1
        let d_call = atm_call().delta();
        let d_put = atm_put().delta();
        assert_near("put-call delta parity", d_call - d_put, 1.0);
    }
}
