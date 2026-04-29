use anyhow::{anyhow, Result};
use futures::future::join_all;
use roxmltree::Document;
use std::sync::Arc;

use super::output::print_table;
use super::OutputFormat;

// SEC EDGAR requires a declared automated tool User-Agent in the format:
// "OrganizationName ApplicationName contact@email.com"
const SEC_UA: &str = "Longbridge longbridge-terminal support@longbridge.com";

fn sec_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(SEC_UA)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("failed to build HTTP client")
}

struct InsiderTrade {
    filing_date: String,
    owner: String,
    title: String,
    date: String,
    code: String,
    shares: f64,
    price: f64,
    value: f64,
    shares_after: f64,
}

// Strip .HK, .US, etc. market suffixes and uppercase the ticker.
fn normalize_ticker(symbol: &str) -> String {
    match symbol.rfind('.') {
        Some(i) => symbol[..i].to_uppercase(),
        None => symbol.to_uppercase(),
    }
}

fn tx_label(code: &str) -> &str {
    match code {
        "P" => "BUY",
        "S" => "SELL",
        "A" => "GRANT",
        "D" => "DISP",
        "F" => "TAX",
        "M" | "X" => "EXERCISE",
        "G" => "GIFT",
        other => other,
    }
}

fn fmt_shares(n: f64) -> String {
    if n >= 1_000_000_000.0 {
        format!("{:.2}B", n / 1_000_000_000.0)
    } else if n >= 1_000_000.0 {
        format!("{:.2}M", n / 1_000_000.0)
    } else if n >= 1_000.0 {
        format!("{:.2}K", n / 1_000.0)
    } else {
        format!("{n:.0}")
    }
}

fn fmt_value(v: f64) -> String {
    if v <= 0.0 {
        return "-".to_string();
    }
    if v >= 1_000_000_000.0 {
        format!("${:.2}B", v / 1_000_000_000.0)
    } else if v >= 1_000_000.0 {
        format!("${:.2}M", v / 1_000_000.0)
    } else if v >= 1_000.0 {
        format!("${:.2}K", v / 1_000.0)
    } else {
        format!("${v:.2}")
    }
}

fn fmt_price(p: f64) -> String {
    if p <= 0.0 {
        "-".to_string()
    } else {
        format!("${p:.2}")
    }
}

// Fetch recent Form 4 filings for a company from the EDGAR Atom feed.
// Returns Vec<(accession_number_with_dashes, filing_date)>.
async fn fetch_form4_accessions(
    client: &reqwest::Client,
    ticker: &str,
    count: usize,
) -> Result<Vec<(String, String)>> {
    let url = format!(
        "https://www.sec.gov/cgi-bin/browse-edgar?action=getcompany&CIK={ticker}&type=4\
         &dateb=&owner=include&count={count}&search_text=&output=atom"
    );
    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!("SEC EDGAR returned HTTP {}: {url}", resp.status()));
    }
    let xml = resp.text().await?;
    let doc = Document::parse(&xml).map_err(|e| anyhow!("failed to parse EDGAR Atom feed: {e}"))?;

    let mut results = Vec::new();
    for entry in doc
        .root_element()
        .descendants()
        .filter(|n| n.has_tag_name("entry"))
    {
        let accession = entry
            .descendants()
            .find(|n| n.has_tag_name("accession-number"))
            .and_then(|n| n.text())
            .unwrap_or("")
            .trim()
            .to_string();
        let filing_date = entry
            .descendants()
            .find(|n| n.has_tag_name("filing-date"))
            .and_then(|n| n.text())
            .unwrap_or("")
            .trim()
            .to_string();
        if !accession.is_empty() {
            results.push((accession, filing_date));
        }
    }

    if results.is_empty() {
        return Err(anyhow!(
            "no Form 4 filings found for '{ticker}' — only US-listed companies are supported"
        ));
    }
    Ok(results)
}

// Fetch the Form 4 XML document for a given accession number.
// The filer CIK is the leading numeric segment of the accession number.
async fn fetch_form4_xml(client: &reqwest::Client, accession: &str) -> Result<String> {
    let filer_cik: u64 = accession
        .split('-')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let accession_no_dashes = accession.replace('-', "");
    let base = format!("https://www.sec.gov/Archives/edgar/data/{filer_cik}/{accession_no_dashes}");

    let index_url = format!("{base}/index.json");
    let resp = client.get(&index_url).send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!("HTTP {} for {index_url}", resp.status()));
    }
    let data: serde_json::Value = resp.json().await?;
    let items = data["directory"]["item"]
        .as_array()
        .ok_or_else(|| anyhow!("no items in filing index for {accession}"))?;

    let xml_name = items
        .iter()
        .filter_map(|item| item["name"].as_str())
        .find(|name| name.to_ascii_lowercase().ends_with(".xml") && *name != "primary_doc.xml")
        .ok_or_else(|| anyhow!("no Form 4 XML found in filing {accession}"))?
        .to_string();

    let xml_url = format!("{base}/{xml_name}");
    let xml_resp = client.get(&xml_url).send().await?;
    if !xml_resp.status().is_success() {
        return Err(anyhow!("HTTP {} fetching {xml_url}", xml_resp.status()));
    }
    xml_resp.text().await.map_err(anyhow::Error::from)
}

// Extract text from a Form 4 XML element.
// Checks for a nested <value> child first (Form 4 uses this wrapper for most fields),
// then falls back to the element's direct text content.
fn form4_val(node: roxmltree::Node<'_, '_>, tag: &str) -> String {
    node.descendants()
        .find(|n| n.has_tag_name(tag))
        .map(|n| {
            n.descendants()
                .find(|c| c.has_tag_name("value"))
                .and_then(|c| c.text())
                .or_else(|| n.text())
                .map_or("", str::trim)
                .to_string()
        })
        .unwrap_or_default()
}

