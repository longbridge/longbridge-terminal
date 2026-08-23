use std::collections::VecDeque;
use std::time::Duration;
use tokio::time::{sleep, Instant};
use tracing::{debug, warn};

/// Rate limiter enforcing both Longbridge `OpenAPI` rules at once:
///
/// - at most `max_per_window` calls in any sliding 1-second window
///   ("rate limit of 1-second interval has been reached", code 429002);
/// - at least `min_gap` between two consecutive calls
///   ("minimum interval between two calls should be 0.02 seconds",
///   code 429003).
///
/// A plain token bucket cannot express the second rule: a burst capacity
/// admits N concurrent callers in the same instant, which the server
/// rejects. `agent list` fans out one request per workspace concurrently,
/// so both rules must be enforced client-side or the fan-out 429s.
pub struct RateLimiter {
    /// Maximum calls in any sliding 1-second window.
    max_per_window: u32,
    /// Minimum spacing between two consecutive calls.
    min_gap: Duration,
    /// Grant timestamps within the last second, oldest first. The mutex is
    /// held across the pacing sleep so concurrent acquirers are granted
    /// one at a time, in FIFO order (tokio's Mutex queues fairly).
    grants: tokio::sync::Mutex<VecDeque<Instant>>,
    /// Server-directed pause: no grants before this instant. Set when a call
    /// is rejected with a "retry after" hint, so every concurrent caller
    /// backs off together instead of burning the remaining quota one by one.
    paused_until: std::sync::Mutex<Option<Instant>>,
}

impl RateLimiter {
    /// Create a new rate limiter
    ///
    /// # Arguments
    /// * `max_per_window` - Maximum calls in any 1-second window (10 for Longbridge API)
    /// * `min_gap` - Minimum interval between two consecutive calls (0.02s for Longbridge API)
    pub fn new(max_per_window: u32, min_gap: Duration) -> Self {
        Self {
            max_per_window,
            min_gap,
            grants: tokio::sync::Mutex::new(VecDeque::new()),
            paused_until: std::sync::Mutex::new(None),
        }
    }

    /// Withhold every grant until `when`, keeping the latest deadline when
    /// several rejections race.
    fn pause_until(&self, when: Instant) {
        let mut paused = self.paused_until.lock().unwrap();
        if paused.is_none_or(|current| when > current) {
            *paused = Some(when);
        }
    }

