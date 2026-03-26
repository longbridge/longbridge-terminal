use anyhow::Result;
use longbridge::statement::{
    CommonStatementContent, GetStatementDataDownloadUrlOptions, GetStatementDataListOptions,
    StatementType,
};

use super::{output::print_table, OutputFormat, StatementCmd, StatementSection};

pub async fn cmd_statement(cmd: StatementCmd, format: &OutputFormat) -> Result<()> {
    match cmd {
        StatementCmd::List {
            statement_type,
            start_date   ,
            limit ,
        } => cmd_list(&statement_type, start_date, limit, format).await,
        StatementCmd::Download {
            file_key,
            section: sections,
            output,
        } => cmd_download(&file_key, &sections, &output).await,
    }
}

async fn cmd_list(
    statement_type: &str,
    page: i32,
    page_size: i32,
    format: &OutputFormat,
) -> Result<()> {
    let st = match statement_type.to_lowercase().as_str() {
        "daily" | "d" => StatementType::Daily,
        "monthly" | "m" => StatementType::Monthly,
        other => anyhow::bail!("Unknown statement type '{other}', expected: daily | monthly"),
    };

    let ctx = crate::openapi::statement();
    let options = GetStatementDataListOptions::new(st)
        .page(page)
        .page_size(page_size);
    let resp = ctx.statement_data_list(options).await?;

    let headers = &["Date", "File Key"];
    let rows: Vec<Vec<String>> = resp
        .list
        .iter()
        .map(|item| vec![item.dt.to_string(), item.file_key.clone()])
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

// TODO: 补充 Asset 段落转成 Csv 的功能，只有一行 csv，表头是 Asset 的每个段落
async fn cmd_download(
    file_key: &str,
    sections: &[StatementSection],
    output_path: &str,
) -> Result<()> {
    let ctx = crate::openapi::statement();
    let options = GetStatementDataDownloadUrlOptions::new(file_key);
    let resp = ctx.statement_data_download_url(options).await?;

    // Download the JSON file
    let client = reqwest::Client::new();
    let body = client.get(&resp.url).send().await?.text().await?;
    let content: CommonStatementContent = serde_json::from_str(&body)?;

    if sections.len() == 1 {
        // Single section: write directly to the output path
        let csv_data = section_to_csv(&content, &sections[0])?;
        std::fs::write(output_path, csv_data)?;
        println!("Saved {:?} to {output_path}", sections[0]);
    } else {
        // Multiple sections: treat output_path as a directory
        let dir = std::path::Path::new(output_path);
        std::fs::create_dir_all(dir)?;
        for section in sections {
            let file_name = format!("{}.csv", section_file_name(section));
            let file_path = dir.join(&file_name);
            let csv_data = section_to_csv(&content, section)?;
            std::fs::write(&file_path, csv_data)?;
            println!("Saved {section:?} to {}", file_path.display());
        }
    }
    Ok(())
}

/// Map a `StatementSection` variant to a file-name-friendly string.
fn section_file_name(section: &StatementSection) -> &'static str {
    match section {
        StatementSection::Asset => "asset",
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
    }
}

