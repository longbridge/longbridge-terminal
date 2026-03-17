use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

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
#[command(about = "Longbridge CLI - CLI for Longbridge OpenAPI")]
#[command(long_about = "\
Longbridge Terminal combines a full-screen TUI (terminal UI) and an AI-native CLI \
that wraps every Longbridge OpenAPI endpoint.\n\n\
When called without a subcommand the TUI launches. When called with a subcommand \
the result is printed to stdout and the process exits — suitable for scripting and \
AI agent tool-calling.\n\n\
Symbol format: <CODE>.<MARKET>  e.g. TSLA.US  700.HK  600519.SH\n\
Markets: HK (Hong Kong)  US (United States)  CN (China A-share)  SG (Singapore)\n\n\
Authentication is shared between TUI and CLI. Run `longbridge login` once; the token \
is stored at ~/.longbridge/terminal/session-<client_id> and reused automatically.\n\n\
Use --format json on any command for machine-readable output suitable for AI agents:\n\
  longbridge quote TSLA.US --format json\n\
  longbridge positions --format json | jq '.[] | {symbol, quantity}'")]
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
    /// Authenticate via browser OAuth and save token for TUI and CLI
    ///
    /// Opens a browser for Longbridge OpenAPI authorization.
    /// Token is stored at ~/.longbridge/terminal/session-<client_id> and shared with the TUI.
    Login,

    /// Clear the locally stored OAuth token
    ///
    /// Next command or TUI launch will trigger re-authentication.
    Logout,

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
    /// Periods: 1m 5m 15m 30m 1h day week month year
    /// Example: longbridge kline TSLA.US --period day --count 100
    /// Example: longbridge kline TSLA.US --period 1h --adjust forward_adjust
    Kline {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Candlestick period: 1m 5m 15m 30m 1h day week month year (default: day)
        #[arg(long, default_value = "day")]
        period: String,
        /// Number of candles to return (default: 100)
        #[arg(long, default_value = "100")]
        count: usize,
        /// Price adjustment: no_adjust (default) or forward_adjust (split/dividend adjusted)
        #[arg(long, default_value = "no_adjust")]
        adjust: String,
    },

    /// Historical OHLCV candlestick data within a date range
    ///
    /// Omit --start/--end to get the most recent 100 candles.
    /// Example: longbridge kline-history TSLA.US --start 2024-01-01 --end 2024-12-31
    KlineHistory {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Candlestick period: 1m 5m 15m 30m 1h day week month year (default: day)
        #[arg(long, default_value = "day")]
        period: String,
        /// Start date (YYYY-MM-DD)
        #[arg(long)]
        start: Option<String>,
        /// End date (YYYY-MM-DD)
        #[arg(long)]
        end: Option<String>,
        /// Price adjustment: no_adjust (default) or forward_adjust
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
    /// Available indexes: pe pb eps turnover_rate total_market_value amplitude volume_ratio
    ///   ytd_change_rate capital_flow five_day_change_rate implied_volatility delta open_interest
    /// Example: longbridge calc-index TSLA.US AAPL.US --index pe,pb,turnover_rate
    CalcIndex {
        /// One or more symbols in <CODE>.<MARKET> format
        symbols: Vec<String>,
        /// Comma-separated indexes to compute (default: pe,pb,eps,turnover_rate,total_market_value)
        #[arg(long, value_delimiter = ',', default_value = "pe,pb,eps,turnover_rate,total_market_value")]
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
        /// Market: HK US CN SG (default: HK)
        #[arg(default_value = "HK")]
        market: String,
        /// Return historical records instead of current value
        #[arg(long)]
        history: bool,
        /// Start date for history (YYYY-MM-DD)
        #[arg(long)]
        start: Option<String>,
        /// End date for history (YYYY-MM-DD)
        #[arg(long)]
        end: Option<String>,
        /// Granularity for history: daily weekly monthly (default: daily)
        #[arg(long, default_value = "daily")]
        granularity: String,
    },

    /// Trading session schedule (open/close times) for all markets
    ///
    /// Returns: market, session type (intraday/pre/post/overnight), begin_time, end_time.
    TradingSession,

    /// Trading days and half-trading days for a market
    ///
    /// Defaults to the next 30 days if no dates are provided.
    /// Example: longbridge trading-days HK --start 2024-01-01 --end 2024-03-31
    TradingDays {
        /// Market: HK US CN SG (default: HK)
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
        /// Market: HK US CN SG (default: HK)
        #[arg(default_value = "HK")]
        market: String,
        /// Board category (default: main)
        #[arg(long, default_value = "main")]
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
    /// Example: longbridge option-chain AAPL
    /// Example: longbridge option-chain AAPL --date 2024-01-19
    OptionChain {
        /// Underlying symbol (e.g. AAPL or TSLA.US)
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
    /// Order types: LO (limit) MO (market) ELO ALO ODD SLO LIT MIT
    /// Example: longbridge buy TSLA.US 100 --price 250.00
    /// Example: longbridge buy 700.HK 1000 --price 300 --order-type ALO
    Buy {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Number of shares/units to buy
        quantity: u64,
        /// Limit price (required for LO/ELO/ALO; omit for market orders)
        #[arg(long)]
        price: Option<String>,
        /// Order type: LO MO ELO ALO ODD SLO LIT MIT (default: LO)
        #[arg(long, default_value = "LO")]
        order_type: String,
        /// Time in force: Day GoodTilCanceled GoodTilDate (default: Day)
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
        /// Number of shares/units to sell
        quantity: u64,
        /// Limit price (required for LO/ELO/ALO; omit for market orders)
        #[arg(long)]
        price: Option<String>,
        /// Order type: LO MO ELO ALO ODD SLO LIT MIT (default: LO)
        #[arg(long, default_value = "LO")]
        order_type: String,
        /// Time in force: Day GoodTilCanceled GoodTilDate (default: Day)
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
    /// At least one of --qty or --price must be provided.
    /// Example: longbridge replace 20240101-123456789 --qty 200 --price 255.00
    Replace {
        /// Order ID to modify
        order_id: String,
        /// New quantity
        #[arg(long)]
        qty: Option<u64>,
        /// New limit price
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
        /// Order side: buy or sell
        #[arg(long)]
        side: String,
        /// Limit price (required for LO orders)
        #[arg(long)]
        price: Option<String>,
        /// Order type: LO MO ELO ALO (default: LO)
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
        Commands::TradingDays {
            market,
            start,
            end,
        } => quote::cmd_trading_days(&market, start, end, format).await,
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
        Commands::Login | Commands::Logout => unreachable!(),
    }
}
