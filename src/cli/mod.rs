use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

pub mod api;
pub mod output;
pub mod quote;
pub mod trade;
pub mod watchlist;

#[derive(ValueEnum, Clone, Default, Debug)]
pub enum OutputFormat {
    #[default]
    Table,
    Json,
}

#[derive(Parser)]
#[command(name = "longbridge")]
#[command(about = "Longbridge CLI - AI-native CLI for Longbridge OpenAPI")]
#[command(long_about = "\
An AI-native CLI that wraps every Longbridge OpenAPI endpoint — real-time quotes, \
order management, watchlists, options, warrants, and more.\n\n\
Symbol format: <CODE>.<MARKET>  e.g. TSLA.US  700.HK  600519.SH\n\
Markets: HK (Hong Kong)  US (United States)  CN (China A-share)  SG (Singapore)\n\n\
Authentication: run `longbridge login` once; the token is stored at \
~/.longbridge/terminal/.openapi-session and reused automatically by all commands.\n\n\
Use --format json on any command for machine-readable output suitable for AI agents:\n\
  longbridge quote TSLA.US --format json\n\
  longbridge positions --format json | jq '.[] | {symbol, quantity}'\n\n\
Use `longbridge tui` to launch the interactive full-screen terminal UI.")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Output format: 'table' for human-readable, 'json' for AI agents and scripting
    #[arg(long, global = true, default_value = "table")]
    pub format: OutputFormat,

    /// Clear stored OAuth token and exit (same as `longbridge logout`)
    #[arg(long)]
    pub logout: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Authenticate via browser OAuth and save token for CLI and TUI
    ///
    /// Opens a browser for Longbridge OpenAPI authorization.
    /// Token is stored at ~/.longbridge/terminal/.openapi-session and shared with the TUI.
    Login,

    /// Clear the locally stored OAuth token
    ///
    /// Next command or TUI launch will trigger re-authentication.
    Logout,

    /// Launch the interactive full-screen TUI (terminal UI)
    ///
    /// Real-time watchlist, candlestick charts, portfolio view, stock search, Vim-like keybindings.
    /// Example: longbridge tui
    Tui,

    // ── Quote ──────────────────────────────────────────────────────────────────
    /// Real-time quotes for one or more symbols
    ///
    /// Returns: symbol, last_done, prev_close, open, high, low, volume, turnover, trade_status.
    /// Example: longbridge quote TSLA.US 700.HK AAPL.US
    Quote {
        /// Symbols in <CODE>.<MARKET> format, e.g. TSLA.US 700.HK 600519.SH
        symbols: Vec<String>,
    },

    /// Level 2 order book depth (bid/ask ladder)
    ///
    /// Returns up to 10 price levels of asks and bids with price, volume, order_num.
    /// Example: longbridge depth TSLA.US
    Depth {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
    },

    /// Broker queue at each price level (HK market)
    ///
    /// Returns which broker IDs are present at each ask/bid level.
    /// Useful for understanding institutional order flow.
    /// Example: longbridge brokers 700.HK
    Brokers {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
    },

    /// Recent tick-by-tick trades
    ///
    /// Returns: timestamp, price, volume, direction (up/down/neutral), trade_type.
    /// Example: longbridge trades TSLA.US --count 50
    Trades {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Number of trades to return (default: 20, max: 1000)
        #[arg(long, default_value = "20")]
        count: usize,
    },

    /// Intraday minute-by-minute price and volume lines for today
    ///
    /// Returns: timestamp, price, volume, turnover, avg_price.
    /// Example: longbridge intraday TSLA.US
    Intraday {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
    },

    /// OHLCV candlestick (K-line) data
    ///
    /// Returns: timestamp, open, high, low, close, volume, turnover.
    /// Periods: 1m  5m  15m  30m  1h  day  week  month  year
    ///   (aliases: minute=1m, hour=1h, d/1d=day, w=week, m/1mo=month, y=year)
    /// Example: longbridge kline TSLA.US --period day --count 100
    /// Example: longbridge kline TSLA.US --period 1h --adjust forward_adjust
    Kline {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Candlestick period: 1m 5m 15m 30m 1h day week month year (default: day)
        /// Aliases: minute=1m, hour=1h, d/1d=day, w=week, m/1mo=month, y=year
        #[arg(long, default_value = "day")]
        period: String,
        /// Number of candles to return (default: 100)
        #[arg(long, default_value = "100")]
        count: usize,
        /// Price adjustment: no_adjust (default) | forward_adjust
        /// Aliases: none=no_adjust, forward=forward_adjust
        #[arg(long, default_value = "no_adjust")]
        adjust: String,
    },

    /// Historical OHLCV candlestick data within a date range
    ///
    /// Both --start and --end must be provided together; if either is omitted the
    /// most recent 100 candles are returned (offset-based, ignores the other flag).
    /// Example: longbridge kline-history TSLA.US --start 2024-01-01 --end 2024-12-31
    KlineHistory {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Candlestick period: 1m 5m 15m 30m 1h day week month year (default: day)
        /// Aliases: minute=1m, hour=1h, d/1d=day, w=week, m/1mo=month, y=year
        #[arg(long, default_value = "day")]
        period: String,
        /// Start date (YYYY-MM-DD). Must be used together with --end.
        #[arg(long)]
        start: Option<String>,
        /// End date (YYYY-MM-DD). Must be used together with --start.
        #[arg(long)]
        end: Option<String>,
        /// Price adjustment: no_adjust (default) | forward_adjust
        /// Aliases: none=no_adjust, forward=forward_adjust
        #[arg(long, default_value = "no_adjust")]
        adjust: String,
    },

    /// Static reference info for one or more symbols
    ///
    /// Returns: name, exchange, currency, lot_size, total_shares, circulating_shares, EPS, BPS, dividend_yield.
    /// Example: longbridge static TSLA.US 700.HK
    Static {
        /// One or more symbols in <CODE>.<MARKET> format
        symbols: Vec<String>,
    },

    /// Calculated financial indexes (PE, PB, EPS, turnover rate, etc.)
    ///
    /// Full index list:
    ///   last_done  change_value  change_rate  volume  turnover  ytd_change_rate
    ///   turnover_rate  total_market_value  capital_flow  amplitude  volume_ratio
    ///   pe (alias: pe_ttm)  pb  eps (alias: dividend_yield)
    ///   five_day_change_rate  ten_day_change_rate  half_year_change_rate  five_minutes_change_rate
    ///   implied_volatility  delta  gamma  theta  vega  rho  open_interest
    ///   expiry_date  strike_price  upper_strike_price  lower_strike_price
    ///   outstanding_qty  outstanding_ratio  premium  itm_otm
    ///   warrant_delta  call_price  to_call_price  effective_leverage
    ///   leverage_ratio  conversion_ratio  balance_point
    /// Example: longbridge calc-index TSLA.US AAPL.US --index pe,pb,turnover_rate
    CalcIndex {
        /// One or more symbols in <CODE>.<MARKET> format
        symbols: Vec<String>,
        /// Comma-separated indexes to compute (default: pe,pb,eps,turnover_rate,total_market_value)
        /// Unknown index names are silently ignored.
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "pe,pb,eps,turnover_rate,total_market_value"
        )]
        index: Vec<String>,
    },

    /// Intraday capital flow time series (large/medium/small money in vs out)
    ///
    /// Returns a time series of inflow values for today's session.
    /// Example: longbridge capital-flow TSLA.US
    CapitalFlow {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
    },

    /// Capital distribution snapshot (large/medium/small inflow and outflow)
    ///
    /// Returns total inflow/outflow broken down by order size for the current session.
    /// Example: longbridge capital-dist TSLA.US
    CapitalDist {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
    },

    /// Market sentiment temperature index (0–100, higher = more bullish)
    ///
    /// Use --history to get a time series instead of the current snapshot.
    /// Example: longbridge market-temp HK
    /// Example: longbridge market-temp US --history --start 2024-01-01 --end 2024-12-31
    MarketTemp {
        /// Market: HK | US | CN (aliases: SH SZ) | SG  (case-insensitive, default: HK)
        #[arg(default_value = "HK")]
        market: String,
        /// Return historical records instead of current value
        #[arg(long)]
        history: bool,
        /// Start date for history (YYYY-MM-DD). Defaults to today if omitted.
        #[arg(long)]
        start: Option<String>,
        /// End date for history (YYYY-MM-DD). Defaults to today if omitted.
        #[arg(long)]
        end: Option<String>,
        /// NOTE: currently unused — the SDK does not expose a granularity parameter.
        #[arg(long, default_value = "daily", hide = true)]
        granularity: String,
    },

    /// Trading session schedule (open/close times) for all markets
    ///
    /// Returns: market, session type (intraday/pre/post/overnight), begin_time, end_time.
    TradingSession,

    /// Trading days and half-trading days for a market
    ///
    /// Defaults to today + 30 days if no dates are provided.
    /// Example: longbridge trading-days HK --start 2024-01-01 --end 2024-03-31
    TradingDays {
        /// Market: HK | US | CN (aliases: SH SZ) | SG  (case-insensitive, default: HK)
        #[arg(default_value = "HK")]
        market: String,
        /// Start date (YYYY-MM-DD), defaults to today
        #[arg(long)]
        start: Option<String>,
        /// End date (YYYY-MM-DD), defaults to 30 days after start
        #[arg(long)]
        end: Option<String>,
    },

    /// Full list of securities available in a market
    ///
    /// Returns: symbol, name_en, name_cn for every listed security.
    /// Example: longbridge security-list HK
    SecurityList {
        /// Market: HK | US | CN (aliases: SH SZ) | SG  (case-insensitive, default: HK)
        #[arg(default_value = "HK")]
        market: String,
        /// NOTE: currently unused — the SDK only exposes the Overnight category.
        #[arg(long, default_value = "main", hide = true)]
        category: String,
    },

    /// Market maker (participant) broker IDs and names
    ///
    /// Use these IDs to interpret results from the `brokers` command.
    Participants,

    /// Active real-time WebSocket subscriptions for this session
    ///
    /// Returns: symbol, sub_types (quote/depth/trade), subscribed candlestick periods.
    Subscriptions,

    // ── Options & Warrants ──────────────────────────────────────────────────────
    /// Real-time quotes for option contracts
    ///
    /// Returns standard quote fields plus implied_volatility, delta, strike_price, expiry_date, contract_type.
    /// Example: longbridge option-quote AAPL240119C190000
    OptionQuote {
        /// Option contract symbols (OCC format for US, e.g. AAPL240119C190000)
        symbols: Vec<String>,
    },

    /// Option chain: expiry dates, or strike prices for a given expiry
    ///
    /// Without --date: returns all available expiry dates.
    /// With --date: returns strike prices and call/put symbols for that expiry.
    /// Example: longbridge option-chain AAPL.US
    /// Example: longbridge option-chain AAPL.US --date 2024-01-19
    OptionChain {
        /// Underlying symbol in <CODE>.<MARKET> format, e.g. AAPL.US
        symbol: String,
        /// Expiry date (YYYY-MM-DD). Omit to list all expiry dates.
        #[arg(long)]
        date: Option<String>,
    },

    /// Real-time quotes for warrant contracts
    ///
    /// Returns: last_done, prev_close, implied_volatility, leverage_ratio, expiry_date, category.
    /// Example: longbridge warrant-quote 12345.HK
    WarrantQuote {
        /// Warrant symbols (e.g. 12345.HK)
        symbols: Vec<String>,
    },

    /// Warrants linked to an underlying security
    ///
    /// Returns: symbol, name, last_done, leverage_ratio, expiry_date, warrant_type.
    /// Example: longbridge warrant-list 700.HK
    WarrantList {
        /// Underlying symbol (e.g. 700.HK)
        symbol: String,
    },

    /// Warrant issuer list (HK market)
    ///
    /// Returns: issuer_id, name_en, name_cn.
    WarrantIssuers,

    // ── Watchlist ───────────────────────────────────────────────────────────────
    /// List watchlist groups, or create/update/delete a group
    ///
    /// Without a subcommand, lists all groups and their securities.
    /// Subcommands: create  update  delete
    /// Example: longbridge watchlist
    /// Example: longbridge watchlist create "My Portfolio"
    /// Example: longbridge watchlist update 123 --add TSLA.US --add AAPL.US
    Watchlist {
        #[command(subcommand)]
        cmd: Option<WatchlistCmd>,
    },

    // ── Trade ───────────────────────────────────────────────────────────────────
    /// Today's orders, or historical orders with --history
    ///
    /// Returns: order_id, symbol, side, order_type, status, quantity, price,
    ///   executed_qty, executed_price, submitted_at.
    /// Example: longbridge orders
    /// Example: longbridge orders --history --start 2024-01-01 --symbol TSLA.US
    Orders {
        /// Return historical orders instead of today's
        #[arg(long)]
        history: bool,
        /// Filter start date (YYYY-MM-DD)
        #[arg(long)]
        start: Option<String>,
        /// Filter end date (YYYY-MM-DD)
        #[arg(long)]
        end: Option<String>,
        /// Filter by symbol (e.g. TSLA.US)
        #[arg(long)]
        symbol: Option<String>,
    },

    /// Full detail for a single order including charges and history
    ///
    /// Returns all fields from `orders` plus charge_detail, history_details, msg.
    /// Example: longbridge order 20240101-123456789
    Order {
        /// Order ID (from `orders` or returned by `buy`/`sell`)
        order_id: String,
    },

    /// Today's trade executions (fills), or historical with --history
    ///
    /// Returns: order_id, trade_id, symbol, price, quantity, trade_done_at.
    /// Example: longbridge executions --history --start 2024-01-01
    Executions {
        /// Return historical executions instead of today's
        #[arg(long)]
        history: bool,
        /// Filter start date (YYYY-MM-DD)
        #[arg(long)]
        start: Option<String>,
        /// Filter end date (YYYY-MM-DD)
        #[arg(long)]
        end: Option<String>,
        /// Filter by symbol
        #[arg(long)]
        symbol: Option<String>,
    },

    /// Submit a buy order (prompts for confirmation)
    ///
    /// Returns order_id on success.
    /// Order types: LO (limit) | MO (market) | ELO | ALO | ODD | SLO | LIT | MIT
    ///   (case-insensitive)
    /// Example: longbridge buy TSLA.US 100 --price 250.00
    /// Example: longbridge buy 700.HK 1000 --price 300 --order-type ALO
    Buy {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Number of shares/units to buy (integer)
        quantity: u64,
        /// Limit price as a decimal string, e.g. 250.00 (required for LO/ELO/ALO; omit for MO)
        #[arg(long)]
        price: Option<String>,
        /// Order type: LO | MO | ELO | ALO | ODD | SLO | LIT | MIT  (case-insensitive, default: LO)
        #[arg(long, default_value = "LO")]
        order_type: String,
        /// Time in force: Day | GoodTilCanceled (alias: gtc) | GoodTilDate (alias: gtd)
        /// (case-insensitive, default: Day)
        #[arg(long, default_value = "Day")]
        tif: String,
    },

    /// Submit a sell order (prompts for confirmation)
    ///
    /// Returns order_id on success.
    /// Example: longbridge sell TSLA.US 100 --price 260.00
    Sell {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Number of shares/units to sell (integer)
        quantity: u64,
        /// Limit price as a decimal string, e.g. 260.00 (required for LO/ELO/ALO; omit for MO)
        #[arg(long)]
        price: Option<String>,
        /// Order type: LO | MO | ELO | ALO | ODD | SLO | LIT | MIT  (case-insensitive, default: LO)
        #[arg(long, default_value = "LO")]
        order_type: String,
        /// Time in force: Day | GoodTilCanceled (alias: gtc) | GoodTilDate (alias: gtd)
        /// (case-insensitive, default: Day)
        #[arg(long, default_value = "Day")]
        tif: String,
    },

    /// Cancel a pending order (prompts for confirmation)
    ///
    /// Only cancellable states (New, PartialFilled, etc.) are accepted.
    /// Example: longbridge cancel 20240101-123456789
    Cancel {
        /// Order ID to cancel
        order_id: String,
    },

    /// Modify quantity or price of a pending order (prompts for confirmation)
    ///
    /// --qty is required. --price is optional (omit to keep current price).
    /// Example: longbridge replace 20240101-123456789 --qty 200 --price 255.00
    Replace {
        /// Order ID to modify
        order_id: String,
        /// New quantity (REQUIRED — integer number of shares/units)
        #[arg(long)]
        qty: Option<u64>,
        /// New limit price as a decimal string, e.g. 255.00 (optional)
        #[arg(long)]
        price: Option<String>,
    },

    /// Account cash balance and financing information
    ///
    /// Returns: currency, total_cash, max_finance_amount, remaining_finance_amount, risk_level, margin_call.
    /// Example: longbridge balance --currency USD
    Balance {
        /// Filter by currency (e.g. USD HKD CNY SGD); returns all if omitted
        #[arg(long)]
        currency: Option<String>,
    },

    /// Cash flow records (deposits, withdrawals, dividends, settlements)
    ///
    /// Returns: flow_name, symbol, business_type, balance, currency, business_time, description.
    /// Defaults to last 30 days if no dates provided.
    /// Example: longbridge cash-flow --start 2024-01-01 --end 2024-03-31
    CashFlow {
        /// Start date (YYYY-MM-DD), defaults to 30 days ago
        #[arg(long)]
        start: Option<String>,
        /// End date (YYYY-MM-DD), defaults to today
        #[arg(long)]
        end: Option<String>,
    },

    /// Current stock (equity) positions across all sub-accounts
    ///
    /// Returns: symbol, name, quantity, available_quantity, cost_price, currency, market.
    /// Example: longbridge positions --format json
    Positions,

    /// Current fund (mutual fund) positions across all sub-accounts
    ///
    /// Returns: symbol, name, current_net_asset_value, cost_net_asset_value, currency, holding_units.
    FundPositions,

    /// Margin ratio requirements for a symbol
    ///
    /// Returns: im_factor (initial), mm_factor (maintenance), fm_factor (forced liquidation).
    /// Example: longbridge margin-ratio TSLA.US
    MarginRatio {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
    },

    /// Estimate maximum buy or sell quantity given current account balance
    ///
    /// Returns: cash_max_qty (cash only), margin_max_qty (with margin financing).
    /// Example: longbridge max-qty TSLA.US --side buy --price 250
    MaxQty {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Order side: buy | sell  (case-insensitive, REQUIRED)
        #[arg(long)]
        side: String,
        /// Limit price as a decimal string, e.g. 250.00 (required for LO orders)
        #[arg(long)]
        price: Option<String>,
        /// Order type: LO | MO | ELO | ALO  (case-insensitive, default: LO)
        #[arg(long, default_value = "LO")]
        order_type: String,
    },
}