    /// Acquire permission to make one call, sleeping as long as either rule
    /// requires. Returns immediately when both are already satisfied.
    pub async fn acquire(&self) {
        const WINDOW: Duration = Duration::from_secs(1);
        let mut grants = self.grants.lock().await;
        loop {
            let now = Instant::now();
            while let Some(&oldest) = grants.front() {
                if now.duration_since(oldest) >= WINDOW {
                    grants.pop_front();
                } else {
                    break;
                }
            }

            let mut wait = Duration::ZERO;
            if let Some(until) = *self.paused_until.lock().unwrap() {
                if until > now {
                    wait = until - now;
                }
            }
            if let Some(&last) = grants.back() {
                let earliest = last + self.min_gap;
                if earliest > now {
                    // `max`, not assignment: every branch here is a lower
                    // bound on the wait, so the longest one wins.
                    wait = wait.max(earliest - now);
                }
            }
            if grants.len() >= self.max_per_window as usize {
                // Front is the call whose expiry frees a window slot.
                let window_frees = *grants.front().unwrap() + WINDOW;
                if window_frees > now {
                    wait = wait.max(window_frees - now);
                }
            }

            if wait.is_zero() {
                grants.push_back(now);
                debug!("Rate limiter: granted, {} calls in window", grants.len());
                return;
            }
            debug!("Rate limiter: pacing, waiting {:?}", wait);
            sleep(wait).await;
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
        // Five, not three: a fan-out (`agent list` issues one request per
        // workspace plus catalog pages) can see the same call rejected
        // several times while its siblings drain the freed quota, and each
        // retry is cheap now that the wait honors the server's own hint.
        const MAX_RETRIES: u32 = 5;
        let mut retry_count = 0;
        let mut backoff_duration = Duration::from_secs(1);

        loop {
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
                        // Prefer the server's own "retry after" hint; fall
                        // back to exponential backoff. Jitter keeps callers
                        // that were rejected together from retrying together
                        // and colliding again.
                        let backoff =
                            parse_retry_after(&error_msg).unwrap_or(backoff_duration) + jitter();
                        // Pause the whole limiter, not just this caller:
                        // the rejection means the account's quota is spent,
                        // so letting the other queued calls proceed would
                        // only get them rejected too.
                        self.pause_until(Instant::now() + backoff);
                        warn!(
                            "Rate limit error for request '{}' (attempt {}/{}), retrying after {:?}",
                            request_name, retry_count, MAX_RETRIES, backoff
                        );

                        sleep(backoff).await;
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
}

/// Extract the wait the server asked for from a rejection like
/// "rate limit of 1-second interval has been reached, please retry after:
/// 0.4s". Returns `None` when the message carries no usable hint.
fn parse_retry_after(error_msg: &str) -> Option<Duration> {
    let rest = &error_msg[error_msg.find("retry after:")? + "retry after:".len()..];
    let number: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let seconds: f64 = number.parse().ok()?;
    // A hint outside (0s, 60s] is likelier to be a parsing artifact than a
    // real instruction; ignore it rather than stall every request on it.
    (seconds > 0.0 && seconds <= 60.0).then(|| Duration::from_secs_f64(seconds))
}

/// Up to half a second of pseudo-random spread, derived from the clock's
/// sub-second noise so no rand dependency is needed.
fn jitter() -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    Duration::from_millis(u64::from(nanos % 500))
}

/// Global rate limiter instance
static RATE_LIMITER: std::sync::OnceLock<RateLimiter> = std::sync::OnceLock::new();

/// Get or initialize the global rate limiter
pub fn global_rate_limiter() -> &'static RateLimiter {
    RATE_LIMITER.get_or_init(|| {
        // Longbridge OpenAPI limits: 10 requests per second, and at least
        // 0.02s between two consecutive calls. The gap is enforced at 100ms
        // — uniform 10/s pacing — rather than the server's bare 0.02s,
        // because the server measures *arrival* gaps: network jitter
        // compresses departures that were 20ms apart into arrivals that are
        // not, and gets the second call rejected (code 429003).
        RateLimiter::new(10, Duration::from_millis(100))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Instant;

    #[tokio::test]
    async fn test_rate_limiter_basic() {
        let limiter = RateLimiter::new(10, Duration::from_millis(20));

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
    async fn test_consecutive_calls_are_spaced_by_min_gap() {
        let limiter = RateLimiter::new(10, Duration::from_millis(20));

        let start = Instant::now();
        for _ in 0..5 {
            limiter.acquire().await;
        }
        let elapsed = start.elapsed();

        // 5 calls with a 20ms gap need at least 4 gaps.
        assert!(
            elapsed >= Duration::from_millis(80),
            "calls must be at least min_gap apart, got {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn test_window_cap_delays_call_beyond_limit() {
        let limiter = RateLimiter::new(3, Duration::from_millis(1));

        for _ in 0..3 {
            limiter.acquire().await;
        }
        // The window holds 3 grants; the 4th must wait for the first to age
        // out of the 1-second window.
        let start = Instant::now();
        limiter.acquire().await;
        let elapsed = start.elapsed();

        assert!(
            elapsed >= Duration::from_millis(900),
            "4th call must wait for the window, got {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn test_concurrent_acquirers_never_share_an_instant() {
        let limiter = std::sync::Arc::new(RateLimiter::new(10, Duration::from_millis(20)));

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let limiter = limiter.clone();
                tokio::spawn(async move {
                    limiter.acquire().await;
                    Instant::now()
                })
            })
            .collect();
        let mut times = Vec::new();
        for h in handles {
            times.push(h.await.unwrap());
        }
        times.sort();
        for pair in times.windows(2) {
            assert!(
                pair[1].duration_since(pair[0]) >= Duration::from_millis(15),
                "concurrent grants must still be min_gap apart"
            );
        }
    }

    #[test]
    fn test_parse_retry_after() {
        assert_eq!(
            parse_retry_after(
                "API error (code 429002): rate limit of 1-second interval has been \
                 reached, please retry after: 0.4s"
            ),
            Some(Duration::from_secs_f64(0.4))
        );
        assert_eq!(
            parse_retry_after("please retry after: 2s"),
            Some(Duration::from_secs(2))
        );
        assert_eq!(parse_retry_after("no hint here"), None);
        assert_eq!(parse_retry_after("retry after: 0s"), None);
        assert_eq!(parse_retry_after("retry after: 9000s"), None);
    }

    #[tokio::test]
    async fn test_a_pause_outlives_a_shorter_min_gap() {
        // A grant is already on the books, so the min-gap branch fires too.
        // Its wait is the shorter of the two and must not shorten the pause.
        let limiter = RateLimiter::new(10, Duration::from_millis(20));
        limiter.acquire().await;
        limiter.pause_until(Instant::now() + Duration::from_millis(300));

        let start = Instant::now();
        limiter.acquire().await;
        let elapsed = start.elapsed();

        assert!(
            elapsed >= Duration::from_millis(250),
            "the longer of the two bounds must win, got {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn test_a_rejection_pauses_every_caller() {
        let limiter = RateLimiter::new(10, Duration::from_millis(1));
        limiter.pause_until(Instant::now() + Duration::from_millis(300));

        let start = Instant::now();
        limiter.acquire().await;
        let elapsed = start.elapsed();

        assert!(
            elapsed >= Duration::from_millis(250),
            "acquire must respect the pause, got {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn test_execute_with_retry() {
        let limiter = RateLimiter::new(10, Duration::from_millis(20));
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

        assert_eq!(result, Ok(42), "Should succeed after retry");
        assert_eq!(attempt, 2, "Should retry once");
    }
}
