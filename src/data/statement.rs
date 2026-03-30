use serde::{Deserialize, Serialize};

/// Root statement content.
///
/// `CompanyData` is intentionally omitted because `spec.md` marks it as
/// non-JSON output.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct CommonStatementContent {
    pub generated_at: String,
    pub date: String,
    pub account_channel: String,
    pub lang: String,
    pub file_type: String,
    pub hide_footer: String,
    pub hide_memo: String,
    pub member_info: MemberInfo,
    pub asset: Asset,
    pub account_balance_sum: AccountBalanceSum,
    pub equity_holding_sums: Vec<EquityHoldingSum>,
    pub bond_equity_holding_sums: Vec<EquityHoldingSum>,
    pub corps: Vec<Corp>,
    pub virtual_trade_sums: Vec<VirtualTradeSum>,
    pub stock_trade_sums: Vec<StockTradeSum>,
    pub option_trade_sums: Vec<StockTradeSum>,
    pub fund_trade_sums: Vec<FundTradeSum>,
    pub ipo_trade_sums: Vec<IpoTradeSum>,
    pub otc_trade_sums: Vec<OtcTradeSum>,
    pub outstanding_sums: Vec<OutstandingSum>,
    pub account_balance_change_sums: Vec<AccountBalanceChangeSum>,
    pub equity_holding_change_sums: Vec<EquityHoldingChangeSum>,
    pub account_balance_lock_sums: Vec<AccountBalanceLockSum>,
    pub equity_holding_lock_sums: Vec<EquityHoldingLockSum>,
    pub financing_transaction_sums: Vec<FinancingTransactionSum>,
    pub interests: Vec<Interest>,
    pub lending_fees: Vec<LendingFee>,
    pub custodian_fees: Vec<CustodianFee>,
    pub interest_deposits: Vec<Interest>,
    pub maintenance_fees: Vec<MaintenanceFee>,
    pub cash_pluses: Vec<CashPlusInfo>,
    pub gst_details: Vec<GstDetail>,
    pub split_chapter_op: SplitChapterOp,
    pub show_ctl: ShowControl,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct MemberInfo {
    pub name: String,
    pub name_en: String,
    pub name_zh: String,
    pub name_hk: String,
    pub phone_number: String,
    pub address: String,
    pub account_type: String,
    pub account_type_code: String,
    pub account_no: String,
    pub ae_code: String,
    pub ae_name: String,
    pub org_id: String,
    pub statement_send: String,
    pub card_id: String,
    pub employer_email: String,
    pub password: String,
    pub account_name: String,
    pub da_flag: String,
    pub mark: String,
    pub va_mark: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Asset {
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct AccountBalanceSum {
    pub account_balances: Vec<AccountBalance>,
    pub sum_begin_amount: String,
    pub sum_change_amount: String,
    pub sum_ledger_amount: String,
    pub sum_settled_amount: String,
    pub sum_outstanding: String,
    pub sum_accrued_interest: String,
    pub currency: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct AccountBalance {
    pub currency: String,
    pub currency_code: String,
    pub begin_amount: String,
    pub begin_amount_as_hkd: String,
    pub change_amount: String,
    pub change_amount_as_hkd: String,
    pub ledger_amount: String,
    pub settled_amount: String,
    #[serde(rename = "SettledAmountAsHKd")]
    pub settled_amount_as_hkd: String,
    pub outstanding_amount: String,
    pub accrued_interest: String,
    #[serde(rename = "OutstandingAmountAsHKd")]
    pub outstanding_amount_as_hkd: String,
    pub rate: String,
    pub ledger_amount_as_hkd: String,
    pub standard_currency: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct EquityHoldingSum {
    pub equity_type: String,
    pub equity_type_code: String,
    pub market: String,
    pub market_code: String,
    pub currency: String,
    pub currency_code: String,
    pub equity_holdings: Vec<EquityHolding>,
    pub sum_margin_value: String,
    pub sum_market_value: String,
    pub sum_income_amount: String,
    pub sum_margin_value_suspend: String,
    pub sum_market_value_suspend: String,
    pub sum_income_amount_suspend: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct EquityHolding {
    pub name: String,
    pub name_en: String,
    pub name_zh: String,
    pub name_hk: String,
    pub code: String,
    pub begin_quantity: String,
    pub change_quantity: String,
    pub ledger_quantity: String,
    pub entity_quantity: String,
    pub entity_quantity_flag: String,
    pub close_price: String,
    pub market_value: String,
    pub margin_rate: String,
    pub margin_value: String,
    pub covered: String,
    pub covered_code: String,
    pub equity_type: String,
    pub equity_type_code: String,
    pub market: String,
    pub market_code: String,
    pub currency: String,
    pub currency_code: String,
    pub cost_price: String,
    pub income_amount: String,
    pub stock_status: String,
    pub stock_status_code: String,
    pub suspend_days: String,
    pub close_price_suspend: String,
    pub market_value_suspend: String,
    pub margin_value_suspend: String,
    pub income_amount_suspend: String,
    pub initial_margin_rate: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct AccountBalanceChangeSum {
    pub currency: String,
    pub currency_code: String,
    pub account_balance_changes: Vec<AccountBalanceChange>,
    pub sum_amount: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct AccountBalanceChange {
    pub date: String,
    pub biz_code: String,
    pub r#type: String,
    pub type_en: String,
    pub type_zh: String,
    pub type_hk: String,
    pub currency: String,
    pub currency_code: String,
    pub amount: String,
    pub remark: String,
    pub remark_en: String,
    pub remark_zh: String,
    pub remark_hk: String,
    pub create_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct EquityHoldingChangeSum {
    pub market: String,
    pub market_code: String,
    pub equity_holding_changes: Vec<EquityHoldingChange>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct EquityHoldingChange {
    pub date: String,
    pub name: String,
    pub name_en: String,
    pub name_zh: String,
    pub name_hk: String,
    pub code: String,
    pub r#type: String,
    pub type_en: String,
    pub type_zh: String,
    pub type_hk: String,
    pub quantity: String,
    pub remark: String,
    pub remark_en: String,
    pub remark_zh: String,
    pub remark_hk: String,
    pub market: String,
    pub market_code: String,
    pub create_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct AccountBalanceLockSum {
    pub currency: String,
    pub currency_code: String,
    pub account_balance_locks: Vec<AccountBalanceLock>,
    pub sum_amount: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct AccountBalanceLock {
    pub date: String,
    pub expire_date: String,
    pub amount: String,
    pub remark: String,
    pub ref_no: String,
    pub currency: String,
    pub currency_code: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct EquityHoldingLockSum {
    pub market: String,
    pub market_code: String,
    pub equity_holding_locks: Vec<EquityHoldingLock>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct EquityHoldingLock {
    pub date: String,
    pub expire_date: String,
    pub name: String,
    pub name_en: String,
    pub name_zh: String,
    pub name_hk: String,
    pub code: String,
    pub quantity: String,
    pub remark: String,
    pub ref_no: String,
    pub market: String,
    pub market_code: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct StockTradeSum {
    pub market: String,
    pub market_code: String,
    pub currency: String,
    pub currency_code: String,
    pub trades: Vec<StockTrade>,
    pub sum_trade_amount: String,
    pub sum_clear_amount: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct StockTrade {
    pub trade_date: String,
    pub settle_date: String,
    pub contract_no: String,
    pub direction: String,
    pub direction_code: String,
    pub name: String,
    pub name_en: String,
    pub name_zh: String,
    pub name_hk: String,
    pub code: String,
    pub trade_quantity: String,
    pub trade_amount: String,
    pub trade_price: String,
    pub clear_amount: String,
    pub fees: Vec<Fee>,
    pub gst_fee_rate: String,
    pub settle_currency_rate: String,
    pub lbsg_fees: LbsgFees,
    pub trade_dones: Vec<TradeDone>,
    pub market: String,
    pub market_code: String,
    pub currency: String,
    pub currency_code: String,
    pub cancel_flag: String,
    pub cancel_contract_no: String,
    pub ibond_flag: String,
    pub accrued_interest: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Fee {
    pub name: String,
    pub name_en: String,
    pub name_zh: String,
    pub name_hk: String,
    pub fee: String,
    pub origin: String,
    pub gst_flag: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct LbsgFees {
    pub gst_origin_fees: Vec<Fee>,
    pub sum_gst_origin_fees: String,
    pub sum_gst_origin_fees_as_hkd: String,
    pub sum_other_fees: String,
    pub sum_other_fees_as_hkd: String,
    pub sum_gst_origin_fees_and_gst_fee: String,
    pub sum_gst_origin_fees_and_gst_fee_as_hkd: String,
    pub gst_fee_rate: String,
    pub gst_fee: String,
    pub gst_fee_as_hkd: String,
    pub fees: Vec<Fee>,
    pub sum_all_fees: String,
    pub sum_all_fees_as_hkd: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct TradeDone {
    pub order_time: String,
    pub trade_done_time: String,
    pub order_tag: String,
    pub order_tag_code: String,
    pub trade_quantity: String,
    pub trade_price: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct FundTradeSum {
    pub currency: String,
    pub currency_code: String,
    pub equity_type: String,
    pub equity_type_code: String,
    pub trades: Vec<FundTrade>,
    pub sum_trade_amount: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct FundTrade {
    pub order_date: String,
    pub confirm_date: String,
    pub status: String,
    pub contract_no: String,
    pub name: String,
    pub name_en: String,
    pub name_zh: String,
    pub name_hk: String,
    pub code: String,
    pub direction: String,
    pub direction_code: String,
    pub trade_amount: String,
    pub trade_quantity: String,
    pub price: String,
    pub currency: String,
    pub currency_code: String,
    pub equity_type: String,
    pub equity_type_code: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct IpoTradeSum {
    pub market: String,
    pub market_code: String,
    pub trades: Vec<IpoTrade>,
    pub sum_trade_amount: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct IpoTrade {
    pub sub_date: String,
    pub name: String,
    pub name_en: String,
    pub name_zh: String,
    pub name_hk: String,
    pub code: String,
    pub sub_method: String,
    pub sub_method_code: String,
    pub sub_quantity: String,
    pub sub_amount: String,
    pub market: String,
    pub market_code: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct VirtualTradeSum {
    pub market: String,
    pub market_code: String,
    pub currency: String,
    pub currency_code: String,
    pub trades: Vec<VirtualTrade>,
    pub sum_trade_amount: String,
    pub sum_clear_amount: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct VirtualTrade {
    pub trade_date: String,
    pub settle_date: String,
    pub contract_no: String,
    pub direction: String,
    pub direction_code: String,
    pub name: String,
    pub name_en: String,
    pub name_zh: String,
    pub name_hk: String,
    pub code: String,
    pub trade_quantity: String,
    pub trade_amount: String,
    pub trade_price: String,
    pub clear_amount: String,
    pub fees: Vec<Fee>,
    pub gst_fee_rate: String,
    pub settle_currency_rate: String,
    pub trade_dones: Vec<TradeDone>,
    pub market: String,
    pub market_code: String,
    pub currency: String,
    pub currency_code: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Interest {
    pub date: String,
    pub currency: String,
    pub currency_code: String,
    pub rate: String,
    pub fine_interest: String,
    pub interest: String,
    pub total: String,
    pub deduction_amount: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct LendingFee {
    pub date: String,
    pub currency: String,
    pub currency_code: String,
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
    pub current_amount: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct CustodianFee {
    pub date: String,
    pub currency: String,
    pub currency_code: String,
    pub rate: String,
    pub fee_amount: String,
    pub fee: String,
    pub total: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Corp {
    pub date: String,
    pub pay_date: String,
    pub market: String,
    pub market_code: String,
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
    pub currency_code: String,
    pub new_amount: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct SplitInfo {
    pub no_group_label: String,
    pub no_group_sum: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct OtcTradeSum {
    pub market: String,
    pub market_code: String,
    pub currency: String,
    pub currency_code: String,
    pub equity_type: String,
    pub equity_type_code: String,
    pub order_type: String,
    pub trades: Vec<StockTrade>,
    pub sum_trade_amount: String,
    pub sum_clear_amount: String,
    #[serde(flatten)]
    pub split_info: SplitInfo,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct OutstandingSum {
    pub market_code: String,
    pub market: String,
    pub currency_code: String,
    pub currency: String,
    pub equity_type_code: String,
    pub equity_type: String,
    pub sum_trade_amount: String,
    pub sum_trade_amount_as_hkd: String,
    pub sum_clear_amount: String,
    pub sum_clear_amount_as_hkd: String,
    pub is_otc: String,
    pub outstanding_trades: Vec<OutstandingTradeInfo>,
    #[serde(flatten)]
    pub split_info: SplitInfo,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct OutstandingTradeInfo {
    pub trade_date: String,
    pub settle_date: String,
    pub equity_settle_date: String,
    pub balance_settle_date: String,
    pub contract_no: String,
    pub direction_code: String,
    pub direction: String,
    pub code: String,
    pub name: String,
    pub name_en: String,
    pub name_zh: String,
    pub name_hk: String,
    pub trade_quantity: String,
    pub trade_price: String,
    pub trade_amount: String,
    pub clear_amount: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct FinancingTransactionSum {
    pub currency: String,
    pub currency_code: String,
    pub sum_amount: String,
    pub financing_balance_settled: String,
    pub transaction_details: Vec<FinancingTransactionDetail>,
    #[serde(flatten)]
    pub split_info: SplitInfo,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct FinancingTransactionDetail {
    pub date: String,
    pub biz_code: String,
    pub r#type: String,
    pub type_en: String,
    pub type_zh: String,
    pub type_hk: String,
    pub currency: String,
    pub currency_code: String,
    pub amount: String,
    pub remark: String,
    pub remark_en: String,
    pub remark_zh: String,
    pub remark_hk: String,
    pub create_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct MaintenanceFee {
    pub year_month: String,
    pub currency_code: String,
    pub currency_name: String,
    pub market_code: String,
    pub market_name: String,
    pub fee_rate: String,
    pub accrued_fee: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct CashPlusInfo {
    pub date: String,
    pub currency_code: String,
    pub currency_name: String,
    pub latest_balance: String,
    pub latest_profit_loss: String,
    pub accum_profit_loss: String,
    pub apr: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct GstDetail {
    pub date: String,
    #[serde(rename = "Ref")]
    pub r#ref: String,
    pub ref_id: String,
    pub biz_code: String,
    pub r#type: String,
    pub type_en: String,
    pub type_zh: String,
    pub type_hk: String,
    pub remark: String,
    pub remark_en: String,
    pub remark_zh: String,
    pub remark_hk: String,
    pub currency: String,
    pub currency_code: String,
    pub amount: String,
    pub fee_rate: String,
    pub fee_amount: String,
    pub total: String,
    pub fx_rate: String,
    pub amount_as_hkd: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct SplitChapterOp {
    pub no_financing_interest_title: String,
    pub no_lending_fee_title: String,
    pub no_custodian_fee_title: String,
    pub no_deposit_interest_title: String,
    pub no_maintenance_fee_title: String,
    pub no_cash_plus_info_title: String,
    pub no_corp_title: String,
    pub no_equity_holding_lock_title: String,
    pub no_account_balance_lock_title: String,
    pub no_equity_holding_change_title: String,
    pub no_account_balance_change_title: String,
    pub no_outstanding_title: String,
    pub no_otc_trade_title: String,
    pub no_ipo_trade_title: String,
    pub no_fund_trade_title: String,
    pub no_option_trade_title: String,
    pub no_stock_trade_title: String,
    pub no_virtual_trade_title: String,
    pub no_bond_equity_holding_title: String,
    pub no_equity_holding_title: String,
    pub no_gst_detail_title: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ShowControl {
    pub is_show_equity_with_loan: String,
}
