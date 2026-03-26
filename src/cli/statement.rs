use anyhow::Result;
use longbridge::statement::{
    CommonStatementContent, GetStatementDataDownloadUrlOptions, GetStatementDataListOptions,
    StatementType,
};

use super::{output::print_table, ExportFormat, OutputFormat, StatementCmd, StatementSection};

pub async fn cmd_statement(cmd: StatementCmd, format: &OutputFormat) -> Result<()> {
    match cmd {
        StatementCmd::List {
            statement_type,
            start_date,
            limit,
        } => cmd_list(&statement_type, start_date, limit, format).await,
        StatementCmd::Export {
            file_key,
            section: sections,
            export_format,
            output,
        } => cmd_export(&file_key, &sections, export_format, output.as_deref()).await,
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

async fn cmd_export(
    file_key: &str,
    sections: &[StatementSection],
    explicit_format: Option<ExportFormat>,
    output_path: Option<&str>,
) -> Result<()> {
    let ctx = crate::openapi::statement();
    let options = GetStatementDataDownloadUrlOptions::new(file_key);
    let resp = ctx.statement_data_download_url(options).await?;

    // Fetch the statement JSON
    let client = reqwest::Client::new();
    let body = client.get(&resp.url).send().await?.text().await?;
    let content: CommonStatementContent = serde_json::from_str(&body)?;

    // Resolve format: explicit flag wins, otherwise csv when -o is given, md when not.
    let format = explicit_format.unwrap_or(if output_path.is_some() {
        ExportFormat::Csv
    } else {
        ExportFormat::Md
    });

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
                for section in sections {
                    let file_name = format!("{}.{ext}", section_file_name(section));
                    let file_path = dir.join(&file_name);
                    let data = section_to_format(&content, section, &format)?;
                    std::fs::write(&file_path, data)?;
                    println!("Saved {section:?} to {}", file_path.display());
                }
            }
        }
        None => {
            // Print to stdout
            for section in sections {
                let data = section_to_format(&content, section, &format)?;
                print!("{data}");
            }
        }
    }
    Ok(())
}

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
                        vec![
                            sum.equity_type.as_str(),
                            sum.market.as_str(),
                            sum.currency.as_str(),
                            h.code.as_str(),
                            h.name.as_str(),
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
                        vec![
                            sum.currency.as_str(),
                            c.date.as_str(),
                            c.r#type.as_str(),
                            c.amount.as_str(),
                            c.remark.as_str(),
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
                        vec![
                            sum.market.as_str(),
                            sum.currency.as_str(),
                            t.trade_date.as_str(),
                            t.settle_date.as_str(),
                            t.contract_no.as_str(),
                            t.direction.as_str(),
                            t.code.as_str(),
                            t.name.as_str(),
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
            headers: &["market", "date", "code", "name", "type", "quantity"],
            rows: content
                .equity_holding_change_sums
                .iter()
                .flat_map(|sum| {
                    sum.equity_holding_changes.iter().map(move |c| {
                        vec![
                            sum.market.as_str(),
                            c.date.as_str(),
                            c.code.as_str(),
                            c.name.as_str(),
                            c.r#type.as_str(),
                            c.quantity.as_str(),
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
                        vec![
                            sum.market.as_str(),
                            l.date.as_str(),
                            l.expire_date.as_str(),
                            l.code.as_str(),
                            l.name.as_str(),
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
                        vec![
                            sum.market.as_str(),
                            sum.currency.as_str(),
                            t.trade_date.as_str(),
                            t.settle_date.as_str(),
                            t.contract_no.as_str(),
                            t.direction.as_str(),
                            t.code.as_str(),
                            t.name.as_str(),
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
                        vec![
                            sum.currency.as_str(),
                            sum.equity_type.as_str(),
                            t.order_date.as_str(),
                            t.confirm_date.as_str(),
                            t.status.as_str(),
                            t.contract_no.as_str(),
                            t.code.as_str(),
                            t.name.as_str(),
                            t.direction.as_str(),
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
                        vec![
                            sum.market.as_str(),
                            t.sub_date.as_str(),
                            t.code.as_str(),
                            t.name.as_str(),
                            t.sub_method.as_str(),
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
                        vec![
                            sum.market.as_str(),
                            sum.currency.as_str(),
                            t.trade_date.as_str(),
                            t.settle_date.as_str(),
                            t.contract_no.as_str(),
                            t.direction.as_str(),
                            t.code.as_str(),
                            t.name.as_str(),
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
                    vec![
                        f.date.as_str(),
                        f.currency.as_str(),
                        f.code.as_str(),
                        f.name.as_str(),
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
                    vec![
                        c.date.as_str(),
                        c.pay_date.as_str(),
                        c.market.as_str(),
                        c.code.as_str(),
                        c.name.as_str(),
                        c.remark.as_str(),
                        c.quantity.as_str(),
                        c.new_code.as_str(),
                        c.new_name.as_str(),
                        c.new_quantity.as_str(),
                        c.currency.as_str(),
                        c.new_amount.as_str(),
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
