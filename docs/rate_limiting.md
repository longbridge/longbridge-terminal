# Rate Limiting Implementation

## Overview

This document describes the rate limiting system implemented to comply with Longport API's "no more than 10 calls per second" limit.

## Architecture

### Components

1. **RateLimiter** (`src/openapi/rate_limiter.rs`)
   - Token bucket algorithm implementation
   - Configured for 10 requests/second with burst capacity of 20
   - Automatic token refill based on elapsed time
   - Built-in retry mechanism with exponential backoff

2. **Rate-Limited Context Wrappers** (`src/openapi/wrapper.rs`)
   - `RateLimitedQuoteContext`: Wraps `QuoteContext` with rate limiting
   - `RateLimitedTradeContext`: Wraps `TradeContext` with rate limiting
   - Provides access to inner context and rate limiter

3. **Helper Functions** (`src/openapi/helpers.rs`)
   - Convenience functions for common API operations
   - Automatic rate limiting built-in
   - Simplified API for common use cases

## Usage

### Option 1: Using Helper Functions (Recommended for Common Operations)

```rust
use crate::openapi::helpers;

// Subscribe to quotes
helpers::subscribe_quotes(
    vec!["700.HK", "AAPL.US"],
    longport::quote::SubFlags::QUOTE
).await?;

// Get quotes
let quotes = helpers::get_quotes(vec!["700.HK"]).await?;

// Get account balance
let balance = helpers::get_account_balance(None).await?;
```

### Option 2: Using Rate-Limited Context Directly

```rust
use crate::openapi::quote_limited;

let ctx = quote_limited();

// Execute rate-limited API call
ctx.execute("custom_operation", || {
    let inner = ctx.inner();
    Box::pin(async move {
        inner.some_api_method().await.map_err(anyhow::Error::from)
    })
}).await?;
```

### Option 3: Manual Rate Limiting

```rust
use crate::openapi::global_rate_limiter;

let limiter = global_rate_limiter();

// Acquire token before API call
limiter.acquire().await;

// Make your API call
let result = ctx.some_api_method().await?;
```

## Configuration

The rate limiter is configured in `src/openapi/rate_limiter.rs`:

```rust
RateLimiter::new(
    10,  // tokens_per_second: Maximum requests per second
    20,  // max_tokens: Burst capacity
)
```

### Parameters

- **tokens_per_second**: 10 (Longport API limit)
- **max_tokens**: 20 (allows short bursts without throttling)

## Features

### 1. Token Bucket Algorithm

- Tokens are refilled at a constant rate (10/second)
- Burst capacity allows temporary spikes in traffic
- Smooth rate limiting without hard blocks

### 2. Automatic Retry

- Detects rate limit errors (429, "rate limit", "too many requests")
- Exponential backoff: 1s → 2s → 4s
- Maximum 3 retries before giving up

### 3. Monitoring

```rust
let limiter = global_rate_limiter();
let available = limiter.available_tokens();
tracing::info!("Available rate limit tokens: {}", available);
```

### 4. Thread-Safe

- Uses `tokio::sync::Semaphore` for token management
- Compatible with Bevy ECS and async/tokio architecture
- No locks or blocking operations

## Migration Guide

### Existing Code Without Rate Limiting

```rust
// Before
let ctx = crate::openapi::quote();
let quotes = ctx.quote(&symbols).await?;
```

### Migrated Code With Rate Limiting

```rust
// After (Option 1: Helper function)
let quotes = crate::openapi::helpers::get_quotes(&symbols).await?;

// After (Option 2: Rate-limited context)
let ctx = crate::openapi::quote_limited();
ctx.execute("quote", || {
    let inner = ctx.inner();
    let symbols = symbols.clone();
    Box::pin(async move {
        inner.quote(&symbols).await.map_err(anyhow::Error::from)
    })
}).await?;
```

## Testing

### Unit Tests

Run rate limiter tests:

```bash
cargo test --lib rate_limiter
```

### Integration Testing

Monitor rate limiting in action:

1. Enable debug logging:
   ```bash
   export RUST_LOG=debug
   ```

2. Look for rate limiter logs:
   ```
   Rate limiter: token acquired, available permits: 19
   Rate limiter: refilled 5 tokens, total available: 15
   ```

### Performance Impact

- Token acquisition: < 1µs (when tokens available)
- Rate limit delay: Calculated dynamically based on token availability
- Retry delay: Exponential backoff (1s, 2s, 4s)

## Troubleshooting

### Still Getting Rate Limit Errors

1. Check if all API calls are using rate limiting
2. Verify token configuration (should be 10/second)
3. Check for concurrent API calls from multiple tasks

### Slow Performance

1. Check burst capacity (should be 20 for normal operation)
2. Reduce concurrent API call patterns
3. Batch API requests where possible

### Debug Logging

Enable debug logging to see rate limiter activity:

```bash
export RUST_LOG=longbridge=debug
cargo run
```

## Future Improvements

1. **Request Batching**: Automatically batch multiple API calls into single requests
2. **Priority Queue**: Prioritize critical operations over background updates
3. **Adaptive Rate Limiting**: Dynamically adjust rate based on API response headers
4. **Metrics Dashboard**: Real-time monitoring of API usage and rate limit status
5. **Per-Endpoint Limits**: Different rate limits for different API endpoints

## References

- Longport API Documentation: https://open.longbridge.com
- Token Bucket Algorithm: https://en.wikipedia.org/wiki/Token_bucket
- Tokio Semaphore: https://docs.rs/tokio/latest/tokio/sync/struct.Semaphore.html
