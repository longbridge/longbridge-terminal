# CLI Test Plan

## Overview

The CLI layer has **three distinct testing concerns**:

| Layer             | What to test                                       | Network required |
| ----------------- | -------------------------------------------------- | ---------------- |
| Argument parsing  | `clap` parses flags/args correctly                 | No               |
| Output formatting | `table`/`json`/`csv` renders mock data correctly   | No               |
| Command dispatch  | Handler calls the right API method with right args | No (via mock)    |
| Integration       | End-to-end with real Longbridge API                | Yes              |

100% command coverage is achieved by combining the first three layers. Integration tests are gated behind a `#[cfg(feature = "integration")]` flag and are never run in CI without real credentials.

---

## Architecture prerequisite: testable design

The existing code uses global `OnceLock` (`QUOTE_CTX`, `TRADE_CTX`). To make the command handlers testable without a real WebSocket connection, each handler must **not** call `openapi::quote()` or `openapi::trade()` directly. Instead, handlers accept trait objects:

```rust
// src/cli/api.rs
#[async_trait::async_trait]
pub trait QuoteApi: Send + Sync {
    async fn quote(&self, symbols: &[&str]) -> anyhow::Result<Vec<longbridge::quote::SecurityQuote>>;
    async fn depth(&self, symbol: &str) -> anyhow::Result<longbridge::quote::SecurityDepth>;
    async fn brokers(&self, symbol: &str) -> anyhow::Result<longbridge::quote::SecurityBrokers>;
    async fn trades(&self, symbol: &str, count: usize) -> anyhow::Result<Vec<longbridge::quote::Trade>>;
    async fn intraday(&self, symbol: &str) -> anyhow::Result<Vec<longbridge::quote::IntradayLine>>;
    async fn candlesticks(&self, symbol: &str, period: longbridge::quote::Period, count: usize) -> anyhow::Result<Vec<longbridge::quote::Candlestick>>;
    async fn history_candlesticks_by_date(&self, symbol: &str, period: longbridge::quote::Period, start: Option<time::Date>, end: Option<time::Date>) -> anyhow::Result<Vec<longbridge::quote::Candlestick>>;
    async fn static_info(&self, symbols: &[&str]) -> anyhow::Result<Vec<longbridge::quote::SecurityStaticInfo>>;
    async fn calc_indexes(&self, symbols: &[&str], indexes: &[longbridge::quote::CalcIndex]) -> anyhow::Result<Vec<longbridge::quote::SecurityCalcIndex>>;
    async fn capital_flow(&self, symbol: &str) -> anyhow::Result<Vec<longbridge::quote::CapitalFlowLine>>;
    async fn capital_distribution(&self, symbol: &str) -> anyhow::Result<longbridge::quote::CapitalDistributionResponse>;
    async fn market_temperature(&self, market: longbridge::Market) -> anyhow::Result<longbridge::quote::MarketTemperatureResponse>;
    async fn trading_session(&self) -> anyhow::Result<Vec<longbridge::quote::MarketTradingSession>>;
    async fn trading_days(&self, market: longbridge::Market, start: time::Date, end: time::Date) -> anyhow::Result<longbridge::quote::MarketTradingDays>;
    async fn security_list(&self, market: longbridge::Market, category: longbridge::quote::SecurityListCategory) -> anyhow::Result<Vec<longbridge::quote::Security>>;
    async fn participants(&self) -> anyhow::Result<Vec<longbridge::quote::ParticipantInfo>>;
    async fn subscriptions(&self) -> anyhow::Result<Vec<longbridge::quote::Subscription>>;
    async fn option_quote(&self, symbols: &[&str]) -> anyhow::Result<Vec<longbridge::quote::OptionQuote>>;
    async fn option_chain_expiry_date_list(&self, symbol: &str) -> anyhow::Result<Vec<time::Date>>;
    async fn option_chain_info_by_date(&self, symbol: &str, expiry_date: time::Date) -> anyhow::Result<Vec<longbridge::quote::StrikeInfo>>;
    async fn warrant_quote(&self, symbols: &[&str]) -> anyhow::Result<Vec<longbridge::quote::WarrantQuote>>;
    async fn warrant_list(&self, symbol: &str, opts: longbridge::quote::QueryWarrantOptions) -> anyhow::Result<Vec<longbridge::quote::Warrant>>;
    async fn warrant_issuers(&self) -> anyhow::Result<Vec<longbridge::quote::IssuerInfo>>;
    async fn watchlist(&self) -> anyhow::Result<Vec<longbridge::quote::WatchlistGroup>>;
    async fn create_watchlist_group(&self, name: &str) -> anyhow::Result<i64>;
    async fn delete_watchlist_group(&self, id: i64) -> anyhow::Result<()>;
    async fn update_watchlist_group(&self, opts: longbridge::quote::UpdateWatchlistGroup) -> anyhow::Result<()>;
}

#[async_trait::async_trait]
pub trait TradeApi: Send + Sync {
    async fn today_orders(&self, opts: longbridge::trade::GetTodayOrdersOptions) -> anyhow::Result<Vec<longbridge::trade::Order>>;
    async fn history_orders(&self, opts: longbridge::trade::GetHistoryOrdersOptions) -> anyhow::Result<Vec<longbridge::trade::Order>>;
    async fn order_detail(&self, order_id: &str) -> anyhow::Result<longbridge::trade::OrderDetail>;
    async fn today_executions(&self, opts: longbridge::trade::GetTodayExecutionsOptions) -> anyhow::Result<Vec<longbridge::trade::Execution>>;
    async fn history_executions(&self, opts: longbridge::trade::GetHistoryExecutionsOptions) -> anyhow::Result<Vec<longbridge::trade::Execution>>;
    async fn submit_order(&self, opts: longbridge::trade::SubmitOrderOptions) -> anyhow::Result<longbridge::trade::SubmitOrderResponse>;
    async fn cancel_order(&self, order_id: &str) -> anyhow::Result<()>;
    async fn replace_order(&self, opts: longbridge::trade::ReplaceOrderOptions) -> anyhow::Result<()>;
    async fn account_balance(&self, currency: Option<&str>) -> anyhow::Result<Vec<longbridge::trade::AccountBalance>>;
    async fn cash_flow(&self, opts: longbridge::trade::GetCashFlowOptions) -> anyhow::Result<Vec<longbridge::trade::CashFlow>>;
    async fn stock_positions(&self, symbols: Option<&[&str]>) -> anyhow::Result<longbridge::trade::StockPositionsResponse>;
    async fn fund_positions(&self, symbols: Option<&[&str]>) -> anyhow::Result<longbridge::trade::FundPositionsResponse>;
    async fn margin_ratio(&self, symbol: &str) -> anyhow::Result<longbridge::trade::MarginRatio>;
    async fn estimate_max_purchase_quantity(&self, opts: longbridge::trade::EstimateMaxPurchaseQuantityOptions) -> anyhow::Result<longbridge::trade::EstimateMaxPurchaseQuantityResponse>;
}
```

