use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Stock identifier (simplified)
/// Format: market.code (e.g., HK.00700)
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Counter {
    inner: String,
}

impl Counter {
    pub fn new(symbol: &str) -> Self {
        Self {
            inner: symbol.to_string(),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.inner
    }

    pub fn code(&self) -> &str {
        self.as_str().split('.').nth(0).unwrap_or("")
    }

    pub fn market(&self) -> &str {
        self.as_str().split('.').nth(1).unwrap_or("")
    }

    /// Get region/market
    pub fn region(&self) -> Market {
        Market::from(self.market())
    }

    /// Check if it's Hong Kong market
    pub fn is_hk(&self) -> bool {
        self.market() == "HK"
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl std::fmt::Display for Counter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl From<&str> for Counter {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl std::str::FromStr for Counter {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(s))
    }
}

impl From<String> for Counter {
    fn from(s: String) -> Self {
        Self::new(&s)
    }
}

/// Re-export `TradeStatus` and `TradeSession` from Longport SDK
pub use longport::quote::{TradeSession, TradeStatus};

/// Extension trait for `TradeSession` to provide helper methods
pub trait TradeSessionExt {
    /// Check if in normal trading session
    #[allow(clippy::wrong_self_convention)]
    fn is_normal_trading(self) -> bool;

    /// Get localized label for display
    fn label(self) -> String;
}

impl TradeSessionExt for TradeSession {
    fn is_normal_trading(self) -> bool {
        matches!(self, TradeSession::Intraday)
    }

    fn label(self) -> String {
        match self {
            TradeSession::Intraday => t!("TradeSession.Intraday"),
            TradeSession::Pre => t!("TradeSession.Pre"),
            TradeSession::Post => t!("TradeSession.Post"),
            TradeSession::Overnight => t!("TradeSession.Overnight"),
        }
    }
}

/// Extension trait for `TradeStatus` to provide additional helper methods
pub trait TradeStatusExt {
    /// Check if currently in active trading state
    #[allow(clippy::wrong_self_convention)]
    fn is_trading(self) -> bool;

    /// Check if market is closed or halted
    #[allow(clippy::wrong_self_convention)]
    fn is_closed(self) -> bool;

    /// Get localized label for display
    fn label(self) -> String;
}

impl TradeStatusExt for TradeStatus {
    fn is_trading(self) -> bool {
        matches!(self, TradeStatus::Normal)
    }

    fn is_closed(self) -> bool {
        !self.is_trading()
    }

    fn label(self) -> String {
        match self {
            TradeStatus::Normal => String::new(), // No label for normal status
            TradeStatus::Halted => t!("TradeStatus.Halted"),
            TradeStatus::Delisted => t!("TradeStatus.Delisted"),
            TradeStatus::Fuse => t!("TradeStatus.Fuse"),
            TradeStatus::PrepareList => t!("TradeStatus.PrepareList"),
            TradeStatus::CodeMoved => t!("TradeStatus.CodeMoved"),
            TradeStatus::ToBeOpened => t!("TradeStatus.ToBeOpened"),
            TradeStatus::SplitStockHalts => t!("TradeStatus.SplitStockHalts"),
            TradeStatus::Expired => t!("TradeStatus.Expired"),
            TradeStatus::WarrantPrepareList => t!("TradeStatus.WarrantPrepareList"),
            TradeStatus::SuspendTrade => t!("TradeStatus.SuspendTrade"),
        }
    }
}

/// Stock color mode
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StockColorMode {
    #[default]
    RedUp,
    GreenUp,
}

/// Candlestick period type
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    bytemuck::NoUninit,
    strum::EnumIter,
)]
#[repr(u8)]
#[derive(Default)]
pub enum KlineType {
    PerMinute = 0,
    PerFiveMinutes = 1,
    PerFifteenMinutes = 2,
    PerThirtyMinutes = 3,
    PerHour = 4,
    #[default]
    PerDay = 5,
    PerWeek = 6,
    PerMonth = 7,
    PerYear = 8,
}

