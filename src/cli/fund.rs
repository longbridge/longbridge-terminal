//! Fund (mutual fund) channel CLI.
//!
//! A full binding of the Longbridge fund `OpenAPI` (`longbridge::fund::FundContext`).
//! Every subcommand mirrors one SDK method. Funds are identified by their
//! `counter_id` (e.g. `UT/FD/HK0000384492`), taken as a positional argument;
//! paging and filter knobs are optional `--flags`. Output flows through the
//! shared [`OutputFormat`] (`json` / `table`), matching the other channels.

use std::io::Write as _;

use anyhow::Result;
use clap::Subcommand;
use longbridge::fund::{
    FundNavRangeOptions, FundPageOptions, GetFundAnalysisOptions, GetFundHoldingsOptions,
    GetFundOrdersOptions, GetFundPositionDividendsOptions, GetFundPositionOptions,
    GetFundPositionProfitsOptions, GetFundPositionsOptions, GetFundStockHoldingsOptions,
    GetFundTransactionsOptions, GetFundsOptions, SubmitFundOrderOptions, ValidateFundOrderOptions,
};
use serde::Serialize;

use super::output::{
    parse_datetime_end_timestamp, parse_datetime_start_timestamp, print_table, strip_private_fields,
};
use super::OutputFormat;

#[derive(Subcommand)]
pub enum FundCommands {
    // ── Catalog / market data ─────────────────────────────────────────────
    /// Hot-selling fund list
    Hot,

    /// Fund list (optionally filtered by quick-filter ids / earning-rate intervals)
    List {
        /// Quick-filter id (repeatable)
        #[arg(long = "quick-id", value_name = "ID")]
        quick_id: Vec<i64>,
        /// Earning-rate time interval (repeatable), e.g. `1m` `3m` `1y`
        #[arg(long = "time-interval", value_name = "INTERVAL")]
        time_interval: Vec<String>,
    },

    /// Fund list filter options
    Filters,

    /// Fund detail
    Detail {
        /// Fund counter id, e.g. UT/FD/HK0000384492
        counter_id: String,
    },

    /// Fund analysis (level 1)
    Analysis {
        /// Fund counter id
        counter_id: String,
        /// Analysis period
        #[arg(long)]
        period: Option<i32>,
    },

    /// Fund analysis detail (level 2)
    #[command(name = "analysis-detail")]
    AnalysisDetail {
        /// Fund counter id
        counter_id: String,
        /// Analysis period
        #[arg(long)]
        period: Option<i32>,
    },

    /// Fund trend chart
    Trend {
        /// Fund counter id
        counter_id: String,
        /// Analysis period
        #[arg(long)]
        period: Option<i32>,
    },

    /// Fund annual returns
    #[command(name = "annual-returns")]
    AnnualReturns {
        /// Fund counter id
        counter_id: String,
        /// Page number
        #[arg(long)]
        page: Option<i32>,
        /// Page size
        #[arg(long)]
        size: Option<i32>,
    },

    /// Fund quarterly returns
    #[command(name = "quarterly-returns")]
    QuarterlyReturns {
        /// Fund counter id
        counter_id: String,
        /// Page number
        #[arg(long)]
        page: Option<i32>,
        /// Page size
        #[arg(long)]
        size: Option<i32>,
    },

    /// Fund performance figures
    Performance {
        /// Fund counter id
        counter_id: String,
    },

    /// Fund performance comparison
    #[command(name = "performance-comparison")]
    PerformanceComparison {
        /// Fund counter id
        counter_id: String,
        /// Comparison period
        #[arg(long)]
        period: Option<i32>,
    },

    /// Fund latest net value (NAV)
    Nav {
        /// Fund counter id
        counter_id: String,
    },

    /// Fund historical net value (paged)
    #[command(name = "nav-history")]
    NavHistory {
        /// Fund counter id
        counter_id: String,
        /// Page number
        #[arg(long)]
        page: Option<i32>,
        /// Page size
        #[arg(long)]
        size: Option<i32>,
    },

    /// Fund historical net value by relative time range
    #[command(name = "nav-range")]
    NavRange {
        /// Fund counter id
        counter_id: String,
        /// Number of months before now
        #[arg(long = "month-before")]
        month_before: Option<i32>,
        /// Number of years before now
        #[arg(long = "year-before")]
        year_before: Option<i32>,
    },

    /// Fund top-10 holdings
    Holdings {
        /// Fund counter id
        counter_id: String,
        /// Scene
        #[arg(long)]
        scene: Option<i32>,
    },