The production `LbQuoteApi` and `LbTradeApi` wrappers simply delegate to the global contexts. Tests use `MockQuoteApi` / `MockTradeApi` built with the `mockall` crate.

---

## Dependencies to add

```toml
[dev-dependencies]
assert_cmd = "2"        # CLI subprocess testing
predicates = "3"        # Output assertions
mockall = "0.13"        # Mock trait generation
tempfile = "3"          # Temp dirs for token file tests
tokio = { version = "1", features = ["rt", "macros"] }

[features]
integration = []        # cargo test --features integration
```

---

## Test file structure

```
tests/
├── cli_parse.rs          # Argument parsing (no network)
├── cli_output.rs         # Formatting layer (no network)
├── cli_quote.rs          # Quote command dispatch (mock API)
├── cli_trade.rs          # Trade command dispatch (mock API)
├── cli_watchlist.rs      # Watchlist command dispatch (mock API)
├── cli_auth.rs           # login/logout (no network, file I/O only)
└── cli_integration.rs    # End-to-end (#[cfg(feature = "integration")])

src/cli/
├── mod.rs
├── api.rs                # QuoteApi + TradeApi traits
├── output.rs             # Formatting (contains unit tests inline)
├── quote.rs
├── trade.rs
└── watchlist.rs
```

---

## Layer 1: Argument parsing tests (`tests/cli_parse.rs`)

Tests use `clap`'s `try_parse_from` — zero network calls.

### Auth commands

```rust
#[test]
fn test_login_subcommand() {
    let cli = Cli::try_parse_from(["longbridge", "login"]).unwrap();
    assert!(matches!(cli.command, Commands::Login));
}

#[test]
fn test_logout_subcommand() {
    let cli = Cli::try_parse_from(["longbridge", "logout"]).unwrap();
    assert!(matches!(cli.command, Commands::Logout));
}
```

### Quote commands

