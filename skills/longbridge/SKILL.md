---
name: longbridge
description: "Longbridge CLI expert for querying US and HK stock market data, executing trades, and managing watchlists via the `longbridge` command. Use when the user wants to: (1) Query real-time or historical stock quotes (TSLA.US, 700.HK, 600519.SH), (2) Get candlestick/kline data for any timeframe, (3) View order book depth, trades, intraday data, (4) Query options/warrants, (5) Submit buy/sell orders, manage positions, check balance, (6) Manage watchlists, (7) Get market info (trading sessions, calendar, market temperature), (8) Fetch news, regulatory filings, or community topics for a symbol. Triggers on any request involving stock symbols, market data, trading operations, or `longbridge` CLI usage."
---

# Longbridge CLI

A CLI that wraps all Longbridge OpenAPI endpoints. Supports Hong Kong (HK), US, and China A-share (SH/SZ) markets.

## Authentication

```bash
longbridge login    # Opens browser OAuth flow; token saved to ~/.longbridge/terminal/.openapi-session
longbridge logout   # Clear token
longbridge check    # Verify token, region, and API endpoint connectivity (no auth required)
```

The CLI auto-detects China Mainland by probing `geotest.lbkrs.com` on each startup (background, non-blocking). The result is cached in `~/.longbridge-openapi/region-cache`; if CN, all API calls use CN endpoints automatically on the next run.

## Symbol Format

All symbols: `<CODE>.<MARKET>`

| Market | Examples |
|--------|---------|
| `HK` | `700.HK` (Tencent), `9988.HK` (Alibaba) |
| `US` | `TSLA.US`, `AAPL.US`, `NVDA.US` |
| `SH` | `600519.SH` (Kweichow Moutai) |
| `SZ` | `000001.SZ` (Ping An Bank) |

## Output Formats

```bash
--format table   # Human-readable (default)
--format json    # Machine-readable for AI agents and scripts
```

## Quick Reference

### Most Common Commands

```bash
# Diagnostics
longbridge check

# News, filings, and community topics
longbridge news TSLA.US
longbridge news-detail <id>
longbridge filings AAPL.US
longbridge topics TSLA.US
longbridge topic-detail <id>

# Real-time quote
longbridge quote TSLA.US 700.HK AAPL.US

# Daily candlesticks (last 100)
longbridge kline TSLA.US --period day --count 100

# Historical data by date range
longbridge kline-history TSLA.US --start 2024-01-01 --end 2024-12-31

# Order book depth
longbridge depth TSLA.US

# Account positions
longbridge positions

# Today's orders
longbridge orders

# Submit a limit buy order
longbridge buy TSLA.US 100 --price 250.00

# Submit a market sell order
longbridge sell AAPL.US 50 --order-type MO
```

## Reference Files

- **Quote commands** (quote, depth, kline, trades, options, warrants, market data): See [references/quote-commands.md](references/quote-commands.md)
- **Trade commands** (orders, buy, sell, positions, balance, executions): See [references/trade-commands.md](references/trade-commands.md)
- **Watchlist commands** (create, update, delete groups): See [references/watchlist-commands.md](references/watchlist-commands.md)

Load the relevant reference file based on what the user is asking about.