    /// Stocks held by a fund (reverse lookup)
    #[command(name = "stock-holdings")]
    StockHoldings {
        /// Fund counter id
        counter_id: String,
        /// Maximum number of stocks to return
        #[arg(long)]
        limit: Option<i32>,
    },

    // ── My positions (requires login) ─────────────────────────────────────
    /// My fund positions overview
    Positions {
        /// Account channel
        #[arg(long = "account-channel")]
        account_channel: Option<String>,
        /// Account id
        #[arg(long)]
        aaid: Option<i64>,
    },

    /// My single fund position detail
    Position {
        /// Fund counter id
        counter_id: String,
        /// Account channel
        #[arg(long = "account-channel")]
        account_channel: Option<String>,
        /// Account id
        #[arg(long)]
        aaid: Option<i64>,
        /// Range start (YYYY-MM-DD or RFC 3339)
        #[arg(long)]
        start: Option<String>,
        /// Range end (YYYY-MM-DD or RFC 3339)
        #[arg(long)]
        end: Option<String>,
    },

    /// Performance figures of a held fund
    #[command(name = "position-performance")]
    PositionPerformance {
        /// Fund counter id
        counter_id: String,
    },

    /// Cumulative-profit series of a held fund
    #[command(name = "position-profits")]
    PositionProfits {
        /// Fund counter id
        counter_id: String,
        /// Range start (YYYY-MM-DD or RFC 3339)
        #[arg(long)]
        start: Option<String>,
        /// Range end (YYYY-MM-DD or RFC 3339)
        #[arg(long)]
        end: Option<String>,
        /// Page number
        #[arg(long)]
        page: Option<i32>,
        /// Page size
        #[arg(long)]
        size: Option<i32>,
    },

    /// Net-value history of a held fund
    #[command(name = "position-nav")]
    PositionNav {
        /// Fund counter id
        counter_id: String,
        /// Number of months before now
        #[arg(long = "month-before")]
        month_before: Option<i32>,
        /// Number of years before now
        #[arg(long = "year-before")]
        year_before: Option<i32>,
    },

    /// Dividend records of a held fund
    #[command(name = "position-dividends")]
    PositionDividends {
        /// Fund counter id
        counter_id: String,
        /// Currency
        #[arg(long)]
        currency: Option<String>,
        /// Range start (YYYY-MM-DD or RFC 3339)
        #[arg(long)]
        start: Option<String>,
        /// Range end (YYYY-MM-DD or RFC 3339)
        #[arg(long)]
        end: Option<String>,
        /// Page number
        #[arg(long)]
        page: Option<i32>,
        /// Page size
        #[arg(long)]
        size: Option<i32>,
    },

    // ── Orders / trading ──────────────────────────────────────────────────
    /// My fund orders (also the trade/execution record)
    Orders {
        /// Filter by fund counter id (repeatable)
        #[arg(long = "counter-id", value_name = "COUNTER_ID")]
        counter_id: Vec<String>,
        /// Filter by action(s), comma-separated (e.g. buy,sell)
        #[arg(long)]
        action: Option<String>,
        /// Filter by state(s), comma-separated
        #[arg(long)]
        state: Option<String>,
        /// Filter by currency
        #[arg(long)]
        currency: Option<String>,
        /// Range start (YYYY-MM-DD or RFC 3339)
        #[arg(long)]
        start: Option<String>,
        /// Range end (YYYY-MM-DD or RFC 3339)
        #[arg(long)]
        end: Option<String>,
        /// Page number
        #[arg(long)]
        page: Option<i32>,
        /// Page size
        #[arg(long)]
        size: Option<i32>,
    },

    /// A single fund order detail
    Order {
        /// Order id
        order_id: i64,
    },

    /// My fund transactions (cash-flow records)
    Transactions {
        /// Business type
        #[arg(long = "business-type")]
        business_type: Option<String>,
        /// Category
        #[arg(long)]
        category: Option<String>,
        /// Currencies, comma-separated
        #[arg(long)]
        currencies: Option<String>,
        /// Range start (YYYY-MM-DD or RFC 3339)
        #[arg(long)]
        start: Option<String>,
        /// Range end (YYYY-MM-DD or RFC 3339)
        #[arg(long)]
        end: Option<String>,
        /// Page number
        #[arg(long)]
        page: Option<i32>,
        /// Page size
        #[arg(long)]
        size: Option<i32>,
    },

