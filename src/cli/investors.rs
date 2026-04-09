use anyhow::{anyhow, Result};
use regex::Regex;
use roxmltree::Document;

use super::output::print_table;
use super::OutputFormat;
use crate::utils::number::format_volume;

struct Investor {
    id: &'static str,
    name: &'static str,
    firm: &'static str,
    cik: &'static str,
}

const INVESTORS: &[Investor] = &[
    Investor {
        id: "warren-buffett",
        name: "Warren Buffett",
        firm: "Berkshire Hathaway",
        cik: "0001067983",
    },
    Investor {
        id: "bill-ackman",
        name: "Bill Ackman",
        firm: "Pershing Square Capital Management",
        cik: "0001336528",
    },
    Investor {
        id: "michael-burry",
        name: "Michael Burry",
        firm: "Scion Asset Management",
        cik: "0001649339",
    },
    Investor {
        id: "george-soros",
        name: "George Soros",
        firm: "Soros Fund Management",
        cik: "0001029160",
    },
    Investor {
        id: "david-tepper",
        name: "David Tepper",
        firm: "Appaloosa Management",
        cik: "0001262463",
    },
    Investor {
        id: "seth-klarman",
        name: "Seth Klarman",
        firm: "Baupost Group",
        cik: "0000887936",
    },
    Investor {
        id: "dan-loeb",
        name: "Dan Loeb",
        firm: "Third Point",
        cik: "0001040273",
    },
    Investor {
        id: "ken-griffin",
        name: "Ken Griffin",
        firm: "Citadel Advisors",
        cik: "0001423298",
    },
    Investor {
        id: "stan-druckenmiller",
        name: "Stanley Druckenmiller",
        firm: "Duquesne Family Office",
        cik: "0001536411",
    },
    Investor {
        id: "david-einhorn",
        name: "David Einhorn",
        firm: "Greenlight Capital",
        cik: "0001079114",
    },
];

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

pub fn cmd_investors_list(format: &OutputFormat) -> Result<()> {
    let rows: Vec<Vec<String>> = INVESTORS
        .iter()
        .enumerate()
        .map(|(i, inv)| {
            vec![
                (i + 1).to_string(),
                inv.name.to_string(),
                inv.firm.to_string(),
                inv.id.to_string(),
            ]
        })
        .collect();

    print_table(&["#", "investor", "firm", "slug"], rows, format);

    if matches!(format, OutputFormat::Pretty) {
        println!("\nRun `longbridge investors <SLUG>` to view their portfolio.");
        println!("Data source: SEC EDGAR 13F filings (sec.gov)");
    }

    Ok(())
}

struct Holding {
    name: String,
    cusip: String,
    value: u64,
    shares: u64,
    share_type: String,
}

// Returns (accession_number, filing_date, report_date)
async fn fetch_latest_13f(client: &reqwest::Client, cik: &str) -> Result<(String, String, String)> {
    let url = format!("https://data.sec.gov/submissions/CIK{cik}.json");
    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!(
            "SEC EDGAR returned HTTP {}: {}",
            resp.status(),
            url
        ));
    }
    let data: serde_json::Value = resp.json().await?;

    let recent = &data["filings"]["recent"];
    let forms = recent["form"]
        .as_array()
        .ok_or_else(|| anyhow!("no filings found for CIK {cik}"))?;
    let accessions = recent["accessionNumber"]
        .as_array()
        .ok_or_else(|| anyhow!("no accessionNumbers in SEC response"))?;
    let filing_dates = recent["filingDate"]
        .as_array()
        .ok_or_else(|| anyhow!("no filingDates in SEC response"))?;
    let report_dates = recent["reportDate"].as_array();

    let idx = forms
        .iter()
        .position(|f| f.as_str() == Some("13F-HR"))
        .ok_or_else(|| {
            anyhow!("no 13F-HR filing found — this investor may not file 13F reports with the SEC")
        })?;

    let accession = accessions[idx]
        .as_str()
        .ok_or_else(|| anyhow!("accessionNumber not a string"))?
        .to_string();
    let filing_date = filing_dates[idx].as_str().unwrap_or("unknown").to_string();
    let report_date = report_dates
        .and_then(|arr| arr.get(idx))
        .and_then(|v| v.as_str())
        .unwrap_or(&filing_date)
        .to_string();

    Ok((accession, filing_date, report_date))
}

