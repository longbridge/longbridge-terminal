use anyhow::Result;
use longbridge::httpclient::{Json, Method};
use serde::{Deserialize, Serialize};

use super::{output::print_table, OutputFormat, StatementSection};

// ── Local export format ─────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum ExportFormat {
    Csv,
    Md,
}

/// Detect output format from the output path.
/// No output path → markdown (stdout).
/// Path ending in `.md` → markdown.
/// Everything else (file or directory) → CSV.
fn detect_format(output: Option<&str>) -> ExportFormat {
    match output {
        None => ExportFormat::Md,
        Some(p)
            if std::path::Path::new(p)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md")) =>
        {
            ExportFormat::Md
        }
        _ => ExportFormat::Csv,
    }
}

// ── API request / response types ────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct ListQuery<'a> {
    r#type: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_date: Option<i32>,
    limit: i32,
}

#[derive(Debug, Serialize)]
struct DownloadQuery<'a> {
    file_key: &'a str,
}

#[derive(Debug, Deserialize, Serialize)]
struct StatementListItem {
    dt: i32,
    file_key: String,
}

#[derive(Debug, Deserialize)]
struct StatementListData {
    list: Vec<StatementListItem>,
}

#[derive(Debug, Deserialize)]
struct StatementDownloadData {
    url: String,
}