```rust
// quote TSLA.US AAPL.US
#[test]
fn test_quote_multiple_symbols() {
    let cli = Cli::try_parse_from(["longbridge", "quote", "TSLA.US", "AAPL.US"]).unwrap();
    let Commands::Quote { symbols, format } = cli.command else { panic!() };
    assert_eq!(symbols, ["TSLA.US", "AAPL.US"]);
    assert_eq!(format, OutputFormat::Table);
}

#[test]
fn test_quote_json_format() {
    let cli = Cli::try_parse_from(["longbridge", "quote", "TSLA.US", "--format", "json"]).unwrap();
    let Commands::Quote { format, .. } = cli.command else { panic!() };
    assert_eq!(format, OutputFormat::Json);
}

#[test]
fn test_quote_csv_format() {
    let cli = Cli::try_parse_from(["longbridge", "quote", "TSLA.US", "--format", "csv"]).unwrap();
    let Commands::Quote { format, .. } = cli.command else { panic!() };
    assert_eq!(format, OutputFormat::Csv);
}

#[test]
fn test_quote_requires_symbol() {
    assert!(Cli::try_parse_from(["longbridge", "quote"]).is_err());
}

// depth
#[test]
fn test_depth_single_symbol() {
    let cli = Cli::try_parse_from(["longbridge", "depth", "TSLA.US"]).unwrap();
    let Commands::Depth { symbol, .. } = cli.command else { panic!() };
    assert_eq!(symbol, "TSLA.US");
}

// trades --count
#[test]
fn test_trades_default_count() {
    let cli = Cli::try_parse_from(["longbridge", "trades", "TSLA.US"]).unwrap();
    let Commands::Trades { count, .. } = cli.command else { panic!() };
    assert_eq!(count, 50); // default
}

#[test]
fn test_trades_custom_count() {
    let cli = Cli::try_parse_from(["longbridge", "trades", "TSLA.US", "--count", "20"]).unwrap();
    let Commands::Trades { count, .. } = cli.command else { panic!() };
    assert_eq!(count, 20);
}

// kline --period --count
#[test]
fn test_kline_defaults() {
    let cli = Cli::try_parse_from(["longbridge", "kline", "TSLA.US"]).unwrap();
    let Commands::Kline { period, count, .. } = cli.command else { panic!() };
    assert_eq!(period, Period::Day);
    assert_eq!(count, 100);
}

#[test]
fn test_kline_period_minute() {
    let cli = Cli::try_parse_from(["longbridge", "kline", "TSLA.US", "--period", "5m"]).unwrap();
    let Commands::Kline { period, .. } = cli.command else { panic!() };
    assert_eq!(period, Period::Min5);
}

// kline-history --start --end
#[test]
fn test_kline_history_date_range() {
    let cli = Cli::try_parse_from([
        "longbridge", "kline-history", "TSLA.US",
        "--start", "2024-01-01", "--end", "2024-12-31",
    ]).unwrap();
    let Commands::KlineHistory { start, end, .. } = cli.command else { panic!() };
    assert_eq!(start.unwrap().to_string(), "2024-01-01");
    assert_eq!(end.unwrap().to_string(), "2024-12-31");
}

// calc-index --index
#[test]
fn test_calc_index_multiple() {
    let cli = Cli::try_parse_from(["longbridge", "calc-index", "TSLA.US", "--index", "pe,pb,eps"]).unwrap();
    let Commands::CalcIndex { indexes, .. } = cli.command else { panic!() };
    assert_eq!(indexes, ["pe", "pb", "eps"]);
}

// market-temp [HK|US|CN|SG]
#[test]
fn test_market_temp_valid_markets() {
    for market in ["HK", "US", "CN", "SG"] {
        let cli = Cli::try_parse_from(["longbridge", "market-temp", market]).unwrap();
        let Commands::MarketTemp { market: m, .. } = cli.command else { panic!() };
        assert_eq!(m.to_string(), market);
    }
}

// trading-days --start --end
#[test]
fn test_trading_days_optional_range() {
    let cli = Cli::try_parse_from(["longbridge", "trading-days", "HK"]).unwrap();
    let Commands::TradingDays { start, end, .. } = cli.command else { panic!() };
    assert!(start.is_none());
    assert!(end.is_none());
}

// security-list --category
#[test]
fn test_security_list_default_category() {
    let cli = Cli::try_parse_from(["longbridge", "security-list", "HK"]).unwrap();
    let Commands::SecurityList { category, .. } = cli.command else { panic!() };
    assert_eq!(category, SecurityCategory::Main);
}
```

### Option / Warrant commands

```rust
#[test]
fn test_option_chain_no_date() {
    let cli = Cli::try_parse_from(["longbridge", "option-chain", "AAPL"]).unwrap();
    let Commands::OptionChain { symbol, date, .. } = cli.command else { panic!() };
    assert_eq!(symbol, "AAPL");
    assert!(date.is_none());
}

#[test]
fn test_option_chain_with_date() {
    let cli = Cli::try_parse_from(["longbridge", "option-chain", "AAPL", "--date", "2024-01-19"]).unwrap();
    let Commands::OptionChain { date, .. } = cli.command else { panic!() };
    assert!(date.is_some());
}

#[test]
fn test_warrant_list_symbol() {
    let cli = Cli::try_parse_from(["longbridge", "warrant-list", "700.HK"]).unwrap();
    let Commands::WarrantList { symbol, .. } = cli.command else { panic!() };
    assert_eq!(symbol, "700.HK");
}
```