#[derive(Subcommand)]
pub enum WatchlistCmd {
    /// Create a new watchlist group
    ///
    /// Returns the new group ID.
    /// Example: longbridge watchlist create "Tech Stocks"
    Create {
        /// Display name for the new group
        name: String,
    },

    /// Delete a watchlist group (prompts for confirmation)
    ///
    /// Example: longbridge watchlist delete 123
    /// Example: longbridge watchlist delete 123 --purge
    Delete {
        /// Group ID (from `longbridge watchlist`)
        id: i64,
        /// Also remove all securities inside the group
        #[arg(long)]
        purge: bool,
    },

    /// Add/remove securities in a group, or rename it
    ///
    /// Example: longbridge watchlist update 123 --add TSLA.US --add AAPL.US
    /// Example: longbridge watchlist update 123 --remove 700.HK
    /// Example: longbridge watchlist update 123 --name "New Name"
    Update {
        /// Group ID (from `longbridge watchlist`)
        id: i64,
        /// New display name (optional)
        #[arg(long)]
        name: Option<String>,
        /// Symbols to add (repeatable: --add TSLA.US --add AAPL.US)
        #[arg(long)]
        add: Vec<String>,
        /// Symbols to remove (repeatable: --remove 700.HK)
        #[arg(long)]
        remove: Vec<String>,
        /// Update mode: add (default) | remove | replace (overwrite with --add list)
        #[arg(long, default_value = "add")]
        mode: String,
    },
}

