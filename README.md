# LongPort Terminal

AI-native CLI for the [LongPort](https://longportapp.com) trading platform — real-time market data, portfolio, and trading.

Covers every LongPort OpenAPI endpoint: real-time quotes, depth, K-lines, options, and warrants for market data; account balances, stock and fund positions for portfolio management; and order submission, modification, cancellation, and execution history for trading. Designed for scripting, AI-agent tool-calling, and daily trading workflows from the terminal.

```bash
$ longport static TSLA.US NVDA.US
| Symbol  | Last    | Prev Close | Open    | High    | Low     | Volume    | Turnover        | Status |
|---------|---------|------------|---------|---------|---------|-----------|-----------------|--------|
| TSLA.US | 395.560 | 391.200    | 396.220 | 403.730 | 394.420 | 58068343  | 23138752546.000 | Normal |
| NVDA.US | 183.220 | 180.250    | 182.970 | 188.880 | 181.410 | 217307380 | 40023702698.000 | Normal |

$ longport quote TSLA.US NVDA.US --format json
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
brew install --cask longport/tap/longport-terminal
```

**Windows** ([Scoop](https://scoop.sh))

```powershell
scoop install https://github.com/longportapp/longport-terminal/raw/refs/heads/main/.scoop/longport.json
```

**Windows** (PowerShell)

```powershell
iwr https://github.com/longportapp/longport-terminal/raw/main/install.ps1 | iex
```

**Install script (macOS / Linux)**

```bash
curl -sSL https://github.com/longportapp/longport-terminal/raw/main/install | sh
```

Installs the `longport` binary to `/usr/local/bin` (macOS/Linux) or `%LOCALAPPDATA%\Programs\longport` (Windows).

## Authentication

Uses **OAuth 2.0** via the LongPort SDK — no manual token management required.

```bash
longport auth login    # Opens browser for OAuth and saves token (managed by SDK)
longport auth logout   # Clear saved token
longport check    # Verify token, region, and API endpoint connectivity
```

After `login`, all commands work without re-authenticating.

The CLI auto-detects China Mainland on each startup by probing `geotest.lbkrs.com` in the background and caches the result. If detected, CN API endpoints are used automatically on the next run.

## Shell Completion

Enable tab-completion for `longport` commands and flags in your shell:

**Bash** — add to `~/.bashrc` or `~/.bash_profile`:

```bash
source <(longport completion bash)
```

**Zsh** — add to `~/.zshrc`:

```zsh
source <(longport completion zsh)
```

**Fish** — add to `~/.config/fish/config.fish`:

```fish
longport completion fish | source
```

After reloading your shell, `longport <TAB>` will suggest subcommands, flags, and values.

## CLI Usage

```
longport <command> [options]
```

All commands support `--format json` for machine-readable output. Commands that accept `--count` also accept `--limit` as an alias (for AI agent compatibility):

```bash
longport quote TSLA.US --format json
longport positions --format json | jq '.[] | {symbol, quantity}'
```

<!-- COMMANDS_START -->

### Diagnostics

```bash
longport check   # Check token validity, and API connectivity
```

### Quotes

```bash
longport quote TSLA.US 700.HK                     # Real-time quotes for one or more symbols
longport depth TSLA.US                            # Level 2 order book depth (bid/ask ladder)
longport brokers 700.HK                           # Broker queue at each price level (HK market)
longport trades TSLA.US [--count 50]              # Recent tick-by-tick trades
longport intraday TSLA.US                         # Intraday minute-by-minute price and volume lines for today
longport kline TSLA.US [--period day]             # OHLCV candlestick (K-line) data [--adjust none|forward]
longport kline history TSLA.US --start 2024-01-01 # Historical OHLCV candlestick data within a date range
longport static TSLA.US                            # Static reference info for one or more symbols
longport calc-index TSLA.US --fields pe,pb,eps     # Calculated financial indexes (PE, PB, EPS, turnover rate, etc.)
longport capital TSLA.US                          # Capital distribution snapshot (large/medium/small inflow and outflow)
longport capital TSLA.US --flow                   # Intraday capital flow time series (large/medium/small money in vs out)
longport market-temp [HK|US|CN|SG]                # Market sentiment temperature index (0–100, higher = more bullish)
longport constituent .SPX.US [--sort market-cap]  # Index constituent stocks (US indexes need a leading dot, e.g. .DJI.US, .SPX.US)
longport constituent IVV.US [--limit 0]           # For a US ETF, full holdings from SEC N-PORT (--limit 0 = all); falls back to platform asset allocation when SEC data is unavailable (e.g. SPY)
longport trading session                          # Trading session schedule (open/close times) for all markets
longport trading days HK                          # Trading days and half-trading days for a market
longport security-list HK                         # Full list of securities available in a market
longport participants                             # Market maker (participant) broker IDs and names
longport subscriptions                            # Active real-time WebSocket subscriptions for this session
```

### News

```bash
longport news TSLA.US [--count 20]             # Latest news articles for a symbol
longport news detail <id>                      # Full Markdown content of a news article
longport filing list AAPL.US [--count 20]      # Regulatory filings and announcements for a symbol
longport filing detail AAPL.US <id>            # Full Markdown content of a filing; --file-index N for multi-file filings (e.g. 8-K exhibit)
longport topic list TSLA.US [--count 20]       # Community discussion topics for a symbol
longport topic detail <id>                     # Full details of a community topic (body, author, tickers, counts, URL)
longport topic replies <id> [--page 1]         # Paginated list of replies for a topic (--size 1–50)
longport topic mine [--type article]           # Topics created by the authenticated user
longport topic create --body "…"               # Publish a new community discussion topic (--title optional)
longport topic create-reply <id> --body "…"    # Post a reply to a topic (--reply-to <reply_id> for nested replies)
```

### Options & Warrants

```bash
longport option quote AAPL240119C190000          # Real-time quotes for option contracts
longport option chain AAPL.US                   # Option chain: list all expiry dates
longport option chain AAPL.US --date 2024-01-19 # Option chain: strike prices for a given expiry
longport option volume AAPL.US                  # Real-time option Call/Put volume and Put/Call ratio
longport option volume daily AAPL.US            # Daily option Call/Put volume and open interest history
longport option volume daily AAPL.US --count 60 # Return last 60 trading days
longport warrant quote 12345.HK                 # Real-time quotes for warrant contracts
longport warrant 700.HK                         # Warrants linked to an underlying security
longport warrant issuers                        # Warrant issuer list (HK market)
```

### Fundamentals

```bash
longport financial-report AAPL.US [--kind IS|BS|CF]               # Multi-period financial statements (income / balance sheet / cash flow)
longport financial-report AAPL.US --latest                         # Latest financial report summary
longport financial-report snapshot AAPL.US --report qf --year N --period N  # Earnings summary + forecast vs actual (revenue/EBIT/EPS beat/miss) + financial ratios
longport financial-statement AAPL.US [--kind IS|BS|CF|ALL] [--report af|saf|qf|cumul]  # Detailed financial statement (v3 endpoint)
longport institution-rating AAPL.US                                # Analyst rating distribution and consensus target price
longport institution-rating AAPL.US --history                      # Rating and target price change history
longport institution-rating AAPL.US --industry-rank [--page 1] [--limit 20]  # Industry-wide institution rating ranking
longport institution-rating AAPL.US --views                        # Monthly buy/hold/sell distribution timeline (institutional views)
longport institution-rating detail AAPL.US                         # Monthly rating trend and analyst accuracy history
longport dividend AAPL.US                                          # Historical dividend records
longport dividend detail AAPL.US                                   # Dividend allocation plan details
longport forecast-eps AAPL.US                                      # Analyst EPS consensus forecast snapshots
longport consensus AAPL.US                                         # Revenue / profit / EPS multi-period comparison with beat/miss markers
longport valuation AAPL.US [--indicator pe|pb|ps|dvd_yld]         # Current valuation snapshot and peer comparison
longport valuation AAPL.US --history [--indicator pe] [--range 5]  # Historical valuation time series (1 / 3 / 5 / 10 years)
longport valuation-rank AAPL.US [--start 20240101] [--end 20241231] # Industry valuation percentile ranking (default: last 30 days)
longport analyst-estimates AAPL.US                                 # Analyst consensus EPS estimates
longport fund-holder AAPL.US [--count 20]                          # Funds and ETFs holding this stock
longport shareholder AAPL.US [--range all|inc|dec] [--sort chg]    # Institutional shareholders with QoQ change tracking
longport shareholder AAPL.US --top                                  # Top 20 major shareholders (includes individuals and insiders, multi-period)
longport shareholder AAPL.US --object-id <ID>                       # Holding and trade detail for a specific shareholder (use ID from --top output)
longport compare AAPL.US                                            # Multi-stock valuation comparison vs server-selected industry peers
longport compare 9988.HK 700.HK 9999.HK [--currency HKD]           # Compare specific stocks side by side (price, market cap, PE/PB/PS, ROE, ROA, div yield, and more)
longport corp-action 700.HK [--all]                                 # Corporate actions (splits, dividends, rights, etc.) — default 30, --all for full history
longport business-segments AAPL.US [--history] [--report qf|saf|af] [--cate <cate>]  # Revenue segment breakdown (current snapshot or historical trends)
longport industry-rank --market US|HK|CN|SG [--indicator leading-gainer|...|net-profit-growth]  # Industry ranking list; output symbols feed into industry-peers
longport industry-peers IN00446.US                                  # Industry peer group hierarchy tree for an industry index symbol (from industry-rank)
```

### Deposits & Withdrawals

```bash
longport bank-cards                                               # List linked bank cards
longport withdrawals [--page 1] [--limit 20]                      # Withdrawal history
longport deposits [--page 1] [--limit 20] [--states 0,1,2] [--currencies HKD,USD]  # Deposit history
```

### Search

```bash
longport search TSLA [--tab market|news|posts|hashtags|help|share-lists|users|institutions]  # Search across multiple content types
longport search-hot                                               # Hot search keywords
```

### IPO

```bash
longport ipo subscriptions                                        # IPO stocks currently in filing or subscription stage
longport ipo wait-listing                                         # IPO stocks in grey-market (wait-listing) stage
longport ipo listed [--page 1] [--limit 20]                       # Recently listed IPO stocks
longport ipo calendar                                             # IPO calendar (all upcoming and recent IPOs)
longport ipo detail <symbol> [--market HK|US]                     # IPO profile, timeline, eligibility, and holdings for a symbol
longport ipo orders [--market HK] [--status 0] [--page 1]         # IPO orders (active + history) for the current account
longport ipo orders detail <order_id>                             # Full detail for a single IPO order
longport ipo profit-loss [--period all|1m|3m|6m|1y] [--page 1]   # IPO P&L summary and item list
longport ipo us-subscriptions                                     # US IPO stocks currently in subscription stage
longport ipo us-wait-listing                                      # US IPO stocks in wait-listing stage
longport ipo us-listed [--page 1] [--limit 20]                    # Recently listed US IPO stocks
longport ipo submit TSLA.US --qty 200 --amount 1000 [--method 2]  # Submit IPO subscription (prompts for confirmation)
longport ipo withdraw <order_id>                                  # Withdraw IPO subscription (prompts for confirmation)
```

### Market Data

```bash
longport rank                                                      # List available popularity ranking tab keys
longport rank --key ib_hot_all-us [--count 20]                     # Stocks ranked by composite heat score (trading activity, media, community, volatility)
longport top-movers [--market HK|US|CN|SG] [--sort hot|time|chg]  # Stocks with abnormal price moves paired with correlated news and reason summaries
longport exchange-rate                                             # Exchange rates for all markets
longport finance-calendar financial [--symbol AAPL.US]             # Earnings guidance announcements from today onward
longport finance-calendar report [--symbol AAPL.US]                # Earnings report release dates from today onward
longport finance-calendar dividend [--symbol AAPL.US]              # Dividend ex-date / payment events from today onward
longport finance-calendar ipo [--market US]                        # IPO listing timeline from today onward
longport finance-calendar macrodata [--star 3]                     # Macro economic events (--star 1–3 filters by importance)
longport finance-calendar closed [--market HK]                     # Market holidays and shortened trading days
```

### Watchlist

```bash
longport watchlist                               # List all watchlist groups and their securities (pinned shown first)
longport watchlist show <id|name>                # Show securities in a specific group (pinned marked)
longport watchlist create "My Portfolio"         # Create a new watchlist group
longport watchlist update <id> --add TSLA.US     # Add securities in a group
longport watchlist update <id> --remove AAPL.US  # Remove securities from a group
longport watchlist delete <id>                   # Delete a watchlist group
longport watchlist pin TSLA.US AAPL.US           # Pin securities to the top of their group
longport watchlist pin --remove 700.HK           # Unpin securities
```

### Sharelist

```bash
longport sharelist                                              # List own and subscribed sharelists
longport sharelist [--count 50]                                 # List with custom page size
longport sharelist detail <id>                                  # Show full details and constituent stocks
longport sharelist create --name "My Picks" [--description "…"] # Create a new sharelist
longport sharelist delete <id>                                  # Delete a sharelist
longport sharelist add <id> TSLA.US AAPL.US 700.HK             # Add stocks to a sharelist
longport sharelist remove <id> TSLA.US                          # Remove stocks from a sharelist
longport sharelist sort <id> TSLA.US AAPL.US 700.HK            # Reorder stocks in a sharelist
longport sharelist popular [--count 10]                         # Get popular (trending) sharelists
```

### Trading

```bash
longport order                                           # Today's orders, or historical with --history
longport order --history [--start 2024-01-01]            # Historical orders (use --symbol to filter)
longport order detail <order_id>                         # Full detail for a single order including charges and history
longport order executions                                # Today's trade executions (fills), or historical with --history
longport order buy TSLA.US 100 --price 250.00            # Submit a buy order (prompts for confirmation)
longport order sell TSLA.US 100 --price 260.00           # Submit a sell order (prompts for confirmation)
longport order cancel <order_id>                         # Cancel a pending order (prompts for confirmation)
longport order replace <order_id> --qty 200 --price 255.00 # Modify quantity or price of a pending order
longport assets [--currency USD]                         # Asset overview: net assets, cash, buy power, margins, and per-currency breakdown
longport cash-flow [--start 2024-01-01]                  # Cash flow records (deposits, withdrawals, dividends, settlements)
longport portfolio                                       # Portfolio overview: total assets, P/L, holdings, and cash breakdown
longport portfolio short-margin                          # Short-selling margin deposit details
longport positions                                       # Current stock (equity) positions across all sub-accounts
longport fund-positions                                  # Current fund (mutual fund) positions across all sub-accounts
longport margin-ratio TSLA.US                            # Margin ratio requirements for a symbol
longport max-qty TSLA.US --side buy --price 250          # Estimate maximum buy or sell quantity given current account balance
```

### Profit Analysis

```bash
longport profit-analysis                                  # P&L summary with stock breakdown
longport profit-analysis detail 700.HK                    # Stock P&L breakdown + transaction flows
longport profit-analysis detail 700.HK --derivative       # Show derivative flows
longport profit-analysis by-market                        # Stock P&L by market (paginated)
longport profit-analysis by-market --market HK --size 50  # Filter by market
```

### Statements

```bash
longport statement list [--type daily|monthly]                        # List available account statements (daily or monthly)
longport statement export --file-key <KEY> --section equity_holdings  # Export statement sections as CSV or Markdown
longport statement export --file-key <KEY> --all                     # Export all non-empty sections
```

### Insider Trades

```bash
longport insider-trades TSLA.US                 # Recent Form 4 insider trades (SEC EDGAR, US stocks only)
longport insider-trades AAPL.US --count 40      # Fetch 40 Form 4 filings instead of the default 20
longport insider-trades NVDA.US --format json   # Export as JSON
```

### Investors

```bash
longport investors                                          # Top 50 active fund managers by AUM (live SEC 13F rankings; passive index giants excluded; use --top N to change)
longport investors 0001067983                               # View 13F holdings for any filer by SEC CIK number
longport investors 0001067983 --top 20                      # Show top 20 positions only
longport investors 0001067983 --format json                 # Export holdings as JSON
longport investors changes 0001067983                       # Quarter-over-quarter changes (NEW/ADDED/REDUCED/EXITED)
longport investors changes 0001067983 --from 2024-12-31     # Compare latest vs a specific period
```

### Recurring Investment

```bash
longport dca                                                # List all recurring investment plans
longport dca --status Active                                # Filter by status: Active | Suspended | Finished
longport dca --symbol TSLA.US                               # Filter by symbol
longport dca create TSLA.US --amount 500 --frequency weekly --day-of-week mon  # Create weekly recurring investment plan
longport dca create 700.HK --amount 1000 --frequency monthly --day-of-month 15  # Monthly recurring investment plan
longport dca update <PLAN_ID> --amount 800                  # Update plan amount
longport dca pause <PLAN_ID>                                # Pause a recurring investment plan
longport dca resume <PLAN_ID>                               # Resume a paused recurring investment plan
longport dca stop <PLAN_ID>                                 # Permanently stop a recurring investment plan
longport dca history <PLAN_ID>                              # Trade history for a plan
longport dca stats                                          # Recurring investment statistics summary
longport dca calc-date TSLA.US --frequency weekly --day-of-week fri  # Calculate next trade date
longport dca check TSLA.US AAPL.US 700.HK                  # Check which symbols support recurring investment
longport dca set-reminder 6                                 # Set reminder hours before trade (1 | 6 | 12)
```

### Short Selling

```bash
longport short-positions AAPL.US                            # US: bi-weekly FINRA short interest (short interest, rate, days to cover)
longport short-positions 700.HK                             # HK: daily HKEX disclosed short positions (open short shares, balance, cost, rate)
longport short-positions TSLA.US --count 50                 # Return last 50 records
longport short-trades AAPL.US                               # Daily short sale volume (FINRA/NASDAQ for US; HKEX for HK)
longport short-trades 700.HK [--count 50]                   # HK: amount, balance, total amount, rate, close per trading day
```

### Stock Screener

```bash
longport screener strategies                                # List recommended stock-selection strategies
longport screener strategies --all                          # List all platform strategies
longport screener strategies --mine                         # List user-created strategies
longport screener strategies --id <ID>                      # Show groups and indicators for a specific strategy
longport screener search --strategy-id <ID>                 # Run a saved strategy and return matching stocks
longport screener search --market HK --filter filter_marketcap:100:1000 --filter filter_divyld:3:  # Custom filter (key:min:max, omit bound to leave open)
longport screener indicators                                # List all available filter indicators with IDs, keys, and default value ranges
```

<!-- COMMANDS_END -->

### Symbol Format

```
<CODE>.<MARKET>   e.g.  TSLA.US   700.HK   600519.SH
```

Markets: `HK` (Hong Kong) · `US` (United States) · `CN` / `SH` / `SZ` (China A-share) · `SG` (Singapore)

## Skill

Install the skill to give your AI tools full knowledge of all `longport` CLI commands:

```bash
npx skills add longport/developers
```

More about LongPort Skill, please visit: https://open.longportapp.com/skill/

Once installed, Claude can query market data, run technical analysis, and manage trades directly from your AI workflow.

```
claude> Show me TSLA and NVDA performance over the last 5 days

● Bash(longport kline TSLA.US --period day --count 5 & longport kline NVDA.US --period day --count 5 & wait)

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

## Output Format

```bash
--format table   # Human-readable ASCII table (default)
--format json    # Machine-readable JSON, suitable for AI agents and piping
```

## Rate Limits

LongPort OpenAPI: maximum 10 calls per second. The SDK auto-refreshes OAuth tokens.

## Requirements

- macOS, Linux, or Windows
- Internet connection and browser access (for initial OAuth)
- [LongPort account](https://open.longportapp.com)

## Documentation

- [LongPort OpenAPI Docs](https://open.longportapp.com)
- [Rust SDK](https://longport.github.io/openapi/rust/longport/)

## License

MIT
