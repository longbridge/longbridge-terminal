//! Asset commands for US data-center accounts.
//!
//! The AP asset endpoints (`/v1/asset/account`, `/v1/asset/cashflow`,
//! `/v1/asset/exchange_rates`, ...) are not deployed in the US data center, so
//! `assets`, `portfolio`, `cash-flow` and `exchange-rate` route US sessions here,
//! to the `/v1/us/assets/*` APIs. Every function takes a [`TradeApi`] so the
//! routing is testable with a mock.

use anyhow::{bail, Result};
use longbridge::trade::{
    GetUSTransactionHistoryOptions, USCashAndBuyPower, USMarginStatus, USTransaction,
};
use serde_json::{json, Value};
use time::OffsetDateTime;

use crate::cli::api::TradeApi;
use crate::cli::output::{print_account_banner, print_table};
use crate::cli::trade::{normalize_us_overview, print_json_us, print_us_holdings_pretty};
use crate::cli::OutputFormat;
use crate::utils::counter::counter_id_to_symbol;
use crate::utils::datetime::fmt_rfc3339;

/// Page size used to walk `us_transaction_history`.
const TRANSACTION_PAGE_SIZE: i32 = 20;

/// Upper bound on pages fetched for one `cash-flow` call, so a misbehaving
/// backend that never returns a short page cannot loop forever.
const TRANSACTION_MAX_PAGES: i32 = 500;

/// Fail early for a command whose backend only exists in the AP data center.
pub fn ensure_not_us(is_us: bool, what: &str) -> Result<()> {
    if is_us {
        bail!("{what} is not available for US accounts");
    }
    Ok(())
}

/// A margin status is meaningful only when the server filled in at least one
/// figure; cash accounts get an all-empty object.
fn margin_status_is_empty(m: &USMarginStatus) -> bool {
    [
        &m.nlv,
        &m.margin_equity,
        &m.margin_maintenance_excess,
        &m.mmr,
        &m.reg_t_excess,
        &m.sma,
    ]
    .iter()
    .all(|v| v.is_empty())
}

/// `--format json` shape of [`USCashAndBuyPower`]: the SDK fields, with an
/// empty `margin_status` reported as `null`.
pub fn cash_and_buy_power_json(data: &USCashAndBuyPower) -> Result<Value> {
    let mut v = serde_json::to_value(data)?;
    if margin_status_is_empty(&data.margin_status) {
        v["margin_status"] = Value::Null;
    }
    Ok(v)
}

fn or_dash(s: &str) -> String {
    if s.is_empty() {
        "-".to_string()
    } else {
        s.to_string()
    }
}

/// Key/value rows, skipping fields the server left empty.
fn field_rows(fields: &[(&str, &str)]) -> Vec<Vec<String>> {
    fields
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| vec![(*k).to_string(), (*v).to_string()])
        .collect()
}

fn print_cash_and_buy_power_pretty(data: &USCashAndBuyPower) {
    let b = &data.buy_limit_info;
    let bp_rows = field_rows(&[
        ("Currency", &b.currency),
        ("Cash Buy Power", &b.cash_buy_power),
        ("Overnight Buy Power", &b.overnight_buy_power),
        ("Day-Trade Buy Power", &b.day_trade_buy_power),
        ("Instant Buy Power", &b.instant_buy_power),
        ("Intraday Margin Buy Power", &b.intraday_margin_buy_power),
        ("Option Buy Power", &b.option_buy_power),
        ("Crypto Buy Power", &b.crypto_buy_power),
        ("Margin Maintenance Excess", &b.mme),
    ]);
    if !bp_rows.is_empty() {
        println!("Buying Power");
        print_table(&["Field", "Value"], bp_rows, &OutputFormat::Pretty);
    }

    if !data.cash_list.is_empty() {
        println!();
        println!("Cash");
        let rows = data
            .cash_list
            .iter()
            .map(|c| {
                vec![
                    or_dash(&c.currency),
                    or_dash(&c.total_amount),
                    or_dash(&c.available_cash),
                    or_dash(&c.withdraw_cash),
                    or_dash(&c.frozen_cash),
                    or_dash(&c.settled_cash),
                    or_dash(&c.outstanding),
                ]
            })
            .collect();
        print_table(
            &[
                "Currency",
                "Total",
                "Available",
                "Withdrawable",
                "Frozen",
                "Settled",
                "Outstanding",
            ],
            rows,
            &OutputFormat::Pretty,
        );
    }

    let m = &data.margin_status;
    if !margin_status_is_empty(m) {
        println!();
        println!("Margin Status");
        print_table(
            &["Field", "Value"],
            field_rows(&[
                ("Currency", &m.currency),
                ("Net Liquidation Value", &m.nlv),
                ("Margin Equity", &m.margin_equity),
                ("Margin Maintenance Excess", &m.margin_maintenance_excess),
                ("Maintenance Margin Requirement", &m.mmr),
                ("Reg T Excess", &m.reg_t_excess),
                ("SMA", &m.sma),
            ]),
            &OutputFormat::Pretty,
        );
    }
}

