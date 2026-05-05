use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};

pub mod api;
pub mod asset;
pub mod auth;
pub mod check;
pub mod completion;
pub mod dca;
pub mod fundamental;
pub mod init;
pub mod insider_trades;
pub mod investors;
pub mod my_quote;
pub mod news;
pub mod output;
pub mod quant_render;
pub mod quote;
pub mod run_script;
pub mod sharelist;
pub mod statement;
pub mod topic;
pub mod trade;
pub mod watchlist;

#[derive(ValueEnum, Clone, Default, Debug)]
pub enum OutputFormat {
    #[default]
    #[value(name = "table", alias = "pretty")]
    Pretty,
    Json,
}

#[derive(Parser)]
#[command(name = "longbridge")]
#[command(about = "\
AI-native CLI for the Longbridge trading platform — real-time market data, portfolio, and trading.\n\n\
Symbol e.g.: TSLA.US 700.HK D05.SG 600519.SH 000568.SZ .VIX.US BTCUSD.HAS ETHBTC.HAS")]
#[command(long_about = "\
AI-native CLI for the Longbridge trading platform — real-time market data, portfolio, and trading.\n\n\
Symbol format: <CODE>.<MARKET>\n\
  TSLA.US      United States (US)\n\
  700.HK       Hong Kong (HK)\n\
  D05.SG       Singapore (SG)\n\
  600519.SH    China A-share Shanghai (SH)\n\
  000568.SZ    China A-share Shenzhen (SZ)\n\
  .VIX.US      Index (US)\n\
  BTCUSD.HAS   Crypto — Longbridge-specific suffix (.HAS); not available to all accounts\n\
  ETHBTC.HAS   Crypto pair (e.g. ETH priced in BTC)\n\n\
Note: crypto symbols use the .HAS suffix (Longbridge-specific). If a .HAS symbol returns no\n\
data, crypto market access may not be enabled for this account — the data exists but is\n\
restricted by account type.\n\n\
Authentication: run `longbridge auth login` once; the token is stored at \
~/.longbridge/openapi/tokens/<client_id> and reused automatically by all commands.\n\n\
Use --format json on any command for machine-readable output suitable for AI agents:\n\
  longbridge quote TSLA.US --format json\n\
  longbridge positions --format json | jq '.[] | {symbol, quantity}'\n\n\
