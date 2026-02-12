use crate::data::{
    Account, AccountBalance, AccountList, CashBalance, CashInfo, Holding, MarketAccount,
    OverviewData, PortfolioView,
};
use crate::openapi;
use anyhow::Result;
use rust_decimal::Decimal;
use std::collections::HashMap;

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
            tracing::warn!(
                "Failed to fetch account balance (may lack trading permission): {}",
                e
            );
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
        status: vec![account],
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
pub fn currencies(account_channel: &str) -> Result<Vec<CurrencyInfo>> {
    // OpenAPI may not directly provide currency list API
    // Return some common currencies as default
    Ok(vec![
        CurrencyInfo {
            currency: "HKD".to_string(),
            currency_iso: "HKD".to_string(),
            symbol: "HK$".to_string(),
            icon: "$".to_string(),
            logo: String::new(),
            abbreviation_multi_name: rust_i18n::t!("Currency.HKD"),
            multi_name: rust_i18n::t!("Currency.HKD"),
            min_exchange_amount: "0".to_string(),
            min_withdrawal_amount: "0".to_string(),
            exchange_rate_precision: 6,
            amount_precision: 2,
            amount_round_mode: "truncate".to_string(),
            json_config: "{}".to_string(),
            account_channel: account_channel.to_string(),
        },
        CurrencyInfo {
            currency: "USD".to_string(),
            currency_iso: "USD".to_string(),
            symbol: "US$".to_string(),
            icon: "$".to_string(),
            logo: String::new(),
            abbreviation_multi_name: rust_i18n::t!("Currency.USD"),
            multi_name: rust_i18n::t!("Currency.USD"),
            min_exchange_amount: "0".to_string(),
            min_withdrawal_amount: "0".to_string(),
            exchange_rate_precision: 6,
            amount_precision: 2,
            amount_round_mode: "truncate".to_string(),
            json_config: "{}".to_string(),
            account_channel: account_channel.to_string(),
        },
        CurrencyInfo {
            currency: "CNY".to_string(),
            currency_iso: "CNY".to_string(),
            symbol: "¥".to_string(),
            icon: "¥".to_string(),
            logo: String::new(),
            abbreviation_multi_name: rust_i18n::t!("Currency.CNY"),
            multi_name: rust_i18n::t!("Currency.CNY"),
            min_exchange_amount: "0".to_string(),
            min_withdrawal_amount: "0".to_string(),
            exchange_rate_precision: 6,
            amount_precision: 2,
            amount_round_mode: "truncate".to_string(),
            json_config: "{}".to_string(),
            account_channel: account_channel.to_string(),
        },
    ])
}

/// Fetch account balance from Longport SDK
pub async fn fetch_account_balance() -> Result<AccountBalance> {
    let ctx = openapi::trade();
    let balances = ctx.account_balance(None).await?;

    // Take the first account (user typically has one main account)
    let response = balances
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No account balance found"))?;

    // Map longport response to our AccountBalance structure
    let mut cash_infos = Vec::new();
    for cash_info in &response.cash_infos {
        cash_infos.push(CashInfo {
            withdraw_cash: cash_info.withdraw_cash,
            available_cash: cash_info.available_cash,
            frozen_cash: cash_info.frozen_cash,
            settling_cash: cash_info.settling_cash,
            currency: cash_info.currency.clone(),
        });
    }

    Ok(AccountBalance {
        total_cash: response.total_cash,
        max_finance_amount: response.max_finance_amount,
        remaining_finance_amount: response.remaining_finance_amount,
        #[allow(clippy::cast_sign_loss)]
        risk_level: response.risk_level as u8,
        margin_call: response.margin_call,
        currency: response.currency,
        net_assets: response.net_assets,
        init_margin: response.init_margin,
        maintenance_margin: response.maintenance_margin,
        buy_power: response.buy_power,
        cash_infos,
    })
}

/// Fetch stock holdings from Longport SDK
pub async fn fetch_stock_holdings() -> Result<Vec<Holding>> {
    let ctx = openapi::trade();
    let response = ctx.stock_positions(None).await?;

    let mut holdings = Vec::new();
    let mut symbols = Vec::new();

    // First, collect all holdings with basic info
    for channel in response.channels {
        for position in &channel.positions {
            // Map currency string to Currency enum
            let currency = match position.currency.as_str() {
                "USD" => crate::data::Currency::USD,
                "CNY" => crate::data::Currency::CNY,
                "SGD" => crate::data::Currency::SGD,
                _ => crate::data::Currency::HKD,
            };

            symbols.push(position.symbol.clone());

            holdings.push(Holding {
                symbol: position.symbol.clone(),
                name: position.symbol_name.clone(),
                currency,
                quantity: position.quantity,
                available_quantity: position.available_quantity,
                cost_price: Some(position.cost_price),
                market_value: position.cost_price * position.quantity, // Will be updated with real price
                market_price: position.cost_price, // Will be updated with real price
            });
        }
    }

    // Fetch real-time quotes for all holdings
    if !symbols.is_empty() {
        let quote_ctx = openapi::quote();
        if let Ok(quotes) = quote_ctx.quote(&symbols).await {
            // Create a map for quick lookup
            let mut quote_map: std::collections::HashMap<String, rust_decimal::Decimal> =
                std::collections::HashMap::new();

            for quote in quotes {
                quote_map.insert(quote.symbol.clone(), quote.last_done);
            }

            // Update market prices and market values with real-time data
            for holding in &mut holdings {
                if let Some(&real_price) = quote_map.get(&holding.symbol) {
                    holding.market_price = real_price;
                    holding.market_value = real_price * holding.quantity;
                }
            }
        }
    }

    Ok(holdings)
}

