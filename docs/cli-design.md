# CLI Design Plan

## Overview

Extend the existing `longport` binary so every subcommand executes a CLI command. OAuth authentication shares the same local token storage.

---

## Command Structure

```
longport login                    # OAuth authentication (shared token)
longport logout                   # Clear token

# ──── Quote ────
longport quote TSLA.US AAPL.US    # Real-time quotes
longport depth TSLA.US            # Order book depth
longport brokers TSLA.US          # Broker queue
longport trades TSLA.US           # Recent trades [--count 50]
longport intraday TSLA.US         # Intraday lines
longport kline TSLA.US            # Candlesticks [--period day|week|month|1m|5m...] [--count 100]
longport kline history TSLA.US    # History candlesticks [--period day] [--start 2024-01-01] [--end 2024-12-31]
longport static TSLA.US           # Static info (name, lot size, currency)
longport calc-index TSLA.US       # Calculated indexes [--index pe,pb,eps]
longport capital flow TSLA.US     # Capital flow
longport capital dist TSLA.US     # Capital distribution
longport market-temp HK           # Market temperature [HK|US|CN|SG]
longport trading session          # Trading sessions per market
longport trading days HK          # Trading calendar [--start ...] [--end ...]
longport security-list HK         # Security list [--category main|gem|...]
longport participants             # Market maker participants
longport subscriptions            # Current subscriptions

# ──── Options / Warrants ────
longport option quote AAPL240119C190000  # Option quote
longport option chain AAPL               # Option chain expiry date list
longport option chain AAPL --date 2024-01-19  # Option chain strike prices
longport warrant quote 12345.HK          # Warrant quote
longport warrant list 700.HK             # Warrant list for a security
longport warrant issuers                 # Warrant issuer list

# ──── Watchlist ────
longport watchlist                        # List all groups
longport watchlist create "My Portfolio"  # Create group
longport watchlist delete <id>            # Delete group
longport watchlist update <id>            # Update [--name ...] [--add TSLA.US] [--remove AAPL.US] [--mode add|remove|replace]

# ──── Trade ────
longport orders                           # Today's orders
longport orders --history                 # History orders [--start ...] [--end ...] [--symbol TSLA.US] [--status filled]
longport order <order_id>                 # Order detail
longport executions                       # Today's executions
longport executions --history             # History executions [--start ...] [--end ...]
longport buy TSLA.US 100 --price 250      # Buy [--type LO|MO|ELO|ALO] [--tif day|gtc]
longport sell TSLA.US 100 --price 260     # Sell
longport cancel <order_id>               # Cancel order
longport replace <order_id>              # Modify order [--qty 200] [--price 255]
longport balance                          # Account balance [--currency USD]
longport cash-flow                        # Cash flow [--start ...] [--end ...] [--type ...]
longport positions                        # Stock positions
longport fund-positions                   # Fund positions
longport margin-ratio TSLA.US            # Margin ratio
longport max-qty TSLA.US --side buy --price 250  # Max purchase quantity

```

---

## Output Format (AI-native)

Every command supports a `--format` flag:

| Flag             | Purpose                                |
| ---------------- | -------------------------------------- |
| `--format table` | Default, human-readable table output   |
| `--format json`  | JSON for AI agents and pipe processing |
| `--format csv`   | CSV for data analysis                  |

Examples:

```bash
# AI agent usage
longport quote TSLA.US AAPL.US --format json | jq '.[] | {symbol, price, change_rate}'
longport positions --format json
longport orders --format csv > orders.csv
```

---

## File Structure

```
src/
├── main.rs              # CLI dispatch
├── cli/
│   ├── mod.rs           # CLI entry, command tree (clap), dispatch
│   ├── output.rs        # Output formatting (table/json/csv)
│   ├── quote.rs         # Quote command implementations
│   ├── trade.rs         # Trade command implementations
│   └── watchlist.rs     # Watchlist command implementations
```

---

## Authentication Design

```
longport login
  └─ calls openapi::init_contexts() (existing OAuth flow)
     └─ token persisted by the longport SDK
        └─ CLI commands reuse the same local token
```