### Watchlist commands

```rust
#[test]
fn test_watchlist_list() {
    let cli = Cli::try_parse_from(["longbridge", "watchlist"]).unwrap();
    assert!(matches!(cli.command, Commands::Watchlist(WatchlistCmd::List { .. })));
}

#[test]
fn test_watchlist_create() {
    let cli = Cli::try_parse_from(["longbridge", "watchlist", "create", "My Portfolio"]).unwrap();
    let Commands::Watchlist(WatchlistCmd::Create { name, .. }) = cli.command else { panic!() };
    assert_eq!(name, "My Portfolio");
}

#[test]
fn test_watchlist_delete() {
    let cli = Cli::try_parse_from(["longbridge", "watchlist", "delete", "123"]).unwrap();
    let Commands::Watchlist(WatchlistCmd::Delete { id, .. }) = cli.command else { panic!() };
    assert_eq!(id, 123);
}

#[test]
fn test_watchlist_update_add_remove() {
    let cli = Cli::try_parse_from([
        "longbridge", "watchlist", "update", "42",
        "--add", "TSLA.US", "--remove", "AAPL.US",
    ]).unwrap();
    let Commands::Watchlist(WatchlistCmd::Update { id, add, remove, .. }) = cli.command else { panic!() };
    assert_eq!(id, 42);
    assert_eq!(add, ["TSLA.US"]);
    assert_eq!(remove, ["AAPL.US"]);
}

#[test]
fn test_watchlist_update_mode() {
    let cli = Cli::try_parse_from([
        "longbridge", "watchlist", "update", "42", "--mode", "replace",
    ]).unwrap();
    let Commands::Watchlist(WatchlistCmd::Update { mode, .. }) = cli.command else { panic!() };
    assert_eq!(mode, WatchlistUpdateMode::Replace);
}
```

### Trade commands

```rust
#[test]
fn test_orders_today() {
    let cli = Cli::try_parse_from(["longbridge", "orders"]).unwrap();
    let Commands::Orders { history, .. } = cli.command else { panic!() };
    assert!(!history);
}

#[test]
fn test_orders_history_with_filters() {
    let cli = Cli::try_parse_from([
        "longbridge", "orders", "--history",
        "--start", "2024-01-01", "--end", "2024-12-31",
        "--symbol", "TSLA.US", "--status", "filled",
    ]).unwrap();
    let Commands::Orders { history, start, end, symbol, status, .. } = cli.command else { panic!() };
    assert!(history);
    assert_eq!(symbol.unwrap(), "TSLA.US");
    assert_eq!(status.unwrap(), "filled");
}

#[test]
fn test_buy_required_args() {
    let cli = Cli::try_parse_from([
        "longbridge", "buy", "TSLA.US", "100", "--price", "250",
    ]).unwrap();
    let Commands::Buy { symbol, qty, price, .. } = cli.command else { panic!() };
    assert_eq!(symbol, "TSLA.US");
    assert_eq!(qty, 100);
    assert_eq!(price, dec!(250));
}

#[test]
fn test_buy_with_order_type_and_tif() {
    let cli = Cli::try_parse_from([
        "longbridge", "buy", "TSLA.US", "100", "--price", "250",
        "--type", "LO", "--tif", "gtc",
    ]).unwrap();
    let Commands::Buy { order_type, tif, .. } = cli.command else { panic!() };
    assert_eq!(order_type, OrderType::LO);
    assert_eq!(tif, TimeInForce::GTC);
}

#[test]
fn test_sell_args() {
    let cli = Cli::try_parse_from(["longbridge", "sell", "TSLA.US", "50", "--price", "260"]).unwrap();
    let Commands::Sell { symbol, qty, price, .. } = cli.command else { panic!() };
    assert_eq!(symbol, "TSLA.US");
    assert_eq!(qty, 50);
    assert_eq!(price, dec!(260));
}

#[test]
fn test_replace_order_args() {
    let cli = Cli::try_parse_from([
        "longbridge", "replace", "ORDER001", "--qty", "200", "--price", "255",
    ]).unwrap();
    let Commands::Replace { order_id, qty, price, .. } = cli.command else { panic!() };
    assert_eq!(order_id, "ORDER001");
    assert_eq!(qty.unwrap(), 200);
    assert_eq!(price.unwrap(), dec!(255));
}

#[test]
fn test_cancel_order() {
    let cli = Cli::try_parse_from(["longbridge", "cancel", "ORDER001"]).unwrap();
    let Commands::Cancel { order_id, .. } = cli.command else { panic!() };
    assert_eq!(order_id, "ORDER001");
}

#[test]
fn test_max_qty_args() {
    let cli = Cli::try_parse_from([
        "longbridge", "max-qty", "TSLA.US", "--side", "buy", "--price", "250",
    ]).unwrap();
    let Commands::MaxQty { symbol, side, price, .. } = cli.command else { panic!() };
    assert_eq!(symbol, "TSLA.US");
    assert_eq!(side, OrderSide::Buy);
    assert_eq!(price, dec!(250));
}

#[test]
fn test_balance_currency_filter() {
    let cli = Cli::try_parse_from(["longbridge", "balance", "--currency", "USD"]).unwrap();
    let Commands::Balance { currency, .. } = cli.command else { panic!() };
    assert_eq!(currency.unwrap(), "USD");
}

#[test]
fn test_cash_flow_date_range() {
    let cli = Cli::try_parse_from([
        "longbridge", "cash-flow",
        "--start", "2024-01-01", "--end", "2024-12-31",
    ]).unwrap();
    let Commands::CashFlow { start, end, .. } = cli.command else { panic!() };
    assert!(start.is_some());
    assert!(end.is_some());
}
```