/// `longbridge assets` for US accounts: cash and buying power.
pub async fn run_us_assets(
    api: &dyn TradeApi,
    currency: Option<String>,
    format: &OutputFormat,
) -> Result<()> {
    let data = api.us_cash_and_buy_power(currency).await?;
    print_account_banner(format);
    match format {
        OutputFormat::Json => print_json_us(&cash_and_buy_power_json(&data)?),
        OutputFormat::Pretty => print_cash_and_buy_power_pretty(&data),
    }
    Ok(())
}

/// `--format json` shape of the US `portfolio`: the normalized asset overview
/// (as `positions --format json` prints it) plus `cash_and_buy_power`.
pub fn us_portfolio_json(overview: Value, cash: &USCashAndBuyPower) -> Result<Value> {
    let mut v = overview;
    if let Some(map) = v.as_object_mut() {
        map.insert(
            "cash_and_buy_power".to_string(),
            cash_and_buy_power_json(cash)?,
        );
    }
    Ok(v)
}

/// `longbridge portfolio` for US accounts: holdings from the asset overview,
/// cash / buying power / margin status from the cash-and-buy-power API.
pub async fn run_us_portfolio(api: &dyn TradeApi, format: &OutputFormat) -> Result<()> {
    let (overview, cash) =
        tokio::try_join!(api.us_asset_overview(), api.us_cash_and_buy_power(None))?;
    let overview = normalize_us_overview(&overview)?;

    print_account_banner(format);
    match format {
        OutputFormat::Json => print_json_us(&us_portfolio_json(overview, &cash)?),
        OutputFormat::Pretty => {
            let text = |key: &str| or_dash(overview[key].as_str().unwrap_or_default());
            println!("Account Type: {}", text("account_type"));
            println!("Currency:     {}", text("currency"));
            println!();
            print_cash_and_buy_power_pretty(&cash);
            println!();
            print_us_holdings_pretty(&overview);
        }
    }
    Ok(())
}

/// Fetch every US transaction between `start_at` and `end_at`, walking pages
/// until one comes back shorter than the page size (the API has no total).
pub async fn fetch_us_transactions(
    api: &dyn TradeApi,
    start_at: OffsetDateTime,
    end_at: OffsetDateTime,
) -> Result<Vec<USTransaction>> {
    let mut all = Vec::new();
    for page in 1..=TRANSACTION_MAX_PAGES {
        let mut opts = GetUSTransactionHistoryOptions::new(start_at, end_at);
        opts.page = page;
        opts.size = TRANSACTION_PAGE_SIZE;
        let resp = api.us_transaction_history(opts).await?;
        let len = resp.items.len();
        all.extend(resp.items);
        if len < TRANSACTION_PAGE_SIZE as usize {
            return Ok(all);
        }
    }
    eprintln!(
        "Warning: stopped after {TRANSACTION_MAX_PAGES} pages; narrow --start/--end to see the rest"
    );
    Ok(all)
}

/// `longbridge cash-flow` for US accounts: the account transaction history.
pub async fn run_us_cash_flow(
    api: &dyn TradeApi,
    start_at: OffsetDateTime,
    end_at: OffsetDateTime,
    format: &OutputFormat,
) -> Result<()> {
    let items = fetch_us_transactions(api, start_at, end_at).await?;
    let rows = items
        .iter()
        .map(|t| {
            vec![
                t.title.clone(),
                t.amount.clone(),
                t.currency.clone(),
                counter_id_to_symbol(&t.symbol),
                t.done_at
                    .or(t.create_at)
                    .map(fmt_rfc3339)
                    .unwrap_or_default(),
                t.remark.clone(),
            ]
        })
        .collect();
    print_table(
        &["Title", "Amount", "Currency", "Symbol", "Time", "Remark"],
        rows,
        format,
    );
    Ok(())
}

