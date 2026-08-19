# Longbridge Terminal

<p align="center">
  <strong>English</strong> | <a href="./README.zh-CN.md">简体中文</a>
</p>

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

The CLI picks its API access point from your location: it asks `geotest.lbkrs.com` which country you are in and caches the answer for 6 hours, so at most one command per session waits on the probe. China Mainland uses the `.cn` endpoints; everywhere else uses the global ones.

```bash
longbridge check              # Re-detect the access point, and show both latencies
LONGBRIDGE_REGION=global ...  # Pin the access point explicitly (cn or global)
```

`check` never trusts the cache. It re-detects, measures both endpoints, and repins to whichever one is decisively better — location only approximates that, and a split-tunnel proxy can route the geo probe and the API over entirely different paths. The result is persisted, so later commands follow it too.

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
longbridge constituent .SPX.US [--sort market-cap]  # Index constituent stocks (US indexes need a leading dot, e.g. .DJI.US, .SPX.US)
longbridge constituent IVV.US [--limit 0]           # For a US ETF, full holdings from SEC N-PORT (--limit 0 = all); falls back to platform asset allocation when SEC data is unavailable (e.g. SPY)
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
longbridge financial-report AAPL.US --latest                         # Latest financial report summary
longbridge financial-report snapshot AAPL.US --report qf --year N --period N  # Earnings summary + forecast vs actual (revenue/EBIT/EPS beat/miss) + financial ratios
longbridge financial-statement AAPL.US [--kind IS|BS|CF|ALL] [--report af|saf|qf|cumul]  # Detailed financial statement (v3 endpoint)
longbridge institution-rating AAPL.US                                # Analyst rating distribution and consensus target price
longbridge institution-rating AAPL.US --history                      # Rating and target price change history
longbridge institution-rating AAPL.US --industry-rank [--page 1] [--limit 20]  # Industry-wide institution rating ranking
longbridge institution-rating AAPL.US --views                        # Monthly buy/hold/sell distribution timeline (institutional views)
longbridge institution-rating detail AAPL.US                         # Monthly rating trend and analyst accuracy history
longbridge dividend AAPL.US                                          # Historical dividend records
longbridge dividend detail AAPL.US                                   # Dividend allocation plan details
longbridge forecast-eps AAPL.US                                      # Analyst EPS consensus forecast snapshots
longbridge consensus AAPL.US                                         # Revenue / profit / EPS multi-period comparison with beat/miss markers
longbridge valuation AAPL.US [--indicator pe|pb|ps|dvd_yld]         # Current valuation snapshot and peer comparison
longbridge valuation AAPL.US --history [--indicator pe] [--range 5]  # Historical valuation time series (1 / 3 / 5 / 10 years)
longbridge valuation-rank AAPL.US [--start 20240101] [--end 20241231] # Industry valuation percentile ranking (default: last 30 days)
longbridge analyst-estimates AAPL.US                                 # Analyst consensus EPS estimates
longbridge fund-holder AAPL.US [--count 20]                          # Funds and ETFs holding this stock
longbridge shareholder AAPL.US [--range all|inc|dec] [--sort chg]    # Institutional shareholders with QoQ change tracking
longbridge shareholder AAPL.US --top                                  # Top 20 major shareholders (includes individuals and insiders, multi-period)
longbridge shareholder AAPL.US --object-id <ID>                       # Holding and trade detail for a specific shareholder (use ID from --top output)
longbridge compare AAPL.US                                            # Multi-stock valuation comparison vs server-selected industry peers
longbridge compare 9988.HK 700.HK 9999.HK [--currency HKD]           # Compare specific stocks side by side (price, market cap, PE/PB/PS, ROE, ROA, div yield, and more)
longbridge corp-action 700.HK [--all]                                 # Corporate actions (splits, dividends, rights, etc.) — default 30, --all for full history
longbridge business-segments AAPL.US [--history] [--report qf|saf|af] [--cate <cate>]  # Revenue segment breakdown (current snapshot or historical trends)
longbridge industry-rank --market US|HK|CN|SG [--indicator leading-gainer|...|net-profit-growth]  # Industry ranking list; output symbols feed into industry-peers
longbridge industry-peers IN00446.US                                  # Industry peer group hierarchy tree for an industry index symbol (from industry-rank)
longbridge macrodata [--country US] [--page 1] [--limit 20]          # List macroeconomic indicators (20/page); names follow --lang
longbridge macrodata US00175 [--start 2024-01-01] [--end 2024-12-31]  # Historical releases for one indicator (actual / forecast / previous)
```

### Deposits & Withdrawals

```bash
longbridge bank-cards                                               # List linked bank cards
longbridge withdrawals [--page 1] [--limit 20]                      # Withdrawal history
longbridge deposits [--page 1] [--limit 20] [--states 0,1,2] [--currencies HKD,USD]  # Deposit history
```

### Search

```bash
longbridge search TSLA [--tab market|news|posts|hashtags|help|share-lists|users|institutions]  # Search across multiple content types
longbridge search-hot                                               # Hot search keywords
```

### IPO

```bash
longbridge ipo subscriptions                                        # IPO stocks currently in filing or subscription stage
longbridge ipo wait-listing                                         # IPO stocks in grey-market (wait-listing) stage
longbridge ipo listed [--page 1] [--limit 20]                       # Recently listed IPO stocks
longbridge ipo calendar                                             # IPO calendar (all upcoming and recent IPOs)
longbridge ipo detail <symbol> [--market HK|US]                     # IPO profile, timeline, eligibility, and holdings for a symbol
longbridge ipo orders [--market HK] [--status 0] [--page 1]         # IPO orders (active + history) for the current account
longbridge ipo orders detail <order_id>                             # Full detail for a single IPO order
longbridge ipo profit-loss [--period all|1m|3m|6m|1y] [--page 1]   # IPO P&L summary and item list
longbridge ipo us-subscriptions                                     # US IPO stocks currently in subscription stage
longbridge ipo us-wait-listing                                      # US IPO stocks in wait-listing stage
longbridge ipo us-listed [--page 1] [--limit 20]                    # Recently listed US IPO stocks
longbridge ipo submit TSLA.US --qty 200 --amount 1000 [--method 2]  # Submit IPO subscription (prompts for confirmation)
longbridge ipo withdraw <order_id>                                  # Withdraw IPO subscription (prompts for confirmation)
```

### Market Data

```bash
longbridge rank                                                      # List available popularity ranking tab keys
longbridge rank --key ib_hot_all-us [--count 20]                     # Stocks ranked by composite heat score (trading activity, media, community, volatility)
longbridge top-movers [--market HK|US|CN|SG] [--sort hot|time|chg]  # Stocks with abnormal price moves paired with correlated news and reason summaries
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