    /// Validate a fund order before submitting (no order is placed)
    #[command(name = "validate-order")]
    ValidateOrder {
        /// Fund counter id
        counter_id: String,
        /// Action: buy | sell
        #[arg(long)]
        action: String,
        /// Currency (e.g. USD HKD)
        #[arg(long)]
        currency: String,
        /// Amount (amount-based orders)
        #[arg(long)]
        amount: Option<String>,
        /// Units (unit-based orders)
        #[arg(long)]
        units: Option<String>,
        /// Dividend option
        #[arg(long = "dividend-option")]
        dividend_option: Option<i32>,
        /// Fund source
        #[arg(long = "fund-source")]
        fund_source: Option<i32>,
    },

    /// Submit a fund order (buy / sell) — asks for confirmation unless --yes
    #[command(name = "submit-order")]
    SubmitOrder {
        /// Fund counter id
        counter_id: String,
        /// Action: buy | sell
        #[arg(long)]
        action: String,
        /// Currency (e.g. USD HKD)
        #[arg(long)]
        currency: String,
        /// Amount (amount-based orders)
        #[arg(long)]
        amount: Option<String>,
        /// Units (unit-based orders)
        #[arg(long)]
        units: Option<String>,
        /// Dividend option
        #[arg(long = "dividend-option")]
        dividend_option: Option<i32>,
        /// Fee
        #[arg(long)]
        fee: Option<String>,
        /// Sell the entire holding
        #[arg(long = "sell-all")]
        sell_all: bool,
        /// Remark
        #[arg(long)]
        remark: Option<String>,
        /// Skip the interactive confirmation
        #[arg(long)]
        yes: bool,
    },

    /// Cancel (withdraw) a fund order — asks for confirmation unless --yes
    #[command(name = "cancel-order")]
    CancelOrder {
        /// Order id
        order_id: i64,
        /// Skip the interactive confirmation
        #[arg(long)]
        yes: bool,
    },
}

/// Serialize an SDK response and print it through the shared output layer.
///
/// JSON output pretty-prints the value (with internal fields such as `aaid`
/// stripped); table output renders an array of objects as a table and a single
/// object as a field/value table.
fn output<T: Serialize>(value: &T, format: &OutputFormat) -> Result<()> {
    output_cols(value, format, &[])
}

/// Like [`output`], but for an array result the table view is restricted (and
/// ordered) to `cols` when they are present — keeping wide list responses (e.g.
/// `list` / `transactions`) to a readable set of columns. `--format json` still
/// returns every field.
fn output_cols<T: Serialize>(value: &T, format: &OutputFormat, cols: &[&str]) -> Result<()> {
    let mut json = serde_json::to_value(value)?;
    strip_private_fields(&mut json);
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
        OutputFormat::Pretty => match &json {
            serde_json::Value::Array(arr) => print_array_table(arr, cols),
            serde_json::Value::Object(map) => {
                // Field/value table; nested collections are summarised by `cell`
                // so a heavy field doesn't blow up the row.
                let rows: Vec<Vec<String>> =
                    map.iter().map(|(k, v)| vec![k.clone(), cell(v)]).collect();
                print_table(&["Field", "Value"], rows, format);
            }
            other => println!("{other}"),
        },
    }
    Ok(())
}

/// Render a JSON array of objects as a table. Falls back to raw JSON for shapes
/// that are not a uniform list of objects.
///
/// When `cols` is non-empty the table is restricted to those columns, in that
/// order (any not present in the data are skipped); otherwise every key is shown.
fn print_array_table(arr: &[serde_json::Value], cols: &[&str]) {
    if arr.is_empty() {
        println!("(no records)");
        return;
    }
    let Some(first) = arr.first().and_then(serde_json::Value::as_object) else {
        println!("{}", serde_json::to_string_pretty(arr).unwrap_or_default());
        return;
    };
    // A single-row result (e.g. one fund's ~30 performance metrics) is far more
    // readable transposed as a field/value table than as one very wide row.
    if arr.len() == 1 {
        let rows: Vec<Vec<String>> = first
            .iter()
            .map(|(k, v)| vec![k.clone(), cell(v)])
            .collect();
        print_table(&["Field", "Value"], rows, &OutputFormat::Pretty);
        return;
    }
    let headers: Vec<String> = if cols.is_empty() {
        first.keys().cloned().collect()
    } else {
        cols.iter()
            .filter(|c| first.contains_key(**c))
            .map(|c| (*c).to_string())
            .collect()
    };
    let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();
    let rows: Vec<Vec<String>> = arr
        .iter()
        .map(|item| {
            headers
                .iter()
                .map(|key| item.get(key).map_or_else(String::new, cell))
                .collect()
        })
        .collect();
    print_table(&header_refs, rows, &OutputFormat::Pretty);
}

/// Longest string a table cell renders before it is truncated with `…`.
const MAX_CELL_CHARS: usize = 80;