// Parse a Form 4 XML document into InsiderTrade records.
// Only non-derivative transactions are included (direct stock buys/sells/grants).
fn parse_form4(xml: &str, filing_date: &str) -> Vec<InsiderTrade> {
    let Ok(doc) = Document::parse(xml) else {
        return Vec::new();
    };
    let root = doc.root_element();

    let owner = form4_val(root, "rptOwnerName");
    let title = {
        let rel = root
            .descendants()
            .find(|n| n.has_tag_name("reportingOwnerRelationship"));
        rel.map_or_else(String::new, |r| {
            let t = form4_val(r, "officerTitle");
            if !t.is_empty() {
                return t;
            }
            if form4_val(r, "isOfficer") == "1" {
                "Officer".to_string()
            } else if form4_val(r, "isDirector") == "1" {
                "Director".to_string()
            } else if form4_val(r, "isTenPercentOwner") == "1" {
                "10% Owner".to_string()
            } else {
                String::new()
            }
        })
    };

    let mut trades = Vec::new();
    for tx in root
        .descendants()
        .filter(|n| n.has_tag_name("nonDerivativeTransaction"))
    {
        let date = form4_val(tx, "transactionDate");
        let code = tx
            .descendants()
            .find(|n| n.has_tag_name("transactionCode"))
            .and_then(|n| n.text())
            .unwrap_or("")
            .trim()
            .to_string();
        let shares: f64 = form4_val(tx, "transactionShares")
            .replace(',', "")
            .parse()
            .unwrap_or(0.0);
        let price: f64 = form4_val(tx, "transactionPricePerShare")
            .replace(',', "")
            .parse()
            .unwrap_or(0.0);
        let shares_after: f64 = form4_val(tx, "sharesOwnedFollowingTransaction")
            .replace(',', "")
            .parse()
            .unwrap_or(0.0);

        if shares <= 0.0 {
            continue;
        }

        trades.push(InsiderTrade {
            filing_date: filing_date.to_string(),
            owner: owner.clone(),
            title: title.clone(),
            date,
            code,
            shares,
            price,
            value: shares * price,
            shares_after,
        });
    }
    trades
}

pub async fn cmd_insider_trades(symbol: &str, count: usize, format: &OutputFormat) -> Result<()> {
    let ticker = normalize_ticker(symbol);
    let client = sec_client();

    if matches!(format, OutputFormat::Pretty) {
        eprintln!("Fetching Form 4 insider trades for {ticker} from SEC EDGAR...");
    }

    let accessions = fetch_form4_accessions(&client, &ticker, count).await?;

    if matches!(format, OutputFormat::Pretty) {
        eprintln!("Fetching {} Form 4 filings...", accessions.len());
    }

    let sem = Arc::new(tokio::sync::Semaphore::new(5));
    let futs: Vec<_> = accessions
        .into_iter()
        .map(|(accession, filing_date)| {
            let client = client.clone();
            let sem = Arc::clone(&sem);
            async move {
                let _permit = sem.acquire().await.ok()?;
                let xml = fetch_form4_xml(&client, &accession).await.ok()?;
                Some(parse_form4(&xml, &filing_date))
            }
        })
        .collect();

    let mut trades: Vec<InsiderTrade> = join_all(futs)
        .await
        .into_iter()
        .flatten()
        .flatten()
        .collect();

    if trades.is_empty() {
        return Err(anyhow!(
            "no transactions found in the fetched Form 4 filings"
        ));
    }

    // Sort by transaction date descending, then filing date.
    trades.sort_unstable_by(|a, b| {
        b.date
            .cmp(&a.date)
            .then_with(|| b.filing_date.cmp(&a.filing_date))
    });

    match format {
        OutputFormat::Json => {
            let json: Vec<serde_json::Value> = trades
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "filing_date": t.filing_date,
                        "owner": t.owner,
                        "title": t.title,
                        "date": t.date,
                        "code": t.code,
                        "type": tx_label(&t.code),
                        "shares": t.shares,
                        "price": t.price,
                        "value": t.value,
                        "shares_after": t.shares_after,
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&json).unwrap_or_default()
            );
        }
        OutputFormat::Html => {
            let rows: Vec<Vec<String>> = trades
                .iter()
                .map(|t| {
                    vec![
                        t.date.clone(),
                        t.owner.clone(),
                        t.title.clone(),
                        tx_label(&t.code).to_string(),
                        fmt_shares(t.shares),
                        fmt_price(t.price),
                        fmt_value(t.value),
                        fmt_shares(t.shares_after),
                    ]
                })
                .collect();
            return crate::cli::html_render::open_html_table(
                &format!("{ticker} Insider Trades"),
                &format!("insider-trades {ticker}"),
                &[
                    "date",
                    "filer",
                    "title",
                    "type",
                    "shares",
                    "price",
                    "value",
                    "owned_after",
                ],
                rows,
            );
        }
        OutputFormat::Pretty => {
            println!();
            let rows: Vec<Vec<String>> = trades
                .iter()
                .map(|t| {
                    vec![
                        t.date.clone(),
                        t.owner.clone(),
                        t.title.clone(),
                        tx_label(&t.code).to_string(),
                        fmt_shares(t.shares),
                        fmt_price(t.price),
                        fmt_value(t.value),
                        fmt_shares(t.shares_after),
                    ]
                })
                .collect();
            print_table(
                &[
                    "date",
                    "filer",
                    "title",
                    "type",
                    "shares",
                    "price",
                    "value",
                    "owned_after",
                ],
                rows,
                format,
            );
            println!("\nSource: SEC EDGAR Form 4 — {ticker}");
        }
    }

    Ok(())
}
