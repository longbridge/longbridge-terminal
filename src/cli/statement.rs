use std::borrow::Cow;

use crate::data::statement::CommonStatementContent;
use crate::openapi::statement::{PdfStatementItem, PdfStatementKind};
use anyhow::Result;
use longbridge::asset::{GetStatementListOptions, GetStatementOptions, StatementType};
use serde_json::Value;
use time::OffsetDateTime;
use unicode_width::UnicodeWidthStr;

use super::{output::print_table, ExportFormat, OutputFormat, StatementCmd, StatementSection};

/// Parse a YYYY-MM-DD date string into YYYYMMDD integer required by the SDK.
fn parse_date_to_yyyymmdd(s: &str) -> anyhow::Result<i32> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        anyhow::bail!("Invalid date '{s}': expected YYYY-MM-DD format (e.g. 2026-01-21)");
    }
    let year: i32 = parts[0]
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid year in date '{s}'"))?;
    let month: i32 = parts[1]
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid month in date '{s}'"))?;
    let day: i32 = parts[2]
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid day in date '{s}'"))?;
    Ok(year * 10000 + month * 100 + day)
}

/// Return the first non-empty string from the candidates, or `""`.
fn first_non_empty<'a>(candidates: &[&'a str]) -> &'a str {
    candidates
        .iter()
        .find(|s| !s.is_empty())
        .copied()
        .unwrap_or("")
}

pub async fn cmd_statement(cmd: StatementCmd, format: &OutputFormat) -> Result<()> {
    match cmd {
        StatementCmd::List {
            statement_type,
            start_date,
            limit,
        } => {
            let is_monthly = matches!(statement_type.to_lowercase().as_str(), "monthly" | "m");
            let start_date = if let Some(s) = start_date {
                parse_date_to_yyyymmdd(&s)?
            } else {
                let now = OffsetDateTime::now_utc();
                if is_monthly {
                    let total_months = now.year() * 12 + now.month() as i32 - 1 - 12;
                    let year = total_months / 12;
                    let month = total_months % 12 + 1;
                    year * 10000 + month * 100 + 1
                } else {
                    let d = now - time::Duration::days(30);
                    d.year() * 10000 + i32::from(d.month() as u8) * 100 + i32::from(d.day())
                }
            };
            let limit = limit.unwrap_or(if is_monthly { 12 } else { 30 });
            cmd_list(&statement_type, start_date, limit, format).await
        }
        StatementCmd::Export {
            file_key,
            section: sections,
            all,
            export_format,
            output,
        } => {
            cmd_export(
                &file_key,
                &sections,
                all,
                export_format,
                output.as_deref(),
                format,
            )
            .await
        }
    }
}

async fn cmd_list(
    statement_type: &str,
    start_date: i32,
    limit: i32,
    format: &OutputFormat,
) -> Result<()> {
    let st = match statement_type.to_lowercase().as_str() {
        "daily" | "d" => StatementType::Daily,
        "monthly" | "m" => StatementType::Monthly,
        other => anyhow::bail!("Unknown statement type '{other}', expected: daily | monthly"),
    };

    let ctx = crate::openapi::statement();
    let options = GetStatementListOptions::new(st)
        .page(start_date)
        .page_size(limit);
    let resp = ctx.statements(options).await?;
    let json: Vec<(i32, String)> = resp
        .list
        .into_iter()
        .map(|item| (item.dt, item.file_key))
        .collect();

    // PDFs only fill in what the JSON list cannot provide, and only for
    // periods that predate the JSON statement service.
    let now = OffsetDateTime::now_utc();
    let now_month = format!("{}{:02}", now.year(), now.month() as u8);
    let pdf = match pdf_fill_window(start_date, &json, limit, &now_month) {
        Some((from, to)) => {
            let kind = match st {
                StatementType::Daily => PdfStatementKind::Daily,
                StatementType::Monthly => PdfStatementKind::Monthly,
            };
            crate::openapi::statement::pdf_statements(kind, &from, &to).await?
        }
        None => Vec::new(),
    };
    let rows = merge_statement_rows(&json, &pdf, limit);

    let headers = &["Date", "File Key", "Format"];
    let rows: Vec<Vec<String>> = rows
        .into_iter()
        .map(|r| vec![r.date, r.file_key, r.format.to_string()])
        .collect();
    print_table(headers, rows, format);
    Ok(())
}