---

## Layer 2: Output formatting tests (`src/cli/output.rs` inline tests)

Formatting is a pure function: `fn render(data: &[T], format: OutputFormat) -> String`. No network required.

```rust
// src/cli/output.rs
#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn make_quote_row() -> QuoteRow {
        QuoteRow {
            symbol: "TSLA.US".into(),
            name: "Tesla".into(),
            price: dec!(250.00),
            change: dec!(5.00),
            change_rate: dec!(2.04),
            volume: 1_000_000,
            turnover: dec!(250_000_000),
        }
    }

    #[test]
    fn quote_table_contains_symbol() {
        let output = render_quotes(&[make_quote_row()], OutputFormat::Table);
        assert!(output.contains("TSLA.US"));
        assert!(output.contains("250.00"));
    }

    #[test]
    fn quote_json_is_valid() {
        let output = render_quotes(&[make_quote_row()], OutputFormat::Json);
        let v: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(v[0]["symbol"], "TSLA.US");
        assert_eq!(v[0]["price"].as_str().unwrap(), "250.00");
    }

    #[test]
    fn quote_csv_header_and_row() {
        let output = render_quotes(&[make_quote_row()], OutputFormat::Csv);
        let lines: Vec<&str> = output.lines().collect();
        assert!(lines[0].contains("symbol"));   // header
        assert!(lines[1].contains("TSLA.US"));  // data
    }

    #[test]
    fn empty_data_json_returns_array() {
        let output = render_quotes(&[], OutputFormat::Json);
        assert_eq!(output.trim(), "[]");
    }

    #[test]
    fn depth_table_shows_bid_ask_labels() {
        let output = render_depth(&make_depth(), OutputFormat::Table);
        assert!(output.contains("Bid"));
        assert!(output.contains("Ask"));
    }

    #[test]
    fn order_table_shows_order_id() {
        let output = render_orders(&[make_order()], OutputFormat::Table);
        assert!(output.contains("ORDER001"));
    }

    #[test]
    fn position_csv_correct_columns() {
        let output = render_positions(&[make_position()], OutputFormat::Csv);
        assert!(output.lines().next().unwrap().contains("cost_price"));
    }

    // ... similar tests for each render_* function
}
```

**Required `render_*` functions** (one per command group):