pub async fn dispatch(cmd: Commands, format: &OutputFormat) -> Result<()> {
    match cmd {
        Commands::Quote { symbols } => quote::cmd_quote(symbols, format).await,
        Commands::Depth { symbol } => quote::cmd_depth(symbol, format).await,
        Commands::Brokers { symbol } => quote::cmd_brokers(symbol, format).await,
        Commands::Trades { symbol, count } => quote::cmd_trades(symbol, count, format).await,
        Commands::Intraday { symbol } => quote::cmd_intraday(symbol, format).await,
        Commands::Kline {
            symbol,
            period,
            count,
            adjust,
        } => quote::cmd_kline(symbol, &period, count, &adjust, format).await,
        Commands::KlineHistory {
            symbol,
            period,
            start,
            end,
            adjust,
        } => quote::cmd_kline_history(symbol, &period, start, end, &adjust, format).await,
        Commands::Static { symbols } => quote::cmd_static(symbols, format).await,
        Commands::CalcIndex { symbols, index } => {
            quote::cmd_calc_index(symbols, index, format).await
        }
        Commands::CapitalFlow { symbol } => quote::cmd_capital_flow(symbol, format).await,
        Commands::CapitalDist { symbol } => quote::cmd_capital_dist(symbol, format).await,
        Commands::MarketTemp {
            market,
            history,
            start,
            end,
            granularity,
        } => quote::cmd_market_temp(&market, history, start, end, &granularity, format).await,
        Commands::TradingSession => quote::cmd_trading_session(format).await,
        Commands::TradingDays { market, start, end } => {
            quote::cmd_trading_days(&market, start, end, format).await
        }
        Commands::SecurityList { market, category } => {
            quote::cmd_security_list(&market, &category, format).await
        }
        Commands::Participants => quote::cmd_participants(format).await,
        Commands::Subscriptions => quote::cmd_subscriptions(format).await,
        Commands::OptionQuote { symbols } => quote::cmd_option_quote(symbols, format).await,
        Commands::OptionChain { symbol, date } => {
            quote::cmd_option_chain(symbol, date, format).await
        }
        Commands::WarrantQuote { symbols } => quote::cmd_warrant_quote(symbols, format).await,
        Commands::WarrantList { symbol } => quote::cmd_warrant_list(symbol, format).await,
        Commands::WarrantIssuers => quote::cmd_warrant_issuers(format).await,
        Commands::Watchlist { cmd } => watchlist::cmd_watchlist(cmd, format).await,
        Commands::Orders {
            history,
            start,
            end,
            symbol,
        } => trade::cmd_orders(history, start, end, symbol, format).await,
        Commands::Order { order_id } => trade::cmd_order_detail(order_id, format).await,
        Commands::Executions {
            history,
            start,
            end,
            symbol,
        } => trade::cmd_executions(history, start, end, symbol, format).await,
        Commands::Buy {
            symbol,
            quantity,
            price,
            order_type,
            tif,
        } => {
            trade::cmd_submit_order(
                symbol,
                quantity,
                price,
                order_type,
                tif,
                longbridge::trade::OrderSide::Buy,
                format,
            )
            .await
        }
        Commands::Sell {
            symbol,
            quantity,
            price,
            order_type,
            tif,
        } => {
            trade::cmd_submit_order(
                symbol,
                quantity,
                price,
                order_type,
                tif,
                longbridge::trade::OrderSide::Sell,
                format,
            )
            .await
        }
        Commands::Cancel { order_id } => trade::cmd_cancel_order(order_id).await,
        Commands::Replace {
            order_id,
            qty,
            price,
        } => trade::cmd_replace_order(order_id, qty, price).await,
        Commands::Balance { currency } => trade::cmd_balance(currency, format).await,
        Commands::CashFlow { start, end } => trade::cmd_cash_flow(start, end, format).await,
        Commands::Positions => trade::cmd_positions(format).await,
        Commands::FundPositions => trade::cmd_fund_positions(format).await,
        Commands::MarginRatio { symbol } => trade::cmd_margin_ratio(symbol, format).await,
        Commands::MaxQty {
            symbol,
            side,
            price,
            order_type,
        } => trade::cmd_max_qty(symbol, &side, price, &order_type, format).await,
        Commands::Login | Commands::Logout | Commands::Tui => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(args)
    }

    // ─── Format flag ──────────────────────────────────────────────────────────

    #[test]
    fn test_format_default_is_table() {
        let cli = parse(&["longbridge", "quote", "TSLA.US"]).unwrap();
        assert!(matches!(cli.format, OutputFormat::Table));
    }

    #[test]
    fn test_format_json_flag() {
        let cli = parse(&["longbridge", "quote", "TSLA.US", "--format", "json"]).unwrap();
        assert!(matches!(cli.format, OutputFormat::Json));
    }

    // ─── Auth ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_login_subcommand() {
        let cli = parse(&["longbridge", "login"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Login)));
    }

    #[test]
    fn test_logout_subcommand() {
        let cli = parse(&["longbridge", "logout"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Logout)));
    }

    #[test]
    fn test_logout_flag() {
        let cli = parse(&["longbridge", "--logout"]).unwrap();
        assert!(cli.logout);
    }

    // ─── Quote commands ───────────────────────────────────────────────────────

    #[test]
    fn test_quote_single_symbol() {
        let cli = parse(&["longbridge", "quote", "TSLA.US"]).unwrap();
        if let Some(Commands::Quote { symbols }) = cli.command {
            assert_eq!(symbols, vec!["TSLA.US"]);
        } else {
            panic!("expected Quote command");
        }
    }

    #[test]
    fn test_quote_multiple_symbols() {
        let cli = parse(&["longbridge", "quote", "TSLA.US", "700.HK", "AAPL.US"]).unwrap();
        if let Some(Commands::Quote { symbols }) = cli.command {
            assert_eq!(symbols.len(), 3);
        } else {
            panic!("expected Quote command");
        }
    }

    #[test]
    fn test_depth_subcommand() {
        let cli = parse(&["longbridge", "depth", "700.HK"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Depth { symbol }) if symbol == "700.HK"));
    }

    #[test]
    fn test_brokers_subcommand() {
        let cli = parse(&["longbridge", "brokers", "700.HK"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Brokers { symbol }) if symbol == "700.HK"));
    }

    #[test]
    fn test_trades_default_count() {
        let cli = parse(&["longbridge", "trades", "TSLA.US"]).unwrap();
        if let Some(Commands::Trades { symbol, count }) = cli.command {
            assert_eq!(symbol, "TSLA.US");
            assert_eq!(count, 20);
        } else {
            panic!("expected Trades command");
        }
    }

    #[test]
    fn test_trades_custom_count() {
        let cli = parse(&["longbridge", "trades", "TSLA.US", "--count", "50"]).unwrap();
        if let Some(Commands::Trades { count, .. }) = cli.command {
            assert_eq!(count, 50);
        } else {
            panic!("expected Trades command");
        }
    }

    #[test]
    fn test_intraday_subcommand() {
        let cli = parse(&["longbridge", "intraday", "TSLA.US"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Intraday { symbol }) if symbol == "TSLA.US"));
    }

    #[test]
    fn test_kline_defaults() {
        let cli = parse(&["longbridge", "kline", "TSLA.US"]).unwrap();
        if let Some(Commands::Kline { symbol, period, count, adjust }) = cli.command {
            assert_eq!(symbol, "TSLA.US");
            assert_eq!(period, "day");
            assert_eq!(count, 100);
            assert_eq!(adjust, "no_adjust");
        } else {
            panic!("expected Kline command");
        }
    }

    #[test]
    fn test_kline_custom_period() {
        let cli = parse(&["longbridge", "kline", "TSLA.US", "--period", "1h", "--count", "200"]).unwrap();
        if let Some(Commands::Kline { period, count, .. }) = cli.command {
            assert_eq!(period, "1h");
            assert_eq!(count, 200);
        } else {
            panic!("expected Kline command");
        }
    }

    #[test]
    fn test_kline_history_with_dates() {
        let cli = parse(&["longbridge", "kline-history", "TSLA.US", "--start", "2024-01-01", "--end", "2024-12-31"]).unwrap();
        if let Some(Commands::KlineHistory { symbol, start, end, .. }) = cli.command {
            assert_eq!(symbol, "TSLA.US");
            assert_eq!(start, Some("2024-01-01".to_string()));
            assert_eq!(end, Some("2024-12-31".to_string()));
        } else {
            panic!("expected KlineHistory command");
        }
    }

    #[test]
    fn test_static_subcommand() {
        let cli = parse(&["longbridge", "static", "TSLA.US", "700.HK"]).unwrap();
        if let Some(Commands::Static { symbols }) = cli.command {
            assert_eq!(symbols.len(), 2);
        } else {
            panic!("expected Static command");
        }
    }

    #[test]
    fn test_calc_index_default_indexes() {
        let cli = parse(&["longbridge", "calc-index", "TSLA.US"]).unwrap();
        if let Some(Commands::CalcIndex { symbols, index }) = cli.command {
            assert_eq!(symbols, vec!["TSLA.US"]);
            assert!(index.contains(&"pe".to_string()));
        } else {
            panic!("expected CalcIndex command");
        }
    }

    #[test]
    fn test_calc_index_custom_indexes() {
        let cli = parse(&["longbridge", "calc-index", "TSLA.US", "--index", "pe,pb,eps"]).unwrap();
        if let Some(Commands::CalcIndex { index, .. }) = cli.command {
            assert_eq!(index, vec!["pe", "pb", "eps"]);
        } else {
            panic!("expected CalcIndex command");
        }
    }

    #[test]
    fn test_capital_flow_subcommand() {
        let cli = parse(&["longbridge", "capital-flow", "TSLA.US"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::CapitalFlow { symbol }) if symbol == "TSLA.US"));
    }

    #[test]
    fn test_capital_dist_subcommand() {
        let cli = parse(&["longbridge", "capital-dist", "TSLA.US"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::CapitalDist { symbol }) if symbol == "TSLA.US"));
    }

    #[test]
    fn test_market_temp_default() {
        let cli = parse(&["longbridge", "market-temp"]).unwrap();
        if let Some(Commands::MarketTemp { market, history, .. }) = cli.command {
            assert_eq!(market, "HK");
            assert!(!history);
        } else {
            panic!("expected MarketTemp command");
        }
    }

    #[test]
    fn test_market_temp_history_flag() {
        let cli = parse(&["longbridge", "market-temp", "US", "--history", "--start", "2024-01-01"]).unwrap();
        if let Some(Commands::MarketTemp { market, history, start, .. }) = cli.command {
            assert_eq!(market, "US");
            assert!(history);
            assert_eq!(start, Some("2024-01-01".to_string()));
        } else {
            panic!("expected MarketTemp command");
        }
    }

    #[test]
    fn test_trading_session_subcommand() {
        let cli = parse(&["longbridge", "trading-session"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::TradingSession)));
    }

    #[test]
    fn test_trading_days_default_market() {
        let cli = parse(&["longbridge", "trading-days"]).unwrap();
        if let Some(Commands::TradingDays { market, .. }) = cli.command {
            assert_eq!(market, "HK");
        } else {
            panic!("expected TradingDays command");
        }
    }

    #[test]
    fn test_security_list_subcommand() {
        let cli = parse(&["longbridge", "security-list", "US"]).unwrap();
        if let Some(Commands::SecurityList { market, .. }) = cli.command {
            assert_eq!(market, "US");
        } else {
            panic!("expected SecurityList command");
        }
    }

    #[test]
    fn test_participants_subcommand() {
        let cli = parse(&["longbridge", "participants"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Participants)));
    }

    #[test]
    fn test_subscriptions_subcommand() {
        let cli = parse(&["longbridge", "subscriptions"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Subscriptions)));
    }

    // ─── Options & Warrants ───────────────────────────────────────────────────

    #[test]
    fn test_option_quote_subcommand() {
        let cli = parse(&["longbridge", "option-quote", "AAPL240119C190000"]).unwrap();
        if let Some(Commands::OptionQuote { symbols }) = cli.command {
            assert_eq!(symbols, vec!["AAPL240119C190000"]);
        } else {
            panic!("expected OptionQuote command");
        }
    }

    #[test]
    fn test_option_chain_no_date() {
        let cli = parse(&["longbridge", "option-chain", "AAPL.US"]).unwrap();
        if let Some(Commands::OptionChain { symbol, date }) = cli.command {
            assert_eq!(symbol, "AAPL.US");
            assert!(date.is_none());
        } else {
            panic!("expected OptionChain command");
        }
    }

    #[test]
    fn test_option_chain_with_date() {
        let cli = parse(&["longbridge", "option-chain", "AAPL.US", "--date", "2024-01-19"]).unwrap();
        if let Some(Commands::OptionChain { date, .. }) = cli.command {
            assert_eq!(date, Some("2024-01-19".to_string()));
        } else {
            panic!("expected OptionChain command");
        }
    }

    #[test]
    fn test_warrant_quote_subcommand() {
        let cli = parse(&["longbridge", "warrant-quote", "12345.HK"]).unwrap();
        if let Some(Commands::WarrantQuote { symbols }) = cli.command {
            assert_eq!(symbols, vec!["12345.HK"]);
        } else {
            panic!("expected WarrantQuote command");
        }
    }

    #[test]
    fn test_warrant_list_subcommand() {
        let cli = parse(&["longbridge", "warrant-list", "700.HK"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::WarrantList { symbol }) if symbol == "700.HK"));
    }

    #[test]
    fn test_warrant_issuers_subcommand() {
        let cli = parse(&["longbridge", "warrant-issuers"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::WarrantIssuers)));
    }

    // ─── Watchlist ────────────────────────────────────────────────────────────

    #[test]
    fn test_watchlist_no_subcommand() {
        let cli = parse(&["longbridge", "watchlist"]).unwrap();
        if let Some(Commands::Watchlist { cmd }) = cli.command {
            assert!(cmd.is_none());
        } else {
            panic!("expected Watchlist command");
        }
    }

    #[test]
    fn test_watchlist_create() {
        let cli = parse(&["longbridge", "watchlist", "create", "Tech Stocks"]).unwrap();
        if let Some(Commands::Watchlist { cmd: Some(WatchlistCmd::Create { name }) }) = cli.command {
            assert_eq!(name, "Tech Stocks");
        } else {
            panic!("expected Watchlist Create command");
        }
    }

    #[test]
    fn test_watchlist_delete() {
        let cli = parse(&["longbridge", "watchlist", "delete", "123"]).unwrap();
        if let Some(Commands::Watchlist { cmd: Some(WatchlistCmd::Delete { id, purge }) }) = cli.command {
            assert_eq!(id, 123);
            assert!(!purge);
        } else {
            panic!("expected Watchlist Delete command");
        }
    }

    #[test]
    fn test_watchlist_delete_purge() {
        let cli = parse(&["longbridge", "watchlist", "delete", "123", "--purge"]).unwrap();
        if let Some(Commands::Watchlist { cmd: Some(WatchlistCmd::Delete { purge, .. }) }) = cli.command {
            assert!(purge);
        } else {
            panic!("expected Watchlist Delete command");
        }
    }

    #[test]
    fn test_watchlist_update_add() {
        let cli = parse(&["longbridge", "watchlist", "update", "123", "--add", "TSLA.US", "--add", "AAPL.US"]).unwrap();
        if let Some(Commands::Watchlist { cmd: Some(WatchlistCmd::Update { id, add, .. }) }) = cli.command {
            assert_eq!(id, 123);
            assert_eq!(add, vec!["TSLA.US", "AAPL.US"]);
        } else {
            panic!("expected Watchlist Update command");
        }
    }

    #[test]
    fn test_watchlist_update_remove() {
        let cli = parse(&["longbridge", "watchlist", "update", "456", "--remove", "700.HK"]).unwrap();
        if let Some(Commands::Watchlist { cmd: Some(WatchlistCmd::Update { id, remove, .. }) }) = cli.command {
            assert_eq!(id, 456);
            assert_eq!(remove, vec!["700.HK"]);
        } else {
            panic!("expected Watchlist Update command");
        }
    }

    // ─── Trade commands ───────────────────────────────────────────────────────

    #[test]
    fn test_orders_defaults() {
        let cli = parse(&["longbridge", "orders"]).unwrap();
        if let Some(Commands::Orders { history, start, end, symbol }) = cli.command {
            assert!(!history);
            assert!(start.is_none());
            assert!(end.is_none());
            assert!(symbol.is_none());
        } else {
            panic!("expected Orders command");
        }
    }

    #[test]
    fn test_orders_history_with_filters() {
        let cli = parse(&["longbridge", "orders", "--history", "--start", "2024-01-01", "--symbol", "TSLA.US"]).unwrap();
        if let Some(Commands::Orders { history, start, symbol, .. }) = cli.command {
            assert!(history);
            assert_eq!(start, Some("2024-01-01".to_string()));
            assert_eq!(symbol, Some("TSLA.US".to_string()));
        } else {
            panic!("expected Orders command");
        }
    }

    #[test]
    fn test_order_detail_subcommand() {
        let cli = parse(&["longbridge", "order", "order-123"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Order { order_id }) if order_id == "order-123"));
    }

    #[test]
    fn test_executions_subcommand() {
        let cli = parse(&["longbridge", "executions"]).unwrap();
        if let Some(Commands::Executions { history, .. }) = cli.command {
            assert!(!history);
        } else {
            panic!("expected Executions command");
        }
    }

    #[test]
    fn test_buy_subcommand() {
        let cli = parse(&["longbridge", "buy", "TSLA.US", "100", "--price", "250.00"]).unwrap();
        if let Some(Commands::Buy { symbol, quantity, price, order_type, tif }) = cli.command {
            assert_eq!(symbol, "TSLA.US");
            assert_eq!(quantity, 100);
            assert_eq!(price, Some("250.00".to_string()));
            assert_eq!(order_type, "LO");
            assert_eq!(tif, "Day");
        } else {
            panic!("expected Buy command");
        }
    }

    #[test]
    fn test_sell_subcommand() {
        let cli = parse(&["longbridge", "sell", "TSLA.US", "50", "--price", "260.00"]).unwrap();
        if let Some(Commands::Sell { symbol, quantity, price, .. }) = cli.command {
            assert_eq!(symbol, "TSLA.US");
            assert_eq!(quantity, 50);
            assert_eq!(price, Some("260.00".to_string()));
        } else {
            panic!("expected Sell command");
        }
    }

    #[test]
    fn test_cancel_subcommand() {
        let cli = parse(&["longbridge", "cancel", "order-456"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Cancel { order_id }) if order_id == "order-456"));
    }

    #[test]
    fn test_replace_subcommand() {
        let cli = parse(&["longbridge", "replace", "order-789", "--qty", "200", "--price", "255.00"]).unwrap();
        if let Some(Commands::Replace { order_id, qty, price }) = cli.command {
            assert_eq!(order_id, "order-789");
            assert_eq!(qty, Some(200));
            assert_eq!(price, Some("255.00".to_string()));
        } else {
            panic!("expected Replace command");
        }
    }

    #[test]
    fn test_balance_no_currency() {
        let cli = parse(&["longbridge", "balance"]).unwrap();
        if let Some(Commands::Balance { currency }) = cli.command {
            assert!(currency.is_none());
        } else {
            panic!("expected Balance command");
        }
    }

    #[test]
    fn test_balance_with_currency() {
        let cli = parse(&["longbridge", "balance", "--currency", "USD"]).unwrap();
        if let Some(Commands::Balance { currency }) = cli.command {
            assert_eq!(currency, Some("USD".to_string()));
        } else {
            panic!("expected Balance command");
        }
    }

    #[test]
    fn test_cash_flow_subcommand() {
        let cli = parse(&["longbridge", "cash-flow", "--start", "2024-01-01", "--end", "2024-03-31"]).unwrap();
        if let Some(Commands::CashFlow { start, end }) = cli.command {
            assert_eq!(start, Some("2024-01-01".to_string()));
            assert_eq!(end, Some("2024-03-31".to_string()));
        } else {
            panic!("expected CashFlow command");
        }
    }

    #[test]
    fn test_positions_subcommand() {
        let cli = parse(&["longbridge", "positions"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Positions)));
    }

    #[test]
    fn test_fund_positions_subcommand() {
        let cli = parse(&["longbridge", "fund-positions"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::FundPositions)));
    }

    #[test]
    fn test_margin_ratio_subcommand() {
        let cli = parse(&["longbridge", "margin-ratio", "TSLA.US"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::MarginRatio { symbol }) if symbol == "TSLA.US"));
    }

    #[test]
    fn test_max_qty_subcommand() {
        let cli = parse(&["longbridge", "max-qty", "TSLA.US", "--side", "buy", "--price", "250"]).unwrap();
        if let Some(Commands::MaxQty { symbol, side, price, order_type }) = cli.command {
            assert_eq!(symbol, "TSLA.US");
            assert_eq!(side, "buy");
            assert_eq!(price, Some("250".to_string()));
            assert_eq!(order_type, "LO");
        } else {
            panic!("expected MaxQty command");
        }
    }

    // ─── Error cases ──────────────────────────────────────────────────────────

    #[test]
    fn test_unknown_subcommand_fails() {
        assert!(parse(&["longbridge", "nonexistent"]).is_err());
    }

    #[test]
    fn test_no_subcommand_is_valid() {
        let cli = parse(&["longbridge"]).unwrap();
        assert!(cli.command.is_none());
    }
}