### AI Agents

```bash
longbridge agent workspaces                                         # List AI workspaces
longbridge agent list [--workspace 33] [--name 选股]                # Discover chat-capable AI agents (--all includes workflow agents)
longbridge agent chat chatbot "分析一下 TSLA"                       # `chatbot` (LongbridgeAI) is public — usable by any account
longbridge agent chat chatbot "分析一下 TSLA"                       # Chat with an agent (SSE; --stream for live tokens)
longbridge agent chat chatbot <CHAT_UID> <MSG_ID> "继续"            # Multi-turn follow-up
longbridge agent continue chatbot <CHAT_UID> <MSG_ID> --answer "…"  # Resume an interrupted run
longbridge agent continue chatbot <CHAT_UID> <MSG_ID> --answers-json '{…}'  # Resume with the raw answers payload
longbridge agent --skill                                            # Print the agent skill doc for AI harnesses
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
longbridge portfolio                                       # Portfolio overview: total assets, P/L, holdings, and cash breakdown
longbridge portfolio short-margin                          # Short-selling margin deposit details
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

### Grid Trading

```bash
longbridge grid                                               # List grid orders
longbridge grid --symbol 700.HK --status Performing           # Filter grid orders by symbol / status
longbridge grid --ids <ORDER_ID1> <ORDER_ID2>                 # Query specific grid orders by ID
longbridge grid submit 700.HK --currency HKD --base-price 300 --upper-price 360 --lower-price 240 \
  --trigger-type percent --trigger-up 2 --trigger-down 2 --quantity 100 \
  --upper-quantity 200 --lower-quantity 100 --order-type GMO --tif gtc   # Submit a grid strategy