/// Last month for which a statement may exist only as a PDF. From 2025-01 on
/// every statement has a JSON file, so the PDF list is never consulted there.
const LAST_PDF_ONLY_MONTH: &str = "202412";

/// The `yyyyMM` range to ask the PDF list for, or `None` when the JSON list
/// already covers the request.
///
/// The PDF endpoint is consulted only when the window starts on or before
/// [`LAST_PDF_ONLY_MONTH`] and the JSON list is short of the request: an
/// entry without a file, fewer entries than asked for, or (monthly) a month
/// missing from the middle of the window.
fn pdf_fill_window(
    start_date: i32,
    json: &[(i32, String)],
    limit: i32,
    now_month: &str,
) -> Option<(String, String)> {
    let from = yyyymm(start_date);
    if from.as_str() > LAST_PDF_ONLY_MONTH {
        return None;
    }
    let short = json.len() < usize::try_from(limit).unwrap_or(usize::MAX);
    let to = if short {
        now_month.to_string()
    } else {
        json.iter().map(|(dt, _)| yyyymm(*dt)).max()?
    };
    let to = if to.as_str() > LAST_PDF_ONLY_MONTH {
        LAST_PDF_ONLY_MONTH.to_string()
    } else {
        to
    };
    if to < from {
        return None;
    }
    let has_gap = json.iter().any(|(_, key)| key.is_empty()) || short || {
        // Monthly dates are exactly `yyyyMM`; a missing month in the window is a gap.
        let monthly = json.iter().all(|(dt, _)| *dt < 1_000_000);
        monthly && {
            let months: std::collections::BTreeSet<String> =
                json.iter().map(|(dt, _)| dt.to_string()).collect();
            months_between(&from, &to).any(|m| !months.contains(&m))
        }
    };
    has_gap.then_some((from, to))
}

/// `yyyyMMdd` or `yyyyMM` -> `yyyyMM`.
fn yyyymm(date: i32) -> String {
    if date >= 1_000_000 {
        format!("{:06}", date / 100)
    } else {
        format!("{date:06}")
    }
}

