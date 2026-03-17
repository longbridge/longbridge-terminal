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
#[command(name = "longbridge", about = "Longbridge Terminal - TUI & CLI for OpenAPI")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Output format
    #[arg(long, global = true, default_value = "table")]
    pub format: OutputFormat,

    /// Clear stored OAuth token and exit
    #[arg(long)]
    pub logout: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Authenticate with Longbridge OpenAPI (shared token for TUI and CLI)
    Login,
    /// Clear stored OAuth token
    Logout,

    // -- Quote --
    /// Get real-time quotes for one or more symbols (e.g. TSLA.US 700.HK)
    Quote {
        /// Stock symbols
        symbols: Vec<String>,
    },
    /// Get order book depth for a symbol
    Depth {
        symbol: String,
    },
    /// Get broker queue for a symbol
    Brokers {
        symbol: String,
    },
    /// Get recent trades for a symbol
    Trades {
        symbol: String,
        #[arg(long, default_value = "20")]
        count: usize,
    },
    /// Get intraday lines for a symbol
    Intraday {
        symbol: String,
    },
    /// Get candlestick data for a symbol
    Kline {
        symbol: String,
        /// Period: 1m 5m 15m 30m 1h day week month year
        #[arg(long, default_value = "day")]
        period: String,
        #[arg(long, default_value = "100")]
        count: usize,
        /// Adjust type: no_adjust forward_adjust
        #[arg(long, default_value = "no_adjust")]
        adjust: String,
    },
    /// Get history candlesticks by date range
    KlineHistory {
        symbol: String,
        #[arg(long, default_value = "day")]
        period: String,
        /// Start date (YYYY-MM-DD)
        #[arg(long)]
        start: Option<String>,
        /// End date (YYYY-MM-DD)
        #[arg(long)]
        end: Option<String>,
        #[arg(long, default_value = "no_adjust")]
        adjust: String,
    },
    /// Get static info for one or more symbols
    Static {
        symbols: Vec<String>,
    },
    /// Get calculated indexes for symbols (e.g. pe pb eps)
    CalcIndex {
        symbols: Vec<String>,
        /// Indexes: pe pb eps turnover_rate total_market_value amplitude volume_ratio
        #[arg(long, value_delimiter = ',', default_value = "pe,pb,eps,turnover_rate,total_market_value")]
        index: Vec<String>,
    },
    /// Get capital flow for a symbol
    CapitalFlow {
        symbol: String,
    },
    /// Get capital distribution for a symbol
    CapitalDist {
        symbol: String,
    },
    /// Get market temperature
    MarketTemp {
        /// Market: HK US CN SG
        #[arg(default_value = "HK")]
        market: String,
        /// Get history market temperature
        #[arg(long)]
        history: bool,
        #[arg(long)]
        start: Option<String>,
        #[arg(long)]
        end: Option<String>,
        /// Granularity for history: daily weekly monthly
        #[arg(long, default_value = "daily")]
        granularity: String,
    },
    /// Get trading sessions for all markets
    TradingSession,
    /// Get trading days for a market
    TradingDays {
        /// Market: HK US CN SG
        #[arg(default_value = "HK")]
        market: String,
        /// Start date (YYYY-MM-DD)
        #[arg(long)]
        start: Option<String>,
        /// End date (YYYY-MM-DD)
        #[arg(long)]
        end: Option<String>,
    },
    /// Get security list for a market
    SecurityList {
        /// Market: HK US CN SG
        #[arg(default_value = "HK")]
        market: String,
        /// Category: main gem pre_release etf lot_stock warrant_bond
        #[arg(long, default_value = "main")]
        category: String,
    },
    /// Get market maker participants
    Participants,
    /// Get current real-time subscriptions
    Subscriptions,

    // -- Options / Warrants --
    /// Get option quotes
    OptionQuote {
        symbols: Vec<String>,
    },
    /// Get option chain expiry dates, or strike prices for a specific date
    OptionChain {
        symbol: String,
        /// Get strike prices for a specific expiry date (YYYY-MM-DD)
        #[arg(long)]
        date: Option<String>,
    },
    /// Get warrant quotes
    WarrantQuote {
        symbols: Vec<String>,
    },
    /// Get warrant list for a symbol
    WarrantList {
        symbol: String,
    },
    /// Get warrant issuer list
    WarrantIssuers,

    // -- Watchlist --
    /// Manage watchlist groups
    Watchlist {
        #[command(subcommand)]
        cmd: Option<WatchlistCmd>,
    },

    // -- Trade --
    /// List orders (today by default)
    Orders {
        /// Get history orders
        #[arg(long)]
        history: bool,
        /// Start date/time (YYYY-MM-DD)
        #[arg(long)]
        start: Option<String>,
        /// End date/time (YYYY-MM-DD)
        #[arg(long)]
        end: Option<String>,
        /// Filter by symbol
        #[arg(long)]
        symbol: Option<String>,
    },
    /// Get order detail by order ID
    Order {
        order_id: String,
    },
    /// List executions (today by default)
    Executions {
        /// Get history executions
        #[arg(long)]
        history: bool,
        #[arg(long)]
        start: Option<String>,
        #[arg(long)]
        end: Option<String>,
        #[arg(long)]
        symbol: Option<String>,
    },
    /// Submit a buy order
    Buy {
        symbol: String,
        quantity: u64,
        /// Limit price (required for LO orders)
        #[arg(long)]
        price: Option<String>,
        /// Order type: LO MO ELO ALO ODD SLO
        #[arg(long, default_value = "LO")]
        order_type: String,
        /// Time in force: Day GoodTilCanceled GoodTilDate
        #[arg(long, default_value = "Day")]
        tif: String,
    },
    /// Submit a sell order
    Sell {
        symbol: String,
        quantity: u64,
        #[arg(long)]
        price: Option<String>,
        #[arg(long, default_value = "LO")]
        order_type: String,
        #[arg(long, default_value = "Day")]
        tif: String,
    },
    /// Cancel an order
    Cancel {
        order_id: String,
    },
    /// Modify an existing order
    Replace {
        order_id: String,
        /// New quantity
        #[arg(long)]
        qty: Option<u64>,
        /// New price
        #[arg(long)]
        price: Option<String>,
    },
    /// Get account balance
    Balance {
        /// Filter by currency (e.g. USD HKD)
        #[arg(long)]
        currency: Option<String>,
    },
    /// Get cash flow records
    CashFlow {
        #[arg(long)]
        start: Option<String>,
        #[arg(long)]
        end: Option<String>,
    },
    /// Get stock positions
    Positions,
    /// Get fund positions
    FundPositions,
    /// Get margin ratio for a symbol
    MarginRatio {
        symbol: String,
    },
    /// Estimate max purchase/sell quantity
    MaxQty {
        symbol: String,
        /// Side: Buy Sell
        #[arg(long)]
        side: String,
        /// Limit price
        #[arg(long)]
        price: Option<String>,
        #[arg(long, default_value = "LO")]
        order_type: String,
    },
}

#[derive(Subcommand)]
pub enum WatchlistCmd {
    /// Create a new watchlist group
    Create {
        /// Group name
        name: String,
    },
    /// Delete a watchlist group
    Delete {
        /// Group ID
        id: i64,
        /// Also remove all securities in the group
        #[arg(long)]
        purge: bool,
    },
    /// Update a watchlist group
    Update {
        /// Group ID
        id: i64,
        #[arg(long)]
        name: Option<String>,
        /// Symbols to add
        #[arg(long)]
        add: Vec<String>,
        /// Symbols to remove
        #[arg(long)]
        remove: Vec<String>,
        /// Update mode: add remove replace
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