longbridge grid submit 700.HK --base-price 300 ... --dry-run  # Validate + print the rule without submitting
longbridge grid detail <ORDER_ID>                             # Grid order detail (rule, sub-orders, history)
longbridge grid triggers <ORDER_ID>                           # Grid trigger history
longbridge grid replace <ORDER_ID> --base-price 305 ...       # Replace (modify) a grid rule
longbridge grid cancel <ORDER_ID>                             # Cancel a grid order
longbridge grid suspend <ORDER_ID>                            # Suspend a grid order
longbridge grid restart <ORDER_ID>                            # Restart a suspended grid order
longbridge grid info 700.HK                                   # Symbol's grid-trading info (lot size, last price, authorization, currency)
longbridge grid questionnaire                                 # Submit the strategy risk-disclosure questionnaire
```

### Short Selling

```bash
longbridge short-positions AAPL.US                            # US: bi-weekly FINRA short interest (short interest, rate, days to cover)
longbridge short-positions 700.HK                             # HK: daily HKEX disclosed short positions (open short shares, balance, cost, rate)
longbridge short-positions TSLA.US --count 50                 # Return last 50 records
longbridge short-trades AAPL.US                               # Daily short sale volume (FINRA/NASDAQ for US; HKEX for HK)
longbridge short-trades 700.HK [--count 50]                   # HK: amount, balance, total amount, rate, close per trading day
```

### Stock Screener

```bash
longbridge screener strategies                                # List recommended stock-selection strategies
longbridge screener strategies --all                          # List all platform strategies
longbridge screener strategies --mine                         # List user-created strategies
longbridge screener strategies --id <ID>                      # Show groups and indicators for a specific strategy
longbridge screener search --strategy-id <ID>                 # Run a saved strategy and return matching stocks
longbridge screener search --market HK --filter filter_marketcap:100:1000 --filter filter_divyld:3:  # Custom filter (key:min:max, omit bound to leave open)
longbridge screener indicators                                # List all available filter indicators with IDs, keys, and default value ranges
```

### Embedding in Another App

```bash
longbridge serve                                              # JSON-RPC 2.0 API endpoint on stdin/stdout, with live quote push
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

Features: real-time watchlist, candlestick charts, portfolio view, stock search, a responsive layout that docks the news list beside the quote panel when there is room for both, Vim-like keybindings, and mouse support — click the tabs, the shortcut hints, a headline, or a Portfolio/Orders row to open its detail panel. Press `?` for the full key list.

## Longbridge AI chat

A full-screen chat TUI backed by Longbridge AI:

```bash
longbridge ai [--agent <agent-id>]
```

Features: streaming answers rendered as Markdown with charts, tables, syntax-highlighted code, and live quote cards; click any security an answer mentions (or `/quote 700.HK`) for a floating live quote, with the securities of the session and their quotes on a rotating title-bar ticker; a `/` command palette (`/new /retry /copy /export /quote /resume /settings /agent /login /logout /exit /help`); opens signed out too, with sign-in completed in place; the session and sign in/out under `/settings`, alongside preferences (tool-call detail, done notification, quote cards, title-bar ticker, up/down colours); server-synced conversations (`/v1/ai/chats`) reopened with `/resume` and searchable in place; search within the open transcript with `Ctrl+F` (Enter/↑↓ to walk the matches); drag-to-select copy (OSC 52) that scrolls to extend across pages, and `/export` to Markdown; multi-line input with undo/redo (Ctrl+Z/Y), a large paste folded to a compact chip, and prompt history that persists across sessions (↑/↓ or Ctrl+P/N). `/agent <agent-id>` switches agent for a fresh conversation and `/agent reset` returns to the default. Type `exit` or press Ctrl+C twice to quit.

## ACP agent server

