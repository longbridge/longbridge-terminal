use crate::data::{
    Account, AccountBalance, AccountList, CashBalance, CashInfo, Currency, Holding, MarketAccount,
    OverviewData, PortfolioView,
};
use crate::openapi;
use anyhow::Result;
use rust_decimal::Decimal;
use std::collections::HashMap;

/// Get account list
pub async fn fetch_account_list() -> Result<AccountList> {
    let ctx = openapi::trade();

    // Longbridge SDK's account_balance returns current account balance info
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

    // Resolve the channel from the access token JWT (`lb`, `lb_papertrading`,
    // etc.) so paper-trading and other non-default channels surface accurately
    // to the TUI / CLI without an extra HTTP roundtrip.
    let account = Account {
        account_channel: crate::auth::account_channel_or_default(),
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
            abbreviation_multi_name: t!("Currency.HKD").to_string(),
            multi_name: t!("Currency.HKD").to_string(),
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
            abbreviation_multi_name: t!("Currency.USD").to_string(),
            multi_name: t!("Currency.USD").to_string(),
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
            abbreviation_multi_name: t!("Currency.CNY").to_string(),
            multi_name: t!("Currency.CNY").to_string(),
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

/// Fetch account balance from Longbridge SDK
pub async fn fetch_account_balance() -> Result<AccountBalance> {
    let ctx = openapi::trade();
    let balances = ctx.account_balance(Some("USD")).await?;

    // Take the first account (user typically has one main account)
    let response = balances
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No account balance found"))?;

    // Map Longbridge response to our AccountBalance structure
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

/// Fetch stock holdings from Longbridge SDK
/// Stock holdings enriched with live quotes from the caller-supplied
/// `fetch_quotes`, so the transport is the caller's choice: the TUI passes the
/// WebSocket `QuoteContext`, the one-shot CLI passes HTTP REST. Keeps `openapi/`
/// free of any dependency on `cli/`.
pub async fn fetch_stock_holdings_with<F, Fut>(fetch_quotes: F) -> Result<Vec<Holding>>
where
    F: Fn(Vec<String>) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Vec<longbridge::quote::SecurityQuote>>>,
{
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
                market_value_usd: None, // Will be updated in fetch_portfolio
                market_price: position.cost_price, // Will be updated with real price
                prev_close: None,
            });
        }
    }

    // Fetch real-time quotes for all holdings
    if !symbols.is_empty() {
        if let Ok(quotes) = fetch_quotes(symbols).await {
            // Create a map for quick lookup: symbol -> (last_done, prev_close)
            let mut quote_map: std::collections::HashMap<
                String,
                (rust_decimal::Decimal, rust_decimal::Decimal),
            > = std::collections::HashMap::new();

            for quote in quotes {
                // Use the most recently traded price across all sessions (regular, post-market, overnight)
                // to match how the mobile client computes P/L (includes overnight session prices).
                let mut best_price = quote.last_done;
                let mut best_ts = quote.timestamp;
                for ext in [
                    &quote.post_market_quote,
                    &quote.overnight_quote,
                    &quote.pre_market_quote,
                ]
                .into_iter()
                .flatten()
                {
                    if ext.last_done > Decimal::ZERO && ext.timestamp > best_ts {
                        best_price = ext.last_done;
                        best_ts = ext.timestamp;
                    }
                }
                quote_map.insert(quote.symbol.clone(), (best_price, quote.prev_close));
            }

            // Update market prices and market values with real-time data
            for holding in &mut holdings {
                if let Some(&(real_price, prev_close)) = quote_map.get(&holding.symbol) {
                    holding.market_price = real_price;
                    holding.market_value = real_price * holding.quantity;
                    holding.prev_close = Some(prev_close);
                }
            }
        }
    }

    Ok(holdings)
}

/// Fetch exchange rates and return a map of currency code -> USD per 1 unit.
/// See [`build_usd_rates`] for how the table is interpreted.
async fn fetch_fx_rates() -> HashMap<String, Decimal> {
    use longbridge::httpclient::Json;
    use reqwest::Method;

    let client = openapi::http_client();
    let Ok(resp) = client
        .request(Method::GET, "/v1/asset/exchange_rates")
        .response::<Json<serde_json::Value>>()
        .send()
        .await
    else {
        return HashMap::new();
    };

    tracing::debug!("exchange_rates raw: {}", resp.0);

    let mut pairs = Vec::new();
    if let Some(exchanges) = resp.0["exchanges"].as_array() {
        for item in exchanges {
            let base = item["base_currency"].as_str().unwrap_or("");
            let other = item["other_currency"].as_str().unwrap_or("");
            let rate = if let Some(n) = item["average_rate"].as_f64() {
                Decimal::try_from(n).unwrap_or(Decimal::ZERO)
            } else if let Some(s) = item["average_rate"].as_str() {
                s.parse::<Decimal>().unwrap_or(Decimal::ZERO)
            } else {
                continue;
            };
            pairs.push((base.to_string(), other.to_string(), rate));
        }
    }
    let rates = build_usd_rates(&pairs);
    tracing::info!("fx_rates built: {:?}", rates);
    rates
}

