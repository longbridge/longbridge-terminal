use anyhow::Result;
use crate::data::{Account, AccountList};
use crate::openapi;

/// Get account list
pub async fn fetch_account_list() -> Result<AccountList> {
    let ctx = openapi::trade();

    // longport SDK's account_balance returns current account balance info
    // For simplicity, we return a default account
    // Note: This call may fail (if Access Token lacks trading permission), but should not block app startup
    match ctx.account_balance(None).await {
        Ok(_balance) => {
            tracing::info!("Successfully fetched account balance");
        }
        Err(e) => {
            tracing::warn!("Failed to fetch account balance (may lack trading permission): {}", e);
            // Continue execution, do not block app startup
        }
    }

    // Create a default account
    let account = Account {
        account_channel: "lb".to_string(),
        aaid: String::new(),
        account_name: "Default Account".to_string(),
        account_type: "CashAccount".to_string(),
        org: crate::data::OrgInfo {
            name: "Longbridge".to_string(),
        },
    };

    Ok(AccountList {
        status: vec![account]
    })
}

/// Currency information (simplified)
#[derive(Clone, Debug, serde::Deserialize)]
pub struct CurrencyInfo {
    pub currency: String,
    pub currency_iso: String,
    pub symbol: String,
    pub icon: String,
    pub logo: String,
    pub abbreviation_multi_name: String,
    pub multi_name: String,
    pub min_exchange_amount: String,
    pub min_withdrawal_amount: String,
    pub exchange_rate_precision: u8,
    pub amount_precision: u8,
    pub amount_round_mode: String,
    pub json_config: String,
    pub account_channel: String,
}

/// Get currency list (simplified implementation, returns common currencies)
pub async fn currencies(_account_channel: &str) -> Result<Vec<CurrencyInfo>> {
    // OpenAPI may not directly provide currency list API
    // Return some common currencies as default
    Ok(vec![
        CurrencyInfo {
            currency: "HKD".to_string(),
            currency_iso: "HKD".to_string(),
            symbol: "HK$".to_string(),
            icon: "$".to_string(),
            logo: String::new(),
            abbreviation_multi_name: "港币".to_string(),
            multi_name: "港币".to_string(),
            min_exchange_amount: "0".to_string(),
            min_withdrawal_amount: "0".to_string(),
            exchange_rate_precision: 6,
            amount_precision: 2,
            amount_round_mode: "truncate".to_string(),
            json_config: "{}".to_string(),
            account_channel: _account_channel.to_string(),
        },
        CurrencyInfo {
            currency: "USD".to_string(),
            currency_iso: "USD".to_string(),
            symbol: "US$".to_string(),
            icon: "$".to_string(),
            logo: String::new(),
            abbreviation_multi_name: "美元".to_string(),
            multi_name: "美元".to_string(),
            min_exchange_amount: "0".to_string(),
            min_withdrawal_amount: "0".to_string(),
            exchange_rate_precision: 6,
            amount_precision: 2,
            amount_round_mode: "truncate".to_string(),
            json_config: "{}".to_string(),
            account_channel: _account_channel.to_string(),
        },
        CurrencyInfo {
            currency: "CNY".to_string(),
            currency_iso: "CNY".to_string(),
            symbol: "¥".to_string(),
            icon: "¥".to_string(),
            logo: String::new(),
            abbreviation_multi_name: "人民币".to_string(),
            multi_name: "人民币".to_string(),
            min_exchange_amount: "0".to_string(),
            min_withdrawal_amount: "0".to_string(),
            exchange_rate_precision: 6,
            amount_precision: 2,
            amount_round_mode: "truncate".to_string(),
            json_config: "{}".to_string(),
            account_channel: _account_channel.to_string(),
        },
    ])
}