Expose the main Longbridge AI agent (`chatbot`) to an [ACP](https://agentclientprotocol.com) client over stdio:

```bash
longbridge acp
```

Zed can register it as a custom external agent:

```json
{
  "agent_servers": {
    "longbridge-ai": {
      "type": "custom",
      "command": "longbridge",
      "args": ["acp"],
      "env": {}
    }
  }
}
```

## Embedding in another app

Third-party clients — desktop widgets, bar plugins, dashboards — can drive the CLI as a
long-lived data source instead of polling one-shot commands:

```bash
longbridge serve
```

The process authenticates and opens the market WebSocket once, then speaks
newline-delimited [JSON-RPC 2.0](https://www.jsonrpc.org/specification) on stdin/stdout —
one compact JSON object per line. That is the same base protocol LSP, MCP and ACP build
on, so a client needs only a JSON parser and a line splitter, no protocol library.

```jsonc
→ {"jsonrpc":"2.0","id":1,"method":"quote.quote","params":{"symbols":["700.HK"]}}
← {"jsonrpc":"2.0","id":1,"result":[{"symbol":"700.HK","last_done":"445.600", ...}]}
→ {"jsonrpc":"2.0","id":2,"method":"quote.subscribe","params":{"symbols":["700.HK"]}}
← {"jsonrpc":"2.0","id":2,"result":{"subscribed":[{"symbol":"700.HK","fields":["quote"]}],"quotes":[…]}}
← {"jsonrpc":"2.0","method":"quote.updated","params":{"symbol":"700.HK","last_done":"446.000", ...}}
```

One request per line: batches (a JSON array) are not accepted, as in LSP and MCP.

### Raw payloads, on purpose

`serve` returns the **raw Longbridge OpenAPI payloads** — not the JSON the CLI prints.
The CLI's `--format json` reshapes data for AI consumption and is free to change with it;
`serve` is an API contract for other people's software, so it tracks the upstream shapes
instead.

### Method surface

`serve` sits below the CLI commands, at the API seam all of them share, so it covers the
whole command surface without a parallel implementation:

| Namespace | Covers |
| --- | --- |
| `quote.*` | Every `QuoteApi` call — `quote.quote`, `quote.depth`, `quote.candlesticks`, `quote.watchlist`, `quote.option_chain_info_by_date`, … |
| `trade.*` | Every `TradeApi` call — `trade.stock_positions`, `trade.account_balance`, `trade.today_orders`, `trade.submit_order`, … |
| `api.get` / `api.post` | Raw passthrough to any REST endpoint, e.g. `{"path":"/v1/quote/dividends","query":{"symbol":"AAPL.US"}}`. This is how the fundamentals, screener, IPO and news commands reach their data. |
| `quote.subscribe` / `quote.unsubscribe` | Live feed; `fields` is any of `quote`, `depth`, `brokers`, `trades` (default `quote`). `subscribe` also returns a `quotes` snapshot to paint the first screen from. No one-shot CLI equivalent. |
| `initialize` / `shutdown` | Session control. `initialize` returns the full method list, so clients discover the surface rather than hard-coding it. |

`longbridge serve -h` prints the protocol, the full method list and a worked exchange —
generated from the routing tables, so the help cannot advertise a method that is not
there. Params and results follow the Longbridge OpenAPI shapes for the same call, so look
a method up under its own name at <https://open.longbridge.com/docs> for its fields.

Server notifications: `quote.updated`, `quote.depth`, `quote.brokers`, `quote.trades`.
`quote.updated` is a tick rather than a full quote, and a push that raced ahead of
`subscribe`'s snapshot can still be the older of the two, so keep whichever `timestamp` is
newer.

A test asserts every `QuoteApi`/`TradeApi` method is reachable over RPC, so adding one
without exposing it fails the build — `serve` cannot drift behind the CLI.

Derived views the CLI computes locally (`portfolio`, for instance, which merges balances,
positions and FX rates) are deliberately not methods: a client composes them from
`trade.account_balance`, `trade.stock_positions` and `quote.quote` rather than depending on
our arithmetic.

Requests are answered concurrently — a slow `trade.stock_positions` never stalls the quote
feed — so responses may arrive out of order; correlate them by `id`. Up to 8 run upstream
at once and the rest queue, so a burst is paced rather than dropped. An error code says
whether a retry can help: `-32602` names the parameter at fault and will fail the same way
again, `-32000` came from Longbridge and may not. The process exits when stdin closes, so
it cannot outlive the client that spawned it.

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

Apache License 2.0. See [LICENSE](LICENSE) for details.
