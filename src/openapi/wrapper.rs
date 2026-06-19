use anyhow::Result;

/// Wrapper for `QuoteContext` with rate limiting
/// Provides access to inner context while tracking rate limits
pub struct RateLimitedQuoteContext {
    inner: &'static longport::quote::QuoteContext,
    limiter: &'static crate::openapi::rate_limiter::RateLimiter,
}

impl RateLimitedQuoteContext {
    /// Create a new rate-limited quote context wrapper
    pub fn new(inner: &'static longport::quote::QuoteContext) -> Self {
        Self {
            inner,
            limiter: crate::openapi::rate_limiter::global_rate_limiter(),
        }
    }

    /// Get reference to inner context
    /// Use this for direct API calls that will be rate-limited by `execute()`
    pub fn inner(&self) -> &'static longport::quote::QuoteContext {
        self.inner
    }

    /// Get reference to rate limiter for manual rate limiting
    pub fn limiter(&self) -> &'static crate::openapi::rate_limiter::RateLimiter {
        self.limiter
    }

    /// Execute a rate-limited API call
    pub async fn execute<F, T, E>(&self, request_name: &str, f: F) -> Result<T, E>
    where
        F: Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, E>> + Send>>,
        E: std::fmt::Display,
    {
        self.limiter.execute(request_name, f).await
    }
}

/// Wrapper for `TradeContext` with rate limiting
/// Provides access to inner context while tracking rate limits
pub struct RateLimitedTradeContext {
    inner: &'static longport::trade::TradeContext,
    limiter: &'static crate::openapi::rate_limiter::RateLimiter,
}

impl RateLimitedTradeContext {
    /// Create a new rate-limited trade context wrapper
    pub fn new(inner: &'static longport::trade::TradeContext) -> Self {
        Self {
            inner,
            limiter: crate::openapi::rate_limiter::global_rate_limiter(),
        }
    }

    /// Get reference to inner context
    /// Use this for direct API calls that will be rate-limited by `execute()`
    pub fn inner(&self) -> &'static longport::trade::TradeContext {
        self.inner
    }

    /// Get reference to rate limiter for manual rate limiting
    pub fn limiter(&self) -> &'static crate::openapi::rate_limiter::RateLimiter {
        self.limiter
    }

    /// Execute a rate-limited API call
    pub async fn execute<F, T, E>(&self, request_name: &str, f: F) -> Result<T, E>
    where
        F: Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, E>> + Send>>,
        E: std::fmt::Display,
    {
        self.limiter.execute(request_name, f).await
    }
}
