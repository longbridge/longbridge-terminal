use statrs::distribution::{Continuous, ContinuousCDF, Normal};
use time::{macros::offset, OffsetDateTime};

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
/// Ported from `engine/src/util/quote/share_option.rs`.
#[derive(Clone, Copy, Debug)]
pub struct OptionGreeks {
    k: f64,
    s0: f64,
    o: f64,
    r: f64,
    t: f64,
    is_call: bool,
    st: f64,
}

impl OptionGreeks {
    const YEAR_DAYS: f64 = 365.0;

    /// Create a new `OptionGreeks` instance.
    ///
    /// - `strike_price`: option strike price
    /// - `underlying_price`: current price of the underlying (dividend already subtracted)
    /// - `implied_volatility`: IV as decimal fraction (e.g. 0.30 for 30%)
    /// - `interest_rate`: risk-free rate as decimal fraction (e.g. 0.043 for 4.3%)
    /// - `remaining_days`: calendar days until expiry (0 = expires today)
    /// - `is_call`: true for call, false for put
    pub fn new(
        strike_price: f64,
        underlying_price: f64,
        implied_volatility: f64,
        interest_rate: f64,
        remaining_days: i32,
        is_call: bool,
    ) -> Option<Self> {
        if implied_volatility <= 0.0 || remaining_days < 0 {
            return None;
        }
        let t = f64::from(remaining_days + 1) / Self::YEAR_DAYS;
        let st = t.sqrt();
        Some(OptionGreeks {
            k: strike_price,
            s0: underlying_price,
            o: implied_volatility,
            r: interest_rate,
            t,
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

/// Compute the number of calendar days remaining until `expiry_date`.
///
/// Uses ET (UTC-5) as the reference timezone, matching the engine.
/// Returns `None` if the option has already expired.
pub fn remaining_days(expiry_date: time::Date) -> Option<i32> {
    let expire = expiry_date.midnight().assume_offset(offset!(-5));
    let now = OffsetDateTime::now_utc().to_offset(offset!(-5));
    let days = expire.to_julian_day() - now.to_julian_day();
    if days < 0 {
        None
    } else {
        Some(days)
    }
}

/// Parse a US equity option symbol (OCC format).
///
/// Returns `(underlying_ticker, is_call)` for symbols like `AAPL240119C190000.US`.
/// Returns `None` for non-option or non-US symbols.
pub fn parse_us_option_symbol(symbol: &str) -> Option<(String, bool)> {
    let sym = symbol.strip_suffix(".US")?;
    // Leading uppercase ASCII letters are the underlying ticker
    let underlying_end = sym.find(|c: char| !c.is_ascii_uppercase())?;
    if underlying_end == 0 {
        return None;
    }
    let after_ticker = &sym[underlying_end..];
    // Next 6 characters must be digits (YYMMDD)
    if after_ticker.len() < 7 {
        return None;
    }
    let (date_part, rest) = after_ticker.split_at(6);
    if !date_part.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let is_call = match rest.chars().next()? {
        'C' => true,
        'P' => false,
        _ => return None,
    };
    let underlying = format!("{}.US", &sym[..underlying_end]);
    Some((underlying, is_call))
}