| Function                      | Input type          | Formats tested   |
| ----------------------------- | ------------------- | ---------------- |
| `render_quotes`               | `QuoteRow`          | table, json, csv |
| `render_depth`                | `DepthRow`          | table, json, csv |
| `render_brokers`              | `BrokerRow`         | table, json, csv |
| `render_trades`               | `TradeRow`          | table, json, csv |
| `render_intraday`             | `IntradayRow`       | table, json, csv |
| `render_klines`               | `KlineRow`          | table, json, csv |
| `render_static_info`          | `StaticInfoRow`     | table, json, csv |
| `render_calc_indexes`         | `CalcIndexRow`      | table, json, csv |
| `render_capital_flow`         | `CapitalFlowRow`    | table, json, csv |
| `render_capital_dist`         | `CapitalDistRow`    | table, json, csv |
| `render_market_temp`          | `MarketTempRow`     | table, json, csv |
| `render_trading_session`      | `TradingSessionRow` | table, json, csv |
| `render_trading_days`         | `TradingDaysRow`    | table, json, csv |
| `render_security_list`        | `SecurityRow`       | table, json, csv |
| `render_participants`         | `ParticipantRow`    | table, json, csv |
| `render_subscriptions`        | `SubscriptionRow`   | table, json, csv |
| `render_option_quotes`        | `OptionQuoteRow`    | table, json, csv |
| `render_option_chain_dates`   | `time::Date`        | table, json, csv |
| `render_option_chain_strikes` | `StrikeRow`         | table, json, csv |
| `render_warrant_quotes`       | `WarrantQuoteRow`   | table, json, csv |
| `render_warrant_list`         | `WarrantRow`        | table, json, csv |
| `render_warrant_issuers`      | `IssuerRow`         | table, json, csv |
| `render_watchlist_groups`     | `WatchlistGroupRow` | table, json, csv |
| `render_orders`               | `OrderRow`          | table, json, csv |
| `render_order_detail`         | `OrderDetailRow`    | table, json, csv |
| `render_executions`           | `ExecutionRow`      | table, json, csv |
| `render_balance`              | `BalanceRow`        | table, json, csv |
| `render_cash_flow`            | `CashFlowRow`       | table, json, csv |
| `render_positions`            | `PositionRow`       | table, json, csv |
| `render_fund_positions`       | `FundPositionRow`   | table, json, csv |
| `render_margin_ratio`         | `MarginRatioRow`    | table, json, csv |
| `render_max_qty`              | `MaxQtyRow`         | table, json, csv |

---

## Layer 3: Command dispatch tests (mock API)

Use `mockall` to generate mock impls of `QuoteApi` and `TradeApi`. Tests verify that the handler calls the correct method with the correct arguments and passes the result to the formatter.

### Setup (`tests/helpers.rs`)

```rust
use mockall::mock;

mock! {
    pub QuoteApi {}
    #[async_trait::async_trait]
    impl QuoteApi for QuoteApi {
        async fn quote(&self, symbols: &[&str]) -> anyhow::Result<Vec<SecurityQuote>>;
        async fn depth(&self, symbol: &str) -> anyhow::Result<SecurityDepth>;
        // ... all methods
    }
}

mock! {
    pub TradeApi {}
    #[async_trait::async_trait]
    impl TradeApi for TradeApi {
        async fn today_orders(&self, opts: GetTodayOrdersOptions) -> anyhow::Result<Vec<Order>>;
        // ... all methods
    }
}
```

### Quote dispatch (`tests/cli_quote.rs`)

```rust
#[tokio::test]
async fn cmd_quote_calls_api_with_correct_symbols() {
    let mut mock = MockQuoteApi::new();
    mock.expect_quote()
        .withf(|symbols| symbols == &["TSLA.US", "AAPL.US"])
        .times(1)
        .returning(|_| Ok(vec![]));

    let mut buf = Vec::new();
    run_quote_cmd(
        &mock,
        &["TSLA.US", "AAPL.US"],
        OutputFormat::Json,
        &mut buf,
    ).await.unwrap();

    // output is valid JSON array
    let v: serde_json::Value = serde_json::from_slice(&buf).unwrap();
    assert!(v.is_array());
}

#[tokio::test]
async fn cmd_depth_calls_api_with_symbol() {
    let mut mock = MockQuoteApi::new();
    mock.expect_depth()
        .with(mockall::predicate::eq("TSLA.US"))
        .times(1)
        .returning(|_| Ok(make_depth_response()));
    run_depth_cmd(&mock, "TSLA.US", OutputFormat::Table, &mut Vec::new()).await.unwrap();
}

#[tokio::test]
async fn cmd_brokers_calls_api() { /* ... */ }

#[tokio::test]
async fn cmd_trades_uses_count_arg() {
    let mut mock = MockQuoteApi::new();
    mock.expect_trades()
        .withf(|sym, count| sym == "TSLA.US" && *count == 20)
        .times(1)
        .returning(|_, _| Ok(vec![]));
    run_trades_cmd(&mock, "TSLA.US", 20, OutputFormat::Table, &mut Vec::new()).await.unwrap();
}

#[tokio::test]
async fn cmd_kline_default_period_day() {
    let mut mock = MockQuoteApi::new();
    mock.expect_candlesticks()
        .withf(|_, period, count| *period == Period::Day && *count == 100)
        .times(1)
        .returning(|_, _, _| Ok(vec![]));
    // ...
}

#[tokio::test]
async fn cmd_kline_history_by_date_range() {
    let mut mock = MockQuoteApi::new();
    mock.expect_history_candlesticks_by_date()
        .withf(|sym, _, start, end| {
            sym == "TSLA.US"
            && start.is_some()
            && end.is_some()
        })
        .times(1)
        .returning(|_, _, _, _| Ok(vec![]));
    // ...
}

// static, calc-index, capital-flow, capital-dist, market-temp,
// trading-session, trading-days, security-list, participants,
// subscriptions, option-quote, option-chain (no date), option-chain (with date),
// warrant-quote, warrant-list, warrant-issuers — one test each.
```

