# Trade Commands Reference

## Table of Contents
- [orders](#orders) - Query orders
- [order](#order) - Single order detail
- [executions](#executions) - Execution records
- [buy](#buy) - Submit buy order
- [sell](#sell) - Submit sell order
- [cancel](#cancel) - Cancel an order
- [replace](#replace) - Modify an order
- [balance](#balance) - Cash balance
- [cash-flow](#cash-flow) - Cash flow history
- [positions](#positions) - Stock positions
- [fund-positions](#fund-positions) - Fund positions
- [margin-ratio](#margin-ratio) - Margin ratios
- [max-qty](#max-qty) - Maximum tradeable quantity

---

## orders

Query today's or historical orders.

```bash
longbridge orders [--history] [--start <DATE>] [--end <DATE>] [--symbol <SYMBOL>]
```

- `--history`: Query historical orders instead of today's
- `--start`/`--end`: Date filter (YYYY-MM-DD), used with `--history`
- `--symbol`: Filter by stock symbol

Returns: `order_id, symbol, side, order_type, status, quantity, price, executed_qty, executed_price, submitted_at`

```bash
longbridge orders
longbridge orders --history
longbridge orders --history --start 2024-01-01 --symbol TSLA.US
longbridge orders --format json
```

---

## order

Detailed info for a single order.

```bash
longbridge order <ORDER_ID>
```

Returns all fields from `orders` plus: `charge_detail, history_details, msg`

```bash
longbridge order 20240101-123456789
longbridge order 20240101-123456789 --format json
```

---

## executions

Query trade execution records.

```bash
longbridge executions [--history] [--start <DATE>] [--end <DATE>] [--symbol <SYMBOL>]
```

Returns: `order_id, trade_id, symbol, price, quantity, trade_done_at`

```bash
longbridge executions
longbridge executions --history --start 2024-01-01
longbridge executions --history --symbol 700.HK --format json
```

---

## buy

Submit a buy order. Prompts for confirmation before submitting.

```bash
longbridge buy SYMBOL QUANTITY [--price <PRICE>] [--order-type <TYPE>] [--tif <TIF>]
```

**Order Types:**

| Code | Description | Price Required |
|------|-------------|---------------|
| `LO` | Limit Order (default) | Yes |
| `MO` | Market Order | No |
| `ELO` | Enhanced Limit Order | Yes |
| `ALO` | At-auction Limit Order | Yes |
| `ODD` | Odd Lot | Yes |
| `SLO` | Special Limit Order | Yes |
| `LIT` | Pre/After Hours Limit Order | Yes |
| `MIT` | Market If Touched | Yes |

**Time In Force (TIF):**

| Value | Aliases | Description |
|-------|---------|-------------|
| `Day` | - | Day order (default) |
| `GoodTilCanceled` | `gtc` | Good till canceled |
| `GoodTilDate` | `gtd` | Good till date |

```bash
longbridge buy TSLA.US 100 --price 250.00
longbridge buy 700.HK 1000 --price 300 --order-type ALO
longbridge buy AAPL.US 50 --order-type MO
longbridge buy TSLA.US 100 --price 250 --tif gtc
```

---

## sell

Submit a sell order. Same options as `buy`.

```bash
longbridge sell SYMBOL QUANTITY [--price <PRICE>] [--order-type <TYPE>] [--tif <TIF>]
```

```bash
longbridge sell TSLA.US 100 --price 260.00
longbridge sell 700.HK 1000 --price 310
longbridge sell AAPL.US 50 --order-type MO
```

---

## cancel

Cancel an order. Prompts for confirmation.

```bash
longbridge cancel <ORDER_ID>
```

Only works on cancelable orders (status: New, PartialFilled, etc.).

```bash
longbridge cancel 20240101-123456789
```

---

## replace

Modify an existing order's quantity and/or price. Prompts for confirmation.

```bash
longbridge replace <ORDER_ID> --qty <QTY> [--price <PRICE>]
```

- `--qty`: New quantity (required)
- `--price`: New limit price (optional; omit to keep current price)

```bash
longbridge replace 20240101-123456789 --qty 200 --price 255.00
longbridge replace 20240101-123456789 --qty 150
```

---

## balance

Account cash balance by currency.

```bash
longbridge balance [--currency <CURRENCY>]
```

- `--currency`: Filter by currency code (e.g., `USD`, `HKD`, `CNY`, `SGD`)

Returns: `currency, total_cash, max_finance_amount, remaining_finance_amount, risk_level, margin_call`

```bash
longbridge balance
longbridge balance --currency USD
longbridge balance --currency HKD --format json
```

---

## cash-flow

Cash flow transaction history.

```bash
longbridge cash-flow [--start <DATE>] [--end <DATE>]
```

- Default: last 30 days

Returns: `flow_name, symbol, business_type, balance, currency, business_time, description`

```bash
longbridge cash-flow
longbridge cash-flow --start 2024-01-01 --end 2024-03-31
longbridge cash-flow --format json
```

---

## positions

Current stock holdings across all sub-accounts.

```bash
longbridge positions [--format json]
```

Returns: `symbol, name, quantity, available_quantity, cost_price, currency, market`

```bash
longbridge positions
longbridge positions --format json
```

---

## fund-positions

Current fund/ETF holdings.

```bash
longbridge fund-positions [--format json]
```

Returns: `symbol, name, current_net_asset_value, cost_net_asset_value, currency, holding_units`

---

## margin-ratio

Margin ratios for a security.

```bash
longbridge margin-ratio SYMBOL
```

Returns:
- `im_factor`: Initial margin ratio
- `mm_factor`: Maintenance margin ratio
- `fm_factor`: Forced liquidation ratio

```bash
longbridge margin-ratio TSLA.US
longbridge margin-ratio 700.HK --format json
```

---

## max-qty

Estimate maximum tradeable quantity given available funds/positions.

```bash
longbridge max-qty SYMBOL --side <SIDE> [--price <PRICE>] [--order-type <TYPE>]
```

- `--side`: `buy` or `sell` (required)
- `--price`: Limit price (required for `LO` orders)
- `--order-type`: `LO` (default), `MO`, `ELO`, `ALO`

Returns: `cash_max_qty` (cash account max), `margin_max_qty` (margin account max)

```bash
longbridge max-qty TSLA.US --side buy --price 250
longbridge max-qty 700.HK --side buy --price 300 --order-type ALO
longbridge max-qty AAPL.US --side sell --order-type MO
```
