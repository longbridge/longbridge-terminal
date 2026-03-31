# Longbridge Terminal

AI-native CLI for the [Longbridge](https://longbridge.com) trading platform — real-time market data, portfolio, and trading.

Covers every Longbridge OpenAPI endpoint: real-time quotes, depth, K-lines, options, and warrants for market data; account balances, stock and fund positions for portfolio management; and order submission, modification, cancellation, and execution history for trading. Designed for scripting, AI-agent tool-calling, and daily trading workflows from the terminal.

```bash
$ longbridge static NVDA.US
| Symbol  | Last    | Prev Close | Open    | High    | Low     | Volume    | Turnover        | Status |
|---------|---------|------------|---------|---------|---------|-----------|-----------------|--------|
| TSLA.US | 395.560 | 391.200    | 396.220 | 403.730 | 394.420 | 58068343  | 23138752546.000 | Normal |
| NVDA.US | 183.220 | 180.250    | 182.970 | 188.880 | 181.410 | 217307380 | 40023702698.000 | Normal |

$ longbridge quote TSLA.US NVDA.US --format json
[
  {
    "high": "403.730",
    "last": "395.560",
    "low": "394.420",
    "open": "396.220",
    "prev_close": "391.200",
    "status": "Normal",
    "symbol": "TSLA.US",
    "turnover": "23138752546.000",
    "volume": "58068343"
  },
  {
    "high": "188.880",
    "last": "183.220",
    "low": "181.410",
    "open": "182.970",
    "prev_close": "180.250",
    "status": "Normal",
    "symbol": "NVDA.US",
    "turnover": "40023702698.000",
    "volume": "217307380"
  }
]
```

Also ships a full-screen TUI for interactive monitoring.

[![asciicast](https://asciinema.org/a/785102.svg)](https://asciinema.org/a/785102)

## Installation

**Homebrew (macOS / Linux)**

```bash
brew install --cask longbridge/tap/longbridge-terminal
```

**Install script**

```bash
curl -sSL https://github.com/longbridge/longbridge-terminal/raw/main/install | sh
```

Installs the `longbridge` binary to `/usr/local/bin`.

## Authentication

Uses **OAuth 2.0** via the Longbridge SDK — no manual token management required.

```bash
longbridge login    # Opens browser for OAuth and saves token (managed by SDK)
longbridge logout   # Clear saved token
longbridge check    # Verify token, region, and API endpoint connectivity
```

Token is shared between CLI and TUI. After `login`, all commands work without re-authenticating.

The CLI auto-detects China Mainland on each startup by probing `geotest.lbkrs.com` in the background and caches the result. If detected, CN API endpoints are used automatically on the next run.

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

### Diagnostics

```bash
longbridge check   # Check token validity, and API connectivity
```

### Quotes

```bash
longbridge quote TSLA.US 700.HK                     # Real-time quotes for one or more symbols
longbridge depth TSLA.US                            # Level 2 order book depth (bid/ask ladder)
longbridge brokers 700.HK                           # Broker queue at each price level (HK market)
longbridge trades TSLA.US [--count 50]              # Recent tick-by-tick trades
longbridge intraday TSLA.US                         # Intraday minute-by-minute price and volume lines for today
longbridge kline TSLA.US [--period day]             # OHLCV candlestick (K-line) data
longbridge kline history TSLA.US --start 2024-01-01 # Historical OHLCV candlestick data within a date range
longbridge static TSLA.US                           # Static reference info for one or more symbols
longbridge calc-index TSLA.US --index pe,pb,eps     # Calculated financial indexes (PE, PB, EPS, turnover rate, etc.)
longbridge capital flow TSLA.US                     # Intraday capital flow time series (large/medium/small money in vs out)
longbridge capital dist TSLA.US                     # Capital distribution snapshot (large/medium/small inflow and outflow)
longbridge market-temp [HK|US|CN|SG]                # Market sentiment temperature index (0–100, higher = more bullish)
longbridge trading session                          # Trading session schedule (open/close times) for all markets
longbridge trading days HK                          # Trading days and half-trading days for a market
longbridge security-list HK                         # Full list of securities available in a market
longbridge participants                             # Market maker (participant) broker IDs and names
longbridge subscriptions                            # Active real-time WebSocket subscriptions for this session
```

### News

```bash
longbridge news TSLA.US [--count 20]             # Latest news articles for a symbol
longbridge news detail <id>                      # Full Markdown content of a news article
longbridge filing list AAPL.US [--count 20]      # Regulatory filings and announcements for a symbol
longbridge filing detail AAPL.US <id>            # Full Markdown content of a filing; --file-index N for multi-file filings (e.g. 8-K exhibit)
longbridge topic list TSLA.US [--count 20]       # Community discussion topics for a symbol
longbridge topic detail <id>                     # Full details of a community topic (body, author, tickers, counts, URL)
longbridge topic replies <id> [--page 1]         # Paginated list of replies for a topic (--size 1–50)
longbridge topic mine [--type article]           # Topics created by the authenticated user
longbridge topic create --body "…"               # Publish a new community discussion topic (--title optional)
longbridge topic create-reply <id> --body "…"    # Post a reply to a topic (--reply-to <reply_id> for nested replies)
```

### Options & Warrants

```bash
longbridge option AAPL240119C190000               # Real-time quote for an option contract
longbridge option chain AAPL.US                   # Option chain: list all expiry dates
longbridge option chain AAPL.US --date 2024-01-19 # Option chain: strike prices for a given expiry
longbridge warrant quote 12345.HK                 # Real-time quotes for warrant contracts
longbridge warrant list 700.HK                    # Warrants linked to an underlying security
longbridge warrant issuers                        # Warrant issuer list (HK market)
```

### Fundamentals

```bash
longbridge financial-report AAPL.US [--kind IS|BS|CF]               # Multi-period financial statements (income / balance sheet / cash flow)
longbridge institution-rating AAPL.US                                # Analyst rating distribution and consensus target price
longbridge institution-rating detail AAPL.US                         # Monthly rating trend and analyst accuracy history
longbridge dividend AAPL.US                                          # Historical dividend records
longbridge dividend detail AAPL.US                                   # Dividend allocation plan details
longbridge forecast-eps AAPL.US                                      # Analyst EPS consensus forecast snapshots
longbridge consensus AAPL.US                                         # Revenue / profit / EPS multi-period comparison with beat/miss markers
longbridge valuation AAPL.US [--indicator pe|pb|ps|dvd_yld]         # Current valuation snapshot and peer comparison
longbridge valuation AAPL.US --history [--indicator pe] [--range 5]  # Historical valuation time series (1 / 3 / 5 / 10 years)
longbridge fund-holder AAPL.US [--count 20]                          # Funds and ETFs holding this stock
longbridge shareholder AAPL.US [--range all|inc|dec] [--sort chg]    # Institutional shareholders with QoQ change tracking
longbridge insider-holding AAPL.US [--period 2024Q4]                 # Executive and director holdings with quarterly buy/sell totals
```

### Market Data

```bash
longbridge exchange-rate                                             # Exchange rates for all markets
longbridge my-rate                                                   # Personal commission rates and fee schedule
longbridge finance-calendar financial [--symbol AAPL.US]             # Earnings guidance announcements from today onward
longbridge finance-calendar report [--symbol AAPL.US]                # Earnings report release dates from today onward
longbridge finance-calendar dividend [--symbol AAPL.US]              # Dividend ex-date / payment events from today onward
longbridge finance-calendar ipo [--market US]                        # IPO listing timeline from today onward
longbridge finance-calendar macrodata [--star 3]                     # Macro economic events (--star 1–3 filters by importance)
longbridge finance-calendar closed [--market HK]                     # Market holidays and shortened trading days
```

### Watchlist

```bash
longbridge watchlist                               # List watchlist groups, or create/update/delete a group
longbridge watchlist create "My Portfolio"         # Create a new watchlist group
longbridge watchlist update <id> --add TSLA.US     # Add securities in a group
longbridge watchlist update <id> --remove AAPL.US  # Remove securities from a group
longbridge watchlist delete <id>                   # Delete a watchlist group
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

### Statements

```bash
longbridge statement list [--type daily|monthly]                        # List available account statements (daily or monthly)
longbridge statement export --file-key <KEY> --section equity_holdings  # Export statement sections as CSV or Markdown
longbridge statement export --file-key <KEY> --all                     # Export all non-empty sections
```

<!-- COMMANDS_END -->

### Symbol Format

```
<CODE>.<MARKET>   e.g.  TSLA.US   700.HK   600519.SH
```

Markets: `HK` (Hong Kong) · `US` (United States) · `CN` / `SH` / `SZ` (China A-share) · `SG` (Singapore)

## Skill

Install the skill to give your AI tools full knowledge of all `longbridge` CLI commands:

```bash
npx skills add longbridge/developers
```

More about Longbridge Skill, please visit: https://open.longbridge.com/skill/

Once installed, Claude can query market data, run technical analysis, and manage trades directly from your AI workflow.

```bash
claude> Show me recent 5 days performance of NVDA and TSLA

⏺ Bash(longbridge kline TSLA.US --period day --count 5 & longbridge kline NVDA.US --period day --count 5 & wait)
  ⎿  Authentication failed: oauth error: failed to refresh token: Failed to parse server response
     | Time                | Open    | High    | Low     | Close   | Volume   | Turnover        |
     |---------------------|---------|---------|---------|---------|----------|-----------------|
     … +11 lines (ctrl+o to expand)

⏺ It seems NVDA failed to authenticate (token refresh issue). TSLA data loaded fine. Here's what we have:

  TSLA.US — Last 5 Days

  ┌────────┬────────┬────────┬────────┬────────┬──────────┐
  │  Date  │  Open  │  High  │  Low   │ Close  │  Change  │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 10 │ 402.22 │ 406.59 │ 398.19 │ 399.24 │ —        │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 11 │ 402.28 │ 416.38 │ 402.15 │ 407.82 │ ▲ +2.15% │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 12 │ 405.18 │ 406.50 │ 394.65 │ 395.01 │ ▼ -3.14% │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 13 │ 399.17 │ 400.20 │ 389.95 │ 391.20 │ ▼ -0.96% │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 16 │ 396.22 │ 403.73 │ 394.42 │ 395.56 │ ▲ +1.11% │
  └────────┴────────┴────────┴────────┴────────┴──────────┘

  5-day return: -0.92% | Range: $389.95 – $416.38
```

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