fn months_between(from: &str, to: &str) -> impl Iterator<Item = String> {
    let parse = |m: &str| -> Option<i32> {
        let y: i32 = m.get(..4)?.parse().ok()?;
        let mo: i32 = m.get(4..6)?.parse().ok()?;
        Some(y * 12 + mo - 1)
    };
    let (a, b) = (parse(from).unwrap_or(0), parse(to).unwrap_or(-1));
    (a..=b).map(|n| format!("{}{:02}", n / 12, n % 12 + 1))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatementRow {
    date: String,
    file_key: String,
    /// `json`, `pdf`, or empty when no file exists in either form.
    format: &'static str,
}

/// Merge the JSON list with PDF entries: a PDF only fills a date whose JSON
/// entry is missing or has no file. Sorted by date, at most `limit` rows.
fn merge_statement_rows(
    json: &[(i32, String)],
    pdf: &[PdfStatementItem],
    limit: i32,
) -> Vec<StatementRow> {
    let mut rows: std::collections::BTreeMap<String, StatementRow> = json
        .iter()
        .map(|(dt, key)| {
            let date = dt.to_string();
            let format = if key.is_empty() { "" } else { "json" };
            (
                date.clone(),
                StatementRow {
                    date,
                    file_key: key.clone(),
                    format,
                },
            )
        })
        .collect();
    for item in pdf {
        let date = item.display_name.replace('.', "");
        let filled = rows.get(&date).is_some_and(|r| !r.file_key.is_empty());
        if !filled {
            rows.insert(
                date.clone(),
                StatementRow {
                    date,
                    file_key: item.key.clone(),
                    format: "pdf",
                },
            );
        }
    }
    rows.into_values()
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
        .collect()
}

const ALL_SECTIONS: &[StatementSection] = &[
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
];

/// Where to find statements that predate the JSON statement service.
///
/// Statements issued before 2024-08 were delivered as password-protected
/// PDFs, so there is usually no JSON file behind the API for those periods.
const PDF_STATEMENT_HINT: &str = "Statements issued before 2024-08 were delivered as \
    password-protected PDFs. Run `longbridge statement list`: periods without a JSON file \
    are listed with a PDF file_key, and `statement export` downloads that PDF together with \
    its password.";

/// Explicit `--section` values win; `--all` (the default) selects every
/// section. `--all` defaults to true, so an explicit `--section` must be
/// checked first or it would never take effect.
fn resolve_sections(all: bool, sections: &[StatementSection]) -> Result<&[StatementSection]> {
    if !sections.is_empty() {
        Ok(sections)
    } else if all {
        Ok(ALL_SECTIONS)
    } else {
        anyhow::bail!("Specify --section or --all")
    }
}

async fn cmd_export(
    file_key: &str,
    sections: &[StatementSection],
    all: bool,
    explicit_format: Option<ExportFormat>,
    output_path: Option<&str>,
    output_format: &OutputFormat,
) -> Result<()> {
    let explicit_sections = !sections.is_empty();
    let sections = resolve_sections(all, sections)?;

    if file_key.trim().is_empty() {
        anyhow::bail!(
            "Empty --file-key: this statement has no downloadable file. \
             Use `longbridge statement list` and pick an entry with a non-empty file_key.\n\
             {PDF_STATEMENT_HINT}"
        );
    }

    if is_pdf_key(file_key) {
        return export_pdf(file_key, explicit_sections, output_path, output_format).await;
    }

    let ctx = crate::openapi::statement();
    let options = GetStatementOptions::new(file_key);
    let resp = ctx.statement_download_url(options).await?;

    // Fetch the statement JSON
    let client = reqwest::Client::new();
    let http = client.get(&resp.url).send().await?;
    let status = http.status().as_u16();
    let content_type = http
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(ToString::to_string);
    let body = http.text().await?;
    let value = parse_statement_body(status, content_type.as_deref(), &body)?;

    if is_legacy_statement(&value) {
        return export_legacy(
            &value,
            explicit_sections,
            explicit_format,
            output_path,
            output_format,
        );
    }
    let content: CommonStatementContent = serde_json::from_value(value)?;

    // --format json: output all sections as a single JSON object keyed by section name.
    if matches!(output_format, OutputFormat::Json) {
        let mut obj = serde_json::Map::new();
        for section in sections {
            let data = section_data(&content, section);
            if data.rows.is_empty() {
                continue;
            }
            let rows: Vec<serde_json::Map<String, Value>> = data
                .rows
                .iter()
                .map(|row| {
                    data.headers
                        .iter()
                        .zip(row.iter())
                        .map(|(k, v)| ((*k).to_string(), Value::String((*v).to_string())))
                        .collect()
                })
                .collect();
            obj.insert(
                section_file_name(section).to_string(),
                Value::Array(rows.into_iter().map(Value::Object).collect()),
            );
        }
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    // Resolve export format: explicit flag wins, otherwise csv when -o is given, md when not.
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
                    let data = section_data(&content, section);
                    if data.rows.is_empty() {
                        continue;
                    }
                    let formatted = match format {
                        ExportFormat::Csv => data.to_csv()?,
                        ExportFormat::Md => data.to_markdown(),
                    };
                    let file_name = format!("{}.{ext}", section_file_name(section));
                    let file_path = dir.join(&file_name);
                    std::fs::write(&file_path, formatted)?;
                    println!("Saved {section:?} to {}", file_path.display());
                }
            }
        }
        None => {
            for section in sections {
                let data = section_data(&content, section);
                if data.rows.is_empty() {
                    continue;
                }
                let formatted = match format {
                    ExportFormat::Csv => data.to_csv()?,
                    ExportFormat::Md => data.to_markdown(),
                };
                print!("{formatted}");
            }
        }
    }
    Ok(())
}