async fn fetch_infotable_xml(
    client: &reqwest::Client,
    cik: &str,
    accession: &str,
) -> Result<String> {
    let cik_no_zeros = cik.trim_start_matches('0');
    let accession_no_dashes = accession.replace('-', "");
    let base =
        format!("https://www.sec.gov/Archives/edgar/data/{cik_no_zeros}/{accession_no_dashes}");

    // Use the EDGAR EFTS search API to find documents in this filing.
    // The response contains "_id" fields in the format "{accession}:{filename}".
    // We filter out "primary_doc.xml" (the cover page) to find the infotable XML.
    // Note: the response may contain control characters so we use regex on raw bytes.
    let efts_url =
        format!("https://efts.sec.gov/LATEST/search-index?q=%22{accession}%22&forms=13F-HR");
    if let Ok(resp) = client.get(&efts_url).send().await {
        if resp.status().is_success() {
            if let Ok(body) = resp.text().await {
                if let Some(filename) = find_infotable_filename_from_efts(&body, accession) {
                    let xml_url = format!("{base}/{filename}");
                    if let Ok(xml_resp) = client.get(&xml_url).send().await {
                        if xml_resp.status().is_success() {
                            return Ok(xml_resp.text().await?);
                        }
                    }
                }
            }
        }
    }

    // Fallback: try well-known filenames used across different filers.
    let common_names = [
        "infotable.xml",
        "form13fInfoTable.xml",
        "13F_INFOTABLE.xml",
        "INFORMATION_TABLE.xml",
    ];
    for name in &common_names {
        let xml_url = format!("{base}/{name}");
        if let Ok(resp) = client.get(&xml_url).send().await {
            if resp.status().is_success() {
                return Ok(resp.text().await?);
            }
        }
    }

    Err(anyhow!(
        "could not find the 13F information table in filing {accession}.\n\
         Check https://www.sec.gov/cgi-bin/browse-edgar?action=getcompany&CIK={cik_no_zeros}&type=13F",
    ))
}