### Watchlist dispatch (`tests/cli_watchlist.rs`)

```rust
#[tokio::test]
async fn cmd_watchlist_list_calls_api() {
    let mut mock = MockQuoteApi::new();
    mock.expect_watchlist().times(1).returning(|| Ok(vec![]));
    run_watchlist_list_cmd(&mock, OutputFormat::Table, &mut Vec::new()).await.unwrap();
}

#[tokio::test]
async fn cmd_watchlist_create_passes_name() {
    let mut mock = MockQuoteApi::new();
    mock.expect_create_watchlist_group()
        .with(mockall::predicate::eq("My Portfolio"))
        .times(1)
        .returning(|_| Ok(42));
    run_watchlist_create_cmd(&mock, "My Portfolio", OutputFormat::Table, &mut Vec::new()).await.unwrap();
}

#[tokio::test]
async fn cmd_watchlist_delete_passes_id() {
    let mut mock = MockQuoteApi::new();
    mock.expect_delete_watchlist_group()
        .with(mockall::predicate::eq(42i64))
        .times(1)
        .returning(|_| Ok(()));
    run_watchlist_delete_cmd(&mock, 42, OutputFormat::Table, &mut Vec::new()).await.unwrap();
}

#[tokio::test]
async fn cmd_watchlist_update_add_mode() { /* verify UpdateWatchlistGroup built correctly */ }
#[tokio::test]
async fn cmd_watchlist_update_remove_mode() { /* ... */ }
#[tokio::test]
async fn cmd_watchlist_update_replace_mode() { /* ... */ }
```

### Trade dispatch (`tests/cli_trade.rs`)

```rust
#[tokio::test]
async fn cmd_orders_today_calls_today_orders() {
    let mut mock = MockTradeApi::new();
    mock.expect_today_orders().times(1).returning(|_| Ok(vec![]));
    run_orders_cmd(&mock, false, None, None, None, None, OutputFormat::Table, &mut Vec::new()).await.unwrap();
}

#[tokio::test]
async fn cmd_orders_history_calls_history_orders() {
    let mut mock = MockTradeApi::new();
    mock.expect_history_orders().times(1).returning(|_| Ok(vec![]));
    run_orders_cmd(&mock, true, None, None, None, None, OutputFormat::Table, &mut Vec::new()).await.unwrap();
}

#[tokio::test]
async fn cmd_order_detail_passes_id() {
    let mut mock = MockTradeApi::new();
    mock.expect_order_detail()
        .with(mockall::predicate::eq("ORDER001"))
        .times(1)
        .returning(|_| Ok(make_order_detail()));
    // ...
}

#[tokio::test]
async fn cmd_executions_today() { /* today_executions */ }
#[tokio::test]
async fn cmd_executions_history() { /* history_executions */ }

#[tokio::test]
async fn cmd_buy_submits_buy_order() {
    let mut mock = MockTradeApi::new();
    mock.expect_submit_order()
        .withf(|opts| {
            opts.symbol() == "TSLA.US"
            && opts.side() == OrderSide::Buy
            && opts.quantity() == dec!(100)
        })
        .times(1)
        .returning(|_| Ok(make_submit_response()));
    // ...
}

#[tokio::test]
async fn cmd_sell_submits_sell_order() { /* side == Sell */ }

#[tokio::test]
async fn cmd_cancel_calls_cancel_order() {
    let mut mock = MockTradeApi::new();
    mock.expect_cancel_order()
        .with(mockall::predicate::eq("ORDER001"))
        .times(1)
        .returning(|_| Ok(()));
    // ...
}

#[tokio::test]
async fn cmd_replace_passes_qty_and_price() { /* verify ReplaceOrderOptions */ }

#[tokio::test]
async fn cmd_balance_no_filter() { /* currency == None */ }
#[tokio::test]
async fn cmd_balance_currency_filter() { /* currency == Some("USD") */ }

#[tokio::test]
async fn cmd_cash_flow_date_range() { /* verify opts built correctly */ }

#[tokio::test]
async fn cmd_positions_calls_stock_positions() { /* ... */ }
#[tokio::test]
async fn cmd_fund_positions_calls_fund_positions() { /* ... */ }

#[tokio::test]
async fn cmd_margin_ratio_passes_symbol() { /* ... */ }

#[tokio::test]
async fn cmd_max_qty_buy_side() {
    let mut mock = MockTradeApi::new();
    mock.expect_estimate_max_purchase_quantity()
        .withf(|opts| opts.side() == OrderSide::Buy)
        .times(1)
        .returning(|_| Ok(make_max_qty_response()));
    // ...
}

#[tokio::test]
async fn cmd_max_qty_sell_side() { /* side == Sell */ }
```