/// PDF statements are listed with keys ending in `.pdf`.
fn is_pdf_key(file_key: &str) -> bool {
    file_key.trim().to_ascii_lowercase().ends_with(".pdf")
}

/// File name for a PDF key such as
/// `*lb-statement*lb*1*202403*statement-monthly-202403-H10062184.pdf`.
fn pdf_file_name(file_key: &str) -> String {
    let last = file_key
        .trim()
        .rsplit(['*', '/'])
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect::<String>();
    if last.is_empty() || last == ".pdf" {
        "statement.pdf".to_string()
    } else {
        last
    }
}

/// Where to write the PDF: `-o` names a directory (existing, or ending in
/// `/`) to drop the file into, or the file path itself; without `-o` the file
/// lands in the current directory under its own name.
fn pdf_output_path(output: Option<&str>, file_name: &str) -> std::path::PathBuf {
    match output {
        None => std::path::PathBuf::from(file_name),
        Some(dir) if dir.ends_with('/') || std::path::Path::new(dir).is_dir() => {
            std::path::Path::new(dir).join(file_name)
        }
        Some(path) => std::path::PathBuf::from(path),
    }
}

/// Download a PDF statement and save it, printing the password that opens it.
async fn export_pdf(
    file_key: &str,
    explicit_sections: bool,
    output_path: Option<&str>,
    output_format: &OutputFormat,
) -> Result<()> {
    if explicit_sections {
        eprintln!(
            "Note: this statement is a PDF; --section does not apply and the whole file is saved."
        );
    }
    let dl = crate::openapi::statement::pdf_download(file_key).await?;

    let http = reqwest::Client::new().get(&dl.url).send().await?;
    let status = http.status().as_u16();
    let bytes = http.bytes().await?;
    if !(200..300).contains(&status) || !bytes.starts_with(b"%PDF") {
        let excerpt: String = String::from_utf8_lossy(&bytes[..bytes.len().min(300)]).into();
        anyhow::bail!("PDF statement download failed (HTTP {status}):\n{excerpt}");
    }

    let path = pdf_output_path(output_path, &pdf_file_name(file_key));
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, &bytes)?;

    if matches!(output_format, OutputFormat::Json) {
        let out = serde_json::json!({
            "file": path.display().to_string(),
            "password": dl.pass,
            "cache_key": dl.cache_key,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Saved PDF statement to {}", path.display());
        println!("Password: {}", dl.pass);
    }
    Ok(())
}

/// Parse the downloaded statement body, reporting HTTP status, content type
/// and a body excerpt when it is not the expected JSON document.
///
/// Statements issued before the current storage format (older `file_key`
/// prefixes) resolve to a download URL whose body is an XML/HTML error page
/// rather than JSON; surfacing that body is the only way to tell why.
fn parse_statement_body(status: u16, content_type: Option<&str>, body: &str) -> Result<Value> {
    let describe = || {
        let content_type = content_type.unwrap_or("unknown");
        let excerpt: String = body.trim().chars().take(300).collect();
        let excerpt = if excerpt.is_empty() {
            "(empty body)".to_string()
        } else {
            excerpt
        };
        format!("HTTP {status}, content-type: {content_type}\n{excerpt}")
    };

    if body.trim().is_empty() {
        anyhow::bail!(
            "Statement download returned an empty body ({}). \
             The file behind this file_key is not available for download.\n{PDF_STATEMENT_HINT}",
            describe()
        );
    }
    let mut value: Value = serde_json::from_str(body).map_err(|e| {
        anyhow::anyhow!(
            "Statement download did not return JSON ({e}).\n{}\n\
             No JSON statement exists behind this file_key.\n{PDF_STATEMENT_HINT}",
            describe()
        )
    })?;
    // Statements issued before 2022-12 serialize empty sections as `null`.
    crate::utils::json::strip_nulls(&mut value);
    Ok(value)
}

/// Statements issued before 2022-03 use a different document layout
/// (`AssetDetail`, `CashDetails`, `StockHoldingDetails`, ...) that the
/// section mapping in this module does not know about.
fn is_legacy_statement(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|o| !o.contains_key("Asset") && o.contains_key("AssetDetail"))
}

