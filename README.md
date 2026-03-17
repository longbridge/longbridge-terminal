# Longbridge Terminal

AI-native CLI for the [Longbridge](https://longbridge.com) trading platform — real-time market data, portfolio, and trading.

Covers every Longbridge OpenAPI endpoint: real-time quotes, depth, K-lines, options, and warrants for market data; account balances, stock and fund positions for portfolio management; and order submission, modification, cancellation, and execution history for trading. Designed for scripting, AI-agent tool-calling, and daily trading workflows from the terminal.

```bash
$ longbridge static NVDA.US
+---------+--------------------+----------+----------+----------+--------------+--------------+--------------------+--------------------+-------------------+----------------+
| Symbol  | Name (EN)          | Exchange | Currency | Lot Size | Total Shares | Circ. Shares | EPS                | EPS TTM            | BPS               | Dividend Yield |
+============================================================================================================================================================================+
| NVDA.US | NVIDIA Corporation | NASD     | USD      | 1        | 24300000000  | 23501828621  | 4.9410288065843621 | 4.9410288065843621 | 6.472962962962963 | 0.04           |
+---------+--------------------+----------+----------+----------+--------------+--------------+--------------------+--------------------+-------------------+----------------+

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
longbridge login    # Opens browser for OAuth, saves token to ~/.longbridge/terminal/.openapi-session
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
longbridge check                                              # Shows token status, cached region, and latency to both Global and CN API endpoints
longbridge check --format json                                # Shows token status, cached region, and latency to both Global and CN API endpoints
```

### Quote

```bash
longbridge quote TSLA.US 700.HK                               # Returns: symbol, `last_done`, `prev_close`, open, high, low, volume, turnover, `trade_status`
longbridge depth TSLA.US                                      # Returns up to 10 price levels of asks and bids with price, volume, `order_num`
longbridge brokers 700.HK                                     # Returns which broker IDs are present at each ask/bid level
longbridge trades TSLA.US --count 10                          # Returns: timestamp, price, volume, direction (up/down/neutral), `trade_type`
longbridge intraday TSLA.US                                   # Returns: timestamp, price, volume, turnover, `avg_price`
longbridge kline TSLA.US --period day --count 10              # Returns: timestamp, open, high, low, close, volume, turnover
longbridge kline TSLA.US --period 1h --count 10               # Returns: timestamp, open, high, low, close, volume, turnover
longbridge kline-history TSLA.US --start 2024-01-01 --end 2024-03-31 # Both --start and --end must be provided together; if either is omitted the most recent 100 candles are returned (offset-based, ignores the other flag)
longbridge static TSLA.US 700.HK                              # Returns: name, exchange, currency, `lot_size`, `total_shares`, `circulating_shares`, EPS, BPS, `dividend_yield`
longbridge calc-index TSLA.US --index pe,pb,eps               # Full index list: `last_done`  `change_value`  `change_rate`  volume  turnover  `ytd_change_rate` `turnover_rate`  `total_market_value`  `capital_flow`  amplitude  `volume_ratio` pe (alias: `pe_ttm`)  pb  eps (alias: `dividend_yield`) `five_day_change_rate`  `ten_day_change_rate`  `half_year_change_rate`  `five_minutes_change_rate` `implied_volatility`  delta  gamma  theta  vega  rho  `open_interest` `expiry_date`  `strike_price`  `upper_strike_price`  `lower_strike_price` `outstanding_qty`  `outstanding_ratio`  premium  `itm_otm` `warrant_delta`  `call_price`  `to_call_price`  `effective_leverage` `leverage_ratio`  `conversion_ratio`  `balance_point` Example: longbridge calc-index TSLA.US AAPL.US --index pe,pb,`turnover_rate`
longbridge capital-flow TSLA.US                               # Returns a time series of inflow values for today's session
longbridge capital-dist TSLA.US                               # Returns total inflow/outflow broken down by order size for the current session
longbridge market-temp HK                                     # Use --history to get a time series instead of the current snapshot
longbridge market-temp US                                     # Use --history to get a time series instead of the current snapshot
longbridge trading-session                                    # Returns: market, session type (intraday/pre/post/overnight), `begin_time`, `end_time`
longbridge trading-days HK                                    # Defaults to today + 30 days if no dates are provided
longbridge security-list HK                                   # Returns: symbol, `name_en`, `name_cn` for every listed security
longbridge participants                                       # Use these IDs to interpret results from the `brokers` command
longbridge subscriptions                                      # Returns: symbol, `sub_types` (quote/depth/trade), subscribed candlestick periods
```

### Options & Warrants

```bash
longbridge option-chain AAPL.US                               # Without --date: returns all available expiry dates
longbridge option-chain AAPL.US --date <expiry>               # Without --date: returns all available expiry dates
longbridge option-quote <symbol>                              # Returns standard quote fields plus `implied_volatility`, delta, `strike_price`, `expiry_date`, `contract_type`
longbridge warrant-list 700.HK                                # Returns: symbol, name, `last_done`, `leverage_ratio`, `expiry_date`, `warrant_type`
longbridge warrant-quote <symbol>                             # Returns: `last_done`, `prev_close`, `implied_volatility`, `leverage_ratio`, `expiry_date`, category
longbridge warrant-issuers                                    # Returns: `issuer_id`, `name_en`, `name_cn`
```

### News

```bash
longbridge news TSLA.US --count 5                             # Returns: id, title, `published_at`, likes, comments
longbridge news-detail <id>                                   # Fetches the article text from <https://longbridge.com/news>/<id>.md Example: longbridge news-detail 12345678
longbridge filings AAPL.US --count 5                          # Returns: id, title, `file_name`, `publish_at`, `file_urls`
longbridge topics TSLA.US --count 5                           # Returns: id, title, description, url, `published_at`, likes, comments, shares
longbridge topic-detail <id>                                  # Fetches the topic text from <https://longbridge.com/topics>/<id>.md Example: longbridge topic-detail 277062200
```

### Watchlist

```bash
longbridge watchlist                                          # Without a subcommand, lists all groups and their securities
```

### Account

```bash
longbridge orders                                             # Returns: `order_id`, symbol, side, `order_type`, status, quantity, price, `executed_qty`, `executed_price`, `submitted_at`
longbridge orders --history                                   # Returns: `order_id`, symbol, side, `order_type`, status, quantity, price, `executed_qty`, `executed_price`, `submitted_at`
longbridge executions                                         # Returns: `order_id`, `trade_id`, symbol, price, quantity, `trade_done_at`
longbridge executions --history                               # Returns: `order_id`, `trade_id`, symbol, price, quantity, `trade_done_at`
longbridge balance                                            # Returns: currency, `total_cash`, `max_finance_amount`, `remaining_finance_amount`, `risk_level`, `margin_call`
longbridge cash-flow                                          # Returns: `flow_name`, symbol, `business_type`, balance, currency, `business_time`, description
longbridge positions                                          # Returns: symbol, name, quantity, `available_quantity`, `cost_price`, currency, market
longbridge fund-positions                                     # Returns: symbol, name, `current_net_asset_value`, `cost_net_asset_value`, currency, `holding_units`
longbridge margin-ratio TSLA.US                               # Returns: `im_factor` (initial), `mm_factor` (maintenance), `fm_factor` (forced liquidation)
longbridge max-qty TSLA.US --side buy --price 200             # Returns: `cash_max_qty` (cash only), `margin_max_qty` (with margin financing)
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
npx skills add longbridge/longbridge-terminal
```

Once installed, Claude can query market data, run technical analysis, and manage trades directly from your AI workflow.

```bash
claude> Show me recent 5 days performance of NVDA and TSLA

⏺ Bash(longbridge kline TSLA.US --period day --count 5 & longbridge kline NVDA.US --period day --count 5 & wait)
  ⎿  Authentication failed: oauth error: failed to refresh token: Failed to parse server response
     +---------------------+---------+---------+---------+---------+----------+-----------------+
     | Time                | Open    | High    | Low     | Close   | Volume   | Turnover        |
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