Use `longbridge tui` to launch the interactive full-screen terminal UI.")]
#[command(version)]
#[command(
    after_help = "Each command has two help levels:\n  longbridge <command> -h       brief summary (options only)\n  longbridge <command> --help   full detail: constraints, rate limits, return fields, examples"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Output format: 'pretty' for human-readable, 'json' for AI agents and scripting
    #[arg(long, global = true, default_value = "pretty")]
    pub format: OutputFormat,

    /// Print verbose request info (host, elapsed) to stderr, prefixed with `*` like curl -v
    #[arg(long, short = 'v', global = true)]
    pub verbose: bool,

    /// Language for content fetched from longbridge.com: zh-CN or en.
    /// Defaults to system LANG env var, then en.
    #[arg(long, global = true)]
    pub lang: Option<String>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Authenticate or clear credentials
    ///
    /// Token is stored at `~/.longbridge/openapi/tokens/<client_id>` and shared with the TUI.
    Auth {
        #[command(subcommand)]
        cmd: AuthCmd,
    },

    /// Set an invite code for affiliate tracking
    ///
    /// Stores the given invite code locally. The code is sent during OAuth authorization
    /// so the server can associate the user with the referral channel (e.g. a KOL campaign).
    /// It is also included as a header in subsequent API requests.
    /// Example: longbridge init KOL-ABC123
    Init {
        /// Invite code provided by the referral channel
        invite_code: String,
    },

    /// Check token validity, and API connectivity
    ///
    /// Shows token status, cached region, and latency to both Global and CN API endpoints.
    /// Does not require authentication.
    /// Example: longbridge check
    /// Example: longbridge check --format json
    Check,

    /// Update longbridge to the latest version
    ///
    /// Downloads and runs the official install script to replace the current binary.
    /// Example: longbridge update
    /// Example: longbridge update --release-notes
    Update {
        /// Show release notes instead of updating
        #[arg(long)]
        release_notes: bool,
        /// Force update even if already on the latest version
        #[arg(short, long)]
        force: bool,
    },

    /// Launch the interactive full-screen TUI (terminal UI)
    ///
    /// Real-time watchlist, candlestick charts, portfolio view, stock search, Vim-like keybindings.
    /// Example: longbridge tui
    Tui,

    /// Generate shell completion script
    ///
    /// Prints a shell completion script to stdout.
    /// Redirect the output to the appropriate file and reload your shell to enable tab-completion.
    ///
    /// Example (bash):  `longbridge completion bash >> ~/.bash_completion`
    /// Example (zsh):   `longbridge completion zsh  > ~/.zfunc/_longbridge`
    ///                  (add `fpath=(~/.zfunc $fpath)` and `autoload -Uz compinit && compinit` to `~/.zshrc`)
    /// Example (fish):  `longbridge completion fish > ~/.config/fish/completions/longbridge.fish`
    Completion {
        /// Target shell: bash, zsh, fish, elvish, or powershell
        shell: clap_complete::Shell,
    },

    // ── Quote ──────────────────────────────────────────────────────────────────
    /// Real-time quotes for one or more symbols
    ///
    /// Returns: symbol, `last_done`, `prev_close`, open, high, low, volume, turnover, `trade_status`.
    /// Also returns `pre_market_quote`, `post_market_quote`, `overnight_quote` when available (US only).
    /// In table format an "Extended Hours" section is appended; in JSON these are nested objects.
    /// Example: longbridge quote TSLA.US 700.HK AAPL.US
    /// Example: longbridge quote TSLA.US NVDA.US --format json
    Quote {
        /// Symbols in <CODE>.<MARKET> format, e.g. TSLA.US QQQ.US 700.HK .VIX.US
        symbols: Vec<String>,
    },

    /// Level 2 order book depth (bid/ask ladder)
    ///
    /// Returns up to 10 price levels of asks and bids with price, volume, `order_num`.
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
    /// Returns: timestamp, price, volume, direction (up/down/neutral), `trade_type`.
    /// Example: longbridge trades TSLA.US --count 50
    Trades {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Number of trades to return (default: 20, max: 1000)
        #[arg(long, alias = "limit", default_value = "20")]
        count: usize,
    },

    /// Intraday minute-by-minute price and volume lines for today (or a historical date)
    ///
    /// Returns: timestamp, price, volume, turnover, `avg_price`.
    /// Use `--session all` to include pre-market and post-market lines.
    /// Use `--date YYYYMMDD` to fetch a historical day's intraday data.
    /// Example: longbridge intraday TSLA.US
    /// Example: longbridge intraday TSLA.US --session all
    /// Example: longbridge intraday TSLA.US --date 20240115
    Intraday {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Trade session filter: `intraday` (default) | `all` (includes pre/post market)
        #[arg(long, default_value = "intraday")]
        session: String,
        /// Historical date in YYYYMMDD format (omit for today's live data)
        #[arg(long)]
        date: Option<String>,
    },

    /// OHLCV candlestick (K-line) data, or historical date-range candlesticks
    ///
    /// Returns: timestamp, open, high, low, close, volume, turnover.
    /// Periods: 1m  5m  15m  30m  1h  day  week  month  year
    ///   (aliases: minute=1m, hour=1h, d/1d=day, w=week, m/1mo=month, y=year)
    /// Use --session all to include pre/post-market candles (adds a Session column).
    /// Use the `history` subcommand to fetch a specific date range.
    /// Example: longbridge kline TSLA.US --period day --count 100
    /// Example: longbridge kline TSLA.US --period 1h --adjust forward
    /// Example: longbridge kline TSLA.US --period 1m --session all
    /// Example: longbridge kline history TSLA.US --start 2024-01-01 --end 2024-12-31
    Kline {
        /// Symbol in <CODE>.<MARKET> format. Omit when using a subcommand.
        symbol: Option<String>,
        /// Candlestick period: 1m 5m 15m 30m 1h day week month year (default: day)
        /// Aliases: minute=1m, hour=1h, d/1d=day, w=week, m/1mo=month, y=year
        #[arg(long, default_value = "day")]
        period: String,
        /// Number of candles to return (default: 100)
        #[arg(long, alias = "limit", default_value = "100")]
        count: usize,
        /// Price adjustment: `none` (default) | `forward`
        #[arg(long, default_value = "none")]
        adjust: String,
        /// Trade session filter: `intraday` (default) | `all` (includes pre/post market)
        #[arg(long, default_value = "intraday")]
        session: String,
        #[command(subcommand)]
        cmd: Option<KlineCmd>,
    },

    /// Static reference info for one or more symbols
    ///
    /// Returns: name, exchange, currency, `lot_size`, `total_shares`, `circulating_shares`, EPS, BPS, dividend.
    /// Example: longbridge static TSLA.US 700.HK
    Static {
        /// One or more symbols in <CODE>.<MARKET> format
        symbols: Vec<String>,
    },

    /// Calculated financial indexes (PE, PB, DPS rate, turnover rate, etc.)
    ///
    /// Full field list:
    ///
    ///   General:
    ///     `last_done`  `change_value`  `change_rate`  `vol`  `turnover`
    ///     `ytd_change_rate`  `turnover_rate`  `mktcap`  `capital_flow`
    ///     `amplitude`  `volume_ratio`  `pe`  `pb`  `dps_rate`
    ///     `five_day_change_rate`  `ten_day_change_rate`  `half_year_change_rate`
    ///     `five_minutes_change_rate`
    ///
    ///   Options / Warrants:
    ///     `iv`  `delta`  `gamma`  `theta`  `vega`  `rho`
    ///     `oi`  `exp`  `strike`  `upper_strike_price`  `lower_strike_price`
    ///     `outstanding_qty`  `outstanding_ratio`  `premium`  `itm_otm`
    ///     `warrant_delta`  `call_price`  `to_call_price`
    ///     `effective_leverage`  `leverage_ratio`  `conversion_ratio`  `balance_point`
    ///
    /// Example: `longbridge calc-index TSLA.US AAPL.US --fields pe,pb,turnover_rate`
    /// Example: `longbridge calc-index SOXL260619C52000.US --fields delta,iv,oi,exp,strike`
    CalcIndex {
        /// One or more symbols in <CODE>.<MARKET> format
        symbols: Vec<String>,
        /// Comma-separated fields to compute. Use --help to see the full field list.
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "pe,pb,dps_rate,turnover_rate,mktcap"
        )]
        fields: Vec<String>,
    },

    /// Intraday capital distribution snapshot, or flow time series with --flow
    ///
    /// Example: longbridge capital TSLA.US
    /// Example: longbridge capital TSLA.US --flow
    Capital {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Show intraday capital flow time series instead of distribution snapshot
        #[arg(long)]
        flow: bool,
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

    /// Trading session schedule and trading calendar
    ///
    /// Subcommands: session  days
    /// Example: longbridge trading session
    /// Example: longbridge trading days HK --start 2024-01-01 --end 2024-03-31
    Trading {
        #[command(subcommand)]
        cmd: TradingCmd,
    },

    /// List of US overnight-eligible securities
    ///
    /// Returns securities that can be traded in the US overnight session.
    /// Only the US market is supported (Longbridge API limitation).
    /// Example: longbridge security-list US
    SecurityList {
        /// Market: only US is supported (overnight category)
        #[arg(default_value = "US")]
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
    /// Returns: symbol, `sub_types` (quote/depth/trade), subscribed candlestick periods.
    Subscriptions,

    // ── Options & Warrants ──────────────────────────────────────────────────────
    /// Option quotes, option chain, and option volume statistics
    ///
    /// Subcommands: chain  quote  volume
    /// Example: longbridge option quote AAPL240119C190000
    /// Example: longbridge option chain AAPL.US --date 2024-01-19
    /// Example: longbridge option volume AAPL.US
    /// Example: longbridge option volume daily AAPL.US
    Option {
        #[command(subcommand)]
        cmd: OptionCmd,
    },

    /// Warrant quotes, warrant list, and issuer list
    ///
    /// Without subcommand: lists warrants for an underlying symbol.
    /// Subcommands: quote  list  issuers
    /// Example: longbridge warrant 700.HK
    /// Example: longbridge warrant quote 12345.HK
    /// Example: longbridge warrant list 700.HK
    Warrant {
        /// Underlying symbol (e.g. 700.HK). Omit when using a subcommand.
        symbol: Option<String>,
        #[command(subcommand)]
        cmd: Option<WarrantCmd>,
    },

    // ── Fundamentals ────────────────────────────────────────────────────────────
    /// Financial statements (income, balance sheet, cash flow) for a symbol
    ///
    /// Example: longbridge financial-report TSLA.US --kind IS --report af
    /// Example: longbridge financial-report TSLA.US --kind BS --format json
    FinancialReport {
        /// Symbol in <CODE>.<MARKET> format, e.g. TSLA.US 700.HK
        symbol: String,
        /// Statement type: IS (income), BS (balance sheet), CF (cash flow), ALL
        #[arg(long, value_name = "TYPE", default_value = "ALL")]
        kind: String,
        /// Report period: af (annual), saf (semi-annual), q1 (Q1), 3q (3 quarters), qf (quarterly)
        #[arg(long)]
        report: Option<String>,
    },

    /// Institution rating overview and target price summary
    ///
    /// Without a subcommand: returns rating distribution (Strong Buy / Buy / Hold /
    /// Underperform / Sell) and the current average target price.
    /// Subcommands: detail
    /// Example: longbridge institution-rating TSLA.US
    /// Example: longbridge institution-rating detail TSLA.US
    /// Example: longbridge institution-rating TSLA.US --format json
    InstitutionRating {
        /// Symbol in <CODE>.<MARKET> format. Omit when using a subcommand.
        symbol: Option<String>,
        #[command(subcommand)]
        cmd: Option<InstitutionRatingCmd>,
    },

    /// Dividend history and distribution details for a symbol
    ///
    /// Example: longbridge dividend AAPL.US
    /// Example: longbridge dividend AAPL.US --page 2
    /// Example: longbridge dividend AAPL.US --year 2025
    /// Example: longbridge dividend detail AAPL.US
    Dividend {
        /// Symbol in <CODE>.<MARKET> format (omit when using a subcommand)
        symbol: Option<String>,
        /// Page number (default: 1)
        #[arg(long, default_value = "1")]
        page: u32,
        /// Filter by year (e.g. 2025)
        #[arg(long)]
        year: Option<u32>,
        #[command(subcommand)]
        cmd: Option<DividendCmd>,
    },

    /// EPS forecasts and analyst consensus estimates for a symbol
    ///
    /// Example: longbridge forecast-eps TSLA.US
    /// Example: longbridge forecast-eps TSLA.US --format json
    ForecastEps {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
    },

    /// Financial consensus detail for a symbol
    ///
    /// Example: longbridge consensus TSLA.US
    /// Example: longbridge consensus TSLA.US --format json
    Consensus {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
    },

    /// Finance calendar: upcoming events by category
    ///
    /// Example: longbridge finance-calendar report
    /// Example: longbridge finance-calendar report --filter watchlist --market US
    /// Example: longbridge finance-calendar dividend --filter positions
    /// Example: longbridge finance-calendar macrodata --star 3
    FinanceCalendar {
        #[command(subcommand)]
        cmd: FinanceCalendarCmd,
    },

    /// Valuation analysis: P/E, P/B, P/S, dividend yield, and peer comparison
    ///
    /// Default: current metrics + 5-year range + peer comparison.
    /// With --history: returns historical valuation time series (default indicator: pe).
    /// Example: longbridge valuation TSLA.US
    /// Example: longbridge valuation TSLA.US --history
    /// Example: longbridge valuation TSLA.US --history --indicator pb --range 5
    /// Example: longbridge valuation TSLA.US --format json
    Valuation {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Show historical valuation time series instead of current snapshot
        #[arg(long)]
        history: bool,
        /// Valuation indicator for history mode: `pe` | `pb` | `ps` | `dvd_yld`
        #[arg(long)]
        indicator: Option<String>,
        /// Historical range in years (history mode, default: 1): 1 | 3 | 5 | 10
        #[arg(long)]
        range: Option<String>,
    },

    // ── News ────────────────────────────────────────────────────────────────────
    /// Latest news articles for a symbol, or fetch full article content
    ///
    /// Without subcommand: lists news articles for a symbol.
    /// Subcommands: detail
    /// Returns: id, title, `published_at`, likes, comments.
    /// Example: longbridge news TSLA.US
    /// Example: longbridge news TSLA.US --count 5
    /// Example: longbridge news detail 12345678
    News {
        /// Symbol in <CODE>.<MARKET> format (e.g. TSLA.US 700.HK). Omit when using a subcommand.
        symbol: Option<String>,
        /// Maximum number of articles to show (default: 20)
        #[arg(long, alias = "limit", default_value = "20")]
        count: usize,
        #[command(subcommand)]
        cmd: Option<NewsCmd>,
    },

    /// Regulatory filings for a symbol, or list/fetch filing content
    ///
    /// Without subcommand: lists filings for a symbol.
    /// Subcommands: list  detail
    /// Example: longbridge filing AAPL.US
    /// Example: longbridge filing list AAPL.US
    /// Example: longbridge filing detail AAPL.US 580265529766123777
    Filing {
        /// Symbol in <CODE>.<MARKET> format (e.g. AAPL.US 700.HK). Omit when using a subcommand.
        symbol: Option<String>,
        /// Maximum number of filings to show (default: 20)
        #[arg(long, alias = "limit", default_value = "20")]
        count: usize,
        #[command(subcommand)]
        cmd: Option<FilingCmd>,
    },

    /// Community discussion topics
    ///
    /// Without subcommand: lists topics for a symbol.
    /// Subcommands: list  detail  mine  create  replies  create-reply
    /// Example: longbridge topic TSLA.US
    /// Example: longbridge topic list TSLA.US
    /// Example: longbridge topic detail 6993508780031016960
    /// Example: longbridge topic create --body "Bullish on TSLA today"
    Topic {
        /// Symbol in <CODE>.<MARKET> format (e.g. TSLA.US 700.HK). Omit when using a subcommand.
        symbol: Option<String>,
        /// Maximum number of topics to show (default: 20)
        #[arg(long, alias = "limit", default_value = "20")]
        count: usize,
        #[command(subcommand)]
        cmd: Option<TopicCmd>,
    },

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

    // ── Statement ──────────────────────────────────────────────────────────────
    /// Download and export account statements (daily/monthly)
    ///
    /// Without a subcommand, lists available statements (equivalent to `statement list`).
    /// Example: longbridge statement
    /// Example: longbridge statement --type monthly
    /// Example: longbridge statement export --file-key KEY --section `equity_holdings`
    Statement {
        /// Statement type: daily (default) | monthly
        #[arg(long = "type", default_value = "daily")]
        statement_type: String,
        /// Start date (YYYY-MM-DD, e.g. 2026-01-21). Defaults to 30 days ago.
        #[arg(long)]
        start_date: Option<String>,
        /// Number of records to return. Defaults to 30 for daily, 12 for monthly.
        #[arg(long)]
        limit: Option<i32>,
        #[command(subcommand)]
        cmd: Option<StatementCmd>,
    },

    // ── Trade ───────────────────────────────────────────────────────────────────
    /// Order management: list, detail, buy, sell, cancel, replace, executions
    ///
    /// Without a subcommand, lists today's orders (or historical with --history).
    /// Example: longbridge order
    /// Example: longbridge order --history --start 2024-01-01 --symbol TSLA.US
    /// Example: longbridge order detail 20240101-123456789
    /// Example: longbridge order buy TSLA.US 100 --price 250.00
    /// Example: longbridge order sell TSLA.US 100 --price 260.00
    /// Example: longbridge order cancel 20240101-123456789
    /// Example: longbridge order replace 20240101-123456789 --qty 200 --price 255.00
    /// Example: longbridge order executions --history --start 2024-01-01
    Order {
        /// Return historical orders instead of today's (list mode only)
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
        #[command(subcommand)]
        cmd: Option<OrderCmd>,
    },

    /// Account asset overview — net assets, cash, buy power, margins, and per-currency breakdown
    ///
    /// Returns: currency, `net_assets`, `total_cash`, `buy_power`, `max_finance_amount`,
    /// `remaining_finance_amount`, `init_margin`, `maintenance_margin`, `margin_call`, `risk_level`,
    /// and a `cash_infos` array with per-currency available/frozen/settling/withdrawable amounts.
    /// Example: longbridge assets
    /// Example: longbridge assets --currency HKD
    Assets {
        /// Filter by currency (e.g. USD HKD CNY SGD)
        #[arg(long, default_value = "USD")]
        currency: Option<String>,
    },

    /// Cash flow records (deposits, withdrawals, dividends, settlements)
    ///
    /// Returns: `flow_name`, symbol, `business_type`, balance, currency, `business_time`, description.
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

    /// Portfolio overview — total assets, P/L, intraday P/L, holdings, and cash breakdown
    ///
    /// Fetches live quotes, FX rates, and account balance concurrently, then
    /// computes all P/L figures in USD.
    ///
    /// Returns: overview (`total_asset`, `market_cap`, `total_cash`, `total_pl`, `total_today_pl`,
    /// `margin_call`, `risk_level`, `credit_limit`, currency), holdings table, and cash balances.
    ///
    /// Example: longbridge portfolio
    /// Example: longbridge portfolio --format json
    Portfolio,

    /// Current stock (equity) positions across all sub-accounts
    ///
    /// Returns: symbol, name, quantity, `available_quantity`, `cost_price`, currency, market.
    /// Example: longbridge positions --format json
    Positions,

    /// Current fund (mutual fund) positions across all sub-accounts
    ///
    /// Returns: symbol, name, `current_net_asset_value`, `cost_net_asset_value`, currency, `holding_units`.
    FundPositions,

    /// Margin ratio requirements for a symbol
    ///
    /// Returns: `im_factor` (initial), `mm_factor` (maintenance), `fm_factor` (forced liquidation).
    /// Example: longbridge margin-ratio TSLA.US
    MarginRatio {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
    },

    /// Estimate maximum buy or sell quantity given current account balance
    ///
    /// Returns: `cash_max_qty` (cash only), `margin_max_qty` (with margin financing).
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

    /// Exchange rates for all supported currencies
    ///
    /// Example: longbridge exchange-rate
    /// Example: longbridge exchange-rate --format json
    ExchangeRate,

    /// Institutional shareholders for a symbol
    ///
    /// Returns: shareholder name, related symbol (if listed), % shares held, share change, report date.
    /// Example: longbridge shareholder AAPL.US
    /// Example: longbridge shareholder AAPL.US --range inc --sort owned
    /// Example: longbridge shareholder AAPL.US --format json
    Shareholder {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Filter by change direction: all | inc (increase) | dec (decrease)
        #[arg(long, default_value = "all")]
        range: String,
        /// Sort field: chg (change) | owned (holdings) | time (report date)
        #[arg(long, default_value = "chg")]
        sort: String,
        /// Sort order: desc | asc
        #[arg(long, default_value = "desc")]
        order: String,
    },

    // ── Pending Commands ──────────────────────────────────────────────────────
    /// Company overview (founding date, employees, IPO price, address, etc.)
    ///
    /// Example: longbridge company AAPL.US
    Company {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
    },

    /// Company executives and key personnel
    ///
    /// Example: longbridge executive AAPL.US
    Executive {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
    },

    /// Industry valuation comparison and distribution
    ///
    /// Default: comparison table with peers.
    /// Use `dist` subcommand for percentile distribution.
    /// Example: longbridge industry-valuation AAPL.US
    /// Example: longbridge industry-valuation dist AAPL.US
    /// Example: longbridge industry-valuation AAPL.US --currency USD
    IndustryValuation {
        /// Symbol in <CODE>.<MARKET> format (omit when using subcommand)
        symbol: Option<String>,
        /// Currency: USD | HKD | CNY | SGD
        #[arg(long, default_value = "USD")]
        currency: String,
        #[command(subcommand)]
        cmd: Option<IndustryValuationCmd>,
    },

    /// Operating reviews and financial indicators by report period
    ///
    /// Example: longbridge operating AAPL.US
    /// Example: longbridge operating AAPL.US --report q1
    Operating {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Report kind filter: af | saf | q1 | q3 (comma-separated for multiple)
        #[arg(long)]
        report: Option<String>,
    },

    /// Corporate actions (splits, dividends, rights, etc.)
    ///
    /// Example: longbridge corp-action 700.HK
    /// Example: longbridge corp-action 700.HK --all
    CorpAction {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Show all records instead of the default 30
        #[arg(long)]
        all: bool,
    },

    /// Investment relations (subsidiary/parent companies)
    ///
    /// Example: longbridge invest-relation 700.HK
    InvestRelation {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
    },

    /// Index or ETF constituent stocks
    ///
    /// Example: longbridge constituent HSI.HK
    /// Example: longbridge constituent HSI.HK --limit 20 --sort change
    Constituent {
        /// Index symbol in <CODE>.<MARKET> format (e.g. HSI.HK, DJI.US)
        symbol: String,
        /// Number of results to return
        #[arg(long, default_value = "50")]
        limit: i32,
        /// Sort indicator: change, price, turnover, inflow, turnover-rate, market-cap
        #[arg(long, default_value = "change")]
        sort: String,
        /// Sort order: desc | asc
        #[arg(long, default_value = "desc")]
        order: String,
    },

    /// Market open/close status for each exchange
    ///
    /// Example: longbridge market-status
    MarketStatus,

    /// Broker holding positions (HK market only)
    ///
    /// Currently only supports HK-listed stocks. US and other markets are not available.
    /// Example: longbridge broker-holding 700.HK
    /// Example: longbridge broker-holding detail 700.HK
    /// Example: longbridge broker-holding daily 700.HK --broker B01224
    BrokerHolding {
        /// Symbol in <CODE>.<MARKET> format (omit when using subcommand)
        symbol: Option<String>,
        /// Period for top buy/sell: `rct_1`, `rct_5`, `rct_20`, `rct_60`
        #[arg(long, default_value = "rct_1")]
        period: String,
        #[command(subcommand)]
        cmd: Option<BrokerHoldingCmd>,
    },

    /// A/H premium ratio for dual-listed stocks (kline or intraday)
    ///
    /// Only works for HK stocks that are also listed on A-share markets (e.g. 939.HK, 1398.HK).
    /// If the API returns no data, the stock is not dual-listed in A-shares.
    /// Example: longbridge ah-premium 939.HK
    /// Example: longbridge ah-premium intraday 939.HK
    /// Example: longbridge ah-premium 939.HK --kline-type day --count 100
    AhPremium {
        /// Symbol in <CODE>.<MARKET> format (omit when using subcommand)
        symbol: Option<String>,
        /// K-line type: 1m | 5m | 15m | 30m | 60m | day | week | month | year
        #[arg(long, default_value = "day")]
        kline_type: String,
        /// Number of K-lines to return
        #[arg(long, alias = "limit", default_value = "100")]
        count: i32,
        #[command(subcommand)]
        cmd: Option<AhPremiumCmd>,
    },

    /// Trade statistics (price distribution by volume)
    ///
    /// Example: longbridge trade-stats 700.HK
    TradeStats {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
    },

    /// Quote anomalies / unusual market movements
    ///
    /// Example: longbridge anomaly --market HK
    /// Example: longbridge anomaly --market US --symbol TSLA.US
    Anomaly {
        /// Market: HK | US | CN | SG
        #[arg(long, default_value = "HK")]
        market: String,
        /// Filter to a specific symbol
        #[arg(long)]
        symbol: Option<String>,
        /// Number of results (max 100)
        #[arg(long, alias = "limit", default_value = "50")]
        count: i32,
    },

    /// Price alerts (list, add, delete)
    ///
    /// Without subcommand: lists all alerts.
    /// Example: longbridge alert
    /// Example: longbridge alert QQQ.US
    /// Example: longbridge alert add TSLA.US --price 200 --direction rise
    /// Example: longbridge alert delete 486469
    Alert {
        /// Filter by symbol (omit to list all)
        symbol: Option<String>,
        #[command(subcommand)]
        cmd: Option<AlertCmd>,
    },

    /// Profit & loss analysis
    ///
    /// Without subcommand: shows full account P&L summary (stocks + funds + MMF)
    /// including simple yield and time-weighted return (TWR).
    /// Subcommands: detail  by-market
    /// Example: longbridge profit-analysis
    /// Example: longbridge profit-analysis --start 2026-01-01 --end 2026-04-16
    /// Example: longbridge profit-analysis detail 700.HK
    /// Example: longbridge profit-analysis by-market --market HK
    ProfitAnalysis {
        /// Start date (YYYY-MM-DD)
        #[arg(long)]
        start: Option<String>,
        /// End date (YYYY-MM-DD)
        #[arg(long)]
        end: Option<String>,
        #[command(subcommand)]
        cmd: Option<ProfitAnalysisCmd>,
    },

    /// Funds and ETFs that hold a given symbol
    ///
    /// Returns: fund name, ticker, currency, weight (position ratio), and report date.
    /// Pass --count -1 to return all holders.
    /// Example: longbridge fund-holder AAPL.US
    /// Example: longbridge fund-holder AAPL.US --count 20
    /// Example: longbridge fund-holder AAPL.US --format json
    FundHolder {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Number of results to return (-1 for all)
        #[arg(long, alias = "limit", default_value = "20")]
        count: i32,
    },

    // ── SEC Insider Trades ────────────────────────────────────────────────────
    /// SEC Form 4 insider trades for a US-listed company
    ///
    /// Shows non-derivative transactions (direct stock buys, sells, grants, etc.)
    /// filed by corporate insiders (officers, directors, 10% owners) with the SEC.
    /// Only US-listed equities are supported (data source: SEC EDGAR Form 4).
    ///
    /// Transaction types: BUY (P) | SELL (S) | GRANT (A) | DISP (D) |
    ///   TAX (F) | EXERCISE (M/X) | GIFT (G)
    ///
    /// Example: longbridge insider-trades TSLA.US
    /// Example: longbridge insider-trades AAPL.US --count 40
    /// Example: longbridge insider-trades NVDA.US --format json
    InsiderTrades {
        /// Symbol in <CODE>.<MARKET> format (US market only, e.g. TSLA.US AAPL.US)
        symbol: String,
        /// Number of Form 4 filings to fetch (default: 20)
        #[arg(long, alias = "limit", default_value = "20")]
        count: usize,
    },

    // ── SEC Investors ──────────────────────────────────────────────────────────
    /// View SEC 13F portfolio holdings for institutional investors
    ///
    /// Without arguments: shows live top-50 active fund manager rankings by AUM.
    /// With a CIK: fetches the latest 13F holdings snapshot.
    /// With subcommand 'changes': shows quarter-over-quarter position changes.
    ///
    /// Example: longbridge investors
    /// Example: longbridge investors 0001067983
    /// Example: longbridge investors 1067983 --top 20
    /// Example: longbridge investors changes 1067983
    Investors {
        /// Numeric CIK from SEC EDGAR (omit to see AUM rankings).
        /// Run `longbridge investors` to see the rankings table with CIK column.
        /// Example: 0001067983 or 1067983
        cik: Option<String>,
        /// Number of top holdings to display, sorted by value (default: 50)
        #[arg(long, default_value = "50")]
        top: usize,
        #[command(subcommand)]
        subcmd: Option<InvestorsSubCmd>,
    },

    // ── Recurring Investment ──────────────────────────────────────────────────────
    /// Recurring Investment: automatically invest a fixed amount at regular intervals
    ///
    /// Create and manage recurring investment plans that execute stock purchases on a daily, weekly,
    /// fortnightly, or monthly schedule. Track trade history, monitor cumulative profit,
    /// and check upcoming trade dates.
    ///
    /// Without a subcommand, lists all recurring investment plans.
    /// Example: longbridge dca
    /// Example: longbridge dca --status Active
    /// Example: longbridge dca --symbol TSLA.US
    /// Example: longbridge dca create AAPL.US --amount 500 --frequency monthly --day-of-month 15
    /// Example: longbridge dca create 700.HK --amount 1000 --frequency weekly --day-of-week mon
    /// Example: longbridge dca pause `<PLAN_ID>`
    /// Example: longbridge dca stop `<PLAN_ID>`
    /// Example: longbridge dca history `<PLAN_ID>`
    /// Example: longbridge dca stats
    /// Example: longbridge dca check AAPL.US 700.HK
    #[command(name = "dca")]
    Dca {
        #[command(subcommand)]
        cmd: Option<DcaCmd>,
        /// Filter plans by status: Active | Suspended | Finished
        #[arg(long)]
        status: Option<String>,
        /// Filter plans by symbol (e.g. AAPL.US)
        #[arg(long)]
        symbol: Option<String>,
        /// Page number (default: 1)
        #[arg(long, default_value = "1")]
        page: u32,
        /// Records per page (default: 20)
        #[arg(long, default_value = "20")]
        limit: u32,
    },

    // ── Short positions ─────────────────────────────────────────────────
    /// US stock short selling data (short interest, short ratio, days to cover)
    ///
    /// Only supports US-listed stocks and ETFs.
    /// Example: longbridge short-positions AAPL.US
    /// Example: longbridge short-positions TSLA.US --count 50
    ShortPositions {
        /// Symbol in <CODE>.<MARKET> format (US market only, e.g. AAPL.US)
        symbol: String,
        /// Number of records to return (1–100, default: 20)
        #[arg(long, alias = "limit", default_value = "20")]
        count: u32,
    },

    // ── Sharelist ───────────────────────────────────────────────────────
    /// Sharelist: community stock lists — list, detail, create, delete, and manage stocks
    ///
    /// Without a subcommand, lists the current user's own and subscribed sharelists.
    /// Without a subcommand, lists own and subscribed sharelists.
    /// Example: longbridge sharelist
    /// Example: longbridge sharelist --count 50
    /// Example: longbridge sharelist detail `<ID>`
    /// Example: longbridge sharelist create --name "My Picks"
    /// Example: longbridge sharelist delete `<ID>`
    /// Example: longbridge sharelist add `<ID>` TSLA.US AAPL.US
    /// Example: longbridge sharelist remove `<ID>` TSLA.US
    /// Example: longbridge sharelist sort `<ID>` TSLA.US AAPL.US 700.HK
    /// Example: longbridge sharelist popular --count 10
    Sharelist {
        #[command(subcommand)]
        cmd: Option<SharelistCmd>,
        /// Number of sharelists to return (default: 20)
        #[arg(long, alias = "limit", default_value = "20")]
        count: u32,
    },

    // ── Quant ────────────────────────────────────────────────────────────────
    /// Quantitative analysis: run indicator scripts against K-line data
    ///
    /// Subcommands: run
    /// Example: longbridge quant run TSLA.US --start 2024-01-01 --end 2024-12-31 --script "..."
    /// Example: cat script.pine | longbridge quant run TSLA.US --start 2024-01-01 --end 2024-12-31
    Quant {
        #[command(subcommand)]
        cmd: QuantCmd,
    },
}

#[derive(Subcommand)]
pub enum QuantCmd {
    /// Run a quant indicator script against historical K-line data on the server
    ///
    /// Executes the script server-side and returns the computed indicator/plot values as JSON.
    /// The script language is compatible with `PineScript` V6 syntax (minor exceptions may apply).
    ///
    /// Periods: 1m  5m  15m  30m  1h  day  week  month  year
    ///
    /// Script source (--script takes priority over stdin):
    ///   --script TEXT   inline script text
    ///   stdin           cat script.pine | longbridge quant run TSLA.US ...
    ///
    /// The optional --input flag accepts a JSON array matching the
    /// order of input.*() calls in the script, e.g. --input '[14,2.0]'
    ///
    /// Example: longbridge quant run TSLA.US --start 2024-01-01 --end 2024-12-31 --script "..."
    /// Example: cat script.pine | longbridge quant run TSLA.US --start 2024-01-01 --end 2024-12-31
    /// Example: longbridge quant run 700.HK --period 1h --start 2024-01-01 --end 2024-06-30 --script "..." --input '[14]'
    /// Example: longbridge quant run TSLA.US --start 2024-01-01 --end 2024-12-31 --script "..." --format json
    /// Example: longbridge quant run 700.HK --period 1m --start "2024-01-02 09:30" --end "2024-01-02 16:00" --script "..."
    Run {
        /// Symbol in <CODE>.<MARKET> format, e.g. TSLA.US 700.HK
        symbol: String,
        /// K-line period: 1m 5m 15m 30m 1h day week month year (default: day)
        #[arg(long, default_value = "day")]
        period: String,
        /// Start date/datetime for the K-line range (YYYY-MM-DD or "YYYY-MM-DD HH:MM")
        #[arg(long)]
        start: String,
        /// End date/datetime for the K-line range (YYYY-MM-DD or "YYYY-MM-DD HH:MM")
        #[arg(long)]
        end: String,
        /// Script text. Omit to read from stdin (e.g. echo "..." | longbridge quant run ...)
        #[arg(long)]
        script: Option<String>,
        /// Script input values as a JSON array, e.g. '[14,2.0]'
        /// Must match the order of input.*() calls in the script.
        #[arg(long)]
        input: Option<String>,
    },
}

#[derive(Args, Debug)]
pub struct FinanceCalendarOpts {
    /// Filter by symbol, repeatable (max 10)
    #[arg(long, value_name = "SYMBOL")]
    pub symbol: Vec<String>,
    /// Filter by symbol group: watchlist or positions (omit for all)
    #[arg(long, value_name = "FILTER")]
    pub filter: Option<String>,
    /// Filter by market: HK, US, CN, SG, JP, UK, DE, AU (omit for all)
    #[arg(long, value_name = "MARKET")]
    pub market: Option<String>,
    /// Start date (YYYY-MM-DD)
    #[arg(long)]
    pub start: Option<String>,
    /// End date (YYYY-MM-DD)
    #[arg(long)]
    pub end: Option<String>,
    /// Max events returned (default: 100)
    #[arg(long, alias = "limit", default_value = "100")]
    pub count: u32,
}

#[derive(Subcommand, Debug)]
pub enum FinanceCalendarCmd {
    /// Earnings reports (upcoming and recently announced)
    ///
    /// Example: longbridge finance-calendar report
    /// Example: longbridge finance-calendar report --symbol AAPL.US --filter watchlist --market US
    Report {
        #[command(flatten)]
        opts: FinanceCalendarOpts,
    },
    /// Dividend announcements
    ///
    /// Example: longbridge finance-calendar dividend
    /// Example: longbridge finance-calendar dividend --filter positions
    Dividend {
        #[command(flatten)]
        opts: FinanceCalendarOpts,
    },
    /// Stock splits and merges
    ///
    /// Example: longbridge finance-calendar split
    /// Example: longbridge finance-calendar split --market HK
    Split {
        #[command(flatten)]
        opts: FinanceCalendarOpts,
    },
    /// IPO listings
    ///
    /// Example: longbridge finance-calendar ipo
    /// Example: longbridge finance-calendar ipo --market HK
    Ipo {
        #[command(flatten)]
        opts: FinanceCalendarOpts,
    },
    /// Macro economic data releases
    ///
    /// Example: longbridge finance-calendar macrodata
    /// Example: longbridge finance-calendar macrodata --star 3
    Macrodata {
        #[command(flatten)]
        opts: FinanceCalendarOpts,
        /// Importance filter, repeatable: 1, 2, or 3 stars (omit for all)
        #[arg(long, value_name = "LEVEL")]
        star: Vec<u32>,
    },
    /// Market closure days
    ///
    /// Example: longbridge finance-calendar closed
    /// Example: longbridge finance-calendar closed --market HK
    Closed {
        #[command(flatten)]
        opts: FinanceCalendarOpts,
    },
}

#[derive(Subcommand)]
pub enum InvestorsSubCmd {
    /// Show position changes between two 13F filings (NEW/ADDED/REDUCED/EXITED)
    ///
    /// By default compares the latest filing against the previous one.
    /// Use --from to compare against a specific period (e.g. 2024-12-31).
    ///
    /// Example: longbridge investors changes 1067983
    /// Example: longbridge investors changes 1067983 --from 2024-12-31
    /// Example: longbridge investors changes 1067983 --top 20
    Changes {
        /// Numeric CIK from SEC EDGAR
        cik: String,
        /// Number of changes to display (default: 50)
        #[arg(long, default_value = "50")]
        top: usize,
        /// Base period to compare against (report date, e.g. 2024-12-31).
        /// Defaults to the filing immediately before the latest one.
        #[arg(long, value_name = "PERIOD")]
        from: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum DcaCmd {
    /// Create a new recurring investment plan
    ///
    /// Frequency: daily | weekly | fortnightly (every two weeks) | monthly
    /// Day of week (weekly/fortnightly): mon tue wed thu fri
    /// Day of month (monthly): 1–28
    /// Example: longbridge dca create AAPL.US --amount 500 --frequency monthly --day-of-month 15
    /// Example: longbridge dca create 700.HK --amount 1000 --frequency weekly --day-of-week mon
    Create {
        /// Symbol in <CODE>.<MARKET> format (e.g. AAPL.US 700.HK)
        symbol: String,
        /// Amount per investment period (as a decimal string, e.g. 500)
        #[arg(long)]
        amount: String,
        /// Investment frequency: daily | weekly | fortnightly (every two weeks) | monthly
        #[arg(long)]
        frequency: DcaFrequency,
        /// Day of week for weekly/fortnightly: mon tue wed thu fri
        #[arg(long)]
        day_of_week: Option<DcaDayOfWeek>,
        /// Day of month for monthly plans (1–28)
        #[arg(long)]
        day_of_month: Option<String>,
        /// Allow margin financing for the investment amount (default: false)
        #[arg(long)]
        allow_margin: bool,
        /// Agree to the Terms and Conditions without interactive prompt
        #[arg(long)]
        agree_terms: bool,
    },

    /// Update an existing recurring investment plan
    ///
    /// Only the fields provided will be updated.
    /// Example: longbridge dca update `<PLAN_ID>` --amount 800
    /// Example: longbridge dca update `<PLAN_ID>` --frequency weekly --day-of-week fri
    Update {
        /// Plan ID (from `longbridge dca`)
        plan_id: String,
        /// New amount per investment period
        #[arg(long)]
        amount: Option<String>,
        /// New investment frequency: daily | weekly | fortnightly (every two weeks) | monthly
        #[arg(long)]
        frequency: Option<DcaFrequency>,
        /// Day of week for weekly/fortnightly: mon tue wed thu fri
        #[arg(long)]
        day_of_week: Option<DcaDayOfWeek>,
        /// Day of month for monthly plans (1–28)
        #[arg(long)]
        day_of_month: Option<String>,
        /// Allow margin financing
        #[arg(long)]
        allow_margin: Option<bool>,
    },

    /// Pause a recurring investment plan
    ///
    /// Example: longbridge dca pause `<PLAN_ID>`
    Pause {
        /// Plan ID to suspend
        plan_id: String,
    },

    /// Resume a paused recurring investment plan
    ///
    /// Example: longbridge dca resume `<PLAN_ID>`
    Resume {
        /// Plan ID to resume
        plan_id: String,
    },

    /// Permanently stop a recurring investment plan
    ///
    /// Example: longbridge dca stop `<PLAN_ID>`
    Stop {
        /// Plan ID to terminate
        plan_id: String,
    },

    /// Show trade history for a recurring investment plan
    ///
    /// Example: longbridge dca history `<PLAN_ID>`
    /// Example: longbridge dca history `<PLAN_ID>` --page 2 --limit 50
    History {
        /// Plan ID (from `longbridge dca`)
        plan_id: String,
        /// Page number (default: 1)
        #[arg(long, default_value = "1")]
        page: u32,
        /// Records per page (default: 20)
        #[arg(long, default_value = "20")]
        limit: u32,
    },

    /// Show recurring investment statistics summary
    ///
    /// Returns total invested amount, total profit, plan counts, and nearest upcoming plans.
    /// Example: longbridge dca stats
    /// Example: longbridge dca stats --symbol AAPL.US
    Stats {
        /// Filter statistics by symbol
        #[arg(long)]
        symbol: Option<String>,
    },

    /// Calculate the next trade date for given plan parameters
    ///
    /// Example: longbridge dca calc-date AAPL.US --frequency monthly --day-of-month 15
    /// Example: longbridge dca calc-date 700.HK --frequency weekly --day-of-week mon
    #[command(name = "calc-date")]
    CalcDate {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Investment frequency: daily | weekly | fortnightly | monthly
        #[arg(long)]
        frequency: DcaFrequency,
        /// Day of week for weekly/fortnightly: mon tue wed thu fri
        #[arg(long)]
        day_of_week: Option<DcaDayOfWeek>,
        /// Day of month for monthly plans (1–28)
        #[arg(long)]
        day_of_month: Option<String>,
    },

    /// Check whether symbols support recurring investment
    ///
    /// Example: longbridge dca check AAPL.US 700.HK TSLA.US
    Check {
        /// One or more symbols in <CODE>.<MARKET> format
        symbols: Vec<String>,
    },

    /// Set the pre-trade reminder hours
    ///
    /// Valid values: 1 | 6 | 12
    /// Example: longbridge dca set-reminder 6
    #[command(name = "set-reminder")]
    SetReminder {
        /// Hours before trade to send reminder: 1 | 6 | 12
        hours: DcaReminderHours,
    },
}

#[derive(Subcommand)]
pub enum IndustryValuationCmd {
    /// Industry valuation distribution (percentile ranking)
    ///
    /// Example: longbridge industry-valuation dist AAPL.US
    Dist {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
    },
}

#[derive(Subcommand)]
pub enum BrokerHoldingCmd {
    /// Full broker holding detail list
    ///
    /// Example: longbridge broker-holding detail 700.HK
    Detail {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
    },
    /// Daily holding history for a specific broker
    ///
    /// The --broker value is the `parti_no` shown in the top/detail tables.
    /// Example: longbridge broker-holding daily 700.HK --broker B01224
    Daily {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Broker participant number from the `parti_no` column (e.g. B01224)
        #[arg(long, value_name = "PARTI_NO")]
        broker: String,
    },
}

#[derive(Subcommand)]
pub enum AhPremiumCmd {
    /// AH premium intraday timeshare data
    ///
    /// Example: longbridge ah-premium intraday 939.HK
    Intraday {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
    },
}

#[derive(Subcommand)]
pub enum ProfitAnalysisCmd {
    /// Individual stock P&L detail with transaction flows
    ///
    /// Example: longbridge profit-analysis detail 700.HK
    /// Example: longbridge profit-analysis detail 700.HK --start 2025-01-01 --end 2025-12-31
    /// Example: longbridge profit-analysis detail 700.HK --derivative
    Detail {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Start date (YYYY-MM-DD)
        #[arg(long)]
        start: Option<String>,
        /// End date (YYYY-MM-DD)
        #[arg(long)]
        end: Option<String>,
        /// Currency filter (e.g. HKD, USD, CNH)
        #[arg(long)]
        currency: Option<String>,
        /// Show derivative flows instead of underlying
        #[arg(long)]
        derivative: bool,
        /// Flows page number (default: 1)
        #[arg(long, default_value = "1")]
        page: u32,
        /// Flows page size (default: 30)
        #[arg(long, default_value = "30")]
        size: u32,
    },

    /// Stock P&L by market with pagination
    ///
    /// Example: longbridge profit-analysis by-market
    /// Example: longbridge profit-analysis by-market HK
    /// Example: longbridge profit-analysis by-market US --page 1 --size 50
    ByMarket {
        /// Market filter (e.g. HK, US, SH, SZ)
        market: Option<String>,
        /// Start date (YYYY-MM-DD)
        #[arg(long)]
        start: Option<String>,
        /// End date (YYYY-MM-DD)
        #[arg(long)]
        end: Option<String>,
        /// Currency filter (e.g. HKD, USD, CNH)
        #[arg(long)]
        currency: Option<String>,
        /// Page number (default: 1)
        #[arg(long, default_value = "1")]
        page: u32,
        /// Page size (default: 50)
        #[arg(long, default_value = "50")]
        size: u32,
    },
}

#[derive(Subcommand)]
pub enum AlertCmd {
    /// Add a price alert
    ///
    /// Example: longbridge alert add TSLA.US --price 200 --direction rise
    Add {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Target price or percentage value
        #[arg(long)]
        price: String,
        /// Direction: rise | fall
        #[arg(long, default_value = "rise")]
        direction: String,
        /// Alert type: price | percent
        #[arg(long, default_value = "price")]
        alert_type: String,
        /// Frequency: once | daily | every
        #[arg(long, default_value = "once")]
        frequency: String,
        /// Optional note
        #[arg(long)]
        note: Option<String>,
    },
    /// Delete a price alert by id (from `longbridge alert` list)
    ///
    /// Example: longbridge alert delete 486469
    Delete {
        /// Alert id from the `id` column in `longbridge alert`
        id: String,
    },
    /// Enable a price alert by id
    ///
    /// Example: longbridge alert enable 486469
    Enable {
        /// Alert id from the `id` column in `longbridge alert`
        id: String,
    },
    /// Disable a price alert by id
    ///
    /// Example: longbridge alert disable 486469
    Disable {
        /// Alert id from the `id` column in `longbridge alert`
        id: String,
    },
}

#[derive(Subcommand)]
pub enum DividendCmd {
    /// Dividend distribution scheme details
    ///
    /// Example: longbridge dividend detail AAPL.US
    Detail {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
    },
}

#[derive(Subcommand)]
pub enum InstitutionRatingCmd {
    /// Historical institution rating and target price detail
    ///
    /// Example: longbridge institution-rating detail TSLA.US
    Detail {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
    },
}

#[derive(Subcommand)]
pub enum WatchlistCmd {
    /// Show securities in a specific watchlist group (by ID or name)
    ///
    /// Example: longbridge watchlist show 123
    /// Example: longbridge watchlist show "Tech Stocks"
    Show {
        /// Group ID (numeric) or group name (string)
        group: String,
    },

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
        /// Skip confirmation prompt (useful for scripting and AI agents)
        #[arg(long, short = 'y')]
        yes: bool,
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

    /// Pin or unpin securities so they appear at the top of a watchlist group
    ///
    /// Example: longbridge watchlist pin TSLA.US AAPL.US
    /// Example: longbridge watchlist pin --remove 700.HK
    Pin {
        /// Symbols to pin (positional; omit to use --remove)
        securities: Vec<String>,
        /// Symbols to unpin (repeatable: --remove 700.HK)
        #[arg(long)]
        remove: Vec<String>,
    },
}

#[derive(ValueEnum, Clone, Debug)]
pub enum StatementSection {
    #[value(name = "asset")]
    Asset,
    #[value(name = "account_balances")]
    AccountBalanceSum,
    #[value(name = "equity_holdings")]
    EquityHoldingSums,
    #[value(name = "account_balance_changes")]
    AccountBalanceChangeSums,
    #[value(name = "stock_trades")]
    StockTradeSums,
    #[value(name = "equity_holding_changes")]
    EquityHoldingChangeSums,
    #[value(name = "account_balance_locks")]
    AccountBalanceLockSums,
    #[value(name = "equity_holding_locks")]
    EquityHoldingLockSums,
    #[value(name = "option_trades")]
    OptionTradeSums,
    #[value(name = "fund_trades")]
    FundTradeSums,
    #[value(name = "ipo_trades")]
    IpoTradeSums,
    #[value(name = "virtual_trades")]
    VirtualTradeSums,
    #[value(name = "interests")]
    Interests,
    #[value(name = "lending_fees")]
    LendingFees,
    #[value(name = "custodian_fees")]
    CustodianFees,
    #[value(name = "corps")]
    Corps,
    #[value(name = "bond_equity_holdings")]
    BondEquityHoldingSums,
    #[value(name = "otc_trades")]
    OtcTradeSums,
    #[value(name = "outstandings")]
    OutstandingSums,
    #[value(name = "financing_transactions")]
    FinancingTransactionSums,
    #[value(name = "interest_deposits")]
    InterestDeposits,
    #[value(name = "maintenance_fees")]
    MaintenanceFees,
    #[value(name = "cash_pluses")]
    CashPluses,
    #[value(name = "gst_details")]
    GstDetails,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum DcaFrequency {
    #[value(name = "daily")]
    Daily,
    #[value(name = "weekly")]
    Weekly,
    #[value(name = "fortnightly")]
    Fortnightly,
    #[value(name = "monthly")]
    Monthly,
}

impl DcaFrequency {
    pub fn as_api_str(&self) -> &'static str {
        match self {
            Self::Daily => "Daily",
            Self::Weekly => "Weekly",
            Self::Fortnightly => "Fortnightly",
            Self::Monthly => "Monthly",
        }
    }
}

#[derive(ValueEnum, Clone, Debug)]
pub enum DcaDayOfWeek {
    #[value(name = "mon")]
    Mon,
    #[value(name = "tue")]
    Tue,
    #[value(name = "wed")]
    Wed,
    #[value(name = "thu")]
    Thu,
    #[value(name = "fri")]
    Fri,
}

impl DcaDayOfWeek {
    pub fn as_api_str(&self) -> &'static str {
        match self {
            Self::Mon => "Mon",
            Self::Tue => "Tue",
            Self::Wed => "Wed",
            Self::Thu => "Thu",
            Self::Fri => "Fri",
        }
    }
}

#[derive(ValueEnum, Clone, Debug)]
pub enum DcaReminderHours {
    #[value(name = "1")]
    One,
    #[value(name = "6")]
    Six,
    #[value(name = "12")]
    Twelve,
}

impl DcaReminderHours {
    pub fn as_api_str(&self) -> &'static str {
        match self {
            Self::One => "1",
            Self::Six => "6",
            Self::Twelve => "12",
        }
    }
}

#[derive(Subcommand)]
pub enum SharelistCmd {
    /// Show full details for a sharelist including its constituent stocks
    ///
    /// Example: longbridge sharelist detail `<ID>`
    Detail {
        /// Sharelist ID
        id: String,
    },

    /// Create a new sharelist
    ///
    /// Example: longbridge sharelist create --name "My Tech Picks"
    Create {
        /// Sharelist name
        #[arg(long)]
        name: String,
        /// Sharelist description
        #[arg(long, default_value = "")]
        description: String,
    },

    /// Delete a sharelist
    ///
    /// Example: longbridge sharelist delete `<ID>`
    Delete {
        /// Sharelist ID to delete
        id: String,
    },

    /// Add stocks to a sharelist
    ///
    /// Example: longbridge sharelist add `<ID>` TSLA.US AAPL.US 700.HK
    Add {
        /// Sharelist ID
        id: String,
        /// Symbols to add (e.g. TSLA.US AAPL.US 700.HK)
        symbols: Vec<String>,
    },

    /// Remove stocks from a sharelist
    ///
    /// Example: longbridge sharelist remove `<ID>` TSLA.US AAPL.US
    Remove {
        /// Sharelist ID
        id: String,
        /// Symbols to remove (e.g. TSLA.US AAPL.US)
        symbols: Vec<String>,
    },

    /// Reorder the stocks in a sharelist
    ///
    /// Pass all symbol in the desired order; the full list replaces the existing order.
    /// Example: longbridge sharelist sort `<ID>` TSLA.US AAPL.US 700.HK
    Sort {
        /// Sharelist ID
        id: String,
        /// Symbols in the desired order (e.g. TSLA.US AAPL.US 700.HK)
        symbols: Vec<String>,
    },

    /// Get popular (trending) sharelists
    ///
    /// Example: longbridge sharelist popular
    /// Example: longbridge sharelist popular --count 10
    Popular {
        /// Number of results to return (default: 20)
        #[arg(long, alias = "limit", default_value = "20")]
        count: u32,
    },
}

#[derive(ValueEnum, Clone, Debug)]
pub enum ExportFormat {
    #[value(name = "csv")]
    Csv,
    #[value(name = "md")]
    Md,
}

#[derive(Subcommand)]
pub enum StatementCmd {
    /// List available statements for an account
    ///
    /// Returns: date (dt), `file_key` for each statement.
    /// Example: longbridge statement list --aaid 12345
    /// Example: longbridge statement list --aaid 12345 --type monthly
    List {
        /// Statement type: daily (default) | monthly
        #[arg(long = "type", default_value = "daily")]
        statement_type: String,
        /// Start date (YYYY-MM-DD, e.g. 2026-01-21). Defaults to 30 days ago.
        #[arg(long)]
        start_date: Option<String>,
        /// Number of records to return. Defaults to 30 for daily, 12 for monthly.
        #[arg(long)]
        limit: Option<i32>,
    },

    /// Export statement sections as CSV files or markdown
    ///
    /// Fetches the statement JSON by `file_key`, extracts the specified sections,
    /// and either saves them as files or prints to stdout.
    ///
    /// When `-o` is provided, defaults to CSV format and saves to file(s).
    /// When `-o` is omitted, defaults to markdown format and prints to stdout.
    ///
    /// Example: longbridge statement export --file-key KEY --section `equity_holdings`
    /// Example: longbridge statement export --file-key KEY --section `equity_holdings` -o holdings.csv
    Export {
        /// File key from `longbridge statement list`
        #[arg(long)]
        file_key: String,
        /// Sections to export (can specify multiple)
        #[arg(long, num_args = 1.., conflicts_with = "all")]
        section: Vec<StatementSection>,
        /// Export all sections (empty sections are skipped). Defaults to true when --section is not specified.
        #[arg(long, default_value_t = true)]
        all: bool,
        /// Export format: csv | md.
        /// Defaults to `md` when `-o` is omitted, `csv` when `-o` is provided.
        #[arg(long = "export-format")]
        export_format: Option<ExportFormat>,
        /// Output directory or file path.
        /// When multiple sections are specified, this is treated as a directory
        /// and each section is saved as a separate file inside it.
        /// Omit to print to stdout.
        #[arg(long, short = 'o')]
        output: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum OrderCmd {
    /// Full detail for a single order including charges and history
    ///
    /// Returns all fields from `order` plus `charge_detail`, `history_details`, msg.
    /// Example: longbridge order detail 20240101-123456789
    Detail {
        /// Order ID (from `longbridge order` or returned by `order buy`/`order sell`)
        order_id: String,
    },

    /// Today's trade executions (fills), or historical with --history
    ///
    /// Returns: `order_id`, `trade_id`, symbol, price, quantity, `trade_done_at`.
    /// Example: longbridge order executions
    /// Example: longbridge order executions --history --start 2024-01-01
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
    /// Returns `order_id` on success.
    /// Order types: LO ELO MO AO ALO ODD SLO LIT MIT TSLPAMT TSLPPCT
    ///   (case-insensitive)
    /// Trailing orders (TSLPAMT/TSLPPCT) require --trailing-amount/--trailing-percent
    ///   and --limit-offset.
    /// Example: longbridge order buy TSLA.US 100 --price 250.00
    /// Example: longbridge order buy 700.HK 1000 --price 300 --order-type ALO
    /// Example: longbridge order buy NVDA.US 10 --order-type MIT --trigger-price 177.89 --tif Day
    /// Example: longbridge order buy TSLA.US 10 --order-type TSLPPCT --trailing-percent 3 --limit-offset 1 --tif gtc
    /// Example: longbridge order buy AAPL.US 10 --price 180 --tif gtd --expire-date 2025-12-31
    Buy {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Number of shares/units to buy (integer)
        quantity: u64,
        /// Limit price as a decimal string, e.g. 250.00 (required for LO/ELO/ALO/LIT; omit for MO/MIT)
        #[arg(long)]
        price: Option<String>,
        /// Trigger price for conditional orders (required for MIT/LIT)
        #[arg(long)]
        trigger_price: Option<String>,
        /// Trailing amount for TSLPAMT/TSMAMT orders
        #[arg(long)]
        trailing_amount: Option<String>,
        /// Trailing percent for TSLPPCT/TSMPCT orders
        #[arg(long)]
        trailing_percent: Option<String>,
        /// Limit offset for TSLPAMT/TSLPPCT orders (spread between trigger and limit price)
        #[arg(long)]
        limit_offset: Option<String>,
        /// Expiry date for GTD orders in YYYY-MM-DD format (required when --tif gtd)
        #[arg(long)]
        expire_date: Option<String>,
        /// Outside regular trading hours: `RTH_ONLY` | `ANY_TIME` | `OVERNIGHT` (US market only)
        #[arg(long)]
        outside_rth: Option<String>,
        /// Order remark (max 255 characters)
        #[arg(long)]
        remark: Option<String>,
        /// Order type: LO ELO MO AO ALO ODD SLO LIT MIT TSLPAMT TSLPPCT
        ///   (case-insensitive, default: LO)
        #[arg(long, default_value = "LO")]
        order_type: String,
        /// Time in force: day | gtc (`GoodTilCanceled`) | gtd (`GoodTilDate`)
        /// (case-insensitive)
        #[arg(long, default_value = "day")]
        tif: String,
        /// Skip confirmation prompt (useful for scripting and AI agents)
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Submit a sell order (prompts for confirmation)
    ///
    /// Returns `order_id` on success.
    /// Order types: LO ELO MO AO ALO ODD SLO LIT MIT TSLPAMT TSLPPCT
    ///   (case-insensitive)
    /// Trailing orders (TSLPAMT/TSLPPCT) require --trailing-amount/--trailing-percent
    ///   and --limit-offset.
    /// Example: longbridge order sell TSLA.US 100 --price 260.00
    /// Example: longbridge order sell NVDA.US 10 --order-type MIT --trigger-price 177.89 --tif Day
    /// Example: longbridge order sell TSLA.US 130 --order-type TSLPPCT --trailing-percent 3 --limit-offset 1 --tif gtc
    /// Example: longbridge order sell AAPL.US 10 --price 180 --tif gtd --expire-date 2025-12-31
    Sell {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Number of shares/units to sell (integer)
        quantity: u64,
        /// Limit price as a decimal string, e.g. 260.00 (required for LO/ELO/ALO/LIT; omit for MO/MIT)
        #[arg(long)]
        price: Option<String>,
        /// Trigger price for conditional orders (required for MIT/LIT)
        #[arg(long)]
        trigger_price: Option<String>,
        /// Trailing amount for TSLPAMT/TSMAMT orders
        #[arg(long)]
        trailing_amount: Option<String>,
        /// Trailing percent for TSLPPCT/TSMPCT orders
        #[arg(long)]
        trailing_percent: Option<String>,
        /// Limit offset for TSLPAMT/TSLPPCT orders (spread between trigger and limit price)
        #[arg(long)]
        limit_offset: Option<String>,
        /// Expiry date for GTD orders in YYYY-MM-DD format (required when --tif gtd)
        #[arg(long)]
        expire_date: Option<String>,
        /// Outside regular trading hours: `RTH_ONLY` | `ANY_TIME` | `OVERNIGHT` (US market only)
        #[arg(long)]
        outside_rth: Option<String>,
        /// Order remark (max 255 characters)
        #[arg(long)]
        remark: Option<String>,
        /// Order type: LO ELO MO AO ALO ODD SLO LIT MIT TSLPAMT TSLPPCT
        ///   (case-insensitive, default: LO)
        #[arg(long, default_value = "LO")]
        order_type: String,
        /// Time in force: day | gtc (`GoodTilCanceled`) | gtd (`GoodTilDate`)
        /// (case-insensitive)
        #[arg(long, default_value = "day")]
        tif: String,
        /// Skip confirmation prompt (useful for scripting and AI agents)
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Cancel a pending order (prompts for confirmation)
    ///
    /// Only cancellable states (New, `PartialFilled`, etc.) are accepted.
    /// Example: longbridge order cancel 20240101-123456789
    Cancel {
        /// Order ID to cancel
        order_id: String,
        /// Skip confirmation prompt (useful for scripting and AI agents)
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Modify quantity or price of a pending order (prompts for confirmation)
    ///
    /// --qty is required. --price is optional (omit to keep current price).
    /// Example: longbridge order replace 20240101-123456789 --qty 200 --price 255.00
    Replace {
        /// Order ID to modify
        order_id: String,
        /// New quantity (REQUIRED — integer number of shares/units)
        #[arg(long)]
        qty: Option<u64>,
        /// New limit price as a decimal string, e.g. 255.00 (optional)
        #[arg(long)]
        price: Option<String>,
        /// Skip confirmation prompt (useful for scripting and AI agents)
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub enum NewsCmd {
    /// Full Markdown content of a news article
    ///
    /// Fetches the article from longbridge.com (or longbridge.cn for CN region).
    /// Use the global --lang flag to select language (zh-CN or en).
    /// Example: longbridge news detail 12345678
    /// Example: longbridge --lang zh-CN news detail 12345678
    Detail {
        /// News article ID (from `longbridge news <SYMBOL>`)
        id: String,
    },
}

#[derive(Subcommand)]
pub enum FilingCmd {
    /// Full Markdown content of a regulatory filing (HTML and TXT only)
    ///
    /// Get the symbol and id from `longbridge filing <SYMBOL>`.
    /// Some filings contain multiple files. Use --list-files to see all, then --file-index N.
    /// Example: longbridge filing detail AAPL.US 580265529766123777
    /// Example: longbridge filing detail AAPL.US 580265529766123777 --list-files
    /// Example: longbridge filing detail AAPL.US 580265529766123777 --file-index 1
    Detail {
        /// Symbol in <CODE>.<MARKET> format, e.g. AAPL.US 700.HK
        symbol: String,
        /// Filing ID (from `longbridge filing list`)
        id: String,
        /// List all available file URLs without fetching content
        #[arg(long)]
        list_files: bool,
        /// Index of the file to fetch (0-based, default 0)
        #[arg(long, default_value = "0")]
        file_index: usize,
    },
}

#[derive(Subcommand)]
pub enum TopicCmd {
    /// Get full details of a community topic by its ID
    ///
    /// Returns: id, `topic_type`, title, description, body, author, tickers, hashtags,
    /// images, `likes_count`, `comments_count`, `views_count`, `shares_count`, `detail_url`,
    /// `created_at`, `updated_at`.
    /// Example: longbridge topic detail 6993508780031016960
    Detail {
        /// Topic ID (e.g. 6993508780031016960)
        id: String,
    },

    /// Topics created by the authenticated user
    ///
    /// Returns: id, title/excerpt, type, `created_at`, likes, comments, views.
    /// Example: longbridge topic mine
    /// Example: longbridge topic mine --type article --size 10
    Mine {
        /// Page number (default: 1)
        #[arg(long, default_value = "1")]
        page: i32,
        /// Records per page, 1-500 (default: 50)
        #[arg(long, default_value = "50")]
        size: i32,
        /// Filter by content type: article | post (omit for all)
        #[arg(long = "type")]
        post_type: Option<String>,
    },

    /// Publish a new community discussion topic
    ///
    /// Two content types:
    ///   --type post (default): plain text only.
    ///   --type article: Markdown body, title required.
    /// Rate limit: max 3 topics per user per minute, 10 per 24 hours.
    /// Example: longbridge topic create --body "Bullish on 700.HK today"
    /// Example: longbridge topic create --title "My Analysis" --body "$(cat post.md)" --type article
    Create {
        /// Topic title. Required for --type article; optional for --type post.
        #[arg(long)]
        title: Option<String>,
        /// Topic body. post: plain text. article: Markdown, title required.
        #[arg(long)]
        body: String,
        /// Content type: post (default) | article
        #[arg(long = "type")]
        post_type: Option<String>,
        /// Extra tickers to associate, comma-separated, e.g. 700.HK,TSLA.US (max 10).
        #[arg(long, value_delimiter = ',')]
        tickers: Vec<String>,
    },

    /// List replies for a community topic (paginated)
    ///
    /// Returns: id, `topic_id`, body, `reply_to_id`, author, `likes_count`, `comments_count`, `created_at`.
    /// Page size is 1-50, default 20.
    /// Example: longbridge topic replies 6993508780031016960
    /// Example: longbridge topic replies 6993508780031016960 --page 2 --size 20
    Replies {
        /// Topic ID (e.g. 6993508780031016960)
        topic_id: String,
        /// Page number, 1-based (default: 1)
        #[arg(long, default_value = "1")]
        page: i32,
        /// Records per page, 1-50 (default: 20)
        #[arg(long, default_value = "20")]
        size: i32,
    },

    /// Post a reply to a community topic
    ///
    /// Body format: plain text only. Rate limit: first 3 replies per topic free,
    /// then incrementally longer waits (4th=3s, 5th=5s, ..., 10th+=55s). Returns 429 when exceeded.
    /// Example: longbridge topic create-reply 6993508780031016960 --body "Great post!"
    /// Example: longbridge topic create-reply 6993508780031016960 --body "Agreed!" --reply-to 7001234567890123456
    CreateReply {
        /// Topic ID to reply to (e.g. 6993508780031016960)
        topic_id: String,
        /// Reply body - plain text only.
        #[arg(long)]
        body: String,
        /// Nest under this reply ID (get IDs from topic-replies). Omit for a top-level reply.
        #[arg(long = "reply-to")]
        reply_to_id: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum OptionCmd {
    /// Option chain: expiry dates, or strike prices for a given expiry
    ///
    /// Without --date: returns all available expiry dates.
    /// With --date: returns strike prices and call/put symbols for that expiry.
    /// Example: longbridge option chain AAPL.US
    /// Example: longbridge option chain AAPL.US --date 2024-01-19
    Chain {
        /// Underlying symbol in <CODE>.<MARKET> format, e.g. AAPL.US
        symbol: String,
        /// Expiry date (YYYY-MM-DD). Omit to list all expiry dates.
        #[arg(long)]
        date: Option<String>,
    },

    /// Real-time quotes for option contracts
    ///
    /// Returns all fields from the option quote API: price, volume, implied/historical
    /// volatility, open interest, strike, expiry, contract type/size/multiplier, direction,
    /// and underlying symbol.
    /// Example: longbridge option quote AAPL240119C190000
    Quote {
        /// Option contract symbols (OCC format for US, e.g. AAPL240119C190000)
        symbols: Vec<String>,
    },

    /// Real-time Call/Put volume snapshot; with `daily` subcommand shows historical data
    ///
    /// Without subcommand: returns today's real-time Call/Put volume and Put/Call ratio.
    /// Example: longbridge option volume AAPL.US
    /// Example: longbridge option volume daily AAPL.US
    /// Example: longbridge option volume daily AAPL.US --count 60
    Volume {
        /// Symbol in <CODE>.<MARKET> format (US market only). Omit when using a subcommand.
        symbol: Option<String>,
        #[command(subcommand)]
        cmd: Option<VolumeSubCmd>,
    },
}

#[derive(Subcommand)]
pub enum VolumeSubCmd {
    /// Daily Call/Put volume and open interest history
    ///
    /// Example: longbridge option volume daily AAPL.US
    /// Example: longbridge option volume daily AAPL.US --count 60
    Daily {
        /// Symbol in <CODE>.<MARKET> format (US market only, e.g. AAPL.US)
        symbol: String,
        /// Number of trading days to return (default: 20)
        #[arg(long, alias = "limit", default_value = "20")]
        count: u32,
    },
}

#[derive(Subcommand)]
pub enum WarrantCmd {
    /// Real-time quotes for warrant contracts
    ///
    /// Returns: `last_done`, `prev_close`, `implied_volatility`, `leverage_ratio`, `expiry_date`, category.
    /// Example: longbridge warrant quote 12345.HK
    Quote {
        /// Warrant symbols (e.g. 12345.HK)
        symbols: Vec<String>,
    },

    /// Warrant issuer list (HK market)
    ///
    /// Returns: `issuer_id`, `name_en`, `name_cn`.
    Issuers,
}

#[derive(Subcommand)]
pub enum KlineCmd {
    /// Historical OHLCV candlestick data within a date range
    ///
    /// Both --start and --end must be provided together; if either is omitted the
    /// most recent 100 candles are returned (offset-based, ignores the other flag).
    /// Use --session all to include pre/post-market candles (adds a Session column).
    /// Example: longbridge kline history TSLA.US --start 2024-01-01 --end 2024-12-31
    /// Example: longbridge kline history TSLA.US --period 1m --session all --start 2024-01-01 --end 2024-01-02
    History {
        /// Symbol in <CODE>.<MARKET> format
        symbol: String,
        /// Candlestick period: 1m 5m 15m 30m 1h day week month year (default: day)
        #[arg(long, default_value = "day")]
        period: String,
        /// Start date (YYYY-MM-DD). Must be used together with --end.
        #[arg(long)]
        start: Option<String>,
        /// End date (YYYY-MM-DD). Must be used together with --start.
        #[arg(long)]
        end: Option<String>,
        /// Price adjustment: `none` (default) | `forward`
        #[arg(long, default_value = "none")]
        adjust: String,
        /// Trade session filter: intraday (default) | all (includes pre/post market)
        #[arg(long, default_value = "intraday")]
        session: String,
    },
}

#[derive(Subcommand)]
pub enum TradingCmd {
    /// Trading session schedule (open/close times) for all markets
    ///
    /// Returns: market, session type (intraday/pre/post/overnight), `begin_time`, `end_time`.
    Session,

    /// Trading days and half-trading days for a market
    ///
    /// Defaults to today + 30 days if no dates are provided.
    /// Example: longbridge trading days HK --start 2024-01-01 --end 2024-03-31
    Days {
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
}

#[derive(Subcommand)]
pub enum AuthCmd {
    /// Authenticate via Device Authorization Flow (default) or browser OAuth
    ///
    /// By default uses the Device Authorization Flow (RFC 8628): displays a URL,
    /// the user opens it in any browser (no localhost redirect needed), and the
    /// CLI polls until authorization is complete. Works on any machine including
    /// SSH sessions and headless servers.
    ///
    /// Use `--auth-code` for the Authorization Code flow: opens a browser on this
    /// machine and listens on `localhost:60355` for the OAuth callback.
    Login {
        /// Authorization Code flow: opens a browser and handles the localhost callback.
        /// Requires the browser to be on the same machine (local use only).
        #[arg(long)]
        auth_code: bool,
        /// Print request/response details for each OAuth step.
        #[arg(short, long)]
        verbose: bool,
    },

    /// Clear the locally stored OAuth token
    ///
    /// Next command or TUI launch will trigger re-authentication.
    Logout,

    /// Show authentication status
    ///
    /// Checks whether a token is stored locally and whether it is still valid.
    /// Also lists the user's quote subscriptions via `/v1/quote/my-quotes`.
    /// Example: longbridge auth status
    /// Example: longbridge auth status --market US
    /// Example: longbridge auth status --format json
    Status {
        /// Market filter for quote subscriptions: `all` (default), `HK`, `US`, `CN`, `SG`.
        #[arg(long, value_name = "MARKET", default_value = "all")]
        market: String,
    },
}

pub async fn dispatch(cmd: Commands, format: &OutputFormat, verbose: bool) -> Result<()> {
    match cmd {
        Commands::Quote { symbols } => quote::cmd_quote(symbols, format).await,
        Commands::Depth { symbol } => quote::cmd_depth(symbol, format).await,
        Commands::Brokers { symbol } => quote::cmd_brokers(symbol, format).await,
        Commands::Trades { symbol, count } => quote::cmd_trades(symbol, count, format).await,
        Commands::Intraday {
            symbol,
            session,
            date,
        } => {
            if let Some(d) = date {
                quote::cmd_history_intraday(symbol, &session, &d, format, verbose).await
            } else {
                quote::cmd_intraday(symbol, &session, format).await
            }
        }
        Commands::Kline {
            symbol,
            period,
            count,
            adjust,
            session,
            cmd,
        } => match cmd {
            Some(KlineCmd::History {
                symbol: h_symbol,
                period: h_period,
                start,
                end,
                adjust: h_adjust,
                session: h_session,
            }) => {
                quote::cmd_kline_history(
                    h_symbol, &h_period, start, end, &h_adjust, &h_session, format,
                )
                .await
            }
            None => {
                let sym = symbol.ok_or_else(|| {
                    anyhow::anyhow!("Symbol required. Usage: longbridge kline <SYMBOL>")
                })?;
                quote::cmd_kline(sym, &period, count, &adjust, &session, format).await
            }
        },
        Commands::Static { symbols } => quote::cmd_static(symbols, format).await,
        Commands::CalcIndex { symbols, fields } => {
            quote::cmd_calc_index(symbols, fields, format).await
        }
        Commands::Capital { symbol, flow } => {
            if flow {
                quote::cmd_capital_flow(symbol, format).await
            } else {
                quote::cmd_capital_dist(symbol, format).await
            }
        }
        Commands::MarketTemp {
            market,
            history,
            start,
            end,
            granularity,
        } => quote::cmd_market_temp(&market, history, start, end, &granularity, format).await,
        Commands::Trading { cmd } => match cmd {
            TradingCmd::Session => quote::cmd_trading_session(format).await,
            TradingCmd::Days { market, start, end } => {
                quote::cmd_trading_days(&market, start, end, format).await
            }
        },
        Commands::SecurityList { market, category } => {
            quote::cmd_security_list(&market, &category, format).await
        }
        Commands::Participants => quote::cmd_participants(format).await,
        Commands::Subscriptions => quote::cmd_subscriptions(format).await,
        Commands::Option { cmd } => match cmd {
            OptionCmd::Quote { symbols } => quote::cmd_option_quote(symbols, format).await,
            OptionCmd::Chain { symbol, date } => {
                quote::cmd_option_chain(symbol, date, format).await
            }
            OptionCmd::Volume { symbol, cmd } => match cmd {
                Some(VolumeSubCmd::Daily { symbol: s, count }) => {
                    quote::cmd_option_volume_daily(s, count, format, verbose).await
                }
                None => {
                    let sym = symbol.ok_or_else(|| {
                        anyhow::anyhow!(
                            "Symbol required. Usage: longbridge option volume <SYMBOL>"
                        )
                    })?;
                    quote::cmd_option_volume_stats(sym, format, verbose).await
                }
            },
        },
        Commands::Warrant { symbol, cmd } => match cmd {
            Some(WarrantCmd::Quote { symbols }) => quote::cmd_warrant_quote(symbols, format).await,
            Some(WarrantCmd::Issuers) => quote::cmd_warrant_issuers(format).await,
            None => {
                let sym = symbol.ok_or_else(|| {
                    anyhow::anyhow!("Symbol required. Usage: longbridge warrant <SYMBOL>")
                })?;
                quote::cmd_warrant_list(sym, format).await
            }
        },
        Commands::FinancialReport {
            symbol,
            kind,
            report,
        } => fundamental::cmd_financial_report(symbol, kind, report, format, verbose).await,
        Commands::InstitutionRating { symbol, cmd } => match cmd {
            Some(InstitutionRatingCmd::Detail { symbol: s }) => {
                fundamental::cmd_institution_rating_detail(s, format, verbose).await
            }
            None => {
                let sym = symbol.ok_or_else(|| {
                    anyhow::anyhow!(
                        "Symbol required. Usage: longbridge institution-rating <SYMBOL>"
                    )
                })?;
                fundamental::cmd_institution_rating(sym, format, verbose).await
            }
        },
        Commands::Dividend { symbol, page, year, cmd } => match cmd {
            Some(DividendCmd::Detail { symbol: s }) => {
                fundamental::cmd_dividend_detail(s, format, verbose).await
            }
            None => {
                let sym = symbol.ok_or_else(|| {
                    anyhow::anyhow!("Symbol required. Usage: longbridge dividend <SYMBOL>")
                })?;
                fundamental::cmd_dividend(sym, page, year, format, verbose).await
            }
        },
        Commands::ForecastEps { symbol } => {
            fundamental::cmd_forecast_eps(symbol, format, verbose).await
        }
        Commands::Consensus { symbol } => fundamental::cmd_consensus(symbol, format, verbose).await,
        Commands::FinanceCalendar { cmd } => {
            let (event_type, opts, star) = match cmd {
                FinanceCalendarCmd::Report { opts } => ("report", opts, vec![]),
                FinanceCalendarCmd::Dividend { opts } => ("dividend", opts, vec![]),
                FinanceCalendarCmd::Split { opts } => ("split", opts, vec![]),
                FinanceCalendarCmd::Ipo { opts } => ("ipo", opts, vec![]),
                FinanceCalendarCmd::Macrodata { opts, star } => ("macrodata", opts, star),
                FinanceCalendarCmd::Closed { opts } => ("closed", opts, vec![]),
            };
            fundamental::cmd_finance_calendar(
                event_type.to_string(),
                opts.symbol,
                opts.filter,
                opts.market,
                opts.start,
                opts.end,
                opts.count,
                star,
                format,
                verbose,
            )
            .await
        }

        Commands::Valuation {
            symbol,
            history,
            indicator,
            range,
        } => {
            if history {
                fundamental::cmd_valuation(symbol, indicator, range, format, verbose).await
            } else {
                fundamental::cmd_valuation_detail(symbol, indicator, format, verbose).await
            }
        }
        Commands::News { symbol, count, cmd } => match cmd {
            Some(NewsCmd::Detail { id }) => news::cmd_news_detail(id).await,
            None => {
                let sym = symbol.ok_or_else(|| {
                    anyhow::anyhow!("Symbol required. Usage: longbridge news <SYMBOL>")
                })?;
                news::cmd_news(sym, count, format).await
            }
        },
        Commands::Filing { symbol, count, cmd } => match cmd {
            Some(FilingCmd::Detail {
                symbol: s,
                id,
                list_files,
                file_index,
            }) => news::cmd_filing_detail(s, id, list_files, file_index).await,
            None => {
                let sym = symbol.ok_or_else(|| {
                    anyhow::anyhow!("Symbol required. Usage: longbridge filing <SYMBOL>")
                })?;
                news::cmd_filings(sym, count, format).await
            }
        },
        Commands::Topic { symbol, count, cmd } => match cmd {
            Some(TopicCmd::Detail { id }) => topic::cmd_topic_detail_api(id, format).await,
            Some(TopicCmd::Mine {
                page,
                size,
                post_type,
            }) => topic::cmd_topics_mine(page, size, post_type, format).await,
            Some(TopicCmd::Create {
                title,
                body,
                post_type,
                tickers,
            }) => topic::cmd_create_topic(title, body, post_type, tickers, format).await,
            Some(TopicCmd::Replies {
                topic_id,
                page,
                size,
            }) => topic::cmd_topic_replies(topic_id, page, size, format).await,
            Some(TopicCmd::CreateReply {
                topic_id,
                body,
                reply_to_id,
            }) => topic::cmd_create_reply(topic_id, body, reply_to_id, format).await,
            None => {
                let sym = symbol.ok_or_else(|| {
                    anyhow::anyhow!("Symbol required. Usage: longbridge topic <SYMBOL>")
                })?;
                news::cmd_topics(sym, count, format).await
            }
        },
        Commands::Watchlist { cmd } => watchlist::cmd_watchlist(cmd, format).await,
        Commands::Statement {
            statement_type,
            start_date,
            limit,
            cmd,
        } => match cmd {
            Some(c) => statement::cmd_statement(c, format).await,
            None => {
                statement::cmd_statement(
                    StatementCmd::List {
                        statement_type,
                        start_date,
                        limit,
                    },
                    format,
                )
                .await
            }
        },
        Commands::Order {
            history,
            start,
            end,
            symbol,
            cmd,
        } => match cmd {
            Some(OrderCmd::Detail { order_id }) => trade::cmd_order_detail(order_id, format).await,
            Some(OrderCmd::Executions {
                history: h,
                start: s,
                end: e,
                symbol: sy,
            }) => trade::cmd_executions(h, s, e, sy, format).await,
            Some(OrderCmd::Buy {
                symbol,
                quantity,
                price,
                trigger_price,
                trailing_amount,
                trailing_percent,
                limit_offset,
                expire_date,
                outside_rth,
                remark,
                order_type,
                tif,
                yes,
            }) => {
                trade::cmd_submit_order(
                    symbol,
                    quantity,
                    price,
                    trigger_price,
                    trailing_amount,
                    trailing_percent,
                    limit_offset,
                    expire_date,
                    outside_rth,
                    remark,
                    order_type,
                    tif,
                    longbridge::trade::OrderSide::Buy,
                    yes,
                    format,
                )
                .await
            }
            Some(OrderCmd::Sell {
                symbol,
                quantity,
                price,
                trigger_price,
                trailing_amount,
                trailing_percent,
                limit_offset,
                expire_date,
                outside_rth,
                remark,
                order_type,
                tif,
                yes,
            }) => {
                trade::cmd_submit_order(
                    symbol,
                    quantity,
                    price,
                    trigger_price,
                    trailing_amount,
                    trailing_percent,
                    limit_offset,
                    expire_date,
                    outside_rth,
                    remark,
                    order_type,
                    tif,
                    longbridge::trade::OrderSide::Sell,
                    yes,
                    format,
                )
                .await
            }
            Some(OrderCmd::Cancel { order_id, yes }) => {
                trade::cmd_cancel_order(order_id, yes).await
            }
            Some(OrderCmd::Replace {
                order_id,
                qty,
                price,
                yes,
            }) => trade::cmd_replace_order(order_id, qty, price, yes).await,
            None => trade::cmd_orders(history, start, end, symbol, format).await,
        },
        Commands::Assets { currency } => trade::cmd_assets(currency, format).await,
        Commands::CashFlow { start, end } => trade::cmd_cash_flow(start, end, format).await,
        Commands::Portfolio => trade::cmd_portfolio(format).await,
        Commands::Positions => trade::cmd_positions(format).await,
        Commands::FundPositions => trade::cmd_fund_positions(format).await,
        Commands::MarginRatio { symbol } => trade::cmd_margin_ratio(symbol, format).await,
        Commands::MaxQty {
            symbol,
            side,
            price,
            order_type,
        } => trade::cmd_max_qty(symbol, &side, price, &order_type, format).await,
        Commands::ExchangeRate => asset::cmd_exchange_rate(format, verbose).await,

        Commands::Shareholder {
            symbol,
            range,
            sort,
            order,
        } => fundamental::cmd_shareholders(symbol, range, sort, order, format, verbose).await,
        Commands::FundHolder { symbol, count } => {
            fundamental::cmd_fund_holders(symbol, count, format, verbose).await
        }
        Commands::InsiderTrades { symbol, count } => {
            insider_trades::cmd_insider_trades(&symbol, count, format).await
        }
        Commands::Investors { cik, top, subcmd } => match subcmd {
            Some(InvestorsSubCmd::Changes {
                cik: changes_cik,
                top: changes_top,
                from,
            }) => {
                investors::cmd_investor_changes(&changes_cik, changes_top, from.as_deref(), format)
                    .await
            }
            None => match cik {
                None => investors::cmd_investors_list(top, format).await,
                Some(s) if s.chars().all(|c| c.is_ascii_digit()) => {
                    investors::cmd_investor_holdings_by_cik(&s, top, format).await
                }
                Some(s) => Err(anyhow::anyhow!(
                    "'{s}' is not a valid CIK — CIK must be numeric.\nRun `longbridge investors` to see rankings with CIK column."
                )),
            },
        },
        // ── New pending commands ──────────────────────────────────────────────
        Commands::Company { symbol } => {
            fundamental::cmd_company(symbol, format, verbose).await
        }
        Commands::Executive { symbol } => {
            fundamental::cmd_executive(symbol, format, verbose).await
        }
        Commands::IndustryValuation {
            symbol,
            currency,
            cmd,
        } => match cmd {
            Some(IndustryValuationCmd::Dist { symbol: s }) => {
                fundamental::cmd_industry_valuation_dist(s, format, verbose).await
            }
            None => {
                let sym = symbol.ok_or_else(|| {
                    anyhow::anyhow!(
                        "Symbol required. Usage: longbridge industry-valuation <SYMBOL>"
                    )
                })?;
                fundamental::cmd_industry_valuation(sym, &currency, format, verbose).await
            }
        },
        Commands::Operating { symbol, report } => {
            fundamental::cmd_operating(symbol, report, format, verbose).await
        }
        Commands::CorpAction { symbol, all } => {
            fundamental::cmd_corp_action(symbol, all, format, verbose).await
        }
        Commands::InvestRelation { symbol } => {
            fundamental::cmd_invest_relation(symbol, format, verbose).await
        }
        Commands::Constituent {
            symbol,
            limit,
            sort,
            order,
        } => quote::cmd_constituent(symbol, limit, &sort, &order, format, verbose).await,
        Commands::MarketStatus => quote::cmd_market_status(format, verbose).await,
        Commands::BrokerHolding {
            symbol,
            period,
            cmd,
        } => match cmd {
            Some(BrokerHoldingCmd::Detail { symbol: s }) => {
                quote::cmd_broker_holding_detail(s, format, verbose).await
            }
            Some(BrokerHoldingCmd::Daily { symbol: s, broker }) => {
                quote::cmd_broker_holding_daily(s, &broker, format, verbose).await
            }
            None => {
                let sym = symbol.ok_or_else(|| {
                    anyhow::anyhow!("Symbol required. Usage: longbridge broker-holding <SYMBOL>")
                })?;
                quote::cmd_broker_holding_top(sym, &period, format, verbose).await
            }
        },
        Commands::AhPremium {
            symbol,
            kline_type,
            count,
            cmd,
        } => match cmd {
            Some(AhPremiumCmd::Intraday { symbol: s }) => {
                quote::cmd_ah_premium_intraday(s, format, verbose).await
            }
            None => {
                let sym = symbol.ok_or_else(|| {
                    anyhow::anyhow!("Symbol required. Usage: longbridge ah-premium <SYMBOL>")
                })?;
                quote::cmd_ah_premium_kline(sym, &kline_type, count, format, verbose).await
            }
        },
        Commands::TradeStats { symbol } => {
            quote::cmd_trade_stats(symbol, format, verbose).await
        }
        Commands::Anomaly {
            market,
            symbol,
            count,
        } => quote::cmd_anomaly(&market, symbol, count, format, verbose).await,
        Commands::Alert { symbol, cmd } => match cmd {
            Some(AlertCmd::Add {
                symbol: s,
                price,
                direction,
                alert_type,
                frequency,
                note,
            }) => {
                trade::cmd_alert_add(s, &price, &direction, &alert_type, &frequency, note, format, verbose).await
            }
            Some(AlertCmd::Delete { id }) => {
                trade::cmd_alert_delete(id, format, verbose).await
            }
            Some(AlertCmd::Enable { id }) => {
                trade::cmd_alert_set_enabled(id, true, format, verbose).await
            }
            Some(AlertCmd::Disable { id }) => {
                trade::cmd_alert_set_enabled(id, false, format, verbose).await
            }
            None => trade::cmd_alert_list(symbol, format, verbose).await,
        },
        Commands::ProfitAnalysis { start, end, cmd } => match cmd {
            None => asset::cmd_profit_analysis(start.as_deref(), end.as_deref(), format, verbose).await,
            Some(ProfitAnalysisCmd::Detail {
                symbol,
                start,
                end,
                currency,
                derivative,
                page,
                size,
            }) => {
                asset::cmd_profit_analysis_detail(
                    &symbol,
                    start.as_deref(),
                    end.as_deref(),
                    currency.as_deref(),
                    derivative,
                    page,
                    size,
                    format,
                    verbose,
                )
                .await
            }
            Some(ProfitAnalysisCmd::ByMarket {
                market,
                start,
                end,
                currency,
                page,
                size,
            }) => {
                asset::cmd_profit_analysis_by_market(
                    market.as_deref(),
                    start.as_deref(),
                    end.as_deref(),
                    currency.as_deref(),
                    page,
                    size,
                    format,
                    verbose,
                )
                .await
            }
        },

        Commands::Dca {
            cmd,
            status,
            symbol,
            page,
            limit,
        } => dca::cmd_dca(cmd, status.as_deref(), symbol.as_deref(), page, limit, format).await,

        Commands::ShortPositions { symbol, count } => {
            quote::cmd_short_positions(symbol, count, format, verbose).await
        }

        Commands::Sharelist { cmd, count } => {
            sharelist::cmd_sharelist(cmd, count, format).await
        }

        Commands::Quant { cmd } => match cmd {
            QuantCmd::Run {
                symbol,
                period,
                start,
                end,
                script,
                input,
            } => run_script::cmd_run_script(symbol, &period, &start, &end, script, input, format, verbose).await,
        },

        Commands::Auth { .. }
        | Commands::Tui
        | Commands::Check
        | Commands::Update { .. }
        | Commands::Completion { .. }
        | Commands::Init { .. } => {
            unreachable!()
        }
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
        assert!(matches!(cli.format, OutputFormat::Pretty));
    }

    #[test]
    fn test_format_json_flag() {
        let cli = parse(&["longbridge", "quote", "TSLA.US", "--format", "json"]).unwrap();
        assert!(matches!(cli.format, OutputFormat::Json));
    }

    // ─── Auth ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_auth_login_subcommand() {
        let cli = parse(&["longbridge", "auth", "login"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Auth {
                cmd: AuthCmd::Login { .. }
            })
        ));
    }

    #[test]
    fn test_auth_logout_subcommand() {
        let cli = parse(&["longbridge", "auth", "logout"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Auth {
                cmd: AuthCmd::Logout
            })
        ));
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
        assert!(
            matches!(cli.command, Some(Commands::Intraday { symbol, .. }) if symbol == "TSLA.US")
        );
    }

    #[test]
    fn test_kline_defaults() {
        let cli = parse(&["longbridge", "kline", "TSLA.US"]).unwrap();
        if let Some(Commands::Kline {
            symbol,
            period,
            count,
            adjust,
            ..
        }) = cli.command
        {
            assert_eq!(symbol, Some("TSLA.US".to_string()));
            assert_eq!(period, "day");
            assert_eq!(count, 100);
            assert_eq!(adjust, "none");
        } else {
            panic!("expected Kline command");
        }
    }

    #[test]
    fn test_kline_custom_period() {
        let cli = parse(&[
            "longbridge",
            "kline",
            "TSLA.US",
            "--period",
            "1h",
            "--count",
            "200",
        ])
        .unwrap();
        if let Some(Commands::Kline { period, count, .. }) = cli.command {
            assert_eq!(period, "1h");
            assert_eq!(count, 200);
        } else {
            panic!("expected Kline command");
        }
    }

    #[test]
    fn test_kline_history_with_dates() {
        let cli = parse(&[
            "longbridge",
            "kline",
            "history",
            "TSLA.US",
            "--start",
            "2024-01-01",
            "--end",
            "2024-12-31",
        ])
        .unwrap();
        if let Some(Commands::Kline {
            cmd: Some(KlineCmd::History {
                symbol, start, end, ..
            }),
            ..
        }) = cli.command
        {
            assert_eq!(symbol, "TSLA.US");
            assert_eq!(start, Some("2024-01-01".to_string()));
            assert_eq!(end, Some("2024-12-31".to_string()));
        } else {
            panic!("expected Kline History command");
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
    fn test_calc_index_default_fields() {
        let cli = parse(&["longbridge", "calc-index", "TSLA.US"]).unwrap();
        if let Some(Commands::CalcIndex { symbols, fields }) = cli.command {
            assert_eq!(symbols, vec!["TSLA.US"]);
            assert!(fields.contains(&"pe".to_string()));
        } else {
            panic!("expected CalcIndex command");
        }
    }

    #[test]
    fn test_calc_index_custom_fields() {
        let cli = parse(&[
            "longbridge",
            "calc-index",
            "TSLA.US",
            "--fields",
            "pe,pb,eps",
        ])
        .unwrap();
        if let Some(Commands::CalcIndex { fields, .. }) = cli.command {
            assert_eq!(fields, vec!["pe", "pb", "eps"]);
        } else {
            panic!("expected CalcIndex command");
        }
    }

    #[test]
    fn test_capital_default_dist() {
        let cli = parse(&["longbridge", "capital", "TSLA.US"]).unwrap();
        assert!(
            matches!(cli.command, Some(Commands::Capital { ref symbol, flow }) if symbol == "TSLA.US" && !flow)
        );
    }

    #[test]
    fn test_capital_flow_flag() {
        let cli = parse(&["longbridge", "capital", "TSLA.US", "--flow"]).unwrap();
        assert!(
            matches!(cli.command, Some(Commands::Capital { ref symbol, flow }) if symbol == "TSLA.US" && flow)
        );
    }

    #[test]
    fn test_market_temp_default() {
        let cli = parse(&["longbridge", "market-temp"]).unwrap();
        if let Some(Commands::MarketTemp {
            market, history, ..
        }) = cli.command
        {
            assert_eq!(market, "HK");
            assert!(!history);
        } else {
            panic!("expected MarketTemp command");
        }
    }

    #[test]
    fn test_market_temp_history_flag() {
        let cli = parse(&[
            "longbridge",
            "market-temp",
            "US",
            "--history",
            "--start",
            "2024-01-01",
        ])
        .unwrap();
        if let Some(Commands::MarketTemp {
            market,
            history,
            start,
            ..
        }) = cli.command
        {
            assert_eq!(market, "US");
            assert!(history);
            assert_eq!(start, Some("2024-01-01".to_string()));
        } else {
            panic!("expected MarketTemp command");
        }
    }

    #[test]
    fn test_trading_session_subcommand() {
        let cli = parse(&["longbridge", "trading", "session"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Trading {
                cmd: TradingCmd::Session
            })
        ));
    }

    #[test]
    fn test_trading_days_default_market() {
        let cli = parse(&["longbridge", "trading", "days"]).unwrap();
        if let Some(Commands::Trading {
            cmd: TradingCmd::Days { market, .. },
        }) = cli.command
        {
            assert_eq!(market, "HK");
        } else {
            panic!("expected Trading Days command");
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
        let cli = parse(&["longbridge", "option", "quote", "AAPL240119C190000"]).unwrap();
        if let Some(Commands::Option {
            cmd: OptionCmd::Quote { symbols },
        }) = cli.command
        {
            assert_eq!(symbols, vec!["AAPL240119C190000"]);
        } else {
            panic!("expected Option Quote command");
        }
    }

    #[test]
    fn test_option_chain_no_date() {
        let cli = parse(&["longbridge", "option", "chain", "AAPL.US"]).unwrap();
        if let Some(Commands::Option {
            cmd: OptionCmd::Chain { symbol, date },
        }) = cli.command
        {
            assert_eq!(symbol, "AAPL.US");
            assert!(date.is_none());
        } else {
            panic!("expected Option Chain command");
        }
    }

    #[test]
    fn test_option_chain_with_date() {
        let cli = parse(&[
            "longbridge",
            "option",
            "chain",
            "AAPL.US",
            "--date",
            "2024-01-19",
        ])
        .unwrap();
        if let Some(Commands::Option {
            cmd: OptionCmd::Chain { date, .. },
        }) = cli.command
        {
            assert_eq!(date, Some("2024-01-19".to_string()));
        } else {
            panic!("expected Option Chain command");
        }
    }

    #[test]
    fn test_warrant_quote_subcommand() {
        let cli = parse(&["longbridge", "warrant", "quote", "12345.HK"]).unwrap();
        if let Some(Commands::Warrant {
            cmd: Some(WarrantCmd::Quote { symbols }),
            ..
        }) = cli.command
        {
            assert_eq!(symbols, vec!["12345.HK"]);
        } else {
            panic!("expected Warrant Quote command");
        }
    }

    #[test]
    fn test_warrant_list_positional() {
        let cli = parse(&["longbridge", "warrant", "700.HK"]).unwrap();
        if let Some(Commands::Warrant { symbol, cmd: None }) = cli.command {
            assert_eq!(symbol, Some("700.HK".to_string()));
        } else {
            panic!("expected Warrant positional command");
        }
    }

    #[test]
    fn test_warrant_issuers_subcommand() {
        let cli = parse(&["longbridge", "warrant", "issuers"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Warrant {
                cmd: Some(WarrantCmd::Issuers),
                ..
            })
        ));
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
        if let Some(Commands::Watchlist {
            cmd: Some(WatchlistCmd::Create { name }),
        }) = cli.command
        {
            assert_eq!(name, "Tech Stocks");
        } else {
            panic!("expected Watchlist Create command");
        }
    }

    #[test]
    fn test_watchlist_delete() {
        let cli = parse(&["longbridge", "watchlist", "delete", "123"]).unwrap();
        if let Some(Commands::Watchlist {
            cmd: Some(WatchlistCmd::Delete { id, purge, .. }),
        }) = cli.command
        {
            assert_eq!(id, 123);
            assert!(!purge);
        } else {
            panic!("expected Watchlist Delete command");
        }
    }

    #[test]
    fn test_watchlist_delete_purge() {
        let cli = parse(&["longbridge", "watchlist", "delete", "123", "--purge"]).unwrap();
        if let Some(Commands::Watchlist {
            cmd: Some(WatchlistCmd::Delete { purge, .. }),
        }) = cli.command
        {
            assert!(purge);
        } else {
            panic!("expected Watchlist Delete command");
        }
    }

    #[test]
    fn test_watchlist_update_add() {
        let cli = parse(&[
            "longbridge",
            "watchlist",
            "update",
            "123",
            "--add",
            "TSLA.US",
            "--add",
            "AAPL.US",
        ])
        .unwrap();
        if let Some(Commands::Watchlist {
            cmd: Some(WatchlistCmd::Update { id, add, .. }),
        }) = cli.command
        {
            assert_eq!(id, 123);
            assert_eq!(add, vec!["TSLA.US", "AAPL.US"]);
        } else {
            panic!("expected Watchlist Update command");
        }
    }

    #[test]
    fn test_watchlist_update_remove() {
        let cli = parse(&[
            "longbridge",
            "watchlist",
            "update",
            "456",
            "--remove",
            "700.HK",
        ])
        .unwrap();
        if let Some(Commands::Watchlist {
            cmd: Some(WatchlistCmd::Update { id, remove, .. }),
        }) = cli.command
        {
            assert_eq!(id, 456);
            assert_eq!(remove, vec!["700.HK"]);
        } else {
            panic!("expected Watchlist Update command");
        }
    }

    // ─── Trade commands ───────────────────────────────────────────────────────

    #[test]
    fn test_order_list_defaults() {
        let cli = parse(&["longbridge", "order"]).unwrap();
        if let Some(Commands::Order {
            history,
            start,
            end,
            symbol,
            cmd: None,
        }) = cli.command
        {
            assert!(!history);
            assert!(start.is_none());
            assert!(end.is_none());
            assert!(symbol.is_none());
        } else {
            panic!("expected Order list command");
        }
    }

    #[test]
    fn test_order_list_history_with_filters() {
        let cli = parse(&[
            "longbridge",
            "order",
            "--history",
            "--start",
            "2024-01-01",
            "--symbol",
            "TSLA.US",
        ])
        .unwrap();
        if let Some(Commands::Order {
            history,
            start,
            symbol,
            cmd: None,
            ..
        }) = cli.command
        {
            assert!(history);
            assert_eq!(start, Some("2024-01-01".to_string()));
            assert_eq!(symbol, Some("TSLA.US".to_string()));
        } else {
            panic!("expected Order list command");
        }
    }

    #[test]
    fn test_order_detail_subcommand() {
        let cli = parse(&["longbridge", "order", "detail", "order-123"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Order {
                cmd: Some(OrderCmd::Detail { order_id }),
                ..
            }) if order_id == "order-123"
        ));
    }

    #[test]
    fn test_order_executions_subcommand() {
        let cli = parse(&["longbridge", "order", "executions"]).unwrap();
        if let Some(Commands::Order {
            cmd: Some(OrderCmd::Executions { history, .. }),
            ..
        }) = cli.command
        {
            assert!(!history);
        } else {
            panic!("expected Order Executions command");
        }
    }

    #[test]
    fn test_order_buy_subcommand() {
        let cli = parse(&[
            "longbridge",
            "order",
            "buy",
            "TSLA.US",
            "100",
            "--price",
            "250.00",
        ])
        .unwrap();
        if let Some(Commands::Order {
            cmd:
                Some(OrderCmd::Buy {
                    symbol,
                    quantity,
                    price,
                    order_type,
                    tif,
                    ..
                }),
            ..
        }) = cli.command
        {
            assert_eq!(symbol, "TSLA.US");
            assert_eq!(quantity, 100);
            assert_eq!(price, Some("250.00".to_string()));
            assert_eq!(order_type, "LO");
            assert_eq!(tif, "day");
        } else {
            panic!("expected Order Buy command");
        }
    }

    #[test]
    fn test_order_sell_subcommand() {
        let cli = parse(&[
            "longbridge",
            "order",
            "sell",
            "TSLA.US",
            "50",
            "--price",
            "260.00",
        ])
        .unwrap();
        if let Some(Commands::Order {
            cmd:
                Some(OrderCmd::Sell {
                    symbol,
                    quantity,
                    price,
                    ..
                }),
            ..
        }) = cli.command
        {
            assert_eq!(symbol, "TSLA.US");
            assert_eq!(quantity, 50);
            assert_eq!(price, Some("260.00".to_string()));
        } else {
            panic!("expected Order Sell command");
        }
    }

    #[test]
    fn test_order_cancel_subcommand() {
        let cli = parse(&["longbridge", "order", "cancel", "order-456"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Order {
                cmd: Some(OrderCmd::Cancel { order_id, .. }),
                ..
            }) if order_id == "order-456"
        ));
    }

    #[test]
    fn test_order_replace_subcommand() {
        let cli = parse(&[
            "longbridge",
            "order",
            "replace",
            "order-789",
            "--qty",
            "200",
            "--price",
            "255.00",
        ])
        .unwrap();
        if let Some(Commands::Order {
            cmd:
                Some(OrderCmd::Replace {
                    order_id,
                    qty,
                    price,
                    ..
                }),
            ..
        }) = cli.command
        {
            assert_eq!(order_id, "order-789");
            assert_eq!(qty, Some(200));
            assert_eq!(price, Some("255.00".to_string()));
        } else {
            panic!("expected Order Replace command");
        }
    }

    #[test]
    fn test_assets_no_currency() {
        let cli = parse(&["longbridge", "assets"]).unwrap();
        if let Some(Commands::Assets { currency }) = cli.command {
            assert_eq!(currency, Some("USD".to_string()));
        } else {
            panic!("expected Assets command");
        }
    }

    #[test]
    fn test_assets_with_currency() {
        let cli = parse(&["longbridge", "assets", "--currency", "HKD"]).unwrap();
        if let Some(Commands::Assets { currency }) = cli.command {
            assert_eq!(currency, Some("HKD".to_string()));
        } else {
            panic!("expected Assets command");
        }
    }

    #[test]
    fn test_cash_flow_subcommand() {
        let cli = parse(&[
            "longbridge",
            "cash-flow",
            "--start",
            "2024-01-01",
            "--end",
            "2024-03-31",
        ])
        .unwrap();
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
        assert!(
            matches!(cli.command, Some(Commands::MarginRatio { symbol }) if symbol == "TSLA.US")
        );
    }

    #[test]
    fn test_max_qty_subcommand() {
        let cli = parse(&[
            "longbridge",
            "max-qty",
            "TSLA.US",
            "--side",
            "buy",
            "--price",
            "250",
        ])
        .unwrap();
        if let Some(Commands::MaxQty {
            symbol,
            side,
            price,
            order_type,
        }) = cli.command
        {
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