/// `longbridge exchange-rate` for US accounts, as `{exchanges: [...]}`.
pub async fn us_exchange_rates_json(api: &dyn TradeApi) -> Result<Value> {
    let rates = api.us_exchange_rates().await?;
    Ok(json!({ "exchanges": serde_json::to_value(rates.exchanges)? }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::api::MockTradeApi;
    use longbridge::trade::{
        USAssetOverview, USBuyLimitInfo, USCashDetail, USExchangeRate, USExchangeRates,
        USTransactionHistory,
    };

    fn cash_fixture(with_margin: bool) -> USCashAndBuyPower {
        USCashAndBuyPower {
            buy_limit_info: USBuyLimitInfo {
                currency: "USD".to_string(),
                cash_buy_power: "100.5".to_string(),
                ..Default::default()
            },
            cash_list: vec![USCashDetail {
                currency: "USD".to_string(),
                total_amount: "120".to_string(),
                available_cash: "100.5".to_string(),
                ..Default::default()
            }],
            margin_status: if with_margin {
                USMarginStatus {
                    currency: "USD".to_string(),
                    nlv: "1200".to_string(),
                    ..Default::default()
                }
            } else {
                USMarginStatus {
                    currency: "USD".to_string(),
                    ..Default::default()
                }
            },
        }
    }

    fn page(len: usize) -> USTransactionHistory {
        USTransactionHistory {
            current_page: 0,
            items: (0..len)
                .map(|i| USTransaction {
                    title: format!("t{i}"),
                    ..Default::default()
                })
                .collect(),
        }
    }

    #[test]
    fn ensure_not_us_rejects_only_us_sessions() {
        assert!(ensure_not_us(false, "Fund positions").is_ok());
        let err = ensure_not_us(true, "Fund positions")
            .unwrap_err()
            .to_string();
        assert_eq!(err, "Fund positions is not available for US accounts");
    }

    #[test]
    fn empty_margin_status_is_null_in_json() {
        let v = cash_and_buy_power_json(&cash_fixture(false)).unwrap();
        assert!(v["margin_status"].is_null());
        assert_eq!(v["buy_limit_info"]["cash_buy_power"], "100.5");
        assert_eq!(v["cash_list"][0]["total_amount"], "120");

        let v = cash_and_buy_power_json(&cash_fixture(true)).unwrap();
        assert_eq!(v["margin_status"]["nlv"], "1200");
    }

    #[tokio::test]
    async fn us_assets_calls_only_the_us_api() {
        // No `account_balance` expectation: calling the AP endpoint panics.
        let mut mock = MockTradeApi::new();
        mock.expect_us_cash_and_buy_power()
            .with(mockall::predicate::eq(Some("USD".to_string())))
            .times(1)
            .returning(|_| Ok(cash_fixture(true)));
        run_us_assets(&mock, Some("USD".to_string()), &OutputFormat::Json)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn us_portfolio_combines_overview_and_cash() {
        let mut mock = MockTradeApi::new();
        mock.expect_us_asset_overview().times(1).returning(|| {
            Ok(USAssetOverview {
                account_type: "1".to_string(),
                currency: "USD".to_string(),
                ..Default::default()
            })
        });
        mock.expect_us_cash_and_buy_power()
            .with(mockall::predicate::eq(None::<String>))
            .times(1)
            .returning(|_| Ok(cash_fixture(false)));
        run_us_portfolio(&mock, &OutputFormat::Json).await.unwrap();

        let overview = normalize_us_overview(&USAssetOverview {
            account_type: "1".to_string(),
            ..Default::default()
        })
        .unwrap();
        let v = us_portfolio_json(overview, &cash_fixture(false)).unwrap();
        assert_eq!(v["account_type"], "Cash");
        assert!(v["stock_list"].is_array());
        assert_eq!(
            v["cash_and_buy_power"]["buy_limit_info"]["cash_buy_power"],
            "100.5"
        );
        assert!(v["cash_and_buy_power"]["margin_status"].is_null());
    }

    #[tokio::test]
    async fn us_transactions_walk_pages_until_a_short_page() {
        let mut mock = MockTradeApi::new();
        let mut seq = mockall::Sequence::new();
        mock.expect_us_transaction_history()
            .withf(|o| o.page == 1 && o.size == TRANSACTION_PAGE_SIZE)
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_| Ok(page(TRANSACTION_PAGE_SIZE as usize)));
        mock.expect_us_transaction_history()
            .withf(|o| o.page == 2)
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_| Ok(page(3)));
        let now = OffsetDateTime::now_utc();
        let items = fetch_us_transactions(&mock, now - time::Duration::days(30), now)
            .await
            .unwrap();
        assert_eq!(items.len(), TRANSACTION_PAGE_SIZE as usize + 3);
    }

    #[tokio::test]
    async fn us_transactions_stop_on_an_empty_first_page() {
        let mut mock = MockTradeApi::new();
        mock.expect_us_transaction_history()
            .times(1)
            .returning(|_| Ok(page(0)));
        let now = OffsetDateTime::now_utc();
        run_us_cash_flow(&mock, now, now, &OutputFormat::Json)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn us_exchange_rates_keep_the_exchanges_key() {
        let mut mock = MockTradeApi::new();
        mock.expect_us_exchange_rates().times(1).returning(|| {
            Ok(USExchangeRates {
                exchanges: vec![USExchangeRate {
                    base_currency: "USD".to_string(),
                    other_currency: "HKD".to_string(),
                    bid_rate: "7.8".to_string(),
                    ..Default::default()
                }],
            })
        });
        let v = us_exchange_rates_json(&mock).await.unwrap();
        assert_eq!(v["exchanges"][0]["other_currency"], "HKD");
        assert_eq!(v["exchanges"][0]["bid_rate"], "7.8");
    }
}
