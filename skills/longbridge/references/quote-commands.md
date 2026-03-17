# Quote Commands Reference

## Table of Contents
- [quote](#quote) - Real-time quotes
- [depth](#depth) - Order book
- [brokers](#brokers) - Broker queues (HK)
- [trades](#trades) - Tick-by-tick trades
- [intraday](#intraday) - Minute-level data
- [kline](#kline) - Candlestick data
- [kline-history](#kline-history) - Historical candlesticks by date
- [static](#static) - Security static info
- [calc-index](#calc-index) - Financial indexes
- [capital-flow](#capital-flow) - Capital flow time series
- [capital-dist](#capital-dist) - Capital distribution snapshot
- [market-temp](#market-temp) - Market sentiment index
- [trading-session](#trading-session) - Trading hours
- [trading-days](#trading-days) - Trading calendar
- [security-list](#security-list) - All listed securities
- [participants](#participants) - Market participants (brokers)
- [subscriptions](#subscriptions) - Active subscriptions
- [option-quote](#option-quote) - Option quotes
- [option-chain](#option-chain) - Option chain
- [warrant-quote](#warrant-quote) - Warrant quotes
- [warrant-list](#warrant-list) - Warrants for a security
- [warrant-issuers](#warrant-issuers) - Warrant issuers
- [news](#news) - Latest news articles for a symbol
- [news-detail](#news-detail) - Full article content
- [filings](#filings) - Regulatory filings and announcements
- [filing-detail](#filing-detail) - Full filing content as Markdown
- [topics](#topics) - Community discussion topics
- [topic-detail](#topic-detail) - Full topic content

---

## quote

Real-time quotes for one or more securities.

```bash
longbridge quote SYMBOL1 [SYMBOL2 ...] [--format json]
```

Returns: `symbol, last_done, prev_close, open, high, low, volume, turnover, trade_status`

```bash
longbridge quote TSLA.US 700.HK AAPL.US
longbridge quote TSLA.US --format json
```

---

## depth

Level 2 order book depth (up to 10 price levels each side).

```bash
longbridge depth SYMBOL [--format json]
```

Returns: bid/ask `price, volume, order_num` for each level.

```bash
longbridge depth TSLA.US
longbridge depth 700.HK --format json
```

---

## brokers

Broker queue per price level (HK market only).

```bash
longbridge brokers SYMBOL
```

Returns: broker IDs at each bid/ask price level. Use `participants` to map IDs to names.

---

## trades

Tick-by-tick trade records.

```bash
longbridge trades SYMBOL --count <N>
```

- `--count`: Number of records (default 20, max 1000)

Returns: `timestamp, price, volume, direction (up/down/neutral), trade_type`

```bash
longbridge trades TSLA.US --count 100
longbridge trades 700.HK --count 50 --format json
```

---

## intraday

Today's minute-level price/volume data.

```bash
longbridge intraday SYMBOL
```

Returns: `timestamp, price, volume, turnover, avg_price` for each minute today.

```bash
longbridge intraday TSLA.US --format json
```

---

## kline

Candlestick data (most recent N bars).

```bash
longbridge kline SYMBOL [--period <PERIOD>] [--count <N>] [--adjust <TYPE>]
```

**Periods:**

| Value | Aliases | Description |
|-------|---------|-------------|
| `1m` | `minute` | 1 minute |
| `5m` | - | 5 minutes |
| `15m` | - | 15 minutes |
| `30m` | - | 30 minutes |
| `1h` | `hour` | 1 hour |
| `day` | `d`, `1d` | Daily (default) |
| `week` | `w` | Weekly |
| `month` | `m`, `1mo` | Monthly |
| `year` | `y` | Yearly |

**Adjust types:** `no_adjust` (default, alias: `none`) or `forward_adjust` (alias: `forward`)

Defaults: `--period day --count 100 --adjust no_adjust`

Returns: `timestamp, open, high, low, close, volume, turnover`

```bash
longbridge kline TSLA.US --period day --count 100
longbridge kline TSLA.US --period 1h --count 50
longbridge kline AAPL.US --period 5m --adjust forward_adjust
longbridge kline 700.HK --period week --count 52 --format json
```

---

## kline-history

Historical candlesticks by date range.

```bash
longbridge kline-history SYMBOL [--period <PERIOD>] [--start <DATE>] [--end <DATE>] [--adjust <TYPE>]
```

- `--start` and `--end` must be used together (format: `YYYY-MM-DD`)
- Without dates: returns most recent 100 bars

```bash
longbridge kline-history TSLA.US --start 2024-01-01 --end 2024-12-31
longbridge kline-history 700.HK --period day --start 2024-01-01 --end 2024-03-31
longbridge kline-history AAPL.US --period week --start 2023-01-01 --end 2024-12-31 --format json
```

---

## static

Static security information (fundamental data).

```bash
longbridge static SYMBOL1 [SYMBOL2 ...]
```

Returns: `name_en, exchange, currency, lot_size, total_shares, circulating_shares, eps, eps_ttm, bps, dividend_yield`

```bash
longbridge static TSLA.US 700.HK AAPL.US
longbridge static 600519.SH --format json
```

---

## calc-index

Calculated financial indexes.

```bash
longbridge calc-index SYMBOL1 [SYMBOL2 ...] [--index <INDEXES>]
```

- `--index`: Comma-separated list (default: `pe,pb,eps,turnover_rate,total_market_value`)

**Available indexes:**

| Category | Indexes |
|----------|---------|
| Basic | `last_done, change_value, change_rate, volume, turnover, ytd_change_rate, turnover_rate, total_market_value, capital_flow, amplitude, volume_ratio` |
| Valuation | `pe` (alias: `pe_ttm`), `pb`, `eps` (alias: `dividend_yield`) |
| Technical | `five_day_change_rate, ten_day_change_rate, half_year_change_rate, five_minutes_change_rate, implied_volatility` |
| Options | `delta, gamma, theta, vega, rho, open_interest, expiry_date, strike_price` |
| Warrants | `outstanding_qty, outstanding_ratio, premium, itm_otm, warrant_delta, leverage_ratio, conversion_ratio` |

```bash
longbridge calc-index TSLA.US AAPL.US --index pe,pb,turnover_rate
longbridge calc-index 700.HK --index pe,pb,eps,total_market_value --format json
```

---

## capital-flow

Capital flow inflow time series for today.

```bash
longbridge capital-flow SYMBOL
```

Returns: `timestamp, inflow` (large/medium/small order net inflow).

---

## capital-dist

Capital distribution snapshot (breakdown by order size).

```bash
longbridge capital-dist SYMBOL
```

Returns: total inflow/outflow by order size category for the current session.

---

## market-temp

Market sentiment temperature index (0–100, higher = more bullish).

```bash
longbridge market-temp [MARKET] [--history] [--start <DATE>] [--end <DATE>]
```

- `MARKET`: `HK` (default), `US`, `CN`, `SG` (case-insensitive; `SH`/`SZ` = `CN`)
- `--history`: Return historical data instead of current value
- `--start`/`--end`: Date range for history (default: today)

```bash
longbridge market-temp HK
longbridge market-temp US
longbridge market-temp US --history --start 2024-01-01 --end 2024-12-31
```

---

## trading-session

Trading hours for all markets.

```bash
longbridge trading-session [--format json]
```

Returns: `market, session_type (intraday/pre/post/overnight), begin_time, end_time`

---

## trading-days

Trading calendar for a market.

```bash
longbridge trading-days [MARKET] [--start <DATE>] [--end <DATE>]
```

- `MARKET`: `HK` (default), `US`, `CN`, `SG`
- Default date range: today to 30 days ahead

Returns: trading days and half-trading days.

```bash
longbridge trading-days HK --start 2024-01-01 --end 2024-12-31
longbridge trading-days US --start 2025-01-01 --end 2025-06-30
```

---

## security-list

All listed securities for a market.

```bash
longbridge security-list [MARKET]
```

Returns: `symbol, name_en, name_cn` for every listed security.

```bash
longbridge security-list HK
longbridge security-list US --format json
```

---

## participants

Market participants (broker ID → name mapping). Used to interpret `brokers` command output.

```bash
longbridge participants
longbridge participants --format json
```

---

## subscriptions

List active WebSocket real-time subscriptions for the current session.

```bash
longbridge subscriptions [--format json]
```

Returns: `symbol, sub_types (quote/depth/trade), candlestick periods`

---

## option-quote

Real-time quotes for option contracts.

```bash
longbridge option-quote SYMBOL1 [SYMBOL2 ...]
```

Returns: standard quote fields + `implied_volatility, delta, strike_price, expiry_date, contract_type`

```bash
longbridge option-quote AAPL240119C190000
```

---

## option-chain

Option chain for an underlying security.

```bash
longbridge option-chain SYMBOL [--date <DATE>]
```

- Without `--date`: returns all available expiry dates
- With `--date YYYY-MM-DD`: returns strike prices and contract symbols for that expiry

```bash
longbridge option-chain AAPL.US
longbridge option-chain AAPL.US --date 2024-01-19
longbridge option-chain TSLA.US --date 2025-03-21 --format json
```

---

## warrant-quote

Real-time quotes for warrant contracts.

```bash
longbridge warrant-quote SYMBOL1 [SYMBOL2 ...]
```

Returns: `last_done, prev_close, implied_volatility, leverage_ratio, expiry_date, category`

```bash
longbridge warrant-quote 12345.HK
```

---

## warrant-list

All warrants linked to a security.

```bash
longbridge warrant-list SYMBOL
```

Returns: `symbol, name, last_done, leverage_ratio, expiry_date, warrant_type`

```bash
longbridge warrant-list 700.HK
longbridge warrant-list 9988.HK --format json
```

---

## warrant-issuers

List of warrant issuers in the HK market.

```bash
longbridge warrant-issuers
longbridge warrant-issuers --format json
```

Returns: `issuer_id, name_en, name_cn`

---

## news

Latest news articles for a symbol.

```bash
longbridge news SYMBOL [--count <N>] [--format json]
```

- `--count`: Max articles to show (default 20)

Returns: `id, title, published_at, likes, comments`

```bash
longbridge news TSLA.US
longbridge news 700.HK --count 5
longbridge news AAPL.US --format json
```

---

## news-detail

Full Markdown content of a news article. Fetches from `https://longbridge.com/news/<id>.md`.

```bash
longbridge news-detail <ID>
```

```bash
longbridge news-detail 12345678
```

---

## filings

Regulatory filings and announcements for a symbol.

```bash
longbridge filings SYMBOL [--count <N>] [--format json]
```

- `--count`: Max filings to show (default 20)

Returns: `id, title, file_name, files, publish_at`. The `files` column shows how many
files the filing contains (some filings like 8-K have multiple: cover page + exhibits).
Use `filing-detail` to read the full content.

```bash
longbridge filings AAPL.US
longbridge filings 700.HK --count 5
longbridge filings AAPL.US --format json
```

JSON output includes `file_count` and `file_urls` for all files.

---

## filing-detail

Full Markdown content of a regulatory filing (HTML and TXT files only).

Fetches the document via the filing's download URL using browser-like headers to
bypass collector restrictions (e.g. SEC EDGAR rate limiting), then converts HTML
to Markdown. TXT files are printed as-is. For unsupported formats (e.g. PDF,
common in HK and A-share filings), the raw download URL is printed so the caller
can handle it independently.

Some filings contain multiple files (e.g. 8-K cover page + Exhibit 99.1 earnings release).
The `filings` command shows the file count. Use `--file-index N` to fetch a specific file.

```bash
longbridge filing-detail SYMBOL ID [--file-index N] [--list-files]
```

```bash
longbridge filing-detail AAPL.US 580265529766123777           # fetch file 0 (default)
longbridge filing-detail AAPL.US 580265529766123777 --file-index 1  # fetch exhibit
longbridge filing-detail AAPL.US 580265529766123777 --list-files    # list all file URLs
longbridge filing-detail 700.HK abc123
```

---

## topics

Community discussion topics for a symbol.

```bash
longbridge topics SYMBOL [--count <N>] [--format json]
```

- `--count`: Max topics to show (default 20)

Returns: `id, title, published_at, likes, comments, shares`

```bash
longbridge topics TSLA.US
longbridge topics 700.HK --count 5
longbridge topics AAPL.US --format json
```

---

## topic-detail

Full Markdown content of a community topic. Fetches from `https://longbridge.com/topics/<id>.md`.

```bash
longbridge topic-detail <ID>
```

```bash
longbridge topic-detail 277062200
```