/// Collapse whitespace/newlines to single spaces and truncate to
/// [`MAX_CELL_CHARS`] so a long prose field (e.g. a fund's `introduce` /
/// `profile`) doesn't blow up the column width. Counts by `char`, not bytes.
fn truncate_cell(s: &str) -> String {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAX_CELL_CHARS {
        collapsed
    } else {
        let head: String = collapsed.chars().take(MAX_CELL_CHARS - 1).collect();
        format!("{head}…")
    }
}

/// Flatten a JSON value into a single table cell.
///
/// Nested arrays and objects are summarised (`[N items]` / `{N fields}`) and
/// long strings are truncated, so a heavy field (an embedded NAV series or a
/// paragraph-long description) doesn't blow up the table width. Use
/// `--format json` for the full payload.
fn cell(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => truncate_cell(s),
        serde_json::Value::Null => "-".to_string(),
        serde_json::Value::Number(_) | serde_json::Value::Bool(_) => v.to_string(),
        serde_json::Value::Array(a) => {
            if a.is_empty() {
                "-".to_string()
            } else {
                format!("[{} items]", a.len())
            }
        }
        serde_json::Value::Object(o) => {
            if o.is_empty() {
                "-".to_string()
            } else {
                format!("{{{} fields}}", o.len())
            }
        }
    }
}

