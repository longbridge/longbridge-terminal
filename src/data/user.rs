use serde::{Deserialize, Serialize};

/// User information (simplified)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    pub account_channel: String,
    pub aaid: String,
    pub base_currency: String,
}

impl Default for User {
    fn default() -> Self {
        Self {
            account_channel: String::new(),
            aaid: String::new(),
            base_currency: "HKD".to_string(),
        }
    }
}

impl User {
    pub fn get_account_channel(&self) -> &str {
        &self.account_channel
    }
}

/// Organization information
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OrgInfo {
    pub name: String,
}

/// Account information
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub account_channel: String,
    pub aaid: String,
    pub account_name: String,
    pub account_type: String,
    pub org: OrgInfo,
}

impl Default for Account {
    fn default() -> Self {
        Self {
            account_channel: String::new(),
            aaid: String::new(),
            account_name: "Default Account".to_string(),
            account_type: "CashAccount".to_string(),
            org: OrgInfo {
                name: "Longbridge".to_string(),
            },
        }
    }
}

impl Account {
    pub fn is_open(&self) -> bool {
        self.account_type == "MarginAccount" || self.account_type == "CashAccount"
    }
}

/// Account list response
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountList {
    pub status: Vec<Account>,
}

/// Cash information
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CashInfo {
    pub withdraw_cash: rust_decimal::Decimal,
    pub available_cash: rust_decimal::Decimal,
    pub frozen_cash: rust_decimal::Decimal,
    pub settling_cash: rust_decimal::Decimal,
    pub currency: String,
}

/// Account balance information
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AccountBalance {
    pub total_cash: rust_decimal::Decimal,
    pub max_finance_amount: rust_decimal::Decimal,
    pub remaining_finance_amount: rust_decimal::Decimal,
    pub risk_level: u8,
    pub margin_call: rust_decimal::Decimal,
    pub currency: String,
    pub net_assets: rust_decimal::Decimal,
    pub init_margin: rust_decimal::Decimal,
    pub maintenance_margin: rust_decimal::Decimal,
    pub buy_power: rust_decimal::Decimal,
    pub cash_infos: Vec<CashInfo>,
}

/// Stock holding information
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Holding {
    pub symbol: String,
    pub name: String,
    pub currency: super::Currency,
    pub quantity: rust_decimal::Decimal,
    pub available_quantity: rust_decimal::Decimal,
    pub cost_price: Option<rust_decimal::Decimal>,
    pub market_value: rust_decimal::Decimal,
    pub market_price: rust_decimal::Decimal,
}

impl Default for Holding {
    fn default() -> Self {
        Self {
            symbol: String::new(),
            name: String::new(),
            currency: super::Currency::default(),
            quantity: rust_decimal::Decimal::ZERO,
            available_quantity: rust_decimal::Decimal::ZERO,
            cost_price: None,
            market_value: rust_decimal::Decimal::ZERO,
            market_price: rust_decimal::Decimal::ZERO,
        }
    }
}

/// Overview data
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OverviewData {
    pub total_asset: rust_decimal::Decimal,
    pub market_cap: rust_decimal::Decimal,
    pub total_cash: rust_decimal::Decimal,
    pub total_pl: rust_decimal::Decimal,
    pub total_today_pl: rust_decimal::Decimal,
    pub margin_call: rust_decimal::Decimal,
    pub risk_level: u8,
    pub credit_limit: rust_decimal::Decimal,
    pub leverage_ratio: rust_decimal::Decimal,
    pub fund_market_value: rust_decimal::Decimal,
    pub currency: String,
}

/// Market-specific account data
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MarketAccount {
    pub market: super::Market,
    pub currency: super::Currency,
    pub net_assets: rust_decimal::Decimal,
    pub market_value: rust_decimal::Decimal,
    pub pl: rust_decimal::Decimal,
    pub today_pl: rust_decimal::Decimal,
    pub balance: rust_decimal::Decimal,
    pub frozen_cash: rust_decimal::Decimal,
    pub withdraw_cash: rust_decimal::Decimal,
    pub max_buy_limit: rust_decimal::Decimal,
}

/// Cash balance by currency
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CashBalance {
    pub currency: super::Currency,
    pub total_amount: rust_decimal::Decimal,
    pub balance: rust_decimal::Decimal,
    pub frozen_cash: rust_decimal::Decimal,
    pub withdraw_cash: rust_decimal::Decimal,
}

/// Complete portfolio view
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PortfolioView {
    pub overview: OverviewData,
    pub market_accounts: std::collections::HashMap<super::Market, MarketAccount>,
    pub cash_balances: Vec<CashBalance>,
    pub holdings: Vec<Holding>,
}
