# CLI Design Plan

## Overview

Extend the existing `longbridge` binary — **no subcommand launches TUI, subcommand executes CLI command**. OAuth authentication shares the same token storage. `longbridge login` triggers the OAuth 2 flow already implemented in the TUI.

---

## Command Structure

```
longbridge                          # Launch TUI (existing behavior)
longbridge login                    # OAuth authentication (shared token)
longbridge logout                   # Clear token

# ──── Quote ────
longbridge quote TSLA.US AAPL.US    # Real-time quotes
longbridge depth TSLA.US            # Order book depth
longbridge brokers TSLA.US          # Broker queue
longbridge trades TSLA.US           # Recent trades [--count 50]
longbridge intraday TSLA.US         # Intraday lines
longbridge kline TSLA.US            # Candlesticks [--period day|week|month|1m|5m...] [--count 100]
longbridge kline-history TSLA.US    # History candlesticks [--period day] [--start 2024-01-01] [--end 2024-12-31]
longbridge static TSLA.US           # Static info (name, lot size, currency)
longbridge calc-index TSLA.US       # Calculated indexes [--index pe,pb,eps]
longbridge capital-flow TSLA.US     # Capital flow
longbridge capital-dist TSLA.US     # Capital distribution
longbridge market-temp HK           # Market temperature [HK|US|CN|SG]
longbridge trading-session          # Trading sessions per market
longbridge trading-days HK          # Trading calendar [--start ...] [--end ...]
longbridge security-list HK         # Security list [--category main|gem|...]
longbridge participants             # Market maker participants
longbridge subscriptions            # Current subscriptions

# ──── Options / Warrants ────
longbridge option-quote AAPL240119C190000  # Option quote
longbridge option-chain AAPL               # Option chain expiry date list
longbridge option-chain AAPL --date 2024-01-19  # Option chain strike prices
longbridge warrant-quote 12345.HK          # Warrant quote
longbridge warrant-list 700.HK             # Warrant list for a security
longbridge warrant-issuers                 # Warrant issuer list

# ──── Watchlist ────
longbridge watchlist                        # List all groups
longbridge watchlist create "My Portfolio"  # Create group
longbridge watchlist delete <id>            # Delete group
longbridge watchlist update <id>            # Update [--name ...] [--add TSLA.US] [--remove AAPL.US] [--mode add|remove|replace]

# ──── Trade ────
longbridge orders                           # Today's orders
longbridge orders --history                 # History orders [--start ...] [--end ...] [--symbol TSLA.US] [--status filled]
longbridge order <order_id>                 # Order detail
longbridge executions                       # Today's executions
longbridge executions --history             # History executions [--start ...] [--end ...]
longbridge buy TSLA.US 100 --price 250      # Buy [--type LO|MO|ELO|ALO] [--tif day|gtc]
longbridge sell TSLA.US 100 --price 260     # Sell
longbridge cancel <order_id>               # Cancel order
longbridge replace <order_id>              # Modify order [--qty 200] [--price 255]
longbridge balance                          # Account balance [--currency USD]
longbridge cash-flow                        # Cash flow [--start ...] [--end ...] [--type ...]
longbridge positions                        # Stock positions
longbridge fund-positions                   # Fund positions
longbridge margin-ratio TSLA.US            # Margin ratio
longbridge max-qty TSLA.US --side buy --price 250  # Max purchase quantity
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
longbridge quote TSLA.US AAPL.US --format json | jq '.[] | {symbol, price, change_rate}'
longbridge positions --format json
longbridge orders --format csv > orders.csv
```

---

## File Structure

```
src/
├── main.rs              # Extended: no subcommand → TUI, subcommand → CLI dispatch
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
longbridge login
  └─ calls openapi::init_contexts() (existing OAuth flow)
     └─ token written to ~/.longbridge/terminal/session-fd52fbc5-02a9-47f5-ad30-0842c841aae9
        └─ both TUI and CLI read from the same location (managed by src/auth.rs)
```

Token storage path: `~/.longbridge/terminal/session-fd52fbc5-02a9-47f5-ad30-0842c841aae9` (managed by `src/auth.rs`).

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
| `subscriptions`                  | `longbridge subscriptions`                        |
| `static_info`                    | `longbridge static <symbols>`                     |
| `quote`                          | `longbridge quote <symbols>`                      |
| `option_quote`                   | `longbridge option-quote <symbols>`               |
| `warrant_quote`                  | `longbridge warrant-quote <symbols>`              |
| `depth`                          | `longbridge depth <symbol>`                       |
| `brokers`                        | `longbridge brokers <symbol>`                     |
| `participants`                   | `longbridge participants`                         |
| `trades`                         | `longbridge trades <symbol>`                      |
| `intraday`                       | `longbridge intraday <symbol>`                    |
| `candlesticks`                   | `longbridge kline <symbol>`                       |
| `history_candlesticks_by_offset` | `longbridge kline-history <symbol>`               |
| `history_candlesticks_by_date`   | `longbridge kline-history <symbol> --start --end` |
| `option_chain_expiry_date_list`  | `longbridge option-chain <symbol>`                |
| `option_chain_info_by_date`      | `longbridge option-chain <symbol> --date`         |
| `warrant_issuers`                | `longbridge warrant-issuers`                      |
| `warrant_list`                   | `longbridge warrant-list <symbol>`                |
| `trading_session`                | `longbridge trading-session`                      |
| `trading_days`                   | `longbridge trading-days <market>`                |
| `capital_flow`                   | `longbridge capital-flow <symbol>`                |
| `capital_distribution`           | `longbridge capital-dist <symbol>`                |
| `calc_indexes`                   | `longbridge calc-index <symbols>`                 |
| `watchlist`                      | `longbridge watchlist`                            |
| `create_watchlist_group`         | `longbridge watchlist create`                     |
| `delete_watchlist_group`         | `longbridge watchlist delete`                     |
| `update_watchlist_group`         | `longbridge watchlist update`                     |
| `security_list`                  | `longbridge security-list <market>`               |
| `market_temperature`             | `longbridge market-temp <market>`                 |
| `history_market_temperature`     | `longbridge market-temp <market> --history`       |

### TradeContext Methods

| Method                           | CLI Command                          |
| -------------------------------- | ------------------------------------ |
| `history_executions`             | `longbridge executions --history`    |
| `today_executions`               | `longbridge executions`              |
| `history_orders`                 | `longbridge orders --history`        |
| `today_orders`                   | `longbridge orders`                  |
| `replace_order`                  | `longbridge replace <order_id>`      |
| `submit_order`                   | `longbridge buy` / `longbridge sell` |
| `cancel_order`                   | `longbridge cancel <order_id>`       |
| `account_balance`                | `longbridge balance`                 |
| `cash_flow`                      | `longbridge cash-flow`               |
| `fund_positions`                 | `longbridge fund-positions`          |
| `stock_positions`                | `longbridge positions`               |
| `margin_ratio`                   | `longbridge margin-ratio <symbol>`   |
| `order_detail`                   | `longbridge order <order_id>`        |
| `estimate_max_purchase_quantity` | `longbridge max-qty <symbol>`        |