impl std::fmt::Display for KlineType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PerMinute => write!(f, "1m"),
            Self::PerFiveMinutes => write!(f, "5m"),
            Self::PerFifteenMinutes => write!(f, "15m"),
            Self::PerThirtyMinutes => write!(f, "30m"),
            Self::PerHour => write!(f, "1h"),
            Self::PerDay => write!(f, "Day"),
            Self::PerWeek => write!(f, "Week"),
            Self::PerMonth => write!(f, "Month"),
            Self::PerYear => write!(f, "Year"),
        }
    }
}

impl KlineType {
    /// Get next period type
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::PerMinute => Self::PerFiveMinutes,
            Self::PerFiveMinutes => Self::PerFifteenMinutes,
            Self::PerFifteenMinutes => Self::PerThirtyMinutes,
            Self::PerThirtyMinutes => Self::PerHour,
            Self::PerHour => Self::PerDay,
            Self::PerDay => Self::PerWeek,
            Self::PerWeek => Self::PerMonth,
            Self::PerMonth | Self::PerYear => Self::PerYear,
        }
    }

    /// Get previous period type
    #[must_use]
    pub fn prev(self) -> Self {
        match self {
            Self::PerMinute | Self::PerFiveMinutes => Self::PerMinute,
            Self::PerFifteenMinutes => Self::PerFiveMinutes,
            Self::PerThirtyMinutes => Self::PerFifteenMinutes,
            Self::PerHour => Self::PerThirtyMinutes,
            Self::PerDay => Self::PerHour,
            Self::PerWeek => Self::PerDay,
            Self::PerMonth => Self::PerWeek,
            Self::PerYear => Self::PerMonth,
        }
    }

    pub fn iter() -> impl Iterator<Item = Self> {
        <Self as strum::IntoEnumIterator>::iter()
    }
}

/// Adjustment type
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AdjustType {
    #[default]
    NoAdjust,
    ForwardAdjust,
}

/// Candlestick data (detailed version with adjustment factors)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Kline {
    pub timestamp: i64,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub amount: u64,       // Volume
    pub balance: Decimal,  // Turnover
    pub factor_a: Decimal, // Adjustment factor A
    pub factor_b: Decimal, // Adjustment factor B
    pub total: u64,        // Number of trades
}

impl Default for Kline {
    fn default() -> Self {
        Self {
            timestamp: 0,
            open: Decimal::ZERO,
            high: Decimal::ZERO,
            low: Decimal::ZERO,
            close: Decimal::ZERO,
            amount: 0,
            balance: Decimal::ZERO,
            factor_a: Decimal::ONE,
            factor_b: Decimal::ZERO,
            total: 0,
        }
    }
}

/// Candlestick collection
pub type Klines = Vec<Kline>;

/// Subscription type
#[derive(Clone, Copy, Debug)]
pub enum SubTypes {
    LIST,
    DETAIL,
    DEPTH,
    TRADES,
}

impl std::ops::BitOr for SubTypes {
    type Output = Self;

    fn bitor(self, _rhs: Self) -> Self::Output {
        // Simplified implementation: return DETAIL (contains most info)
        Self::DETAIL
    }
}

/// Currency
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Currency {
    #[default]
    HKD,
    USD,
    CNY,
    SGD,
}

impl Currency {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::HKD => "HKD",
            Self::USD => "USD",
            Self::CNY => "CNY",
            Self::SGD => "SGD",
        }
    }
}

/// Market/Region
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum Market {
    #[default]
    HK,
    US,
    CN,
    SG,
}

impl From<&str> for Market {
    fn from(s: &str) -> Self {
        match s {
            "US" => Self::US,
            "CN" | "SH" | "SZ" => Self::CN,
            "SG" => Self::SG,
            _ => Self::HK,
        }
    }
}

