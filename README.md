# Longbridge Terminal

An AI-native CLI that wraps every [Longbridge OpenAPI](https://open.longbridge.com) endpoints — real-time quotes, order management, watchlists, options, warrants, and more. Designed for scripting, AI-agent tool-calling, and daily trading workflows from the terminal.

Also ships a full-screen TUI for interactive market monitoring.

[![asciicast](https://asciinema.org/a/785102.svg)](https://asciinema.org/a/785102)

## Installation

```bash
curl -sSL https://github.com/longbridge/longbridge-terminal/raw/main/install | sh
```

Installs the `longbridge` binary to `/usr/local/bin`.

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

### Quotes

| Command                                                                | Description                                                 |
| ---------------------------------------------------------------------- | ----------------------------------------------------------- |
| `longbridge quote TSLA.US 700.HK`                                      | Real-time quotes                                            |
| `longbridge depth TSLA.US`                                             | Level 2 order book (bid/ask)                                |
| `longbridge brokers 700.HK`                                            | Broker queue at each price level                            |
| `longbridge trades TSLA.US [--count 50]`                               | Recent tick-by-tick trades                                  |
| `longbridge intraday TSLA.US`                                          | Intraday minute-by-minute lines                             |
| `longbridge kline TSLA.US [--period day] [--count 100]`                | OHLCV candlesticks (`1m 5m 15m 30m 1h day week month year`) |
| `longbridge kline-history TSLA.US --start 2024-01-01 --end 2024-12-31` | Historical candlesticks by date range                       |
| `longbridge static TSLA.US`                                            | Reference info (name, lot size, currency, shares)           |
| `longbridge calc-index TSLA.US --index pe,pb,eps`                      | Calculated indexes (PE, PB, EPS, turnover rate…)            |
| `longbridge capital-flow TSLA.US`                                      | Intraday capital flow time series                           |
| `longbridge capital-dist TSLA.US`                                      | Capital distribution (large/medium/small)                   |
| `longbridge market-temp [HK\|US\|CN\|SG]`                              | Market sentiment temperature (0–100)                        |
| `longbridge trading-session`                                           | Trading session schedule for all markets                    |
| `longbridge trading-days HK`                                           | Trading calendar                                            |
| `longbridge security-list HK`                                          | Full security list for a market                             |
| `longbridge participants`                                              | Market maker broker list                                    |
| `longbridge subscriptions`                                             | Active WebSocket subscriptions                              |

### Options & Warrants

| Command                                             | Description                         |
| --------------------------------------------------- | ----------------------------------- |
| `longbridge option-quote AAPL240119C190000`         | Option contract quotes              |
| `longbridge option-chain AAPL.US`                   | Option chain expiry dates           |
| `longbridge option-chain AAPL.US --date 2024-01-19` | Strike prices for a specific expiry |
| `longbridge warrant-quote 12345.HK`                 | Warrant quotes                      |
| `longbridge warrant-list 700.HK`                    | Warrants linked to an underlying    |
| `longbridge warrant-issuers`                        | Warrant issuer list                 |

### Watchlist

| Command                                                           | Description                          |
| ----------------------------------------------------------------- | ------------------------------------ |
| `longbridge watchlist`                                            | List all groups and their securities |
| `longbridge watchlist create "My Portfolio"`                      | Create a group                       |
| `longbridge watchlist update <id> --add TSLA.US --remove AAPL.US` | Add/remove securities                |
| `longbridge watchlist delete <id>`                                | Delete a group                       |

### Trading

| Command                                                  | Description                                 |
| -------------------------------------------------------- | ------------------------------------------- |
| `longbridge orders`                                      | Today's orders                              |
| `longbridge orders --history [--start 2024-01-01]`       | Historical orders                           |
| `longbridge order <order_id>`                            | Full order detail                           |
| `longbridge executions`                                  | Today's fills                               |
| `longbridge buy TSLA.US 100 --price 250.00`              | Submit buy order (prompts for confirmation) |
| `longbridge sell TSLA.US 100 --price 260.00`             | Submit sell order                           |
| `longbridge cancel <order_id>`                           | Cancel a pending order                      |
| `longbridge replace <order_id> --qty 200 --price 255.00` | Modify a pending order                      |
| `longbridge balance`                                     | Account cash balance                        |
| `longbridge cash-flow [--start 2024-01-01]`              | Cash flow records                           |
| `longbridge positions`                                   | Stock positions                             |
| `longbridge fund-positions`                              | Fund positions                              |
| `longbridge margin-ratio TSLA.US`                        | Margin ratio requirements                   |
| `longbridge max-qty TSLA.US --side buy --price 250`      | Estimate max buy/sell quantity              |

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
