# Longbridge Terminal

AI-native CLI for the [Longbridge](https://longbridge.com) trading platform — real-time market data, portfolio, and trading. Also ships a full-screen TUI for interactive monitoring.

Covers every Longbridge OpenAPI endpoint: real-time quotes, depth, K-lines, options, and warrants for market data; account balances, stock and fund positions for portfolio management; and order submission, modification, cancellation, and execution history for trading. Designed for scripting, AI-agent tool-calling, and daily trading workflows from the terminal.

```bash
$ longbridge static TSLA.US NVDA.US
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

[![asciicast](https://asciinema.org/a/785102.svg)](https://asciinema.org/a/785102)

## Installation

**Homebrew (macOS / Linux)**

```bash
brew install --cask longbridge/tap/longbridge-terminal
```

**Windows** ([Scoop](https://scoop.sh))

```powershell
scoop install https://github.com/longbridge/longbridge-terminal/raw/refs/heads/main/.scoop/longbridge.json
```

**Windows** (PowerShell)

```powershell
iwr https://github.com/longbridge/longbridge-terminal/raw/main/install.ps1 | iex
```

**Install script (macOS / Linux)**

```bash
curl -sSL https://github.com/longbridge/longbridge-terminal/raw/main/install | sh
```

Installs the `longbridge` binary to `/usr/local/bin` (macOS/Linux) or `%LOCALAPPDATA%\Programs\longbridge` (Windows).

## Authentication

Uses **OAuth 2.0** via the Longbridge SDK — no manual token management required.

```bash
longbridge auth login    # Opens browser for OAuth and saves token (managed by SDK)
longbridge auth logout   # Clear saved token
longbridge check    # Verify token, region, and API endpoint connectivity
```

Token is shared between CLI and TUI. After `login`, all commands work without re-authenticating.

The CLI auto-detects China Mainland on each startup by probing `geotest.lbkrs.com` in the background and caches the result. If detected, CN API endpoints are used automatically on the next run.

## Shell Completion

Enable tab-completion for `longbridge` commands and flags in your shell:

**Bash** — add to `~/.bashrc` or `~/.bash_profile`:

```bash
source <(longbridge completion bash)
```

**Zsh** — add to `~/.zshrc`:

```zsh
source <(longbridge completion zsh)
```

**Fish** — add to `~/.config/fish/config.fish`:

```fish
longbridge completion fish | source
```

After reloading your shell, `longbridge <TAB>` will suggest subcommands, flags, and values.

## CLI Usage

```
longbridge <command> [options]
```

All commands support `--format json` for machine-readable output. Commands that accept `--count` also accept `--limit` as an alias (for AI agent compatibility):

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
longbridge kline TSLA.US [--period day]             # OHLCV candlestick (K-line) data [--adjust none|forward]
longbridge kline history TSLA.US --start 2024-01-01 # Historical OHLCV candlestick data within a date range
longbridge static TSLA.US                            # Static reference info for one or more symbols
longbridge calc-index TSLA.US --fields pe,pb,eps     # Calculated financial indexes (PE, PB, EPS, turnover rate, etc.)
longbridge capital TSLA.US                          # Capital distribution snapshot (large/medium/small inflow and outflow)
longbridge capital TSLA.US --flow                   # Intraday capital flow time series (large/medium/small money in vs out)
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
longbridge option quote AAPL240119C190000          # Real-time quotes for option contracts
longbridge option chain AAPL.US                   # Option chain: list all expiry dates
longbridge option chain AAPL.US --date 2024-01-19 # Option chain: strike prices for a given expiry
longbridge option volume AAPL.US                  # Real-time option Call/Put volume and Put/Call ratio
longbridge option volume daily AAPL.US            # Daily option Call/Put volume and open interest history
longbridge option volume daily AAPL.US --count 60 # Return last 60 trading days
longbridge warrant quote 12345.HK                 # Real-time quotes for warrant contracts
longbridge warrant 700.HK                         # Warrants linked to an underlying security
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
longbridge corp-action 700.HK [--all]                                 # Corporate actions (splits, dividends, rights, etc.) — default 30, --all for full history
```

### Market Data

```bash
longbridge exchange-rate                                             # Exchange rates for all markets
longbridge finance-calendar financial [--symbol AAPL.US]             # Earnings guidance announcements from today onward
longbridge finance-calendar report [--symbol AAPL.US]                # Earnings report release dates from today onward
longbridge finance-calendar dividend [--symbol AAPL.US]              # Dividend ex-date / payment events from today onward
longbridge finance-calendar ipo [--market US]                        # IPO listing timeline from today onward
longbridge finance-calendar macrodata [--star 3]                     # Macro economic events (--star 1–3 filters by importance)
longbridge finance-calendar closed [--market HK]                     # Market holidays and shortened trading days
```

### Watchlist

```bash
longbridge watchlist                               # List all watchlist groups and their securities (pinned shown first)
longbridge watchlist show <id|name>                # Show securities in a specific group (pinned marked)
longbridge watchlist create "My Portfolio"         # Create a new watchlist group
longbridge watchlist update <id> --add TSLA.US     # Add securities in a group
longbridge watchlist update <id> --remove AAPL.US  # Remove securities from a group
longbridge watchlist delete <id>                   # Delete a watchlist group
longbridge watchlist pin TSLA.US AAPL.US           # Pin securities to the top of their group
longbridge watchlist pin --remove 700.HK           # Unpin securities
```

### Sharelist

```bash
longbridge sharelist                                              # List own and subscribed sharelists
longbridge sharelist [--count 50]                                 # List with custom page size
longbridge sharelist detail <id>                                  # Show full details and constituent stocks
longbridge sharelist create --name "My Picks" [--description "…"] # Create a new sharelist
longbridge sharelist delete <id>                                  # Delete a sharelist
longbridge sharelist add <id> TSLA.US AAPL.US 700.HK             # Add stocks to a sharelist
longbridge sharelist remove <id> TSLA.US                          # Remove stocks from a sharelist
longbridge sharelist sort <id> TSLA.US AAPL.US 700.HK            # Reorder stocks in a sharelist
longbridge sharelist popular [--count 10]                         # Get popular (trending) sharelists
```

### Trading

```bash
longbridge order                                           # Today's orders, or historical with --history
longbridge order --history [--start 2024-01-01]            # Historical orders (use --symbol to filter)
longbridge order detail <order_id>                         # Full detail for a single order including charges and history
longbridge order executions                                # Today's trade executions (fills), or historical with --history
longbridge order buy TSLA.US 100 --price 250.00            # Submit a buy order (prompts for confirmation)
longbridge order sell TSLA.US 100 --price 260.00           # Submit a sell order (prompts for confirmation)
longbridge order cancel <order_id>                         # Cancel a pending order (prompts for confirmation)
longbridge order replace <order_id> --qty 200 --price 255.00 # Modify quantity or price of a pending order
longbridge assets [--currency USD]                         # Asset overview: net assets, cash, buy power, margins, and per-currency breakdown
longbridge cash-flow [--start 2024-01-01]                  # Cash flow records (deposits, withdrawals, dividends, settlements)
longbridge positions                                       # Current stock (equity) positions across all sub-accounts
longbridge fund-positions                                  # Current fund (mutual fund) positions across all sub-accounts
longbridge margin-ratio TSLA.US                            # Margin ratio requirements for a symbol
longbridge max-qty TSLA.US --side buy --price 250          # Estimate maximum buy or sell quantity given current account balance
```

### Profit Analysis

```bash
longbridge profit-analysis                                  # P&L summary with stock breakdown
longbridge profit-analysis detail 700.HK                    # Stock P&L breakdown + transaction flows
longbridge profit-analysis detail 700.HK --derivative       # Show derivative flows
longbridge profit-analysis by-market                        # Stock P&L by market (paginated)
longbridge profit-analysis by-market --market HK --size 50  # Filter by market
```

### Statements

```bash
longbridge statement list [--type daily|monthly]                        # List available account statements (daily or monthly)
longbridge statement export --file-key <KEY> --section equity_holdings  # Export statement sections as CSV or Markdown
longbridge statement export --file-key <KEY> --all                     # Export all non-empty sections
```

### Insider Trades

```bash
longbridge insider-trades TSLA.US                 # Recent Form 4 insider trades (SEC EDGAR, US stocks only)
longbridge insider-trades AAPL.US --count 40      # Fetch 40 Form 4 filings instead of the default 20
longbridge insider-trades NVDA.US --format json   # Export as JSON
```

### Investors

```bash
longbridge investors                                          # Top 50 active fund managers by AUM (live SEC 13F rankings; passive index giants excluded; use --top N to change)
longbridge investors 0001067983                               # View 13F holdings for any filer by SEC CIK number
longbridge investors 0001067983 --top 20                      # Show top 20 positions only
longbridge investors 0001067983 --format json                 # Export holdings as JSON
longbridge investors changes 0001067983                       # Quarter-over-quarter changes (NEW/ADDED/REDUCED/EXITED)
longbridge investors changes 0001067983 --from 2024-12-31     # Compare latest vs a specific period
```

### Recurring Investment

```bash
longbridge dca                                                # List all recurring investment plans
longbridge dca --status Active                                # Filter by status: Active | Suspended | Finished
longbridge dca --symbol TSLA.US                               # Filter by symbol
longbridge dca create TSLA.US --amount 500 --frequency weekly --day-of-week mon  # Create weekly recurring investment plan
longbridge dca create 700.HK --amount 1000 --frequency monthly --day-of-month 15  # Monthly recurring investment plan
longbridge dca update <PLAN_ID> --amount 800                  # Update plan amount
longbridge dca pause <PLAN_ID>                                # Pause a recurring investment plan
longbridge dca resume <PLAN_ID>                               # Resume a paused recurring investment plan
longbridge dca stop <PLAN_ID>                                 # Permanently stop a recurring investment plan
longbridge dca history <PLAN_ID>                              # Trade history for a plan
longbridge dca stats                                          # Recurring investment statistics summary
longbridge dca calc-date TSLA.US --frequency weekly --day-of-week fri  # Calculate next trade date
longbridge dca check TSLA.US AAPL.US 700.HK                  # Check which symbols support recurring investment
longbridge dca set-reminder 6                                 # Set reminder hours before trade (1 | 6 | 12)
```

### Short Selling

```bash
longbridge short-positions AAPL.US                            # US stock short selling data (short interest, ratio, days to cover)
longbridge short-positions TSLA.US --count 50                 # Return last 50 short interest records
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

```
claude> Show me TSLA and NVDA performance over the last 5 days

● Bash(longbridge kline TSLA.US --period day --count 5 & longbridge kline NVDA.US --period day --count 5 & wait)

● Here's the 5-day performance for both:

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

  NVDA.US — Last 5 Days

  ┌────────┬────────┬────────┬────────┬────────┬──────────┐
  │  Date  │  Open  │  High  │  Low   │ Close  │  Change  │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 10 │ 112.34 │ 115.20 │ 111.80 │ 114.50 │ —        │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 11 │ 114.80 │ 117.60 │ 114.20 │ 116.90 │ ▲ +2.10% │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 12 │ 116.50 │ 118.30 │ 115.40 │ 115.80 │ ▼ -0.94% │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 13 │ 115.20 │ 116.80 │ 113.90 │ 114.60 │ ▼ -1.04% │
  ├────────┼────────┼────────┼────────┼────────┼──────────┤
  │ Mar 16 │ 114.90 │ 117.50 │ 114.30 │ 116.80 │ ▲ +1.92% │
  └────────┴────────┴────────┴────────┴────────┴──────────┘

  5-day return: +2.01% | Range: $111.80 – $118.30
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

- macOS, Linux, or Windows
- Internet connection and browser access (for initial OAuth)
- [Longbridge account](https://open.longbridge.com)

## Documentation

- [Longbridge OpenAPI Docs](https://open.longbridge.com)
- [Rust SDK](https://longbridge.github.io/openapi/rust/longbridge/)

## License

MIT