/// Build a map of currency code -> USD per 1 unit from `(base, other, rate)`
/// rows of `/v1/asset/exchange_rates`.
///
/// Each row means 1 `other` = `rate` `base`. The table is anchored on one
/// currency, which differs by account:
///   base="USD", other="HKD", rate=0.12746 -> 1 HKD = 0.12746 USD
///   base="HKD", other="USD", rate=7.8     -> 1 USD = 7.8 HKD, so 1 HKD = 1/7.8 USD
///   base="HKD", other="SGD", rate=6.0     -> 1 SGD = 6.0 HKD, converted via HKD
///
/// Currencies that cannot be linked to USD through the table are left out.
fn build_usd_rates(pairs: &[(String, String, Decimal)]) -> HashMap<String, Decimal> {
    let mut rates = HashMap::from([("USD".to_string(), Decimal::ONE)]);
    // Each pass resolves currencies one hop further from USD; a table needs at
    // most `pairs.len()` hops, and stops early once nothing new is resolved.
    for _ in 0..pairs.len() {
        let mut changed = false;
        for (base, other, rate) in pairs {
            if *rate <= Decimal::ZERO || base == other {
                continue;
            }
            match (rates.get(base).copied(), rates.get(other).copied()) {
                (Some(usd_per_base), None) => {
                    rates.insert(other.clone(), *rate * usd_per_base);
                    changed = true;
                }
                (None, Some(usd_per_other)) => {
                    rates.insert(base.clone(), usd_per_other / *rate);
                    changed = true;
                }
                _ => {}
            }
        }
        if !changed {
            break;
        }
    }
    rates
}

/// Convert an amount in `currency` to USD.
/// `rates` maps currency code -> USD per 1 unit of that currency.
/// Returns `None` when there is no rate, rather than passing the amount through
/// unconverted under a USD label.
fn to_usd(
    amount: Decimal,
    currency: Currency,
    rates: &HashMap<String, Decimal>,
) -> Option<Decimal> {
    let code = currency.as_str();
    if code == "USD" {
        return Some(amount);
    }
    rates
        .get(code)
        .filter(|usd_per_unit| **usd_per_unit > Decimal::ZERO)
        .map(|usd_per_unit| amount * usd_per_unit)
}