### Auth tests (`tests/cli_auth.rs`)

```rust
#[test]
fn logout_clears_token_file() {
    let dir = tempfile::tempdir().unwrap();
    let token_path = dir.path().join("session-test");
    std::fs::write(&token_path, b"dummy-token").unwrap();

    // Override token path via env or inject path
    clear_token_at(&token_path).unwrap();
    assert!(!token_path.exists());
}

#[test]
fn logout_nonexistent_token_is_ok() {
    let dir = tempfile::tempdir().unwrap();
    let token_path = dir.path().join("session-missing");
    // Should not error when file doesn't exist
    assert!(clear_token_at(&token_path).is_ok());
}
```

### Error propagation tests

One test per command verifying that API errors are surfaced rather than swallowed:

```rust
#[tokio::test]
async fn cmd_quote_propagates_api_error() {
    let mut mock = MockQuoteApi::new();
    mock.expect_quote()
        .returning(|_| Err(anyhow::anyhow!("network error")));

    let result = run_quote_cmd(&mock, &["TSLA.US"], OutputFormat::Json, &mut Vec::new()).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("network error"));
}
// Repeat for: depth, trades, brokers, kline, orders, buy, sell, balance, positions, ...
```

---

## Layer 4: Integration tests (feature-gated)

```rust
// tests/cli_integration.rs
#![cfg(feature = "integration")]

// Requires valid token in ~/.longbridge/openapi/tokens/<client_id>
// Run with: cargo test --features integration -- --test-threads=1

#[tokio::test]
async fn integration_quote_tsla() {
    let ctx = create_real_contexts().await;
    let result = ctx.quote(&["TSLA.US"]).await.unwrap();
    assert!(!result.is_empty());
    assert!(result[0].last_done > dec!(0));
}

#[tokio::test]
async fn integration_positions_no_panic() {
    let ctx = create_real_trade_contexts().await;
    let _ = ctx.stock_positions(None).await.unwrap();
}
```

---

## Coverage matrix

| Command                      | Parse | Format (table/json/csv) | Dispatch (mock) | Error path |
| ---------------------------- | ----- | ----------------------- | --------------- | ---------- |
| `login`                      | ✓     | —                       | —               | —          |
| `logout`                     | ✓     | —                       | ✓ (file I/O)    | ✓          |
| `quote`                      | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `depth`                      | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `brokers`                    | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `trades`                     | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `intraday`                   | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `kline`                      | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `kline-history`              | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `static`                     | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `calc-index`                 | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `capital-flow`               | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `capital-dist`               | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `market-temp`                | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `trading-session`            | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `trading-days`               | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `security-list`              | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `participants`               | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `subscriptions`              | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `option-quote`               | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `option-chain` (list)        | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `option-chain --date`        | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `warrant-quote`              | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `warrant-list`               | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `warrant-issuers`            | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `watchlist` (list)           | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `watchlist create`           | ✓     | —                       | ✓               | ✓          |
| `watchlist delete`           | ✓     | —                       | ✓               | ✓          |
| `watchlist update` (add)     | ✓     | —                       | ✓               | ✓          |
| `watchlist update` (remove)  | ✓     | —                       | ✓               | ✓          |
| `watchlist update` (replace) | ✓     | —                       | ✓               | ✓          |
| `orders` (today)             | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `orders --history`           | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `order <id>`                 | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `executions`                 | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `executions --history`       | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `buy`                        | ✓     | —                       | ✓               | ✓          |
| `sell`                       | ✓     | —                       | ✓               | ✓          |
| `cancel`                     | ✓     | —                       | ✓               | ✓          |
| `replace`                    | ✓     | —                       | ✓               | ✓          |
| `balance`                    | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `cash-flow`                  | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `positions`                  | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `fund-positions`             | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `margin-ratio`               | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `max-qty` (buy)              | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |
| `max-qty` (sell)             | ✓     | ✓ / ✓ / ✓               | ✓               | ✓          |

**Estimated test count**: ~200 tests across all layers (excluding integration).

---

## Running tests

```bash
# All unit tests (no network)
cargo test

# With output formatting captured
cargo test -- --nocapture

# Integration tests (requires Longbridge auth)
cargo test --features integration -- --test-threads=1

# Coverage report (requires cargo-llvm-cov)
cargo llvm-cov --html
```