Token storage is managed internally by the `longport-oauth` SDK crate.

In CLI mode, no persistent WebSocket connection is needed: create Context → call HTTP API → output → exit.

---

## New Dependencies

```toml
comfy-table = "7"    # Table output (lightweight)
clap = { version = "4", features = ["derive"] }  # Upgrade from v3 for derive macro support
```

---

## Implementation Priority

| Priority | Command Group                                            | Reason                              |
| -------- | -------------------------------------------------------- | ----------------------------------- |
| P0       | `login/logout`, `quote`, `depth`, `watchlist`            | Most common, validates flow         |
| P1       | `trades`, `kline`, `orders`, `positions`, `balance`      | Core trading scenarios              |
| P2       | `buy/sell/cancel/replace`                                | Risk operations, needs confirmation |
| P3       | Option chain, warrants, market temperature, capital flow | Advanced features                   |

---

## API Coverage

### QuoteContext Methods

| Method                           | CLI Command                                       |
| -------------------------------- | ------------------------------------------------- |
| `subscribe` / `unsubscribe`      | (used internally by TUI)                          |
| `subscribe_candlesticks`         | (used internally by TUI)                          |
| `subscriptions`                  | `longport subscriptions`                        |
| `static_info`                    | `longport static <symbols>`                     |
| `quote`                          | `longport quote <symbols>`                      |
| `option_quote`                   | `longport option quote <symbols>`               |
| `warrant_quote`                  | `longport warrant quote <symbols>`              |
| `depth`                          | `longport depth <symbol>`                       |
| `brokers`                        | `longport brokers <symbol>`                     |
| `participants`                   | `longport participants`                         |
| `trades`                         | `longport trades <symbol>`                      |
| `intraday`                       | `longport intraday <symbol>`                    |
| `candlesticks`                   | `longport kline <symbol>`                       |
| `history_candlesticks_by_offset` | `longport kline history <symbol>`               |
| `history_candlesticks_by_date`   | `longport kline history <symbol> --start --end` |
| `option_chain_expiry_date_list`  | `longport option chain <symbol>`                |
| `option_chain_info_by_date`      | `longport option chain <symbol> --date`         |
| `warrant_issuers`                | `longport warrant issuers`                      |
| `warrant_list`                   | `longport warrant list <symbol>`                |
| `trading_session`                | `longport trading session`                      |
| `trading_days`                   | `longport trading days <market>`                |
| `capital_flow`                   | `longport capital flow <symbol>`                |
| `capital_distribution`           | `longport capital dist <symbol>`                |
| `calc_indexes`                   | `longport calc-index <symbols>`                 |
| `watchlist`                      | `longport watchlist`                            |
| `create_watchlist_group`         | `longport watchlist create`                     |
| `delete_watchlist_group`         | `longport watchlist delete`                     |
| `update_watchlist_group`         | `longport watchlist update`                     |
| `security_list`                  | `longport security-list <market>`               |
| `market_temperature`             | `longport market-temp <market>`                 |
| `history_market_temperature`     | `longport market-temp <market> --history`       |

### TradeContext Methods

| Method                           | CLI Command                          |
| -------------------------------- | ------------------------------------ |
| `history_executions`             | `longport executions --history`    |
| `today_executions`               | `longport executions`              |
| `history_orders`                 | `longport orders --history`        |
| `today_orders`                   | `longport orders`                  |
| `replace_order`                  | `longport replace <order_id>`      |
| `submit_order`                   | `longport buy` / `longport sell` |
| `cancel_order`                   | `longport cancel <order_id>`       |
| `account_balance`                | `longport balance`                 |
| `cash_flow`                      | `longport cash-flow`               |
| `fund_positions`                 | `longport fund-positions`          |
| `stock_positions`                | `longport positions`               |
| `margin_ratio`                   | `longport margin-ratio <symbol>`   |
| `order_detail`                   | `longport order <order_id>`        |
| `estimate_max_purchase_quantity` | `longport max-qty <symbol>`        |
