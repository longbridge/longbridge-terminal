# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

A Rust-based TUI (Terminal User Interface) stock trading terminal using the Longport OpenAPI SDK for market data and trading operations.

## Core Architecture

### Tech Stack

- **UI Framework**: Ratatui (v0.24.0) - TUI rendering
- **Async Runtime**: Tokio (v1.33.0) - Async I/O
- **ECS Framework**: Bevy ECS (v0.11) - Entity-Component-System architecture
- **Market SDK**: longport (v3.0.7) - Longport OpenAPI Rust SDK
- **State Management**: DashMap, Atomic, RwLock - Thread-safe global state

### Key Modules

#### 1. `src/openapi/` - OpenAPI Integration Layer

- `context.rs` - Global context management
  - `init_contexts()` - Initialize QuoteContext and TradeContext, returns WebSocket receiver
  - `quote()` - Get global QuoteContext (for quotes, subscriptions)
  - `trade()` - Get global TradeContext (for trading operations)
  - Uses `OnceLock` for global singleton

#### 2. `src/data/` - Data Layer

- `types.rs` - Base type definitions
  - `Counter` - Stock identifier (format: `700.HK`, `AAPL.US`)
  - `TradeStatus`, `Currency`, `Market` - Enum types
  - `QuoteData`, `Candlestick`, `Depth` - Market data structures
- `stock.rs` - Stock data structure
  - `update_from_quote()` - Update from longport quote
  - `update_from_depth()` - Update from longport depth
- `stocks.rs` - Global stock cache (based on `DashMap`)
  - `STOCKS` - Global singleton, provides `get()`, `mget()`, `insert()`, `modify()` methods

#### 3. `src/app.rs` - Application Main Loop

- Uses Bevy ECS to manage app state (`AppState`)
- Handles UI updates via `mpsc::unbounded_channel`
- Subscribes to index quotes (HSI, DJI, Shanghai Composite, etc.)
- Integrates search, selection, popup components

#### 4. `src/system.rs` - System Logic and UI Rendering

- Contains rendering logic for pages (Watchlist, Stock, Portfolio, etc.)
- Handles user input and state transitions

#### 5. `src/api/` - API Call Layer

- `search.rs` - Stock search
- `quote.rs` - Quote queries
- `account.rs` - Account information
- Uses `openapi::quote()` and `openapi::trade()`

#### 6. `src/widgets/` and `src/views/` - UI Components

- `Terminal` - Terminal management
- `Search`, `LocalSearch` - Search components
- `Carousel` - Carousel component
- `Loading` - Loading animation
- Various popups and navigation components

### Data Flow

1. **Initialization**: `main.rs` → `openapi::init_contexts()` → Get WebSocket receiver
2. **Subscribe Quotes**: `app.rs` → `openapi::quote().subscribe()` → longport SDK
3. **Receive Push**: WebSocket receiver → Parse `PushEvent` → Update `STOCKS` cache
4. **UI Rendering**: Bevy ECS systems → Read `STOCKS` → Ratatui rendering

## Development Commands

### Build and Run

```bash
# Development build
cargo build

# Release build (with LTO and optimizations)
cargo build --release

# Run
cargo run
```

### Code Checks

```bash
# Clippy check (project uses strict pedantic rules)
cargo clippy

# Format
cargo fmt
```

### Configuration

Requires Longport OpenAPI credentials via environment variables or `.env` file:

```bash
# Copy example config
cp .env.example .env

# Edit config (get credentials from https://open.longbridge.com)
# LONGPORT_APP_KEY=...
# LONGPORT_APP_SECRET=...
# LONGPORT_ACCESS_TOKEN=...
```

## Code Style

### Clippy Rules

Project uses strict `clippy::pedantic` rules with the following exceptions:

- `cast_possible_truncation`
- `ignored_unit_patterns`
- `implicit_hasher`
- `missing_errors_doc` / `missing_panics_doc`
- `module_name_repetitions`
- `must_use_candidate`
- `needless_pass_by_value`
- `too_many_arguments` / `too_many_lines`

### Naming Conventions

- Types use UpperCamelCase
- Functions and variables use snake_case
- Constants use SCREAMING_SNAKE_CASE

### Language and Localization

**IMPORTANT**: All code comments and documentation MUST be written in English only.

- **Never** write Chinese or other non-English text in code comments
- **Never** use Chinese strings directly in code
- Use `rust-i18n` (`t!` macro) for all user-facing text and messages
- All locale strings should be defined in `locales/*.yml` files
- Example:
  ```rust
  // Good: English comment
  let status = t!("TradeStatus.Normal");  // Use i18n for display text

  // Bad: Chinese comment
  // let status = "交易中";  // Never hardcode Chinese strings
  ```

## Longport SDK Reference

### Documentation

- Rust SDK Docs: https://longportapp.github.io/openapi/rust/longport/
- OpenAPI Full Docs: https://open.longbridge.com/llms-full.txt
- Developer Portal: https://open.longbridge.com

### Common API Patterns

```rust
// Get quotes
let ctx = crate::openapi::quote();
let quotes = ctx.quote(vec!["700.HK", "AAPL.US"]).await?;

// Subscribe to real-time quotes
ctx.subscribe(&symbols, longport::quote::SubFlags::QUOTE).await?;

// Query candlesticks
let klines = ctx.candlesticks("AAPL.US", longport::quote::Period::Day, 100, None).await?;

// Submit order
let ctx = crate::openapi::trade();
let opts = longport::trade::SubmitOrderOptions::new(
    "700.HK",
    longport::trade::OrderType::LO,
    longport::trade::OrderSide::Buy,
    decimal!(500),
    longport::trade::TimeInForceType::Day,
);
let order = ctx.submit_order(opts).await?;
```

## Important Notes

1. **Rate Limiting**: Longport API limits to "no more than 10 calls per second"
2. **Token Expiration**: Access Token expires every 3 months, requires manual renewal
3. **Market Support**: Supports Hong Kong, US, and China A-share markets
4. **Testing**: Per user instructions, update flow has no test coverage
5. **Logging**: Uses `tracing` library, log files configured via `logger::init()`

## Skills

For Ratatui-specific questions or when working with TUI components, use the `rs-ratatui-crate` skill.