/// Ask the operator to confirm a write operation on the terminal.
///
/// Returns `Ok(true)` only for an explicit yes; defaults to no on empty input
/// or a non-interactive stdin.
fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt} [y/N] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line)? == 0 {
        return Ok(false);
    }
    Ok(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// Convert a date/datetime string to a unix-second timestamp (start of day).
fn start_ts(s: &str) -> Result<i64> {
    Ok(parse_datetime_start_timestamp(s)?.parse()?)
}

/// Convert a date/datetime string to a unix-second timestamp (end of day).
fn end_ts(s: &str) -> Result<i64> {
    Ok(parse_datetime_end_timestamp(s)?.parse()?)
}

#[allow(clippy::too_many_lines)]
pub async fn run(cmd: FundCommands, format: &OutputFormat) -> Result<()> {
    let ctx = crate::openapi::fund();
    match cmd {
        FundCommands::Hot => output(&ctx.hot_funds().await?, format),
        FundCommands::List {
            quick_id,
            time_interval,
        } => {
            let mut opts = GetFundsOptions::new();
            if !quick_id.is_empty() {
                opts = opts.quick_ids(quick_id);
            }
            if !time_interval.is_empty() {
                opts = opts.time_interval(time_interval);
            }
            output_cols(
                &ctx.funds(opts).await?,
                format,
                &[
                    "counter_id",
                    "name",
                    "currency",
                    "asset_class_name",
                    "risk_level_name",
                    "unit_value",
                    "purchase_amount",
                ],
            )
        }
        FundCommands::Filters => output(&ctx.filters().await?, format),
        FundCommands::Detail { counter_id } => output(&ctx.detail(counter_id).await?, format),
        FundCommands::Analysis { counter_id, period } => {
            let opts = period.map(|p| GetFundAnalysisOptions::new().period(p));
            output(&ctx.analysis(counter_id, opts).await?, format)
        }
        FundCommands::AnalysisDetail { counter_id, period } => {
            let opts = period.map(|p| GetFundAnalysisOptions::new().period(p));
            output(&ctx.analysis_detail(counter_id, opts).await?, format)
        }
        FundCommands::Trend { counter_id, period } => {
            let opts = period.map(|p| GetFundAnalysisOptions::new().period(p));
            output(&ctx.trend(counter_id, opts).await?, format)
        }
        FundCommands::AnnualReturns {
            counter_id,
            page,
            size,
        } => {
            let opts = page_options(page, size);
            output(&ctx.annual_returns(counter_id, opts).await?, format)
        }
        FundCommands::QuarterlyReturns {
            counter_id,
            page,
            size,
        } => {
            let opts = page_options(page, size);
            output(&ctx.quarterly_returns(counter_id, opts).await?, format)
        }
        FundCommands::Performance { counter_id } => {
            output(&ctx.performance(counter_id).await?, format)
        }
        FundCommands::PerformanceComparison { counter_id, period } => {
            let opts = period.map(|p| GetFundAnalysisOptions::new().period(p));
            output(&ctx.performance_comparison(counter_id, opts).await?, format)
        }
        FundCommands::Nav { counter_id } => output(&ctx.nav(counter_id).await?, format),
        FundCommands::NavHistory {
            counter_id,
            page,
            size,
        } => {
            let opts = page_options(page, size);
            output(&ctx.nav_history(counter_id, opts).await?, format)
        }
        FundCommands::NavRange {
            counter_id,
            month_before,
            year_before,
        } => {
            let opts = nav_range_options(month_before, year_before);
            output(&ctx.nav_range(counter_id, opts).await?, format)
        }
        FundCommands::Holdings { counter_id, scene } => {
            let opts = scene.map(|s| GetFundHoldingsOptions::new().scene(s));
            output(&ctx.holdings(counter_id, opts).await?, format)
        }
        FundCommands::StockHoldings { counter_id, limit } => {
            let opts = limit.map(|l| GetFundStockHoldingsOptions::new().limit(l));
            output(&ctx.stock_holdings(counter_id, opts).await?, format)
        }

        // ── My positions ─────────────────────────────────────────────────
        FundCommands::Positions {
            account_channel,
            aaid,
        } => {
            let mut opts = GetFundPositionsOptions::new();
            if let Some(c) = account_channel {
                opts = opts.account_channel(c);
            }
            if let Some(a) = aaid {
                opts = opts.aaid(a);
            }
            output(&ctx.positions(opts).await?, format)
        }
        FundCommands::Position {
            counter_id,
            account_channel,
            aaid,
            start,
            end,
        } => {
            let mut opts = GetFundPositionOptions::new();
            if let Some(c) = account_channel {
                opts = opts.account_channel(c);
            }
            if let Some(a) = aaid {
                opts = opts.aaid(a);
            }
            if let Some(s) = start {
                opts = opts.start(s);
            }
            if let Some(e) = end {
                opts = opts.end(e);
            }
            output(&ctx.position(counter_id, opts).await?, format)
        }
        FundCommands::PositionPerformance { counter_id } => {
            output(&ctx.position_performance(counter_id).await?, format)
        }
        FundCommands::PositionProfits {
            counter_id,
            start,
            end,
            page,
            size,
        } => {
            let mut opts = GetFundPositionProfitsOptions::new();
            if let Some(s) = start {
                opts = opts.start(s);
            }
            if let Some(e) = end {
                opts = opts.end(e);
            }
            if let Some(p) = page {
                opts = opts.page(p);
            }
            if let Some(s) = size {
                opts = opts.size(s);
            }
            output(&ctx.position_profits(counter_id, opts).await?, format)
        }
        FundCommands::PositionNav {
            counter_id,
            month_before,
            year_before,
        } => {
            let opts = nav_range_options(month_before, year_before);
            output(&ctx.position_nav(counter_id, opts).await?, format)
        }
        FundCommands::PositionDividends {
            counter_id,
            currency,
            start,
            end,
            page,
            size,
        } => {
            let mut opts = GetFundPositionDividendsOptions::new();
            if let Some(c) = currency {
                opts = opts.currency(c);
            }
            if let Some(s) = start {
                opts = opts.start(start_ts(&s)?);
            }
            if let Some(e) = end {
                opts = opts.end(end_ts(&e)?);
            }
            if let Some(p) = page {
                opts = opts.page(p);
            }
            if let Some(s) = size {
                opts = opts.size(s);
            }
            output(&ctx.position_dividends(counter_id, opts).await?, format)
        }

        // ── Orders / trading ─────────────────────────────────────────────
        FundCommands::Orders {
            counter_id,
            action,
            state,
            currency,
            start,
            end,
            page,
            size,
        } => {
            let mut opts = GetFundOrdersOptions::new();
            if !counter_id.is_empty() {
                opts = opts.counter_ids(counter_id);
            }
            if let Some(a) = action {
                opts = opts.actions(a);
            }
            if let Some(s) = state {
                opts = opts.states(s);
            }
            if let Some(c) = currency {
                opts = opts.currency(c);
            }
            if let Some(s) = start {
                opts = opts.start(start_ts(&s)?);
            }
            if let Some(e) = end {
                opts = opts.end(end_ts(&e)?);
            }
            if let Some(p) = page {
                opts = opts.page(p);
            }
            if let Some(s) = size {
                opts = opts.size(s);
            }
            output(&ctx.orders(opts).await?, format)
        }
        FundCommands::Order { order_id } => output(&ctx.order(order_id).await?, format),
        FundCommands::Transactions {
            business_type,
            category,
            currencies,
            start,
            end,
            page,
            size,
        } => {
            let mut opts = GetFundTransactionsOptions::new();
            if let Some(b) = business_type {
                opts = opts.business_type(b);
            }
            if let Some(c) = category {
                opts = opts.category(c);
            }
            if let Some(c) = currencies {
                opts = opts.currencies(c);
            }
            if let Some(s) = start {
                opts = opts.start(start_ts(&s)?);
            }
            if let Some(e) = end {
                opts = opts.end(end_ts(&e)?);
            }
            if let Some(p) = page {
                opts = opts.page(p);
            }
            if let Some(s) = size {
                opts = opts.size(s);
            }
            output_cols(
                &ctx.transactions(opts).await?,
                format,
                &[
                    "done_at",
                    "tx_type",
                    "type_name",
                    "amount",
                    "currency",
                    "description",
                ],
            )
        }
        FundCommands::ValidateOrder {
            counter_id,
            action,
            currency,
            amount,
            units,
            dividend_option,
            fund_source,
        } => {
            let mut opts = ValidateFundOrderOptions::new(counter_id, action, currency);
            if let Some(a) = amount {
                opts = opts.amount(a);
            }
            if let Some(u) = units {
                opts = opts.units(u);
            }
            if let Some(d) = dividend_option {
                opts = opts.dividend_option(d);
            }
            if let Some(f) = fund_source {
                opts = opts.fund_source(f);
            }
            output(&ctx.validate_order(opts).await?, format)
        }
        FundCommands::SubmitOrder {
            counter_id,
            action,
            currency,
            amount,
            units,
            dividend_option,
            fee,
            sell_all,
            remark,
            yes,
        } => {
            let qty = amount
                .as_deref()
                .map(|a| format!("amount {a} {currency}"))
                .or_else(|| units.as_deref().map(|u| format!("{u} units")))
                .unwrap_or_else(|| "(no amount/units)".to_string());
            if !yes && !confirm(&format!("Submit fund order: {action} {counter_id} {qty}?"))? {
                println!("Aborted.");
                return Ok(());
            }
            let mut opts = SubmitFundOrderOptions::new(counter_id, action, currency);
            if let Some(a) = amount {
                opts = opts.amount(a);
            }
            if let Some(u) = units {
                opts = opts.units(u);
            }
            if let Some(d) = dividend_option {
                opts = opts.dividend_option(d);
            }
            if let Some(f) = fee {
                opts = opts.fee(f);
            }
            if sell_all {
                opts = opts.is_sell_all(true);
            }
            if let Some(r) = remark {
                opts = opts.remark(r);
            }
            let resp = ctx.submit_order(opts).await?;
            let mut json = serde_json::to_value(&resp)?;
            strip_private_fields(&mut json);
            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&json)?);
                }
                OutputFormat::Pretty => {
                    println!("Order submitted successfully.");
                    println!("Order ID: {}", resp.id);
                    if let serde_json::Value::Object(map) = &json {
                        let rows: Vec<Vec<String>> =
                            map.iter().map(|(k, v)| vec![k.clone(), cell(v)]).collect();
                        print_table(&["Field", "Value"], rows, format);
                    }
                }
            }
            Ok(())
        }
        FundCommands::CancelOrder { order_id, yes } => {
            if !yes && !confirm(&format!("Cancel fund order {order_id}?"))? {
                println!("Aborted.");
                return Ok(());
            }
            ctx.cancel_order(order_id).await?;
            // Re-query the order to echo the resulting state.
            let detail = ctx.order(order_id).await?;
            match format {
                OutputFormat::Json => {
                    let mut json = serde_json::to_value(&detail)?;
                    strip_private_fields(&mut json);
                    let state = detail.order.as_ref().map(|o| o.state.clone());
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "order_id": order_id,
                            "cancelled": true,
                            "state": state,
                            "detail": json,
                        }))?
                    );
                }
                OutputFormat::Pretty => {
                    println!("Order {order_id} cancel requested.");
                    match &detail.order {
                        Some(o) => println!("State: {} ({})", o.state, o.state_desc),
                        None => println!("State: (order detail unavailable)"),
                    }
                }
            }
            Ok(())
        }
    }
}