/// Calculate overview data from balance and holdings
fn calculate_overview(balance: &AccountBalance, holdings: &[Holding]) -> OverviewData {
    // Trust SDK's net_assets which includes cash + actual market value
    // Market cap = net_assets - total_cash
    let market_cap: Decimal = balance.net_assets - balance.total_cash;

    // Calculate total cost (all holdings)
    let total_cost: Decimal = holdings
        .iter()
        .filter_map(|h| h.cost_price.map(|cost| cost * h.quantity))
        .sum();

    // Calculate total P/L: market_value - cost
    // Since market_cap = actual market value from SDK
    let total_pl: Decimal = market_cap - total_cost;

    // Calculate intraday P/L using STOCKS cache (if available)
    // Intraday P/L = (current_price - prev_close) * quantity
    let total_today_pl: Decimal = holdings
        .iter()
        .map(|h| {
            let counter = crate::data::Counter::new(&h.symbol);
            if let Some(stock) = crate::data::STOCKS.get(&counter) {
                if let (Some(last_done), Some(prev_close)) =
                    (stock.quote.last_done, stock.quote.prev_close)
                {
                    // Intraday P/L = (current_price - prev_close) * quantity
                    return (last_done - prev_close) * h.quantity;
                }
            }
            Decimal::ZERO
        })
        .sum();

    // Calculate leverage ratio (simplified)
    let leverage_ratio = if balance.net_assets > Decimal::ZERO {
        (balance.total_cash - balance.net_assets) / balance.net_assets
    } else {
        Decimal::ZERO
    };

    OverviewData {
        // net_assets from SDK already includes cash and market value
        total_asset: balance.net_assets,
        market_cap,
        total_cash: balance.total_cash,
        total_pl,
        total_today_pl,
        margin_call: balance.margin_call,
        risk_level: balance.risk_level,
        credit_limit: balance.max_finance_amount,
        leverage_ratio,
        fund_market_value: Decimal::ZERO, // Not implemented yet
        currency: balance.currency.clone(),
    }
}

/// Group holdings by market
fn group_by_market(holdings: &[Holding]) -> HashMap<crate::data::Market, MarketAccount> {
    let mut markets = HashMap::new();

    for holding in holdings {
        // Parse market from symbol (e.g., "700.HK" -> HK)
        let market = if let Some(dot_pos) = holding.symbol.rfind('.') {
            let market_str = &holding.symbol[dot_pos + 1..];
            match market_str {
                "US" => crate::data::Market::US,
                "SH" | "SZ" => crate::data::Market::CN,
                "SG" => crate::data::Market::SG,
                _ => crate::data::Market::HK,
            }
        } else {
            crate::data::Market::HK
        };

        let account = markets.entry(market).or_insert_with(|| MarketAccount {
            market,
            currency: holding.currency,
            ..Default::default()
        });

        account.market_value += holding.market_value;
        // Calculate P/L if cost_price is available
        if let Some(cost) = holding.cost_price {
            account.pl += holding.market_value - (cost * holding.quantity);
        }
    }

    markets
}

/// Extract cash balances from account balance
fn extract_cash_balances(balance: &AccountBalance) -> Vec<CashBalance> {
    balance
        .cash_infos
        .iter()
        .map(|info| {
            let currency = match info.currency.as_str() {
                "USD" => crate::data::Currency::USD,
                "CNY" => crate::data::Currency::CNY,
                "SGD" => crate::data::Currency::SGD,
                _ => crate::data::Currency::HKD,
            };

            CashBalance {
                currency,
                total_amount: info.available_cash + info.frozen_cash,
                balance: info.available_cash,
                frozen_cash: info.frozen_cash,
                withdraw_cash: info.withdraw_cash,
            }
        })
        .collect()
}

/// Fetch complete portfolio data
pub async fn fetch_portfolio() -> Result<PortfolioView> {
    // Fetch data concurrently
    let (balance_result, holdings_result) =
        tokio::join!(fetch_account_balance(), fetch_stock_holdings());

    let balance = balance_result?;
    let holdings = holdings_result?;

    // Calculate aggregated metrics
    let overview = calculate_overview(&balance, &holdings);
    let market_accounts = group_by_market(&holdings);
    let cash_balances = extract_cash_balances(&balance);

    Ok(PortfolioView {
        overview,
        market_accounts,
        cash_balances,
        holdings,
    })
}