impl Market {
    /// Get local time string (simplified implementation)
    pub fn local_time(self) -> String {
        use time::OffsetDateTime;
        let now = OffsetDateTime::now_utc();
        format!("{:02}:{:02}", now.hour(), now.minute())
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::US => "US",
            Self::HK => "HK",
            Self::CN => "CN",
            Self::SG => "SG",
        }
    }

    /// Check if a date is in US Daylight Saving Time (EDT)
    /// DST starts: Second Sunday of March at 02:00
    /// DST ends: First Sunday of November at 02:00
    fn is_us_daylight_saving_time(dt: time::OffsetDateTime) -> bool {
        use time::{Month, Weekday};

        let month = dt.month();
        let year = dt.year();

        // DST is only between March and November
        match month {
            Month::January | Month::February | Month::December => false,
            Month::April
            | Month::May
            | Month::June
            | Month::July
            | Month::August
            | Month::September
            | Month::October => true,
            Month::March => {
                // Find second Sunday of March
                let second_sunday =
                    Self::nth_weekday_of_month(year, Month::March, Weekday::Sunday, 2);
                dt.ordinal() >= second_sunday
            }
            Month::November => {
                // Find first Sunday of November
                let first_sunday =
                    Self::nth_weekday_of_month(year, Month::November, Weekday::Sunday, 1);
                dt.ordinal() < first_sunday
            }
        }
    }

    /// Find the Nth occurrence of a weekday in a given month
    fn nth_weekday_of_month(year: i32, month: time::Month, weekday: time::Weekday, n: u8) -> u16 {
        use time::Date;

        // Start from the first day of the month
        let first_day = Date::from_calendar_date(year, month, 1).unwrap();
        let first_weekday = first_day.weekday();

        // Calculate days until first occurrence of target weekday
        #[allow(clippy::cast_sign_loss)]
        let days_until_first = ((i16::from(weekday.number_from_monday())
            - i16::from(first_weekday.number_from_monday())
            + 7)
            % 7) as u8;

        // Calculate the date of the Nth occurrence
        let target_day = 1 + days_until_first + (n - 1) * 7;

        // Convert to ordinal (day of year)
        Date::from_calendar_date(year, month, target_day)
            .unwrap()
            .ordinal()
    }

    /// Check if market is in trading session (simplified implementation)
    pub fn is_trading(self) -> bool {
        use time::{OffsetDateTime, Weekday};
        let now = OffsetDateTime::now_utc();

        // Check if it's weekend (Saturday or Sunday)
        // Note: Need to check in the market's local timezone, not UTC
        let local_time = match self {
            Self::US => {
                // Use correct offset based on DST
                let offset = if Self::is_us_daylight_saving_time(now) {
                    time::UtcOffset::from_hms(-4, 0, 0).unwrap() // EDT
                } else {
                    time::UtcOffset::from_hms(-5, 0, 0).unwrap() // EST
                };
                now.to_offset(offset)
            }
            Self::HK | Self::CN | Self::SG => {
                now.to_offset(time::UtcOffset::from_hms(8, 0, 0).unwrap())
            } // HKT/CST/SGT
        };

        // Markets are closed on weekends
        if matches!(local_time.weekday(), Weekday::Saturday | Weekday::Sunday) {
            return false;
        }

        // Get current hour and minute (UTC)
        let hour = now.hour();
        let minute = now.minute();
        let time_minutes = u32::from(hour) * 60 + u32::from(minute);

        match self {
            // US: Trading hours 09:30-16:00 local time
            // EST (UTC-5): 14:30-21:00 UTC (November - March)
            // EDT (UTC-4): 13:30-20:00 UTC (March - November)
            Self::US => {
                if Self::is_us_daylight_saving_time(now) {
                    // EDT: 13:30-20:00 UTC
                    (13 * 60 + 30..20 * 60).contains(&time_minutes)
                } else {
                    // EST: 14:30-21:00 UTC
                    (14 * 60 + 30..21 * 60).contains(&time_minutes)
                }
            }
            // HK: 01:30-08:00 UTC (Hong Kong time 09:30-16:00)
            Self::HK => {
                (60 + 30..4 * 60).contains(&time_minutes)
                    || (5 * 60..8 * 60).contains(&time_minutes)
            }
            // CN: 01:30-07:00 UTC (Beijing time 09:30-15:00)
            Self::CN => {
                (60 + 30..3 * 60).contains(&time_minutes)
                    || (5 * 60..7 * 60).contains(&time_minutes)
            }
            // SG: 01:00-09:00 UTC (Singapore time 09:00-17:00)
            Self::SG => (60..9 * 60).contains(&time_minutes),
        }
    }

    /// Get market sort priority (lower number = higher priority)
    pub fn sort_priority(self) -> u8 {
        if self.is_trading() {
            // Markets in trading session have highest priority
            0
        } else {
            // Non-trading hours use default order: US=1, HK=2, CN=3, SG=4
            match self {
                Self::US => 1,
                Self::HK => 2,
                Self::CN => 3,
                Self::SG => 4,
            }
        }
    }

    /// Get market color
    pub fn color(self) -> (u8, u8, u8) {
        match self {
            Self::US => (0x5F, 0xD7, 0xFF), // LightBlue
            Self::HK => (0xFF, 0x5F, 0xFF), // LightMagenta
            Self::CN => (0xFF, 0x5F, 0x5F), // LightRed
            Self::SG => (0x5F, 0xFF, 0xFF), // LightCyan
        }
    }
}