/// Export a legacy-layout statement by flattening the raw document into
/// generic tables. `--section` cannot be honored because the section names do
/// not exist in that layout, so every populated table is exported.
fn export_legacy(
    value: &Value,
    explicit_sections: bool,
    explicit_format: Option<ExportFormat>,
    output_path: Option<&str>,
    output_format: &OutputFormat,
) -> Result<()> {
    if explicit_sections {
        eprintln!(
            "Note: this statement uses the legacy layout (issued before 2022-03); \
             --section is ignored and all populated tables are exported."
        );
    }

    if matches!(output_format, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(value)?);
        return Ok(());
    }

    let tables = crate::utils::json::flatten_tables(value);
    let format = explicit_format.unwrap_or(if output_path.is_some() {
        ExportFormat::Csv
    } else {
        ExportFormat::Md
    });
    let ext = match format {
        ExportFormat::Csv => "csv",
        ExportFormat::Md => "md",
    };

    let dir = output_path.map(std::path::Path::new);
    if let Some(dir) = dir {
        std::fs::create_dir_all(dir)?;
    }
    for table in &tables {
        let headers: Vec<&str> = table.headers.iter().map(String::as_str).collect();
        let rows: Vec<Vec<&str>> = table
            .rows
            .iter()
            .map(|r| r.iter().map(String::as_str).collect())
            .collect();
        let data = SectionData {
            title: &table.title,
            headers: &headers,
            rows,
        };
        let formatted = match format {
            ExportFormat::Csv => data.to_csv()?,
            ExportFormat::Md => data.to_markdown(),
        };
        match dir {
            Some(dir) => {
                let file_path = dir.join(format!("{}.{ext}", table.name));
                std::fs::write(&file_path, formatted)?;
                println!("Saved {} to {}", table.title, file_path.display());
            }
            None => print!("{formatted}"),
        }
    }
    Ok(())
}

struct SectionData<'a> {
    title: &'a str,
    headers: &'a [&'a str],
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
                            // Files issued before 2024-08 carry only `Currency`.
                            first_non_empty(&[&b.currency_code, &b.currency]),
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

pub(crate) fn schema_for_path(path: &[String]) -> Option<super::schema::ResponseSchema> {
    use super::schema::{array, object};

    let command = path.join(" ");
    let schema = match command.as_str() {
        "statement" | "statement list" => {
            array("Available statements", &["date", "file_key", "format"])
        }
        "statement export" => object("Exported statement sections", statement_section_fields()),
        _ => return None,
    };
    Some(schema)
}

fn statement_section_fields() -> &'static [&'static str] {
    &[
        "asset",
        "account_balances",
        "equity_holdings",
        "account_balance_changes",
        "stock_trades",
        "equity_holding_changes",
        "option_trades",
        "fund_trades",
        "ipo_trades",
        "interests",
        "corps",
        "outstandings",
    ]
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
        let widths = self.markdown_column_widths();
        Self::push_markdown_row(&mut out, self.headers.iter().copied(), &widths);
        Self::push_markdown_separator(&mut out, &widths);
        for row in &self.rows {
            debug_assert_eq!(row.len(), self.headers.len());
            Self::push_markdown_row(&mut out, row.iter().copied(), &widths);
        }
        out.push('\n');
        out
    }

    fn markdown_column_widths(&self) -> Vec<usize> {
        let mut widths = self
            .headers
            .iter()
            .map(|header| markdown_cell_width(header).max(3))
            .collect::<Vec<_>>();

        for row in &self.rows {
            debug_assert_eq!(row.len(), self.headers.len());
            for (width, cell) in widths.iter_mut().zip(row.iter()) {
                *width = (*width).max(markdown_cell_width(cell));
            }
        }

        widths
    }

    fn push_markdown_row<'a>(
        out: &mut String,
        cells: impl IntoIterator<Item = &'a str>,
        widths: &[usize],
    ) {
        out.push('|');
        for (cell, width) in cells.into_iter().zip(widths.iter().copied()) {
            let escaped = escape_markdown_cell(cell);
            let padding = width.saturating_sub(UnicodeWidthStr::width(escaped.as_ref()));
            out.push(' ');
            out.push_str(escaped.as_ref());
            out.push_str(&" ".repeat(padding));
            out.push(' ');
            out.push('|');
        }
        out.push('\n');
    }

    fn push_markdown_separator(out: &mut String, widths: &[usize]) {
        out.push('|');
        for width in widths {
            out.push(' ');
            out.push_str(&"-".repeat(*width));
            out.push(' ');
            out.push('|');
        }
        out.push('\n');
    }
}

