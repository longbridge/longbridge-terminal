# Longbridge Terminal

AI-native CLI for the [Longbridge](https://longbridge.com) trading platform — real-time market data, portfolio, and trading.

Covers every Longbridge OpenAPI endpoint: real-time quotes, depth, K-lines, options, and warrants for market data; account balances, stock and fund positions for portfolio management; and order submission, modification, cancellation, and execution history for trading. Designed for scripting, AI-agent tool-calling, and daily trading workflows from the terminal.

Also ships a full-screen TUI for interactive monitoring.

[![asciicast](https://asciinema.org/a/785102.svg)](https://asciinema.org/a/785102)

## Installation

```bash
curl -sSL https://github.com/longbridge/longbridge-terminal/raw/main/install | sh
```

Installs the `longbridge` binary to `/usr/local/bin`.

## Claude Code Skill

Install the skill to give Claude Code full knowledge of all `longbridge` CLI commands:

```bash
npx skills add longbridge/longbridge-terminal
```

Once installed, Claude can query market data, run technical analysis, and manage trades directly from your AI workflow.

## Authentication

Uses **OAuth 2.0** via the Longbridge SDK — no manual token management required.

```bash
longbridge login    # Opens browser for OAuth, saves token to ~/.longbridge/terminal/.openapi-session
longbridge logout   # Clear saved token
```

Token is shared between CLI and TUI. After `login`, all commands work without re-authenticating.

## CLI Usage

```
longbridge <command> [options]
```

All commands support `--format json` for machine-readable output:

```bash
longbridge quote TSLA.US --format json
longbridge positions --format json | jq '.[] | {symbol, quantity}'
```

<!-- COMMANDS_START -->

### Quotes

```bash
longbridge quote TSLA.US 700.HK                                       # Real-time quotes for one or more symbols
longbridge depth TSLA.US                                              # Level 2 order book depth (bid/ask ladder)
longbridge brokers 700.HK                                             # Broker queue at each price level (HK market)
longbridge trades TSLA.US [--count 50]                                # Recent tick-by-tick trades
longbridge intraday TSLA.US                                           # Intraday minute-by-minute price and volume lines for today
longbridge kline TSLA.US [--period day] [--count 100]                 # OHLCV candlestick (K-line) data
longbridge kline-history TSLA.US --start 2024-01-01 --end 2024-12-31 # Historical OHLCV candlestick data within a date range
longbridge static TSLA.US                                             # Static reference info for one or more symbols
longbridge calc-index TSLA.US --index pe,pb,eps                       # Calculated financial indexes (PE, PB, EPS, turnover rate, etc.)
longbridge capital-flow TSLA.US                                       # Intraday capital flow time series (large/medium/small money in vs out)
longbridge capital-dist TSLA.US                                       # Capital distribution snapshot (large/medium/small inflow and outflow)
longbridge market-temp [HK|US|CN|SG]                                  # Market sentiment temperature index (0–100, higher = more bullish)
longbridge trading-session                                            # Trading session schedule (open/close times) for all markets
longbridge trading-days HK                                            # Trading days and half-trading days for a market
longbridge security-list HK                                           # Full list of securities available in a market
longbridge participants                                               # Market maker (participant) broker IDs and names
longbridge subscriptions                                              # Active real-time WebSocket subscriptions for this session
```

### Options & Warrants

```bash
longbridge option-quote AAPL240119C190000         # Real-time quotes for option contracts
longbridge option-chain AAPL.US                   # Option chain: list all expiry dates
longbridge option-chain AAPL.US --date 2024-01-19 # Option chain: strike prices for a given expiry
longbridge warrant-quote 12345.HK                 # Real-time quotes for warrant contracts
longbridge warrant-list 700.HK                    # Warrants linked to an underlying security
longbridge warrant-issuers                        # Warrant issuer list (HK market)
```

### Watchlist

```bash
longbridge watchlist                                             # List watchlist groups, or create/update/delete a group
longbridge watchlist create "My Portfolio"                       # Create a new watchlist group
longbridge watchlist update <id> --add TSLA.US --remove AAPL.US  # Add/remove securities in a group, or rename it
longbridge watchlist delete <id>                                 # Delete a watchlist group
```

### Trading

```bash
longbridge orders                                      # Today's orders, or historical orders with --history
longbridge orders --history [--start 2024-01-01]       # Historical orders (use --symbol to filter)
longbridge order <order_id>                            # Full detail for a single order including charges and history
longbridge executions                                  # Today's trade executions (fills), or historical with --history
longbridge buy TSLA.US 100 --price 250.00              # Submit a buy order (prompts for confirmation)
longbridge sell TSLA.US 100 --price 260.00             # Submit a sell order (prompts for confirmation)
longbridge cancel <order_id>                           # Cancel a pending order (prompts for confirmation)
longbridge replace <order_id> --qty 200 --price 255.00 # Modify quantity or price of a pending order
longbridge balance                                     # Account cash balance and financing information
longbridge cash-flow [--start 2024-01-01]              # Cash flow records (deposits, withdrawals, dividends, settlements)
longbridge positions                                   # Current stock (equity) positions across all sub-accounts
longbridge fund-positions                              # Current fund (mutual fund) positions across all sub-accounts
longbridge margin-ratio TSLA.US                        # Margin ratio requirements for a symbol
longbridge max-qty TSLA.US --side buy --price 250      # Estimate maximum buy or sell quantity given current account balance
```

<!-- COMMANDS_END -->

### Symbol Format

```
<CODE>.<MARKET>   e.g.  TSLA.US   700.HK   600519.SH
```

Markets: `HK` (Hong Kong) · `US` (United States) · `CN` / `SH` / `SZ` (China A-share) · `SG` (Singapore)

## TUI

```bash
longbridge tui
```

Features: real-time watchlist, candlestick charts, portfolio view, stock search, Vim-like keybindings.

## Output Format

```bash
--format table   # Human-readable ASCII table (default)
--format json    # Machine-readable JSON, suitable for AI agents and piping
```

## Rate Limits

Longbridge OpenAPI: maximum 10 calls per second. The SDK auto-refreshes OAuth tokens.

## Requirements

- macOS or Linux
- Internet connection and browser access (for initial OAuth)
- [Longbridge account](https://open.longbridge.com)

## Documentation

- [Longbridge OpenAPI Docs](https://open.longbridge.com)
- [Rust SDK](https://longbridge.github.io/openapi/rust/longbridge/)

## License

MIT