fn page_options(page: Option<i32>, size: Option<i32>) -> Option<FundPageOptions> {
    if page.is_none() && size.is_none() {
        return None;
    }
    let mut opts = FundPageOptions::new();
    if let Some(p) = page {
        opts = opts.page(p);
    }
    if let Some(s) = size {
        opts = opts.size(s);
    }
    Some(opts)
}

/// Response-schema provider for every `fund <sub>` leaf command.
///
/// Mirrors the `--format json` shape each subcommand emits: list-returning
/// commands describe an `Array` root, single-record commands an `Object`. Field
/// lists are grounded in the real SDK response structs (`longbridge::fund`) and
/// captured live output. Follows the same pattern as `grid::schema_for_path`.
pub(crate) fn schema_for_path(path: &[String]) -> Option<super::schema::ResponseSchema> {
    use super::schema::{array, object};

    let sub = path.get(1)?.as_str();
    let schema = match sub {
        // ── Catalog / market data (arrays) ────────────────────────────────
        "hot" => array(
            "Hot-selling fund list",
            &[
                "asset_class",
                "asset_class_name",
                "counter_id",
                "currency",
                "earning_rate",
                "fund_performances",
                "name",
                "purchase_amount",
                "recommendation_text",
                "risk_level",
                "risk_level_name",
                "time_interval",
            ],
        ),
        "list" => array(
            "Fund list (filtered)",
            &[
                "asset_class",
                "asset_class_name",
                "code",
                "counter_id",
                "currency",
                "description",
                "earning_rate",
                "holding",
                "isin",
                "name",
                "product",
                "purchase_amount",
                "recommendation_text",
                "risk_level",
                "risk_level_name",
                "time_interval",
                "unit_value",
            ],
        ),
        "annual-returns" => array("Fund annual returns", &["change_percent", "year"]),
        "quarterly-returns" => array(
            "Fund quarterly returns",
            &["change_percent", "quarter", "year"],
        ),
        "performance" => array(
            "Fund performance figures (ranks, returns, totals across periods)",
            &[
                "annualized_return_five",
                "annualized_return_one",
                "annualized_return_ten",
                "annualized_return_three",
                "annualized_return_two",
                "counter_id",
                "fund_name",
                "performance_rank_five_years",
                "performance_rank_one_day",
                "performance_rank_one_month",
                "performance_rank_one_week",
                "performance_rank_one_year",
                "performance_rank_six_months",
                "performance_rank_ten_years",
                "performance_rank_three_months",
                "performance_rank_three_years",
                "performance_rank_two_years",
                "performance_rank_ytd",
                "performance_return_five_years",
                "performance_return_one_day",
                "performance_return_one_month",
                "performance_return_one_week",
                "performance_return_one_year",
                "performance_return_six_months",
                "performance_return_ten_years",
                "performance_return_three_months",
                "performance_return_three_years",
                "performance_return_two_years",
                "performance_return_ytd",
                "performance_total_five_years",
                "performance_total_one_day",
                "performance_total_one_month",
                "performance_total_one_week",
                "performance_total_one_year",
                "performance_total_six_months",
                "performance_total_ten_years",
                "performance_total_three_months",
                "performance_total_three_years",
                "performance_total_two_years",
                "performance_total_ytd",
                "seven_days_annualized",
                "ten_thousand_price",
                "update_time",
            ],
        ),
        "nav" | "nav-history" | "nav-range" => array(
            "Fund net-value (NAV) points",
            &[
                "change",
                "change_percent",
                "change_percent_format",
                "counter_id",
                "counter_name",
                "currency",
                "date_format",
                "isin",
                "last_update_time",
                "value",
                "value_format",
            ],
        ),
        "stock-holdings" => array(
            "Stocks held by a fund (reverse lookup)",
            &[
                "code",
                "counter_id",
                "currency",
                "name",
                "position_ratio",
                "report_date",
            ],
        ),
        "orders" => array(
            "My fund orders (trade / execution record)",
            &[
                "action",
                "amount",
                "counter_id",
                "created_at",
                "currency",
                "fund_name",
                "id",
                "is_auto",
                "net_worth",
                "product_type",
                "state",
                "state_desc",
                "units",
            ],
        ),
        "transactions" => array(
            "My fund transactions (cash-flow records)",
            &[
                "amount",
                "category",
                "created_at",
                "currency",
                "description",
                "detail_created_at",
                "detail_type",
                "done_at",
                "quantity_description",
                "redirect_page",
                "redirect_page_v2",
                "ref_no",
                "stock_quantity",
                "tx_type",
                "type_name",
            ],
        ),

        // ── My positions (arrays) ─────────────────────────────────────────
        "position-performance" => array(
            "Performance figures of a held fund",
            &[
                "annualized_return_five",
                "annualized_return_one",
                "annualized_return_ten",
                "annualized_return_three",
                "annualized_return_two",
                "counter_id",
                "fund_name",
                "performance_return_five_years",
                "performance_return_one_day",
                "performance_return_one_month",
                "performance_return_one_week",
                "performance_return_one_year",
                "performance_return_six_months",
                "performance_return_ten_years",
                "performance_return_three_months",
                "performance_return_three_years",
                "performance_return_two_years",
                "performance_return_ytd",
                "update_time",
            ],
        ),
        "position-nav" => array(
            "Net-value history of a held fund",
            &[
                "change",
                "change_percent",
                "counter_id",
                "counter_name",
                "last_update_time",
                "value",
            ],
        ),

        // ── Catalog / analysis (objects) ──────────────────────────────────
        "filters" => object(
            "Fund list filter options",
            &[
                "asset_class",
                "company",
                "currency",
                "industry_category_name",
                "risk_level",
            ],
        ),
        "detail" => object(
            "Fund detail",
            &[
                "additional_purchase_amount",
                "affirm_day",
                "amount_affirm_day",
                "asset_allocation",
                "asset_class",
                "asset_class_name",
                "bill_purchase_rate",
                "channel",
                "close_period",
                "code",
                "currency",
                "cut_off_time",
                "derivatives",
                "done_day",
                "excess_return_fee",
                "gst_rate",
                "introduce",
                "is_cash_plus",
                "is_complex",
                "is_new_cash_plus",
                "is_yinghebao",
                "isin",
                "manage_rate",
                "manager",
                "min_hold_cash",
                "min_hold_share",
                "min_sell_share",
                "month_raise_day",
                "name",
                "nav_deadline",
                "no_load",
                "open_date",
                "open_period",
                "product",
                "product_information_locals",
                "profile",
                "purchasable",
                "purchase_affirm_day",
                "purchase_amount",
                "purchase_rate",
                "rating",
                "redeemable",
                "redemption_advance_day",
                "redemption_amount",
                "redemption_close_period_shows",
                "redemption_done_day",
                "redemption_open_day_shows",
                "risk_level",
                "risk_level_name",
                "verify_status",
                "virtual_currency",
                "year_to_date_yield",
                "ytd_yield_type",
            ],
        ),
        "analysis" => object(
            "Fund analysis (level 1)",
            &[
                "actual_period",
                "cost_level",
                "return_ability",
                "risk_ability",
                "updated_at",
                "value_for_money",
                "visible",
            ],
        ),
        "analysis-detail" => object(
            "Fund analysis detail (level 2)",
            &[
                "actual_period",
                "available_periods",
                "cost_level",
                "return_ability",
                "risk_ability",
                "updated_at",
                "value_for_money",
                "visible",
            ],
        ),
        "trend" => object(
            "Fund trend chart",
            &[
                "actual_period",
                "available_periods",
                "category_average_performances",
                "contrast_performances",
                "fund_performances",
            ],
        ),
        "performance-comparison" => object(
            "Fund performance comparison",
            &["contrast_performances", "fund_performances"],
        ),
        "holdings" => object(
            "Fund top-10 holdings",
            &["holdings", "report_date", "weighting"],
        ),

        // ── My positions (objects) ────────────────────────────────────────
        "positions" => object(
            "My fund positions overview",
            &[
                "sold_pending_credit_orders",
                "list",
                "pending_buy_orders",
                "recent_trading_day",
            ],
        ),
        "position" => object(
            "My single fund position detail",
            &["detail_values", "sum_profit", "ut_value"],
        ),
        "position-profits" => object(
            "Cumulative-profit series of a held fund",
            &[
                "currency",
                "history_value",
                "last_update_time",
                "sum_profit",
            ],
        ),
        "position-dividends" => object(
            "Dividend records of a held fund",
            &[
                "currency",
                "div_cash_infos",
                "lastest_date",
                "total_div_cash",
            ],
        ),

        // ── Orders / trading (objects) ────────────────────────────────────
        "order" => object(
            "A single fund order detail",
            &["keywords", "order", "stages"],
        ),
        "validate-order" => object(
            "Fund order validation result (no order placed)",
            &[
                "auth_token",
                "eval_address",
                "fund_risk_level",
                "msg",
                "user_pi",
                "user_risk_level",
            ],
        ),
        "submit-order" => object(
            "Fund order submission result",
            &[
                "action",
                "amount",
                "counter_id",
                "created_at",
                "fund_name",
                "id",
                "msg",
                "status",
                "units",
            ],
        ),
        "cancel-order" => object(
            "Fund order cancellation result (re-queried order state)",
            &["order_id", "cancelled", "state", "detail"],
        ),

        _ => return None,
    };
    Some(schema)
}

fn nav_range_options(
    month_before: Option<i32>,
    year_before: Option<i32>,
) -> Option<FundNavRangeOptions> {
    if month_before.is_none() && year_before.is_none() {
        return None;
    }
    let mut opts = FundNavRangeOptions::new();
    if let Some(m) = month_before {
        opts = opts.month_before(m);
    }
    if let Some(y) = year_before {
        opts = opts.year_before(y);
    }
    Some(opts)
}