fn escape_markdown_cell(cell: &str) -> Cow<'_, str> {
    if cell.contains('|') {
        Cow::Owned(cell.replace('|', "\\|"))
    } else {
        Cow::Borrowed(cell)
    }
}

fn markdown_cell_width(cell: &str) -> usize {
    UnicodeWidthStr::width(escape_markdown_cell(cell).as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pdf(display_name: &str, key: &str) -> PdfStatementItem {
        PdfStatementItem {
            display_name: display_name.to_string(),
            key: key.to_string(),
            cache_key: String::new(),
        }
    }

    #[test]
    fn pdf_fills_only_dates_without_a_json_file() {
        let json = vec![
            (202403, String::new()),
            (202408, "/statement_data/x/202408.json".to_string()),
        ];
        let pdf = vec![
            pdf("2024.03", "*a*202403.pdf"),
            pdf("2024.05", "*a*202405.pdf"),
            pdf("2024.08", "*a*202408.pdf"),
        ];
        let rows = merge_statement_rows(&json, &pdf, 10);
        let got: Vec<(&str, &str, &str)> = rows
            .iter()
            .map(|r| (r.date.as_str(), r.file_key.as_str(), r.format))
            .collect();
        assert_eq!(
            got,
            [
                ("202403", "*a*202403.pdf", "pdf"),
                ("202405", "*a*202405.pdf", "pdf"),
                ("202408", "/statement_data/x/202408.json", "json"),
            ]
        );
        assert_eq!(merge_statement_rows(&json, &pdf, 2).len(), 2);
    }

    #[test]
    fn pdf_window_skips_periods_that_always_have_json() {
        let full = vec![(202501, "k".to_string()), (202502, "k".to_string())];
        assert_eq!(pdf_fill_window(20250101, &full, 2, "202609"), None);
        // Complete JSON coverage inside 2024 needs no PDF lookup either.
        let complete = vec![(202411, "k".to_string()), (202412, "k".to_string())];
        assert_eq!(pdf_fill_window(20241101, &complete, 2, "202609"), None);
    }

    #[test]
    fn pdf_window_covers_gaps_and_caps_at_last_pdf_month() {
        // Empty key -> gap; JSON window runs into 2025 but PDFs stop at 2024-12.
        let json = vec![(202403, String::new()), (202501, "k".to_string())];
        assert_eq!(
            pdf_fill_window(20240301, &json, 2, "202609"),
            Some(("202403".to_string(), "202412".to_string()))
        );
        // A month missing from the middle of a full-length list is a gap.
        let holes = vec![(202404, "k".to_string()), (202407, "k".to_string())];
        assert_eq!(
            pdf_fill_window(20240401, &holes, 2, "202609"),
            Some(("202404".to_string(), "202407".to_string()))
        );
        // Fewer rows than asked for: window extends to now, capped at 2024-12.
        let short = vec![(20240319, "k".to_string())];
        assert_eq!(
            pdf_fill_window(20240301, &short, 30, "202609"),
            Some(("202403".to_string(), "202412".to_string()))
        );
    }

    #[test]
    fn pdf_key_detection_and_file_name() {
        let key = "*lb-statement*lb*1*202403*statement-monthly-202403-H10062184.pdf";
        assert!(is_pdf_key(key));
        assert!(!is_pdf_key(
            "/statement_data/data/lb/1/202411/10000104.json"
        ));
        assert_eq!(pdf_file_name(key), "statement-monthly-202403-H10062184.pdf");
        assert_eq!(pdf_file_name("*.pdf"), "statement.pdf");
    }

    #[test]
    fn pdf_output_path_treats_trailing_slash_as_directory() {
        assert_eq!(
            pdf_output_path(None, "a.pdf"),
            std::path::PathBuf::from("a.pdf")
        );
        assert_eq!(
            pdf_output_path(Some("out/"), "a.pdf"),
            std::path::PathBuf::from("out/a.pdf")
        );
        assert_eq!(
            pdf_output_path(Some("out/x.pdf"), "a.pdf"),
            std::path::PathBuf::from("out/x.pdf")
        );
    }

    #[test]
    fn explicit_sections_override_default_all() {
        let picked = [StatementSection::Asset];
        assert_eq!(resolve_sections(true, &picked).unwrap().len(), 1);
        assert_eq!(
            resolve_sections(true, &[]).unwrap().len(),
            ALL_SECTIONS.len()
        );
        assert!(resolve_sections(false, &[]).is_err());
    }

    #[test]
    fn account_balances_fall_back_to_currency_name() {
        let content: CommonStatementContent = serde_json::from_str(
            r#"{"AccountBalanceSum":{"AccountBalances":[{"Currency":"美元","BeginAmount":"1"}]}}"#,
        )
        .unwrap();
        let data = section_data(&content, &StatementSection::AccountBalanceSum);
        assert_eq!(data.rows[0][0], "美元");
    }

    #[test]
    fn parse_statement_body_rejects_non_json_with_context() {
        let body = r#"<?xml version="1.0" encoding="UTF-8"?><Error><Code>NoSuchKey</Code><Message>The specified key does not exist.</Message></Error>"#;
        let err = parse_statement_body(404, Some("application/xml"), body).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("HTTP 404"), "{msg}");
        assert!(msg.contains("application/xml"), "{msg}");
        assert!(msg.contains("NoSuchKey"), "{msg}");
    }

    #[test]
    fn parse_statement_body_reports_empty_body() {
        let err = parse_statement_body(200, None, "").unwrap_err();
        assert!(err.to_string().contains("empty"), "{err}");
    }

    #[test]
    fn parse_statement_body_accepts_json() {
        let value =
            parse_statement_body(200, Some("application/json"), r#"{"Date":"202411"}"#).unwrap();
        assert_eq!(value["Date"], "202411");
    }

    #[test]
    fn parse_statement_body_tolerates_null_sections() {
        let value = parse_statement_body(
            200,
            None,
            r#"{"Asset":{"Currency":"HKD"},"Interests":null,"FundTradeSums":null}"#,
        )
        .unwrap();
        let content: CommonStatementContent = serde_json::from_value(value).unwrap();
        assert_eq!(content.asset.currency, "HKD");
        assert!(content.interests.is_empty());
    }

    #[test]
    fn legacy_layout_is_detected_by_asset_detail() {
        assert!(is_legacy_statement(&serde_json::json!({"AssetDetail": {}})));
        assert!(!is_legacy_statement(&serde_json::json!({"Asset": {}})));
        assert!(!is_legacy_statement(&serde_json::json!({})));
    }

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

    #[test]
    fn markdown_output_pads_empty_cells_to_column_width() {
        let data = SectionData {
            title: "Test",
            headers: &["alpha", "beta", "gamma"],
            rows: vec![vec!["1", "", "333"], vec!["long", "22", ""]],
        };

        assert_eq!(
            data.to_markdown(),
            concat!(
                "## Test\n\n",
                "| alpha | beta | gamma |\n",
                "| ----- | ---- | ----- |\n",
                "| 1     |      | 333   |\n",
                "| long  | 22   |       |\n\n",
            )
        );
    }

    #[test]
    fn markdown_output_escapes_pipe_characters_in_cells() {
        let data = SectionData {
            title: "Test",
            headers: &["name", "remark"],
            rows: vec![vec!["A|B", "x"]],
        };

        let markdown = data.to_markdown();

        assert!(markdown.contains("| A\\|B | x      |\n"));
    }
}