/// Calculate overview data from balance and holdings.
/// All P/L figures are expressed in USD (matching the balance currency).
fn calculate_overview(
    balance: &AccountBalance,
    holdings: &[Holding],
    fx_rates: &HashMap<String, Decimal>,
) -> OverviewData {
    // market_cap from SDK balance (balance currency is already USD)
    let market_cap: Decimal = balance.net_assets - balance.total_cash;

    // Total P/L: compute directly per holding to avoid net_assets/total_cash ambiguity.
    // Each holding's P/L is in its native currency; convert to USD via fx_rates.
    // `None` if any holding's currency has no rate, so a partial sum is never
    // reported as the total.
    let total_pl: Option<Decimal> = holdings
        .iter()
        .map(|h| {
            if let Some(cost) = h.cost_price {
                to_usd((h.market_price - cost) * h.quantity, h.currency, fx_rates)
            } else {
                Some(Decimal::ZERO)
            }
        })
        .sum();

    // Intraday P/L in USD: (current_price - prev_close) * quantity, converted
    let total_today_pl: Option<Decimal> = holdings
        .iter()
        .map(|h| {
            if let Some(prev_close) = h.prev_close {
                if prev_close > Decimal::ZERO {
                    let daily_change = (h.market_price - prev_close) * h.quantity;
                    return to_usd(daily_change, h.currency, fx_rates);
                }
            }
            Some(Decimal::ZERO)
        })
        .sum();

    OverviewData {
        total_asset: balance.net_assets,
        market_cap,
        total_cash: balance.total_cash,
        total_pl,
        total_today_pl,
        margin_call: balance.margin_call,
        risk_level: balance.risk_level,
        credit_limit: balance.max_finance_amount,
        leverage_ratio: Decimal::ZERO,
        fund_market_value: Decimal::ZERO,
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

/// Fetch complete portfolio data, with holdings quoted over the WebSocket
/// `QuoteContext` (the TUI's path). One-shot CLI callers use
/// [`fetch_portfolio_with`] to quote over HTTP REST instead.
pub async fn fetch_portfolio() -> Result<PortfolioView> {
    fetch_portfolio_with(|symbols| async move {
        openapi::quote()
            .quote(&symbols)
            .await
            .map_err(anyhow::Error::from)
    })
    .await
}

/// [`fetch_portfolio`] with a caller-supplied quote fetcher (see
/// [`fetch_stock_holdings_with`]), so the transport is the caller's choice.
pub async fn fetch_portfolio_with<F, Fut>(fetch_quotes: F) -> Result<PortfolioView>
where
    F: Fn(Vec<String>) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Vec<longbridge::quote::SecurityQuote>>>,
{
    // Fetch balance, holdings, and FX rates concurrently
    let (balance_result, holdings_result, fx_rates) = tokio::join!(
        fetch_account_balance(),
        fetch_stock_holdings_with(fetch_quotes),
        fetch_fx_rates()
    );

    let balance = balance_result?;
    let mut holdings = holdings_result?;

    // Compute USD market value for each holding
    for holding in &mut holdings {
        holding.market_value_usd = to_usd(holding.market_value, holding.currency, &fx_rates);
    }

    // Calculate aggregated metrics with FX-aware P/L
    let overview = calculate_overview(&balance, &holdings, &fx_rates);
    let market_accounts = group_by_market(&holdings);
    let cash_balances = extract_cash_balances(&balance);

    Ok(PortfolioView {
        overview,
        market_accounts,
        cash_balances,
        holdings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn pairs(rows: &[(&str, &str, &str)]) -> Vec<(String, String, Decimal)> {
        rows.iter()
            .map(|(b, o, r)| ((*b).to_string(), (*o).to_string(), d(r)))
            .collect()
    }

    #[test]
    fn usd_anchored_table_is_stored_as_is() {
        // Live AP account shape: 1 other = rate USD.
        let rates = build_usd_rates(&pairs(&[
            ("USD", "USD", "1"),
            ("USD", "HKD", "0.12746"),
            ("USD", "SGD", "0.78008"),
        ]));
        assert_eq!(rates["HKD"], d("0.12746"));
        assert_eq!(rates["SGD"], d("0.78008"));
        assert_eq!(
            to_usd(d("1000"), Currency::HKD, &rates),
            Some(d("127.46000"))
        );
    }

    #[test]
    fn hkd_anchored_table_is_inverted_and_cross_converted() {
        // Paper account shape: 1 other = rate HKD, with only HKD/USD linking to USD.
        let rates = build_usd_rates(&pairs(&[
            ("HKD", "HKD", "1"),
            ("HKD", "USD", "7.0001"),
            ("HKD", "SGD", "6.1442"),
            ("HKD", "CNY", "1.1465"),
        ]));
        let usd_per_hkd = Decimal::ONE / d("7.0001");
        assert_eq!(rates["HKD"], usd_per_hkd);
        assert_eq!(rates["SGD"], d("6.1442") * usd_per_hkd);
        assert_eq!(rates["CNY"], d("1.1465") * usd_per_hkd);

        // 20,320 HKD is about 2,902.82 USD, not 20,320 * 7.0001.
        let usd = to_usd(d("20320"), Currency::HKD, &rates).unwrap();
        assert_eq!(usd.round_dp(2), d("2902.82"));
    }

    #[test]
    fn currency_without_a_rate_is_not_passed_through() {
        let rates = build_usd_rates(&pairs(&[("USD", "HKD", "0.12746")]));
        assert_eq!(to_usd(d("100"), Currency::SGD, &rates), None);
        assert_eq!(to_usd(d("100"), Currency::HKD, &HashMap::new()), None);
        assert_eq!(
            to_usd(d("100"), Currency::USD, &HashMap::new()),
            Some(d("100"))
        );
    }

    #[test]
    fn overview_totals_are_none_when_a_holding_cannot_be_converted() {
        let rates = build_usd_rates(&pairs(&[("USD", "HKD", "0.12746")]));
        let holding = |currency, price: &str, cost: &str| Holding {
            currency,
            quantity: d("10"),
            market_price: d(price),
            cost_price: Some(d(cost)),
            prev_close: Some(d(cost)),
            ..Default::default()
        };
        let balance = AccountBalance::default();

        let converted = calculate_overview(
            &balance,
            &[
                holding(Currency::USD, "11", "10"),
                holding(Currency::HKD, "110", "100"),
            ],
            &rates,
        );
        assert_eq!(converted.total_pl, Some(d("10") + d("100") * d("0.12746")));

        let partial = calculate_overview(
            &balance,
            &[
                holding(Currency::USD, "11", "10"),
                holding(Currency::SGD, "2", "1"),
            ],
            &rates,
        );
        assert_eq!(partial.total_pl, None);
        assert_eq!(partial.total_today_pl, None);
    }
}
