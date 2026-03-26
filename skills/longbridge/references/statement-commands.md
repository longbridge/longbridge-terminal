# Statement Commands

Query and download account statements (daily or monthly) as CSV.

## Workflow

1. **List** statements to get available `file_key` values
2. **Download** a statement by `file_key`, selecting one or more sections to export

## Commands

### `statement list` — Query statement list

```bash
longbridge statement list [--type daily|monthly] [--start-date <YYYYMMDD>] [--limit <N>]
```

| Flag           | Required | Default    | Description                          |
|----------------|----------|------------|--------------------------------------|
| `--type`       | No       | `daily`    | Statement type: `daily` / `monthly`  |
| `--start-date` | No       |  -         | Start date for the query (YYYYMMDD)  |
| `--limit`      | No       | `5`        | Number of results to return          |

**Examples:**

```bash
# List recent 5 daily statements
longbridge statement list 

# List monthly statements
longbridge statement list  --type monthly

# List with custom date range and limit
longbridge statement list  --start-date 20250101 --limit 10

# Output as csv
longbridge statement list  --format csv
```

**Output columns:** `Date`, `File Key`

### `statement download` — Download and export sections as CSV

```bash
longbridge statement download --file-key <KEY> --section <SECTION>... -o <OUTPUT>
```

| Flag           | Required | Description                                                       |
|----------------|----------|-------------------------------------------------------------------|
| `--file-key`   | Yes      | File key obtained from `statement list`                           |
| `--section`    | Yes      | One or more sections to export (see table below)                  |
| `-o, --output` | Yes      | Output path. Single section: file path. Multiple sections: directory. |

**Single section** — writes one CSV file:

```bash
longbridge statement download --file-key abc123 --section stock_trades -o trades.csv
```

**Multiple sections** — creates a directory with one CSV per section:

```bash
longbridge statement download --file-key abc123 \
  --section equity_holdings stock_trades interests \
  -o ./statement-2025-03/
# produces:
#   ./statement-2025-03/equity_holdings.csv
#   ./statement-2025-03/stock_trades.csv
#   ./statement-2025-03/interests.csv
```

## StatementSection Reference

| Value                      | Description                                | CSV Columns |
|----------------------------|--------------------------------------------|-------------|
| `equity_holdings`          | Equity/stock holdings summary              | equity_type, market, currency, code, name, begin_quantity, change_quantity, ledger_quantity, close_price, market_value, margin_rate, margin_value, cost_price, income_amount |
| `account_balance_changes`  | Account balance change records             | currency, date, type, amount, remark, biz_code |
| `stock_trades`             | Stock trade records                        | market, currency, trade_date, settle_date, contract_no, direction, code, name, trade_quantity, trade_price, trade_amount, clear_amount |
| `equity_holding_changes`   | Equity holding change records              | market, date, code, name, type, quantity |
| `account_balance_locks`    | Account balance lock records               | currency, date, expire_date, amount, remark, ref_no |
| `equity_holding_locks`     | Equity holding lock records                | market, date, expire_date, code, name, quantity, remark, ref_no |
| `option_trades`            | Option trade records                       | market, currency, trade_date, settle_date, contract_no, direction, code, name, trade_quantity, trade_price, trade_amount, clear_amount |
| `fund_trades`              | Fund trade records                         | currency, equity_type, order_date, confirm_date, status, contract_no, code, name, direction, trade_amount, trade_quantity, price |
| `ipo_trades`               | IPO subscription records                   | market, sub_date, code, name, sub_method, sub_quantity, sub_amount |
| `virtual_trades`           | Virtual asset trade records                | market, currency, trade_date, settle_date, contract_no, direction, code, name, trade_quantity, trade_price, trade_amount, clear_amount |
| `interests`                | Interest charges/credits                   | date, currency, rate, fine_interest, interest, total |
| `lending_fees`             | Securities lending fee records             | date, currency, code, name, quantity, settle_price, lending_market_value, rate, amount |
| `custodian_fees`           | Custodian fee records                      | date, currency, rate, fee_amount, fee, total |
| `corps`                    | Corporate actions (dividends, splits, etc) | date, pay_date, market, code, name, remark, quantity, new_code, new_name, new_quantity, currency, new_amount |

## Scenario Guide — Which sections to query

| User intent | Recommended sections | Description |
|-------------|---------------------|-------------|
| Check current holdings / positions | `equity_holdings` | Shows all equity positions with quantity, market value, cost price, and P&L |
| Review recent asset changes | `account_balance_changes` `equity_holding_changes` | Balance changes show cash movements (deposits, withdrawals, fees); holding changes show stock quantity movements (transfers, corporate actions) |
| Check recent order / trade history | `stock_trades` `fund_trades` `ipo_trades` `virtual_trades` | Covers all trade types — stock, fund, IPO subscriptions, and virtual asset trades. Pick the relevant ones or use all four for a complete picture |
| Check margin interest / financing costs | `interests` | Shows daily interest charges with rate, fine interest, and totals by currency |
| Review lending and custody costs | `lending_fees` `custodian_fees` | Lending fees for borrowed securities; custodian fees for asset custody |
| Check corporate actions (dividends, splits) | `corps` | Dividend payouts, stock splits, name changes, and other corporate events |
| Full statement export | all sections | Download every section into a directory for archival or analysis |

### Examples by scenario

```bash
# 1. "What are my current holdings?"
longbridge statement list
longbridge statement download --file-key <KEY> \
  --section equity_holdings -o holdings.csv

# 2. "What asset changes happened recently?"
longbridge statement download --file-key <KEY> \
  --section account_balance_changes equity_holding_changes \
  -o ./asset-changes/

# 3. "Show me my recent trades / orders"
longbridge statement download --file-key <KEY> \
  --section stock_trades fund_trades ipo_trades virtual_trades \
  -o ./trades/

# 4. "How much margin interest am I paying?"
longbridge statement download --file-key <KEY> \
  --section interests -o interests.csv

# 5. "Give me all fees and costs"
longbridge statement download --file-key <KEY> \
  --section interests lending_fees custodian_fees \
  -o ./fees/

# 6. "Any corporate actions on my holdings?"
longbridge statement download --file-key <KEY> \
  --section corps -o corps.csv

# 7. Full daily export
longbridge statement download --file-key <KEY> \
  --section equity_holdings account_balance_changes stock_trades \
    equity_holding_changes account_balance_locks equity_holding_locks \
    option_trades fund_trades ipo_trades virtual_trades \
    interests lending_fees custodian_fees corps \
  -o ./full-statement/
```

## Common Recipes

```bash
# Quick daily workflow: list → download holdings + trades
longbridge statement list
longbridge statement download --file-key <KEY> \
  --section equity_holdings stock_trades account_balance_changes \
  -o ./daily-report/

# Export only interest and fee sections from a monthly statement
longbridge statement list --type monthly
longbridge statement download --file-key <KEY> \
  --section interests lending_fees custodian_fees \
  -o ./monthly-fees/

# Single section to a specific file
longbridge statement download --file-key <KEY> --section corps -o corps.csv
```
