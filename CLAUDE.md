# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

A Rust-based CLI (`longbridge`) that wraps every Longbridge OpenAPI endpoint for scripting, AI-agent tool-calling, and daily trading workflows. Also ships a full-screen TUI for interactive market monitoring.

## Core Architecture

### Tech Stack

- **UI Framework**: Ratatui (v0.24.0) - TUI rendering
- **Async Runtime**: Tokio (v1.33.0) - Async I/O
- **ECS Framework**: Bevy ECS (v0.11) - Entity-Component-System architecture
- **Market SDK**: longbridge (v4.0.0) - Longbridge OpenAPI Rust SDK (dependency alias: `longbridge-sdk`)
- **State Management**: DashMap, Atomic, RwLock - Thread-safe global state

### Key Modules

#### 1. `src/auth.rs` - Auth utilities

- `clear_token()` - Clear stored OAuth token (logout). Deletes token file used by the SDK.
- OAuth and token refresh are handled by the longbridge SDK: use `longbridge::oauth::OAuthBuilder` in `openapi::context::init_contexts()`. Token is loaded from `~/.longbridge/openapi/tokens/<client_id>` or browser flow is started; the SDK auto-refreshes the token.
- Local callback server: default port `60355` (configurable via `OAuthBuilder::callback_port()`)

#### 2. `src/openapi/` - OpenAPI Integration Layer

- `context.rs` - Global context management
  - `init_contexts()` - Initialize QuoteContext and TradeContext with OAuth token, returns WebSocket receiver
  - `quote()` - Get global QuoteContext (for quotes, subscriptions)
  - `trade()` - Get global TradeContext (for trading operations)
  - Uses `OnceLock` for global singleton

#### 3. `src/data/` - Data Layer

- `types.rs` - Base type definitions
  - `Counter` - Stock identifier (format: `700.HK`, `AAPL.US`)
  - `TradeStatus`, `Currency`, `Market` - Enum types
  - `QuoteData`, `Candlestick`, `Depth` - Market data structures
- `stock.rs` - Stock data structure
  - `update_from_quote()` - Update from longbridge quote
  - `update_from_depth()` - Update from longbridge depth
- `stocks.rs` - Global stock cache (based on `DashMap`)
  - `STOCKS` - Global singleton, provides `get()`, `mget()`, `insert()`, `modify()` methods

#### 4. `src/app.rs` - Application Main Loop

- Uses Bevy ECS to manage app state (`AppState`)
- Handles UI updates via `mpsc::unbounded_channel`
- Subscribes to index quotes (HSI, DJI, Shanghai Composite, etc.)
- Integrates search, selection, popup components

#### 5. `src/system.rs` - System Logic and UI Rendering

- Contains rendering logic for pages (Watchlist, Stock, Portfolio, etc.)
- Handles user input and state transitions

#### 6. `src/api/` - API Call Layer

- `search.rs` - Stock search
- `quote.rs` - Quote queries
- `account.rs` - Account information
- Uses `openapi::quote()` and `openapi::trade()`

#### 7. `src/widgets/` and `src/views/` - UI Components

- `Terminal` - Terminal management
- `Search`, `LocalSearch` - Search components
- `Carousel` - Carousel component
- `Loading` - Loading animation
- Various popups and navigation components

### Data Flow

1. **Authentication**: `main.rs` → `openapi::init_contexts()` → `longbridge::oauth::OAuthBuilder::build()` (loads token from disk or browser flow) → `Config::from_oauth(oauth)` → SDK handles token refresh automatically
2. **Initialization**: `main.rs` → `openapi::init_contexts()` → QuoteContext and TradeContext created with config → Get WebSocket receiver
3. **Subscribe Quotes**: `app.rs` → `openapi::quote().subscribe()` → longbridge SDK
4. **Receive Push**: WebSocket receiver → Parse `PushEvent` → Update `STOCKS` cache
5. **UI Rendering**: Bevy ECS systems → Read `STOCKS` → Ratatui rendering

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

**Before every `git push` or `gh pr create`, always run both and fix all issues:**

```bash
cargo fmt && cargo clippy
```

### Verifying Changes

After any data-layer or CLI output change, verify correctness by comparing the installed release binary against the local build using the same command and `--format json`:

```bash
# Run both and compare — output should be identical (timestamps may differ for live data)
longbridge <command> <args> --format json
cargo run -- <command> <args> --format json
```

Pick commands that exercise the modified code paths. Common ones:

| Changed area             | Verification command                         |
| ------------------------ | -------------------------------------------- |
| Trade direction / trades | `longbridge trades 700.HK --format json`     |
| Kline / AdjustType       | `longbridge kline 700.HK --format json`      |
| Quote / calc-index       | `longbridge calc-index 700.HK --format json` |
| Static info              | `longbridge static 700.HK --format json`     |

### Configuration

**Authentication Method: OAuth 2.0 (longbridge SDK)**

The application uses the longbridge SDK's built-in OAuth. On first run:

1. `OAuthBuilder::build()` loads token from `~/.longbridge/openapi/tokens/<client_id>` or starts browser authorization
2. Token is persisted by the SDK; refresh is automatic (no manual expiry check)

**No environment variables or manual configuration required!**

Requirements:

- Internet connection
- Browser access
- Longbridge account (register at https://open.longbridge.com)

**Token storage:** `~/.longbridge/openapi/tokens/<client_id>` (managed by SDK). Use `--logout` to clear.

**Troubleshooting:**

```bash
# View detailed OAuth flow logs
RUST_LOG=debug cargo run
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

## Longbridge SDK Reference

### Documentation

- Rust SDK (crates.io): longbridge 4.0.0
- OpenAPI Full Docs: https://open.longbridge.com/llms-full.txt
- Developer Portal: https://open.longbridge.com

### Common API Patterns

```rust
// Get quotes
let ctx = crate::openapi::quote();
let quotes = ctx.quote(vec!["700.HK", "AAPL.US"]).await?;

// Subscribe to real-time quotes
ctx.subscribe(&symbols, longbridge::quote::SubFlags::QUOTE).await?;

// Query candlesticks
let klines = ctx.candlesticks("AAPL.US", longbridge::quote::Period::Day, 100, None).await?;

// Submit order
let ctx = crate::openapi::trade();
let opts = longbridge::trade::SubmitOrderOptions::new(
 "700.HK",
 longbridge::trade::OrderType::LO,
 longbridge::trade::OrderSide::Buy,
 decimal!(500),
 longbridge::trade::TimeInForceType::Day,
);
let order = ctx.submit_order(opts).await?;
```

## Important Notes

1. **Rate Limiting**: Longbridge OpenAPI rate limits to "no more than 10 calls per second"
2. **Token Expiration**: The SDK automatically refreshes the access token when needed
3. **CN / Global token interoperability**: The `.cn` and `.com` OAuth endpoints share the same user data and token validation. A token issued by one endpoint is accepted by the other. The two regions differ only in routing/acceleration, not in auth logic. Do not treat region as a token-refresh issue.
4. **Market Support**: Supports Hong Kong, US, and China A-share markets
4. **Testing**: Per user instructions, update flow has no test coverage
5. **Logging**: Uses `tracing` library, log files configured via `logger::init()`

## Skills

For Ratatui-specific questions or when working with TUI components, use the `rs-ratatui-crate` skill.

## Commit and PR Title Conventions

Use a prefix to indicate the area of change. The word after the colon must be **capitalized**.

- `cli:` — changes to CLI commands (`src/cli/`) or shared infrastructure (`src/openapi/`, `src/region.rs`, `src/auth.rs`, etc.)
- `tui:` — changes that touch TUI-specific code (`src/tui/app.rs`, `src/tui/views/`, `src/tui/widgets/`, `src/tui/systems/`, etc.)
- `chore:` — other changes that don't fit the above (e.g. docs, formatting, refactors that don't modify behavior)

Only use `tui:` when the diff actually modifies TUI files. Changes to shared modules that happen to be triggered by a TUI bug should still use `cli:` or a more specific prefix.

Example: `cli: Add statement export command`, `tui: Fix quit confirmation dialog`

## Keeping Docs in Sync

When adding, removing, or modifying any CLI command (in `src/cli/`), always update all of the following in the same PR:

1. **`README.md`** — the `<!-- COMMANDS_START -->` / `<!-- COMMANDS_END -->` block
2. **`../developers/skills/longbridge/`** — all skill files are maintained in the `developers` repo; this repo no longer has its own `skills/` directory:
   - `SKILL.md` — quick reference, if the command is common enough to mention
   - `references/cli/overview.md` — CLI overview (features, patterns, notable flags)
   - `references/python-sdk/` / `references/rust-sdk/` — corresponding SDK reference

Skill files should stay high-level. Defer to the CLI's built-in `--help` for flag details — do not duplicate help text in skill files.

If `../developers` is not available locally, the repository is https://github.com/longbridge/developers