fn section_to_csv(content: &CommonStatementContent, section: &StatementSection) -> Result<String> {
    match section {
        StatementSection::Asset => {
            let mut wtr = csv::Writer::from_writer(vec![]);
            wtr.write_record([
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
            ])?;
            let a = &content.asset;
            wtr.write_record([
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
            ])?;
            Ok(String::from_utf8(wtr.into_inner()?)?)
        }
        StatementSection::EquityHoldingSums => {
            let mut wtr = csv::Writer::from_writer(vec![]);
            wtr.write_record([
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
            ])?;
            for sum in &content.equity_holding_sums {
                for h in &sum.equity_holdings {
                    wtr.write_record([
                        &sum.equity_type,
                        &sum.market,
                        &sum.currency,
                        &h.code,
                        &h.name,
                        &h.begin_quantity,
                        &h.change_quantity,
                        &h.ledger_quantity,
                        &h.close_price,
                        &h.market_value,
                        &h.margin_rate,
                        &h.margin_value,
                        &h.cost_price,
                        &h.income_amount,
                    ])?;
                }
            }
            Ok(String::from_utf8(wtr.into_inner()?)?)
        }
        StatementSection::AccountBalanceChangeSums => {
            let mut wtr = csv::Writer::from_writer(vec![]);
            wtr.write_record(["currency", "date", "type", "amount", "remark", "biz_code"])?;
            for sum in &content.account_balance_change_sums {
                for c in &sum.account_balance_changes {
                    wtr.write_record([
                        &sum.currency,
                        &c.date,
                        &c.r#type,
                        &c.amount,
                        &c.remark,
                        &c.biz_code,
                    ])?;
                }
            }
            Ok(String::from_utf8(wtr.into_inner()?)?)
        }
        StatementSection::StockTradeSums => {
            let mut wtr = csv::Writer::from_writer(vec![]);
            wtr.write_record([
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
            ])?;
            for sum in &content.stock_trade_sums {
                for t in &sum.trades {
                    wtr.write_record([
                        &sum.market,
                        &sum.currency,
                        &t.trade_date,
                        &t.settle_date,
                        &t.contract_no,
                        &t.direction,
                        &t.code,
                        &t.name,
                        &t.trade_quantity,
                        &t.trade_price,
                        &t.trade_amount,
                        &t.clear_amount,
                    ])?;
                }
            }
            Ok(String::from_utf8(wtr.into_inner()?)?)
        }
        StatementSection::EquityHoldingChangeSums => {
            let mut wtr = csv::Writer::from_writer(vec![]);
            wtr.write_record(["market", "date", "code", "name", "type", "quantity"])?;
            for sum in &content.equity_holding_change_sums {
                for c in &sum.equity_holding_changes {
                    wtr.write_record([
                        &sum.market,
                        &c.date,
                        &c.code,
                        &c.name,
                        &c.r#type,
                        &c.quantity,
                    ])?;
                }
            }
            Ok(String::from_utf8(wtr.into_inner()?)?)
        }
        StatementSection::AccountBalanceLockSums => {
            let mut wtr = csv::Writer::from_writer(vec![]);
            wtr.write_record([
                "currency",
                "date",
                "expire_date",
                "amount",
                "remark",
                "ref_no",
            ])?;
            for sum in &content.account_balance_lock_sums {
                for l in &sum.account_balance_locks {
                    wtr.write_record([
                        &sum.currency,
                        &l.date,
                        &l.expire_date,
                        &l.amount,
                        &l.remark,
                        &l.ref_no,
                    ])?;
                }
            }
            Ok(String::from_utf8(wtr.into_inner()?)?)
        }
        StatementSection::EquityHoldingLockSums => {
            let mut wtr = csv::Writer::from_writer(vec![]);
            wtr.write_record([
                "market",
                "date",
                "expire_date",
                "code",
                "name",
                "quantity",
                "remark",
                "ref_no",
            ])?;
            for sum in &content.equity_holding_lock_sums {
                for l in &sum.equity_holding_locks {
                    wtr.write_record([
                        &sum.market,
                        &l.date,
                        &l.expire_date,
                        &l.code,
                        &l.name,
                        &l.quantity,
                        &l.remark,
                        &l.ref_no,
                    ])?;
                }
            }
            Ok(String::from_utf8(wtr.into_inner()?)?)
        }
        StatementSection::OptionTradeSums => {
            let mut wtr = csv::Writer::from_writer(vec![]);
            wtr.write_record([
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
            ])?;
            for sum in &content.option_trade_sums {
                for t in &sum.trades {
                    wtr.write_record([
                        &sum.market,
                        &sum.currency,
                        &t.trade_date,
                        &t.settle_date,
                        &t.contract_no,
                        &t.direction,
                        &t.code,
                        &t.name,
                        &t.trade_quantity,
                        &t.trade_price,
                        &t.trade_amount,
                        &t.clear_amount,
                    ])?;
                }
            }
            Ok(String::from_utf8(wtr.into_inner()?)?)
        }
        StatementSection::FundTradeSums => {
            let mut wtr = csv::Writer::from_writer(vec![]);
            wtr.write_record([
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
            ])?;
            for sum in &content.fund_trade_sums {
                for t in &sum.trades {
                    wtr.write_record([
                        &sum.currency,
                        &sum.equity_type,
                        &t.order_date,
                        &t.confirm_date,
                        &t.status,
                        &t.contract_no,
                        &t.code,
                        &t.name,
                        &t.direction,
                        &t.trade_amount,
                        &t.trade_quantity,
                        &t.price,
                    ])?;
                }
            }
            Ok(String::from_utf8(wtr.into_inner()?)?)
        }
        StatementSection::IpoTradeSums => {
            let mut wtr = csv::Writer::from_writer(vec![]);
            wtr.write_record([
                "market",
                "sub_date",
                "code",
                "name",
                "sub_method",
                "sub_quantity",
                "sub_amount",
            ])?;
            for sum in &content.ipo_trade_sums {
                for t in &sum.trades {
                    wtr.write_record([
                        &sum.market,
                        &t.sub_date,
                        &t.code,
                        &t.name,
                        &t.sub_method,
                        &t.sub_quantity,
                        &t.sub_amount,
                    ])?;
                }
            }
            Ok(String::from_utf8(wtr.into_inner()?)?)
        }
        StatementSection::VirtualTradeSums => {
            let mut wtr = csv::Writer::from_writer(vec![]);
            wtr.write_record([
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
            ])?;
            for sum in &content.virtual_trade_sums {
                for t in &sum.trades {
                    wtr.write_record([
                        &sum.market,
                        &sum.currency,
                        &t.trade_date,
                        &t.settle_date,
                        &t.contract_no,
                        &t.direction,
                        &t.code,
                        &t.name,
                        &t.trade_quantity,
                        &t.trade_price,
                        &t.trade_amount,
                        &t.clear_amount,
                    ])?;
                }
            }
            Ok(String::from_utf8(wtr.into_inner()?)?)
        }
        StatementSection::Interests => {
            let mut wtr = csv::Writer::from_writer(vec![]);
            wtr.write_record([
                "date",
                "currency",
                "rate",
                "fine_interest",
                "interest",
                "total",
            ])?;
            for i in &content.interests {
                wtr.write_record([
                    &i.date,
                    &i.currency,
                    &i.rate,
                    &i.fine_interest,
                    &i.interest,
                    &i.total,
                ])?;
            }
            Ok(String::from_utf8(wtr.into_inner()?)?)
        }
        StatementSection::LendingFees => {
            let mut wtr = csv::Writer::from_writer(vec![]);
            wtr.write_record([
                "date",
                "currency",
                "code",
                "name",
                "quantity",
                "settle_price",
                "lending_market_value",
                "rate",
                "amount",
            ])?;
            for f in &content.lending_fees {
                wtr.write_record([
                    &f.date,
                    &f.currency,
                    &f.code,
                    &f.name,
                    &f.quantity,
                    &f.settle_price,
                    &f.lending_market_value,
                    &f.rate,
                    &f.amount,
                ])?;
            }
            Ok(String::from_utf8(wtr.into_inner()?)?)
        }
        StatementSection::CustodianFees => {
            let mut wtr = csv::Writer::from_writer(vec![]);
            wtr.write_record(["date", "currency", "rate", "fee_amount", "fee", "total"])?;
            for f in &content.custodian_fees {
                wtr.write_record([
                    &f.date,
                    &f.currency,
                    &f.rate,
                    &f.fee_amount,
                    &f.fee,
                    &f.total,
                ])?;
            }
            Ok(String::from_utf8(wtr.into_inner()?)?)
        }
        StatementSection::Corps => {
            let mut wtr = csv::Writer::from_writer(vec![]);
            wtr.write_record([
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
            ])?;
            for c in &content.corps {
                wtr.write_record([
                    &c.date,
                    &c.pay_date,
                    &c.market,
                    &c.code,
                    &c.name,
                    &c.remark,
                    &c.quantity,
                    &c.new_code,
                    &c.new_name,
                    &c.new_quantity,
                    &c.currency,
                    &c.new_amount,
                ])?;
            }
            Ok(String::from_utf8(wtr.into_inner()?)?)
        }
    }
}
