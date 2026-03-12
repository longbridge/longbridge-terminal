use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::time::{sleep, Instant};
use tracing::{debug, warn};

/// Rate limiter using token bucket algorithm
/// Ensures API requests stay under 10 calls per second
pub struct RateLimiter {
    /// Semaphore for token bucket (capacity = max burst size)
    semaphore: Arc<Semaphore>,
    /// Token refill rate: tokens per second
    tokens_per_second: u32,
    /// Maximum burst capacity
    max_tokens: u32,
    /// Last refill timestamp
    last_refill: tokio::sync::Mutex<Instant>,
}

impl RateLimiter {
    /// Create a new rate limiter
    ///
    /// # Arguments
    /// * `tokens_per_second` - Maximum requests per second (10 for Longport API)
    /// * `max_tokens` - Maximum burst capacity (20 allows short bursts)
    pub fn new(tokens_per_second: u32, max_tokens: u32) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_tokens as usize)),
            tokens_per_second,
            max_tokens,
            last_refill: tokio::sync::Mutex::new(Instant::now()),
        }
    }

    /// Acquire a token (wait if necessary)
    /// Returns immediately if token is available, otherwise waits
    pub async fn acquire(&self) {
        loop {
            // Refill tokens based on elapsed time
            self.refill_tokens().await;

            // Try to acquire a permit without blocking
            if let Ok(permit) = self.semaphore.try_acquire() {
                // Release permit immediately after acquisition
                // The delay is handled by refill_tokens()
                permit.forget();

                debug!(
                    "Rate limiter: token acquired, available permits: {}",
                    self.semaphore.available_permits()
                );
                return;
            }

            // No tokens available, calculate wait time for next token
            let last_refill = *self.last_refill.lock().await;
            let now = Instant::now();
            let elapsed = now.duration_since(last_refill);
            let time_per_token = Duration::from_secs_f64(1.0 / f64::from(self.tokens_per_second));
            let wait_time = time_per_token.saturating_sub(elapsed);

            // Wait for at least 1ms to avoid busy loop
            let wait_time = wait_time.max(Duration::from_millis(1));

            debug!(
                "Rate limiter: no tokens available, waiting {:?} for refill",
                wait_time
            );
            sleep(wait_time).await;
        }
    }

    /// Refill tokens based on elapsed time since last refill
    async fn refill_tokens(&self) {
        let mut last_refill = self.last_refill.lock().await;
        let now = Instant::now();
        let elapsed = now.duration_since(*last_refill);

        // Calculate tokens to add based on elapsed time
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let tokens_to_add = (elapsed.as_secs_f64() * f64::from(self.tokens_per_second)) as u32;

        if tokens_to_add > 0 {
            // Add tokens up to max capacity
            let current_tokens = self.semaphore.available_permits() as u32;
            let tokens_needed = self.max_tokens.saturating_sub(current_tokens);
            let tokens_to_add = tokens_to_add.min(tokens_needed);

            if tokens_to_add > 0 {
                self.semaphore.add_permits(tokens_to_add as usize);
                *last_refill = now;
                debug!(
                    "Rate limiter: refilled {} tokens, total available: {}",
                    tokens_to_add,
                    self.semaphore.available_permits()
                );
            }
        }
    }

    /// Execute a request with rate limiting and retry logic
    ///
    /// # Arguments
    /// * `request_name` - Name of the request for logging
    /// * `f` - Async function to execute
    ///
    /// # Returns
    /// Result from the async function
    pub async fn execute<F, T, E>(&self, request_name: &str, mut f: F) -> Result<T, E>
    where
        F: FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, E>> + Send>>,
        E: std::fmt::Display,
    {
        const MAX_RETRIES: u32 = 3;
        let mut retry_count = 0;
        let mut backoff_duration = Duration::from_secs(1);

        loop {
            // Acquire token before making request
            self.acquire().await;

            debug!("Executing rate-limited request: {}", request_name);

            // Execute the request
            match f().await {
                Ok(result) => {
                    if retry_count > 0 {
                        debug!(
                            "Request succeeded after {} retries: {}",
                            retry_count, request_name
                        );
                    }
                    return Ok(result);
                }
                Err(e) => {
                    // Check if this is a rate limit error
                    let error_msg = format!("{e}");
                    let is_rate_limit_error = error_msg.contains("429")
                        || error_msg.contains("rate limit")
                        || error_msg.contains("too many requests");

                    if is_rate_limit_error && retry_count < MAX_RETRIES {
                        retry_count += 1;
                        warn!(
                            "Rate limit error for request '{}' (attempt {}/{}), retrying after {:?}",
                            request_name, retry_count, MAX_RETRIES, backoff_duration
                        );

                        // Exponential backoff
                        sleep(backoff_duration).await;
                        backoff_duration *= 2;
                        continue;
                    }

                    // Non-rate-limit error or max retries reached
                    if retry_count > 0 {
                        warn!(
                            "Request failed after {} retries: {}",
                            retry_count, request_name
                        );
                    }
                    return Err(e);
                }
            }
        }
    }

    /// Get current available tokens (for monitoring)
    pub fn available_tokens(&self) -> usize {
        self.semaphore.available_permits()
    }
}

/// Global rate limiter instance
static RATE_LIMITER: std::sync::OnceLock<RateLimiter> = std::sync::OnceLock::new();

/// Get or initialize the global rate limiter
pub fn global_rate_limiter() -> &'static RateLimiter {
    RATE_LIMITER.get_or_init(|| {
        // Longport API limit: 10 requests per second
        // Max burst: 20 tokens (allows short bursts without throttling)
        RateLimiter::new(10, 20)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Instant;

    #[tokio::test]
    async fn test_rate_limiter_basic() {
        let limiter = RateLimiter::new(10, 20);

        // Should acquire immediately
        let start = Instant::now();
        limiter.acquire().await;
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_millis(100),
            "First acquire should be immediate"
        );
    }

    #[tokio::test]
    async fn test_rate_limiter_burst() {
        let limiter = RateLimiter::new(10, 5);

        // Exhaust all tokens
        for _ in 0..5 {
            limiter.acquire().await;
        }

        // Next acquire should wait for refill
        let start = Instant::now();
        limiter.acquire().await;
        let elapsed = start.elapsed();

        // Should wait at least 100ms (1 token at 10/sec = 0.1s)
        assert!(
            elapsed >= Duration::from_millis(90),
            "Should wait for token refill"
        );
    }

    #[tokio::test]
    async fn test_execute_with_retry() {
        let limiter = RateLimiter::new(10, 20);
        let mut attempt = 0;

        let result = limiter
            .execute("test_request", || {
                attempt += 1;
                Box::pin(async move {
                    if attempt < 2 {
                        Err("429 rate limit exceeded")
                    } else {
                        Ok(42)
                    }
                })
            })
            .await;

        assert_eq!(result, Ok(42));
        assert_eq!(attempt, 2, "Should retry once");
    }
}