impl std::fmt::Display for Market {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Quote data
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QuoteData {
    pub last_done: Option<Decimal>,  // Last price
    pub prev_close: Option<Decimal>, // Previous close
    pub open: Option<Decimal>,       // Open price
    pub high: Option<Decimal>,       // High price
    pub low: Option<Decimal>,        // Low price
    pub volume: u64,                 // Volume
    pub turnover: Decimal,           // Turnover
    pub timestamp: i64,              // Timestamp
}

/// Candlestick data
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candlestick {
    pub timestamp: i64,
    pub open: Decimal,
    pub high: Decimal,
    pub low: Decimal,
    pub close: Decimal,
    pub volume: u64,
    pub turnover: Decimal,
}

/// Depth data
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Depth {
    pub position: i32,  // Position level
    pub price: Decimal, // Price
    pub volume: i64,    // Volume
    pub order_num: i64, // Number of orders
}

/// Depth view
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DepthData {
    pub asks: Vec<Depth>, // Ask orders
    pub bids: Vec<Depth>, // Bid orders
}

/// Static stock information
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StaticInfo {
    pub symbol: String,                  // Stock symbol
    pub name_cn: String,                 // Chinese name
    pub name_en: String,                 // English name
    pub name_hk: String,                 // Traditional Chinese name
    pub exchange: String,                // Exchange
    pub currency: String,                // Currency
    pub lot_size: i32,                   // Lot size
    pub total_shares: i64,               // Total shares
    pub circulating_shares: i64,         // Circulating shares
    pub hk_shares: i64,                  // Hong Kong shares
    pub eps: Option<Decimal>,            // Earnings per share
    pub eps_ttm: Option<Decimal>,        // Earnings per share (TTM)
    pub bps: Option<Decimal>,            // Book value per share
    pub dividend_yield: Option<Decimal>, // Dividend yield
    pub stock_derivatives: Vec<i32>,     // Supported derivative types
    pub board: String,                   // Board
}

/// Trade direction
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TradeDirection {
    Neutral,
    Up,
    Down,
}

/// Single trade record
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TradeData {
    pub price: Decimal,
    pub volume: i64,
    pub timestamp: i64,
    pub trade_type: String,
    pub direction: TradeDirection,
}

impl Default for TradeData {
    fn default() -> Self {
        Self {
            price: Decimal::ZERO,
            volume: 0,
            timestamp: 0,
            trade_type: String::new(),
            direction: TradeDirection::Neutral,
        }
    }
}