// Extract the infotable XML filename from an EDGAR EFTS search response.
// The response contains "_id" fields in the format "{accession}:{filename}".
// We skip "primary_doc.xml" (the cover page form) and return the infotable filename.
fn find_infotable_filename_from_efts(body: &str, accession: &str) -> Option<String> {
    // Match "_id":"<accession>:<filename>" entries in the raw response body.
    // The accession contains dashes which are safe in regex but we escape to be sure.
    let escaped = accession.replace('-', r"\-");
    let pattern = format!(r#""_id"\s*:\s*"{escaped}:([^"]+\.xml)""#);
    let re = Regex::new(&pattern).ok()?;
    // Collect all matching filenames into an owned Vec so `re` can be dropped.
    let filenames: Vec<String> = re.captures_iter(body).map(|c| c[1].to_string()).collect();
    drop(re);
    filenames.into_iter().find(|f| f != "primary_doc.xml")
}

fn parse_holdings(xml: &str) -> Result<Vec<Holding>> {
    let doc = Document::parse(xml)
        .map_err(|e| anyhow!("failed to parse 13F information table XML: {e}"))?;

    // Traverse all <infoTable> elements regardless of namespace prefix.
    // roxmltree matches by local name so namespace prefixes (e.g. ns1:infoTable) are ignored.
    let holdings = doc
        .root_element()
        .descendants()
        .filter(|n| n.has_tag_name("infoTable"))
        .filter_map(|table| {
            let child_text = |tag: &str| -> String {
                table
                    .descendants()
                    .find(|n| n.has_tag_name(tag))
                    .and_then(|n| n.text())
                    .map_or("", str::trim)
                    .to_string()
            };

            let name = child_text("nameOfIssuer");
            if name.is_empty() {
                return None;
            }

            let cusip = child_text("cusip");

            let value = child_text("value")
                .replace(',', "")
                .parse::<u64>()
                .unwrap_or(0);

            let shares = child_text("sshPrnamt")
                .replace(',', "")
                .parse::<u64>()
                .unwrap_or(0);

            let share_type = {
                let t = child_text("sshPrnamtType");
                if t.is_empty() {
                    "SH".to_string()
                } else {
                    t
                }
            };

            Some(Holding {
                name,
                cusip,
                value,
                shares,
                share_type,
            })
        })
        .collect();

    Ok(holdings)
}

pub async fn cmd_investor_holdings(slug: &str, top: usize, format: &OutputFormat) -> Result<()> {
    let investor = INVESTORS.iter().find(|inv| inv.id == slug).ok_or_else(|| {
        let slugs: Vec<&str> = INVESTORS.iter().map(|i| i.id).collect();
        anyhow!(
            "unknown investor '{}'. Available slugs:\n  {}",
            slug,
            slugs.join("\n  ")
        )
    })?;

    let client = sec_client();

    if matches!(format, OutputFormat::Pretty) {
        eprintln!(
            "Fetching 13F filings for {} from SEC EDGAR...",
            investor.name
        );
    }

    let (accession, filing_date, report_date) = fetch_latest_13f(&client, investor.cik).await?;

    if matches!(format, OutputFormat::Pretty) {
        eprintln!("Period: {report_date}  (filed: {filing_date})");
    }

    let xml = fetch_infotable_xml(&client, investor.cik, &accession).await?;

    let raw_holdings = parse_holdings(&xml)?;

    if raw_holdings.is_empty() {
        return Err(anyhow!("no holdings found in 13F filing {accession}"));
    }

    // Consolidate duplicate CUSIP entries (different managers within the same filer
    // report positions separately; merge them into a single row per security).
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut holdings: Vec<Holding> = Vec::new();
    for h in raw_holdings {
        if let Some(&idx) = seen.get(&h.cusip) {
            holdings[idx].value += h.value;
            holdings[idx].shares += h.shares;
        } else {
            seen.insert(h.cusip.clone(), holdings.len());
            holdings.push(h);
        }
    }

    holdings.sort_unstable_by(|a, b| b.value.cmp(&a.value));

    #[allow(clippy::cast_precision_loss)]
    let total_value: u64 = holdings.iter().map(|h| h.value).sum();
    let total_count = holdings.len();

    let displayed: Vec<_> = holdings.into_iter().take(top).collect();

    match format {
        OutputFormat::Json => {
            #[allow(clippy::cast_precision_loss)]
            let json_holdings: Vec<serde_json::Value> = displayed
                .iter()
                .map(|h| {
                    serde_json::json!({
                        "name": h.name,
                        "cusip": h.cusip,
                        "value_usd": h.value,
                        "shares": h.shares,
                        "share_type": h.share_type,
                        "weight_pct": if total_value > 0 {
                            format!(
                                "{:.2}",
                                h.value as f64 / total_value as f64 * 100.0
                            )
                        } else {
                            "0.00".to_string()
                        },
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "investor": investor.name,
                    "firm": investor.firm,
                    "cik": investor.cik,
                    "period": report_date,
                    "filing_date": filing_date,
                    "accession_number": accession,
                    "total_holdings": total_count,
                    "total_value_usd": total_value,
                    "holdings": json_holdings,
                }))
                .unwrap_or_default()
            );
        }
        OutputFormat::Pretty => {
            println!(
                "\n{} — {} (period: {})\n",
                investor.name, investor.firm, report_date
            );
            println!(
                "Portfolio: {} positions, total value ~{}",
                total_count,
                format_large_usd(total_value)
            );
            if total_count > top {
                println!("Showing top {top} positions by value (use --top N to change).");
            }
            println!();

            #[allow(clippy::cast_precision_loss)]
            let rows: Vec<Vec<String>> = displayed
                .iter()
                .map(|h| {
                    let weight = if total_value > 0 {
                        format!("{:.1}%", h.value as f64 / total_value as f64 * 100.0)
                    } else {
                        "-".to_string()
                    };
                    vec![
                        h.name.clone(),
                        format_large_usd(h.value),
                        format_volume(h.shares),
                        weight,
                    ]
                })
                .collect();

            print_table(&["company", "value", "shares", "weight"], rows, format);

            println!("\nSource: SEC EDGAR 13F — {accession} (filed {filing_date})");
        }
    }

    Ok(())
}

#[allow(clippy::cast_precision_loss)]
fn format_large_usd(usd: u64) -> String {
    let f = usd as f64;
    if f >= 1_000_000_000.0 {
        format!("${:.2}B", f / 1_000_000_000.0)
    } else if f >= 1_000_000.0 {
        format!("${:.2}M", f / 1_000_000.0)
    } else if f >= 1_000.0 {
        format!("${:.2}K", f / 1_000.0)
    } else {
        format!("${usd}")
    }
}