// ── Statement content types (PascalCase JSON from the downloaded file) ──────

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct CommonStatementContent {
    pub asset: AssetInfo,
    pub account_balance_sum: AccountBalanceSum,
    pub equity_holding_sums: Vec<EquityHoldingSum>,
    pub account_balance_change_sums: Vec<AccountBalanceChangeSum>,
    pub stock_trade_sums: Vec<StockTradeSum>,
    pub equity_holding_change_sums: Vec<EquityHoldingChangeSum>,
    pub account_balance_lock_sums: Vec<AccountBalanceLockSum>,
    pub equity_holding_lock_sums: Vec<EquityHoldingLockSum>,
    pub option_trade_sums: Vec<OptionTradeSum>,
    pub fund_trade_sums: Vec<FundTradeSum>,
    pub ipo_trade_sums: Vec<IpoTradeSum>,
    pub virtual_trade_sums: Vec<VirtualTradeSum>,
    pub interests: Vec<Interest>,
    pub lending_fees: Vec<LendingFee>,
    pub custodian_fees: Vec<CustodianFee>,
    pub corps: Vec<Corp>,
    pub bond_equity_holding_sums: Vec<BondEquityHoldingSum>,
    pub otc_trade_sums: Vec<OtcTradeSum>,
    pub outstanding_sums: Vec<OutstandingSum>,
    pub financing_transaction_sums: Vec<FinancingTransactionSum>,
    pub interest_deposits: Vec<InterestDeposit>,
    pub maintenance_fees: Vec<MaintenanceFee>,
    pub cash_pluses: Vec<CashPlus>,
    pub gst_details: Vec<GstDetail>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct AssetInfo {
    pub currency: String,
    pub ledger_amount: String,
    pub outstanding_amount: String,
    pub debit_amount: String,
    pub nav_margin: String,
    pub warning_value: String,
    pub total: String,
    pub market_value: String,
    pub im_margin: String,
    pub mm_margin: String,
    pub total_suspend: String,
    pub market_value_suspend: String,
    pub margin_limit: String,
    pub im_margin_suspend: String,
    pub mm_margin_suspend: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct AccountBalanceSum {
    pub account_balances: Vec<AccountBalance>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct AccountBalance {
    pub currency_code: String,
    pub begin_amount: String,
    pub begin_amount_as_hkd: String,
    pub change_amount: String,
    pub change_amount_as_hkd: String,
    pub ledger_amount: String,
    pub ledger_amount_as_hkd: String,
    pub settled_amount: String,
    pub settled_amount_as_hkd: String,
    pub outstanding_amount: String,
    pub outstanding_amount_as_hkd: String,
    pub accrued_interest: String,
    pub rate: String,
    pub standard_currency: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct EquityHoldingSum {
    pub equity_type: String,
    pub market: String,
    pub market_code: String,
    pub currency: String,
    pub currency_code: String,
    pub equity_holdings: Vec<EquityHolding>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct EquityHolding {
    pub name: String,
    pub name_en: String,
    pub name_zh: String,
    pub name_hk: String,
    pub market: String,
    pub market_code: String,
    pub currency: String,
    pub currency_code: String,
    pub code: String,
    pub begin_quantity: String,
    pub change_quantity: String,
    pub ledger_quantity: String,
    pub close_price: String,
    pub market_value: String,
    pub margin_rate: String,
    pub margin_value: String,
    pub cost_price: String,
    pub income_amount: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct AccountBalanceChangeSum {
    pub currency: String,
    pub account_balance_changes: Vec<AccountBalanceChange>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct AccountBalanceChange {
    #[serde(rename = "Type")]
    pub r#type: String,
    pub type_en: String,
    pub type_zh: String,
    pub type_hk: String,
    pub remark: String,
    pub remark_en: String,
    pub remark_zh: String,
    pub remark_hk: String,
    pub date: String,
    pub amount: String,
    pub biz_code: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct StockTradeSum {
    pub market: String,
    pub currency: String,
    pub trades: Vec<TradeRecord>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct TradeRecord {
    pub direction: String,
    pub direction_code: String,
    pub name: String,
    pub name_en: String,
    pub name_zh: String,
    pub name_hk: String,
    pub trade_date: String,
    pub settle_date: String,
    pub contract_no: String,
    pub code: String,
    pub trade_quantity: String,
    pub trade_price: String,
    pub trade_amount: String,
    pub clear_amount: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct EquityHoldingChangeSum {
    pub market: String,
    pub market_code: String,
    pub equity_holding_changes: Vec<EquityHoldingChange>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct EquityHoldingChange {
    pub name: String,
    pub name_en: String,
    pub name_zh: String,
    pub name_hk: String,
    #[serde(rename = "Type")]
    pub r#type: String,
    pub type_en: String,
    pub type_zh: String,
    pub type_hk: String,
    pub remark: String,
    pub remark_en: String,
    pub remark_zh: String,
    pub remark_hk: String,
    pub date: String,
    pub code: String,
    pub quantity: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct AccountBalanceLockSum {
    pub currency: String,
    pub account_balance_locks: Vec<AccountBalanceLock>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct AccountBalanceLock {
    pub date: String,
    pub expire_date: String,
    pub amount: String,
    pub remark: String,
    pub ref_no: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct EquityHoldingLockSum {
    pub market: String,
    pub equity_holding_locks: Vec<EquityHoldingLock>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct EquityHoldingLock {
    pub name: String,
    pub name_en: String,
    pub name_zh: String,
    pub name_hk: String,
    pub date: String,
    pub expire_date: String,
    pub code: String,
    pub quantity: String,
    pub remark: String,
    pub ref_no: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct OptionTradeSum {
    pub market: String,
    pub currency: String,
    pub trades: Vec<TradeRecord>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct FundTradeSum {
    pub currency: String,
    pub equity_type: String,
    pub trades: Vec<FundTrade>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct FundTrade {
    pub name: String,
    pub name_en: String,
    pub name_zh: String,
    pub name_hk: String,
    pub direction: String,
    pub direction_code: String,
    pub order_date: String,
    pub confirm_date: String,
    pub status: String,
    pub contract_no: String,
    pub code: String,
    pub trade_amount: String,
    pub trade_quantity: String,
    pub price: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct IpoTradeSum {
    pub market: String,
    pub trades: Vec<IpoTrade>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct IpoTrade {
    pub name: String,
    pub name_en: String,
    pub name_zh: String,
    pub name_hk: String,
    pub sub_method: String,
    pub sub_method_code: String,
    pub sub_date: String,
    pub code: String,
    pub sub_quantity: String,
    pub sub_amount: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct VirtualTradeSum {
    pub market: String,
    pub currency: String,
    pub trades: Vec<TradeRecord>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Interest {
    pub date: String,
    pub currency: String,
    pub rate: String,
    pub fine_interest: String,
    pub interest: String,
    pub total: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct LendingFee {
    pub date: String,
    pub currency: String,
    pub code: String,
    pub name: String,
    pub name_en: String,
    pub name_zh: String,
    pub name_hk: String,
    pub quantity: String,
    pub settle_price: String,
    pub lending_market_value: String,
    pub rate: String,
    pub amount: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct CustodianFee {
    pub date: String,
    pub currency: String,
    pub rate: String,
    pub fee_amount: String,
    pub fee: String,
    pub total: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Corp {
    pub date: String,
    pub pay_date: String,
    pub market: String,
    pub code: String,
    pub name: String,
    pub name_en: String,
    pub name_zh: String,
    pub name_hk: String,
    pub remark: String,
    pub quantity: String,
    pub new_code: String,
    pub new_name: String,
    pub new_name_en: String,
    pub new_name_zh: String,
    pub new_name_hk: String,
    pub new_quantity: String,
    pub currency: String,
    pub new_amount: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct BondEquityHoldingSum {
    pub equity_type: String,
    pub market: String,
    pub market_code: String,
    pub currency: String,
    pub currency_code: String,
    pub equity_holdings: Vec<EquityHolding>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct OtcTradeSum {
    pub market: String,
    pub currency: String,
    pub equity_type: String,
    pub order_type: String,
    pub trades: Vec<TradeRecord>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct OutstandingSum {
    pub market: String,
    pub currency: String,
    pub equity_type: String,
    pub outstanding_trades: Vec<TradeRecord>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct FinancingTransactionSum {
    pub currency: String,
    pub transaction_details: Vec<AccountBalanceChange>,
}

pub type InterestDeposit = Interest;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct MaintenanceFee {
    pub year_month: String,
    pub currency_name: String,
    pub market_name: String,
    pub fee_rate: String,
    pub accrued_fee: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct CashPlus {
    pub date: String,
    pub currency_name: String,
    pub latest_balance: String,
    pub latest_profit_loss: String,
    pub accum_profit_loss: String,
    pub apr: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct GstDetail {
    pub date: String,
    #[serde(rename = "Ref")]
    pub r#ref: String,
    #[serde(rename = "Type")]
    pub r#type: String,
    pub type_en: String,
    pub type_zh: String,
    pub type_hk: String,
    pub remark: String,
    pub remark_en: String,
    pub remark_zh: String,
    pub remark_hk: String,
    pub currency: String,
    pub amount: String,
    pub fee_rate: String,
    pub fee_amount: String,
    pub total: String,
    pub fx_rate: String,
    pub amount_as_hkd: String,
}

// ── Helper ───────────────────────────────────────────────────────────────────

/// Return the first non-empty string from the candidates, or `""`.
fn first_non_empty<'a>(candidates: &[&'a str]) -> &'a str {
    candidates
        .iter()
        .find(|s| !s.is_empty())
        .copied()
        .unwrap_or("")
}

/// Format a statement date integer (e.g. 20241231) as "YYYY-MM-DD".
fn fmt_date(dt: i32) -> String {
    let d = dt.unsigned_abs();
    let year = d / 10000;
    let month = (d % 10000) / 100;
    let day = d % 100;
    format!("{year}-{month:02}-{day:02}")
}

fn all_sections() -> Vec<StatementSection> {
    vec![
        StatementSection::Asset,
        StatementSection::AccountBalanceSum,
        StatementSection::EquityHoldingSums,
        StatementSection::AccountBalanceChangeSums,
        StatementSection::StockTradeSums,
        StatementSection::EquityHoldingChangeSums,
        StatementSection::AccountBalanceLockSums,
        StatementSection::EquityHoldingLockSums,
        StatementSection::OptionTradeSums,
        StatementSection::FundTradeSums,
        StatementSection::IpoTradeSums,
        StatementSection::VirtualTradeSums,
        StatementSection::Interests,
        StatementSection::LendingFees,
        StatementSection::CustodianFees,
        StatementSection::Corps,
        StatementSection::BondEquityHoldingSums,
        StatementSection::OtcTradeSums,
        StatementSection::OutstandingSums,
        StatementSection::FinancingTransactionSums,
        StatementSection::InterestDeposits,
        StatementSection::MaintenanceFees,
        StatementSection::CashPluses,
        StatementSection::GstDetails,
    ]
}

// ── Commands ─────────────────────────────────────────────────────────────────

/// List account statements (daily or monthly).
pub async fn cmd_statements(
    statement_type: &str,
    start_date: Option<i32>,
    limit: i32,
    format: &OutputFormat,
) -> Result<()> {
    let type_str = match statement_type.to_lowercase().as_str() {
        "monthly" | "m" => "monthly",
        _ => "daily",
    };

    let client = crate::openapi::http_client();
    let resp = client
        .request(Method::GET, "/v1/asset/statement/list")
        .query_params(ListQuery {
            r#type: type_str,
            start_date,
            limit,
        })
        .response::<Json<StatementListData>>()
        .send()
        .await?
        .0;

    if resp.list.is_empty() {
        println!("No statements found.");
        return Ok(());
    }

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp.list)?);
        }
        OutputFormat::Table => {
            let headers = &["Date", "File Key"];
            let rows = resp
                .list
                .iter()
                .map(|item| vec![fmt_date(item.dt), item.file_key.clone()])
                .collect();
            print_table(headers, rows, format);
        }
    }
    Ok(())
}

/// Export sections from a statement identified by `file_key`.
pub async fn cmd_statement_export(
    file_key: &str,
    sections: &[StatementSection],
    output_path: Option<&str>,
) -> Result<()> {
    let client = crate::openapi::http_client();
    let resp = client
        .request(Method::GET, "/v1/asset/statement/download-url")
        .query_params(DownloadQuery { file_key })
        .response::<Json<StatementDownloadData>>()
        .send()
        .await?
        .0;

    let body = reqwest::Client::new()
        .get(&resp.url)
        .send()
        .await?
        .text()
        .await?;
    let content: CommonStatementContent = serde_json::from_str(&body)?;

    let sections: Vec<StatementSection> = if sections.is_empty() {
        all_sections()
    } else {
        sections.to_vec()
    };

    let format = detect_format(output_path);

    let ext = match format {
        ExportFormat::Csv => "csv",
        ExportFormat::Md => "md",
    };

    match output_path {
        Some(path) => {
            if sections.len() == 1 {
                let data = section_to_format(&content, &sections[0], &format)?;
                std::fs::write(path, data)?;
                println!("Saved {:?} to {path}", sections[0]);
            } else {
                let dir = std::path::Path::new(path);
                std::fs::create_dir_all(dir)?;
                for section in &sections {
                    let file_name = format!("{}.{ext}", section_file_name(section));
                    let file_path = dir.join(&file_name);
                    let data = section_to_format(&content, section, &format)?;
                    std::fs::write(&file_path, data)?;
                    println!("Saved {section:?} to {}", file_path.display());
                }
            }
        }
        None => {
            for section in &sections {
                let data = section_to_format(&content, section, &format)?;
                print!("{data}");
            }
        }
    }
    Ok(())
}

// ── Section rendering ────────────────────────────────────────────────────────

struct SectionData<'a> {
    title: &'static str,
    headers: &'static [&'static str],
    rows: Vec<Vec<&'a str>>,
}

fn section_data<'a>(
    content: &'a CommonStatementContent,
    section: &StatementSection,
) -> SectionData<'a> {
    match section {
        StatementSection::Asset => {
            let a = &content.asset;
            SectionData {
                title: "Asset",
                headers: &[
                    "currency",
                    "ledger_amount",
                    "outstanding_amount",
                    "debit_amount",
                    "nav_margin",
                    "warning_value",
                    "total",
                    "market_value",
                    "im_margin",
                    "mm_margin",
                    "total_suspend",
                    "market_value_suspend",
                    "margin_limit",
                    "im_margin_suspend",
                    "mm_margin_suspend",
                ],
                rows: vec![vec![
                    &a.currency,
                    &a.ledger_amount,
                    &a.outstanding_amount,
                    &a.debit_amount,
                    &a.nav_margin,
                    &a.warning_value,
                    &a.total,
                    &a.market_value,
                    &a.im_margin,
                    &a.mm_margin,
                    &a.total_suspend,
                    &a.market_value_suspend,
                    &a.margin_limit,
                    &a.im_margin_suspend,
                    &a.mm_margin_suspend,
                ]],
            }
        }
        StatementSection::AccountBalanceSum => {
            let abs = &content.account_balance_sum;
            SectionData {
                title: "Account Balances",
                headers: &[
                    "currency",
                    "begin_amount",
                    "begin_amount_as_hkd",
                    "change_amount",
                    "change_amount_as_hkd",
                    "ledger_amount",
                    "ledger_amount_as_hkd",
                    "settled_amount",
                    "settled_amount_as_hkd",
                    "outstanding_amount",
                    "outstanding_amount_as_hkd",
                    "accrued_interest",
                    "rate",
                    "standard_currency",
                ],
                rows: abs
                    .account_balances
                    .iter()
                    .map(|b| {
                        vec![
                            b.currency_code.as_str(),
                            b.begin_amount.as_str(),
                            b.begin_amount_as_hkd.as_str(),
                            b.change_amount.as_str(),
                            b.change_amount_as_hkd.as_str(),
                            b.ledger_amount.as_str(),
                            b.ledger_amount_as_hkd.as_str(),
                            b.settled_amount.as_str(),
                            b.settled_amount_as_hkd.as_str(),
                            b.outstanding_amount.as_str(),
                            b.outstanding_amount_as_hkd.as_str(),
                            b.accrued_interest.as_str(),
                            b.rate.as_str(),
                            b.standard_currency.as_str(),
                        ]
                    })
                    .collect(),
            }
        }
        StatementSection::EquityHoldingSums => SectionData {
            title: "Equity Holdings",
            headers: &[
                "equity_type",
                "market",
                "currency",
                "code",
                "name",
                "begin_quantity",
                "change_quantity",
                "ledger_quantity",
                "close_price",
                "market_value",
                "margin_rate",
                "margin_value",
                "cost_price",
                "income_amount",
            ],
            rows: content
                .equity_holding_sums
                .iter()
                .flat_map(|sum| {
                    sum.equity_holdings.iter().map(move |h| {
                        let name = first_non_empty(&[&h.name, &h.name_en, &h.name_zh, &h.name_hk]);
                        let market = first_non_empty(&[
                            h.market.as_str(),
                            h.market_code.as_str(),
                            sum.market.as_str(),
                            sum.market_code.as_str(),
                        ]);
                        let currency = first_non_empty(&[
                            h.currency.as_str(),
                            h.currency_code.as_str(),
                            sum.currency.as_str(),
                            sum.currency_code.as_str(),
                        ]);
                        vec![
                            sum.equity_type.as_str(),
                            market,
                            currency,
                            h.code.as_str(),
                            name,
                            h.begin_quantity.as_str(),
                            h.change_quantity.as_str(),
                            h.ledger_quantity.as_str(),
                            h.close_price.as_str(),
                            h.market_value.as_str(),
                            h.margin_rate.as_str(),
                            h.margin_value.as_str(),
                            h.cost_price.as_str(),
                            h.income_amount.as_str(),
                        ]
                    })
                })
                .collect(),
        },
        StatementSection::AccountBalanceChangeSums => SectionData {
            title: "Account Balance Changes",
            headers: &["currency", "date", "type", "amount", "remark", "biz_code"],
            rows: content
                .account_balance_change_sums
                .iter()
                .flat_map(|sum| {
                    sum.account_balance_changes.iter().map(move |c| {
                        let typ = first_non_empty(&[&c.r#type, &c.type_en, &c.type_zh, &c.type_hk]);
                        let remark =
                            first_non_empty(&[&c.remark_en, &c.remark_zh, &c.remark_hk, &c.remark]);
                        vec![
                            sum.currency.as_str(),
                            c.date.as_str(),
                            typ,
                            c.amount.as_str(),
                            remark,
                            c.biz_code.as_str(),
                        ]
                    })
                })
                .collect(),
        },
        StatementSection::StockTradeSums => SectionData {
            title: "Stock Trades",
            headers: &[
                "market",
                "currency",
                "trade_date",
                "settle_date",
                "contract_no",
                "direction",
                "code",
                "name",
                "trade_quantity",
                "trade_price",
                "trade_amount",
                "clear_amount",
            ],
            rows: content
                .stock_trade_sums
                .iter()
                .flat_map(|sum| {
                    sum.trades.iter().map(move |t| {
                        let direction = first_non_empty(&[&t.direction, &t.direction_code]);
                        let name = first_non_empty(&[&t.name, &t.name_en, &t.name_zh, &t.name_hk]);
                        vec![
                            sum.market.as_str(),
                            sum.currency.as_str(),
                            t.trade_date.as_str(),
                            t.settle_date.as_str(),
                            t.contract_no.as_str(),
                            direction,
                            t.code.as_str(),
                            name,
                            t.trade_quantity.as_str(),
                            t.trade_price.as_str(),
                            t.trade_amount.as_str(),
                            t.clear_amount.as_str(),
                        ]
                    })
                })
                .collect(),
        },
        StatementSection::EquityHoldingChangeSums => SectionData {
            title: "Equity Holding Changes",
            headers: &[
                "market", "date", "code", "name", "type", "quantity", "remark",
            ],
            rows: content
                .equity_holding_change_sums
                .iter()
                .flat_map(|sum| {
                    let market = if sum.market.is_empty() {
                        sum.market_code.as_str()
                    } else {
                        sum.market.as_str()
                    };
                    sum.equity_holding_changes.iter().map(move |c| {
                        let name = first_non_empty(&[&c.name, &c.name_en, &c.name_zh, &c.name_hk]);
                        let typ = first_non_empty(&[&c.r#type, &c.type_en, &c.type_zh, &c.type_hk]);
                        let remark =
                            first_non_empty(&[&c.remark_en, &c.remark_zh, &c.remark_hk, &c.remark]);
                        vec![
                            market,
                            c.date.as_str(),
                            c.code.as_str(),
                            name,
                            typ,
                            c.quantity.as_str(),
                            remark,
                        ]
                    })
                })
                .collect(),
        },
        StatementSection::AccountBalanceLockSums => SectionData {
            title: "Account Balance Locks",
            headers: &[
                "currency",
                "date",
                "expire_date",
                "amount",
                "remark",
                "ref_no",
            ],
            rows: content
                .account_balance_lock_sums
                .iter()
                .flat_map(|sum| {
                    sum.account_balance_locks.iter().map(move |l| {
                        vec![
                            sum.currency.as_str(),
                            l.date.as_str(),
                            l.expire_date.as_str(),
                            l.amount.as_str(),
                            l.remark.as_str(),
                            l.ref_no.as_str(),
                        ]
                    })
                })
                .collect(),
        },
        StatementSection::EquityHoldingLockSums => SectionData {
            title: "Equity Holding Locks",
            headers: &[
                "market",
                "date",
                "expire_date",
                "code",
                "name",
                "quantity",
                "remark",
                "ref_no",
            ],
            rows: content
                .equity_holding_lock_sums
                .iter()
                .flat_map(|sum| {
                    sum.equity_holding_locks.iter().map(move |l| {
                        let name = first_non_empty(&[&l.name, &l.name_en, &l.name_zh, &l.name_hk]);
                        vec![
                            sum.market.as_str(),
                            l.date.as_str(),
                            l.expire_date.as_str(),
                            l.code.as_str(),
                            name,
                            l.quantity.as_str(),
                            l.remark.as_str(),
                            l.ref_no.as_str(),
                        ]
                    })
                })
                .collect(),
        },
        StatementSection::OptionTradeSums => SectionData {
            title: "Option Trades",
            headers: &[
                "market",
                "currency",
                "trade_date",
                "settle_date",
                "contract_no",
                "direction",
                "code",
                "name",
                "trade_quantity",
                "trade_price",
                "trade_amount",
                "clear_amount",
            ],
            rows: content
                .option_trade_sums
                .iter()
                .flat_map(|sum| {
                    sum.trades.iter().map(move |t| {
                        let direction = first_non_empty(&[&t.direction, &t.direction_code]);
                        let name = first_non_empty(&[&t.name, &t.name_en, &t.name_zh, &t.name_hk]);
                        vec![
                            sum.market.as_str(),
                            sum.currency.as_str(),
                            t.trade_date.as_str(),
                            t.settle_date.as_str(),
                            t.contract_no.as_str(),
                            direction,
                            t.code.as_str(),
                            name,
                            t.trade_quantity.as_str(),
                            t.trade_price.as_str(),
                            t.trade_amount.as_str(),
                            t.clear_amount.as_str(),
                        ]
                    })
                })
                .collect(),
        },
        StatementSection::FundTradeSums => SectionData {
            title: "Fund Trades",
            headers: &[
                "currency",
                "equity_type",
                "order_date",
                "confirm_date",
                "status",
                "contract_no",
                "code",
                "name",
                "direction",
                "trade_amount",
                "trade_quantity",
                "price",
            ],
            rows: content
                .fund_trade_sums
                .iter()
                .flat_map(|sum| {
                    sum.trades.iter().map(move |t| {
                        let name = first_non_empty(&[&t.name, &t.name_en, &t.name_zh, &t.name_hk]);
                        let direction = first_non_empty(&[&t.direction, &t.direction_code]);
                        vec![
                            sum.currency.as_str(),
                            sum.equity_type.as_str(),
                            t.order_date.as_str(),
                            t.confirm_date.as_str(),
                            t.status.as_str(),
                            t.contract_no.as_str(),
                            t.code.as_str(),
                            name,
                            direction,
                            t.trade_amount.as_str(),
                            t.trade_quantity.as_str(),
                            t.price.as_str(),
                        ]
                    })
                })
                .collect(),
        },
        StatementSection::IpoTradeSums => SectionData {
            title: "IPO Trades",
            headers: &[
                "market",
                "sub_date",
                "code",
                "name",
                "sub_method",
                "sub_quantity",
                "sub_amount",
            ],
            rows: content
                .ipo_trade_sums
                .iter()
                .flat_map(|sum| {
                    sum.trades.iter().map(move |t| {
                        let name = first_non_empty(&[&t.name, &t.name_en, &t.name_zh, &t.name_hk]);
                        let sub_method = first_non_empty(&[&t.sub_method, &t.sub_method_code]);
                        vec![
                            sum.market.as_str(),
                            t.sub_date.as_str(),
                            t.code.as_str(),
                            name,
                            sub_method,
                            t.sub_quantity.as_str(),
                            t.sub_amount.as_str(),
                        ]
                    })
                })
                .collect(),
        },
        StatementSection::VirtualTradeSums => SectionData {
            title: "Virtual Trades",
            headers: &[
                "market",
                "currency",
                "trade_date",
                "settle_date",
                "contract_no",
                "direction",
                "code",
                "name",
                "trade_quantity",
                "trade_price",
                "trade_amount",
                "clear_amount",
            ],
            rows: content
                .virtual_trade_sums
                .iter()
                .flat_map(|sum| {
                    sum.trades.iter().map(move |t| {
                        let direction = first_non_empty(&[&t.direction, &t.direction_code]);
                        let name = first_non_empty(&[&t.name, &t.name_en, &t.name_zh, &t.name_hk]);
                        vec![
                            sum.market.as_str(),
                            sum.currency.as_str(),
                            t.trade_date.as_str(),
                            t.settle_date.as_str(),
                            t.contract_no.as_str(),
                            direction,
                            t.code.as_str(),
                            name,
                            t.trade_quantity.as_str(),
                            t.trade_price.as_str(),
                            t.trade_amount.as_str(),
                            t.clear_amount.as_str(),
                        ]
                    })
                })
                .collect(),
        },
        StatementSection::Interests => SectionData {
            title: "Interests",
            headers: &[
                "date",
                "currency",
                "rate",
                "fine_interest",
                "interest",
                "total",
            ],
            rows: content
                .interests
                .iter()
                .map(|i| {
                    vec![
                        i.date.as_str(),
                        i.currency.as_str(),
                        i.rate.as_str(),
                        i.fine_interest.as_str(),
                        i.interest.as_str(),
                        i.total.as_str(),
                    ]
                })
                .collect(),
        },
        StatementSection::LendingFees => SectionData {
            title: "Lending Fees",
            headers: &[
                "date",
                "currency",
                "code",
                "name",
                "quantity",
                "settle_price",
                "lending_market_value",
                "rate",
                "amount",
            ],
            rows: content
                .lending_fees
                .iter()
                .map(|f| {
                    let name = first_non_empty(&[&f.name, &f.name_en, &f.name_zh, &f.name_hk]);
                    vec![
                        f.date.as_str(),
                        f.currency.as_str(),
                        f.code.as_str(),
                        name,
                        f.quantity.as_str(),
                        f.settle_price.as_str(),
                        f.lending_market_value.as_str(),
                        f.rate.as_str(),
                        f.amount.as_str(),
                    ]
                })
                .collect(),
        },
        StatementSection::CustodianFees => SectionData {
            title: "Custodian Fees",
            headers: &["date", "currency", "rate", "fee_amount", "fee", "total"],
            rows: content
                .custodian_fees
                .iter()
                .map(|f| {
                    vec![
                        f.date.as_str(),
                        f.currency.as_str(),
                        f.rate.as_str(),
                        f.fee_amount.as_str(),
                        f.fee.as_str(),
                        f.total.as_str(),
                    ]
                })
                .collect(),
        },
        StatementSection::Corps => SectionData {
            title: "Corporate Actions",
            headers: &[
                "date",
                "pay_date",
                "market",
                "code",
                "name",
                "remark",
                "quantity",
                "new_code",
                "new_name",
                "new_quantity",
                "currency",
                "new_amount",
            ],
            rows: content
                .corps
                .iter()
                .map(|c| {
                    let name = first_non_empty(&[&c.name, &c.name_en, &c.name_zh, &c.name_hk]);
                    let new_name = first_non_empty(&[
                        &c.new_name,
                        &c.new_name_en,
                        &c.new_name_zh,
                        &c.new_name_hk,
                    ]);
                    vec![
                        c.date.as_str(),
                        c.pay_date.as_str(),
                        c.market.as_str(),
                        c.code.as_str(),
                        name,
                        c.remark.as_str(),
                        c.quantity.as_str(),
                        c.new_code.as_str(),
                        new_name,
                        c.new_quantity.as_str(),
                        c.currency.as_str(),
                        c.new_amount.as_str(),
                    ]
                })
                .collect(),
        },
        StatementSection::BondEquityHoldingSums => SectionData {
            title: "Bond Equity Holdings",
            headers: &[
                "equity_type",
                "market",
                "currency",
                "code",
                "name",
                "begin_quantity",
                "change_quantity",
                "ledger_quantity",
                "close_price",
                "market_value",
                "margin_rate",
                "margin_value",
                "cost_price",
                "income_amount",
            ],
            rows: content
                .bond_equity_holding_sums
                .iter()
                .flat_map(|sum| {
                    sum.equity_holdings.iter().map(move |h| {
                        let name = first_non_empty(&[&h.name, &h.name_en, &h.name_zh, &h.name_hk]);
                        let market = first_non_empty(&[
                            h.market.as_str(),
                            h.market_code.as_str(),
                            sum.market.as_str(),
                            sum.market_code.as_str(),
                        ]);
                        let currency = first_non_empty(&[
                            h.currency.as_str(),
                            h.currency_code.as_str(),
                            sum.currency.as_str(),
                            sum.currency_code.as_str(),
                        ]);
                        vec![
                            sum.equity_type.as_str(),
                            market,
                            currency,
                            h.code.as_str(),
                            name,
                            h.begin_quantity.as_str(),
                            h.change_quantity.as_str(),
                            h.ledger_quantity.as_str(),
                            h.close_price.as_str(),
                            h.market_value.as_str(),
                            h.margin_rate.as_str(),
                            h.margin_value.as_str(),
                            h.cost_price.as_str(),
                            h.income_amount.as_str(),
                        ]
                    })
                })
                .collect(),
        },
        StatementSection::OtcTradeSums => SectionData {
            title: "OTC Trades",
            headers: &[
                "market",
                "currency",
                "equity_type",
                "order_type",
                "trade_date",
                "settle_date",
                "contract_no",
                "direction",
                "code",
                "name",
                "trade_quantity",
                "trade_price",
                "trade_amount",
                "clear_amount",
            ],
            rows: content
                .otc_trade_sums
                .iter()
                .flat_map(|sum| {
                    sum.trades.iter().map(move |t| {
                        let direction = first_non_empty(&[&t.direction, &t.direction_code]);
                        let name = first_non_empty(&[&t.name, &t.name_en, &t.name_zh, &t.name_hk]);
                        vec![
                            sum.market.as_str(),
                            sum.currency.as_str(),
                            sum.equity_type.as_str(),
                            sum.order_type.as_str(),
                            t.trade_date.as_str(),
                            t.settle_date.as_str(),
                            t.contract_no.as_str(),
                            direction,
                            t.code.as_str(),
                            name,
                            t.trade_quantity.as_str(),
                            t.trade_price.as_str(),
                            t.trade_amount.as_str(),
                            t.clear_amount.as_str(),
                        ]
                    })
                })
                .collect(),
        },
        StatementSection::OutstandingSums => SectionData {
            title: "Outstandings",
            headers: &[
                "market",
                "currency",
                "equity_type",
                "trade_date",
                "settle_date",
                "contract_no",
                "direction",
                "code",
                "name",
                "trade_quantity",
                "trade_price",
                "trade_amount",
                "clear_amount",
            ],
            rows: content
                .outstanding_sums
                .iter()
                .flat_map(|sum| {
                    sum.outstanding_trades.iter().map(move |t| {
                        let direction = first_non_empty(&[&t.direction, &t.direction_code]);
                        let name = first_non_empty(&[&t.name, &t.name_en, &t.name_zh, &t.name_hk]);
                        vec![
                            sum.market.as_str(),
                            sum.currency.as_str(),
                            sum.equity_type.as_str(),
                            t.trade_date.as_str(),
                            t.settle_date.as_str(),
                            t.contract_no.as_str(),
                            direction,
                            t.code.as_str(),
                            name,
                            t.trade_quantity.as_str(),
                            t.trade_price.as_str(),
                            t.trade_amount.as_str(),
                            t.clear_amount.as_str(),
                        ]
                    })
                })
                .collect(),
        },
        StatementSection::FinancingTransactionSums => SectionData {
            title: "Financing Transactions",
            headers: &["currency", "date", "type", "amount", "remark", "biz_code"],
            rows: content
                .financing_transaction_sums
                .iter()
                .flat_map(|sum| {
                    sum.transaction_details.iter().map(move |d| {
                        let typ = first_non_empty(&[&d.r#type, &d.type_en, &d.type_zh, &d.type_hk]);
                        let remark =
                            first_non_empty(&[&d.remark_en, &d.remark_zh, &d.remark_hk, &d.remark]);
                        vec![
                            sum.currency.as_str(),
                            d.date.as_str(),
                            typ,
                            d.amount.as_str(),
                            remark,
                            d.biz_code.as_str(),
                        ]
                    })
                })
                .collect(),
        },
        StatementSection::InterestDeposits => SectionData {
            title: "Interest Deposits",
            headers: &[
                "date",
                "currency",
                "rate",
                "fine_interest",
                "interest",
                "total",
            ],
            rows: content
                .interest_deposits
                .iter()
                .map(|i| {
                    vec![
                        i.date.as_str(),
                        i.currency.as_str(),
                        i.rate.as_str(),
                        i.fine_interest.as_str(),
                        i.interest.as_str(),
                        i.total.as_str(),
                    ]
                })
                .collect(),
        },
        StatementSection::MaintenanceFees => SectionData {
            title: "Maintenance Fees",
            headers: &[
                "year_month",
                "currency",
                "market",
                "fee_rate",
                "accrued_fee",
            ],
            rows: content
                .maintenance_fees
                .iter()
                .map(|f| {
                    vec![
                        f.year_month.as_str(),
                        f.currency_name.as_str(),
                        f.market_name.as_str(),
                        f.fee_rate.as_str(),
                        f.accrued_fee.as_str(),
                    ]
                })
                .collect(),
        },
        StatementSection::CashPluses => SectionData {
            title: "Cash Plus",
            headers: &[
                "date",
                "currency",
                "latest_balance",
                "latest_profit_loss",
                "accum_profit_loss",
                "apr",
            ],
            rows: content
                .cash_pluses
                .iter()
                .map(|c| {
                    vec![
                        c.date.as_str(),
                        c.currency_name.as_str(),
                        c.latest_balance.as_str(),
                        c.latest_profit_loss.as_str(),
                        c.accum_profit_loss.as_str(),
                        c.apr.as_str(),
                    ]
                })
                .collect(),
        },
        StatementSection::GstDetails => SectionData {
            title: "GST Details",
            headers: &[
                "date",
                "ref",
                "type",
                "remark",
                "currency",
                "amount",
                "fee_rate",
                "fee_amount",
                "total",
                "fx_rate",
                "amount_as_hkd",
            ],
            rows: content
                .gst_details
                .iter()
                .map(|g| {
                    let typ = first_non_empty(&[&g.r#type, &g.type_en, &g.type_zh, &g.type_hk]);
                    let remark =
                        first_non_empty(&[&g.remark_en, &g.remark_zh, &g.remark_hk, &g.remark]);
                    vec![
                        g.date.as_str(),
                        g.r#ref.as_str(),
                        typ,
                        remark,
                        g.currency.as_str(),
                        g.amount.as_str(),
                        g.fee_rate.as_str(),
                        g.fee_amount.as_str(),
                        g.total.as_str(),
                        g.fx_rate.as_str(),
                        g.amount_as_hkd.as_str(),
                    ]
                })
                .collect(),
        },
    }
}

fn section_to_format(
    content: &CommonStatementContent,
    section: &StatementSection,
    format: &ExportFormat,
) -> Result<String> {
    let data = section_data(content, section);
    match format {
        ExportFormat::Csv => data.to_csv(),
        ExportFormat::Md => Ok(data.to_markdown()),
    }
}

/// Map a `StatementSection` variant to a file-name-friendly string.
fn section_file_name(section: &StatementSection) -> &'static str {
    match section {
        StatementSection::Asset => "asset",
        StatementSection::AccountBalanceSum => "account_balances",
        StatementSection::EquityHoldingSums => "equity_holdings",
        StatementSection::AccountBalanceChangeSums => "account_balance_changes",
        StatementSection::StockTradeSums => "stock_trades",
        StatementSection::EquityHoldingChangeSums => "equity_holding_changes",
        StatementSection::AccountBalanceLockSums => "account_balance_locks",
        StatementSection::EquityHoldingLockSums => "equity_holding_locks",
        StatementSection::OptionTradeSums => "option_trades",
        StatementSection::FundTradeSums => "fund_trades",
        StatementSection::IpoTradeSums => "ipo_trades",
        StatementSection::VirtualTradeSums => "virtual_trades",
        StatementSection::Interests => "interests",
        StatementSection::LendingFees => "lending_fees",
        StatementSection::CustodianFees => "custodian_fees",
        StatementSection::Corps => "corps",
        StatementSection::BondEquityHoldingSums => "bond_equity_holdings",
        StatementSection::OtcTradeSums => "otc_trades",
        StatementSection::OutstandingSums => "outstandings",
        StatementSection::FinancingTransactionSums => "financing_transactions",
        StatementSection::InterestDeposits => "interest_deposits",
        StatementSection::MaintenanceFees => "maintenance_fees",
        StatementSection::CashPluses => "cash_pluses",
        StatementSection::GstDetails => "gst_details",
    }
}

impl SectionData<'_> {
    fn to_csv(&self) -> Result<String> {
        let mut wtr = csv::Writer::from_writer(vec![]);
        wtr.write_record(self.headers)?;
        for row in &self.rows {
            wtr.write_record(row)?;
        }
        Ok(String::from_utf8(wtr.into_inner()?)?)
    }

    fn to_markdown(&self) -> String {
        let mut out = format!("## {}\n\n", self.title);
        out.push_str("| ");
        out.push_str(&self.headers.join(" | "));
        out.push_str(" |\n| ");
        out.push_str(
            &self
                .headers
                .iter()
                .map(|_| "---")
                .collect::<Vec<_>>()
                .join(" | "),
        );
        out.push_str(" |\n");
        for row in &self.rows {
            out.push_str("| ");
            out.push_str(&row.join(" | "));
            out.push_str(" |\n");
        }
        out.push('\n');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csv_record(data: &str) -> Vec<String> {
        let mut reader = csv::Reader::from_reader(data.as_bytes());
        reader
            .records()
            .next()
            .unwrap()
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn equity_holdings_fall_back_to_market_code_and_currency_code() {
        let content: CommonStatementContent = serde_json::from_str(
            r#"
            {
                "EquityHoldingSums": [
                    {
                        "EquityType": "Stock",
                        "Market": "",
                        "MarketCode": "HK",
                        "Currency": "",
                        "CurrencyCode": "HKD",
                        "EquityHoldings": [
                            {
                                "Code": "AAPL",
                                "NameEn": "Apple Inc.",
                                "Market": "",
                                "MarketCode": "HK",
                                "Currency": "",
                                "CurrencyCode": "HKD",
                                "BeginQuantity": "10",
                                "ChangeQuantity": "2",
                                "LedgerQuantity": "12",
                                "ClosePrice": "100",
                                "MarketValue": "1200",
                                "MarginRate": "0.5",
                                "MarginValue": "600",
                                "CostPrice": "80",
                                "IncomeAmount": "240"
                            }
                        ]
                    }
                ]
            }
            "#,
        )
        .unwrap();

        let csv = section_to_format(
            &content,
            &StatementSection::EquityHoldingSums,
            &ExportFormat::Csv,
        )
        .unwrap();
        let record = csv_record(&csv);

        assert_eq!(
            record,
            vec![
                "Stock",
                "HK",
                "HKD",
                "AAPL",
                "Apple Inc.",
                "10",
                "2",
                "12",
                "100",
                "1200",
                "0.5",
                "600",
                "80",
                "240",
            ]
        );
    }

    #[test]
    fn bond_equity_holdings_prefer_item_market_and_currency_when_present() {
        let content: CommonStatementContent = serde_json::from_str(
            r#"
            {
                "BondEquityHoldingSums": [
                    {
                        "EquityType": "Bond",
                        "Market": "HK",
                        "Currency": "HKD",
                        "EquityHoldings": [
                            {
                                "Code": "BOND1",
                                "NameEn": "Bond One",
                                "Market": "US",
                                "Currency": "USD",
                                "BeginQuantity": "1",
                                "ChangeQuantity": "0",
                                "LedgerQuantity": "1",
                                "ClosePrice": "99",
                                "MarketValue": "99",
                                "MarginRate": "0.1",
                                "MarginValue": "9.9",
                                "CostPrice": "100",
                                "IncomeAmount": "-1"
                            }
                        ]
                    }
                ]
            }
            "#,
        )
        .unwrap();

        let csv = section_to_format(
            &content,
            &StatementSection::BondEquityHoldingSums,
            &ExportFormat::Csv,
        )
        .unwrap();
        let record = csv_record(&csv);

        assert_eq!(record[0], "Bond");
        assert_eq!(record[1], "US");
        assert_eq!(record[2], "USD");
        assert_eq!(record[3], "BOND1");
    }
}
