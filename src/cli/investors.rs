use std::collections::HashMap;
use std::io::Read as IoRead;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use flate2::read::DeflateDecoder;
use futures::future::join_all;
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

// Download size for the ZIP tail; 1 MB covers the central directory and both TSV files.
const ZIP_TAIL_SIZE: u64 = 1_048_576;

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct RankedFund {
    cik: String,
    name: String,
    aum_thousands: u64,
    period: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct RankingCache {
    zip_url: String,
    rankings: Vec<RankedFund>,
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

// Generates candidate ZIP URLs for the last 6 quarterly 13F data set periods,
// newest first. The SEC publishes one ZIP per calendar quarter:
//   Dec 1 – Feb 28/29 | Mar 1 – May 31 | Jun 1 – Aug 31 | Sep 1 – Nov 30
fn dataset_url_candidates() -> Vec<String> {
    use time::OffsetDateTime;
    let now = OffsetDateTime::now_utc();
    let year = now.year();
    let month = now.month() as u8; // 1–12

    // Starting period: the quarter whose start month ≤ current month.
    let (mut pm, mut py) = match month {
        1 | 2 => (12u8, year - 1),
        3..=5 => (3u8, year),
        6..=8 => (6u8, year),
        9..=11 => (9u8, year),
        _ => (12u8, year), // December
    };

    let base = "https://www.sec.gov/files/structureddata/data/form-13f-data-sets";
    let mut urls = Vec::new();
    for _ in 0..6 {
        let url = match pm {
            12 => {
                let feb_days = if is_leap_year(py + 1) { 29 } else { 28 };
                format!("{base}/01dec{py}-{feb_days:02}feb{}_form13f.zip", py + 1)
            }
            3 => format!("{base}/01mar{py}-31may{py}_form13f.zip"),
            6 => format!("{base}/01jun{py}-31aug{py}_form13f.zip"),
            _ => format!("{base}/01sep{py}-30nov{py}_form13f.zip"), // 9
        };
        urls.push(url);
        (pm, py) = match pm {
            12 => (9u8, py),
            9 => (6u8, py),
            6 => (3u8, py),
            _ => (12u8, py - 1), // 3
        };
    }
    urls
}

// Finds the most recent SEC 13F quarterly data set ZIP by probing candidate URLs with HEAD.
async fn fetch_latest_dataset_url(client: &reqwest::Client) -> Result<String> {
    for url in dataset_url_candidates() {
        match client.head(&url).send().await {
            Ok(resp) if resp.status().is_success() => return Ok(url),
            _ => {}
        }
    }
    Err(anyhow!(
        "could not find a current 13F quarterly data set on SEC EDGAR after trying 6 candidates"
    ))
}

async fn download_zip_tail(client: &reqwest::Client, url: &str) -> Result<(Vec<u8>, u64)> {
    let head_resp = client.head(url).send().await?;
    if !head_resp.status().is_success() {
        return Err(anyhow!("HEAD {url}: HTTP {}", head_resp.status()));
    }
    let content_length: u64 = head_resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("no Content-Length in HEAD response for {url}"))?;
    let chunk_start = content_length.saturating_sub(ZIP_TAIL_SIZE);
    let range_header = format!("bytes={chunk_start}-{}", content_length - 1);
    let resp = client
        .get(url)
        .header("Range", &range_header)
        .send()
        .await?;
    if resp.status().as_u16() != 206 {
        return Err(anyhow!(
            "Range request returned HTTP {} (expected 206)",
            resp.status()
        ));
    }
    let data = resp.bytes().await?.to_vec();
    Ok((data, chunk_start))
}

fn read_u16_le(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

// Scans backwards for the ZIP End of Central Directory record (signature PK\x05\x06).
#[allow(clippy::indexing_slicing)]
fn find_eocd(data: &[u8]) -> Option<usize> {
    let sig = [0x50u8, 0x4B, 0x05, 0x06];
    for i in (0..data.len().saturating_sub(21)).rev() {
        if data[i..i + 4] == sig {
            return Some(i);
        }
    }
    None
}

// Parses the ZIP central directory; returns filename → (local_offset_abs, comp_size, comp_method).
#[allow(clippy::indexing_slicing)]
fn parse_central_directory(
    data: &[u8],
    chunk_start: u64,
    cd_offset_abs: u64,
    cd_size: usize,
) -> HashMap<String, (u64, u32, u16)> {
    let mut result = HashMap::new();
    let cd_in_chunk = cd_offset_abs.saturating_sub(chunk_start) as usize;
    if cd_in_chunk >= data.len() {
        return result;
    }
    let cd_end = (cd_in_chunk + cd_size).min(data.len());
    let cd = &data[cd_in_chunk..cd_end];
    let mut pos = 0usize;
    while pos + 46 <= cd.len() {
        if cd[pos..pos + 4] != [0x50u8, 0x4B, 0x01, 0x02] {
            break;
        }
        let comp_method = read_u16_le(cd, pos + 10);
        let comp_size = read_u32_le(cd, pos + 20);
        let filename_len = read_u16_le(cd, pos + 28) as usize;
        let extra_len = read_u16_le(cd, pos + 30) as usize;
        let comment_len = read_u16_le(cd, pos + 32) as usize;
        let local_offset_abs = u64::from(read_u32_le(cd, pos + 42));
        if pos + 46 + filename_len <= cd.len() {
            let name = String::from_utf8_lossy(&cd[pos + 46..pos + 46 + filename_len]).to_string();
            result.insert(name, (local_offset_abs, comp_size, comp_method));
        }
        pos += 46 + filename_len + extra_len + comment_len;
    }
    result
}

// Decompresses a file from the downloaded ZIP tail chunk.
#[allow(clippy::indexing_slicing)]
fn extract_file_from_chunk(
    data: &[u8],
    chunk_start: u64,
    local_offset_abs: u64,
    comp_size: u32,
    comp_method: u16,
) -> Result<String> {
    let in_chunk = local_offset_abs.saturating_sub(chunk_start) as usize;
    if in_chunk + 30 > data.len() {
        return Err(anyhow!(
            "local file header at offset {local_offset_abs} is not within the downloaded chunk \
             (chunk_start={chunk_start}); the file may be too large — try increasing ZIP_TAIL_SIZE"
        ));
    }
    if data[in_chunk..in_chunk + 4] != [0x50u8, 0x4B, 0x03, 0x04] {
        return Err(anyhow!(
            "invalid local file header signature at offset {local_offset_abs}"
        ));
    }
    let filename_len = read_u16_le(data, in_chunk + 26) as usize;
    let extra_len = read_u16_le(data, in_chunk + 28) as usize;
    let data_start = in_chunk + 30 + filename_len + extra_len;
    let data_end = data_start + comp_size as usize;
    if data_end > data.len() {
        return Err(anyhow!(
            "compressed data at offset {local_offset_abs} extends beyond the downloaded chunk; \
             increase ZIP_TAIL_SIZE"
        ));
    }
    let comp_data = &data[data_start..data_end];
    match comp_method {
        0 => Ok(String::from_utf8_lossy(comp_data).to_string()),
        8 => {
            let mut decoder = DeflateDecoder::new(comp_data);
            let mut out = String::new();
            decoder
                .read_to_string(&mut out)
                .map_err(|e| anyhow!("DEFLATE decompression failed: {e}"))?;
            Ok(out)
        }
        _ => Err(anyhow!("unsupported ZIP compression method {comp_method}")),
    }
}

// Parses SUBMISSION.tsv; returns accession → (cik, period) for 13F-HR filings only.
fn parse_submission_tsv(text: &str) -> HashMap<String, (String, String)> {
    let mut result = HashMap::new();
    let mut lines = text.lines();
    let Some(header) = lines.next() else {
        return result;
    };
    let cols: Vec<&str> = header.split('\t').collect();
    let (Some(acc_i), Some(cik_i), Some(period_i)) = (
        cols.iter().position(|&c| c == "ACCESSION_NUMBER"),
        cols.iter().position(|&c| c == "CIK"),
        cols.iter().position(|&c| c == "PERIODOFREPORT"),
    ) else {
        return result;
    };
    let subtype_i = cols.iter().position(|&c| c == "SUBMISSIONTYPE");
    let min_len = [acc_i, cik_i, period_i, subtype_i.unwrap_or(0)]
        .iter()
        .copied()
        .max()
        .unwrap_or(0)
        + 1;
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < min_len {
            continue;
        }
        // Skip amendments (13F-HR/A) to avoid double-counting.
        if let Some(i) = subtype_i {
            if fields.get(i).copied() != Some("13F-HR") {
                continue;
            }
        }
        result.insert(
            fields[acc_i].to_string(),
            (fields[cik_i].to_string(), fields[period_i].to_string()),
        );
    }
    result
}

// Parses SUMMARYPAGE.tsv; returns accession → total_value_thousands.
fn parse_summarypage_tsv(text: &str) -> HashMap<String, u64> {
    let mut result = HashMap::new();
    let mut lines = text.lines();
    let Some(header) = lines.next() else {
        return result;
    };
    let cols: Vec<&str> = header.split('\t').collect();
    let (Some(acc_i), Some(val_i)) = (
        cols.iter().position(|&c| c == "ACCESSION_NUMBER"),
        cols.iter().position(|&c| c == "TABLEVALUETOTAL"),
    ) else {
        return result;
    };
    let min_len = acc_i.max(val_i) + 1;
    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < min_len {
            continue;
        }
        let value: u64 = fields[val_i].replace(',', "").parse().unwrap_or(0);
        result.insert(fields[acc_i].to_string(), value);
    }
    result
}

// Fetches entity names from SEC EDGAR for a list of 10-digit zero-padded CIKs.
async fn fetch_entity_names(
    client: &reqwest::Client,
    ciks: Vec<String>,
) -> HashMap<String, String> {
    let sem = Arc::new(tokio::sync::Semaphore::new(5));
    let futs: Vec<_> = ciks
        .into_iter()
        .map(|cik| {
            let client = client.clone();
            let sem = Arc::clone(&sem);
            async move {
                let _permit = sem.acquire().await.ok()?;
                let url = format!("https://data.sec.gov/submissions/CIK{cik}.json");
                let resp = client.get(&url).send().await.ok()?;
                if !resp.status().is_success() {
                    return None;
                }
                let data: serde_json::Value = resp.json().await.ok()?;
                let name = data["name"].as_str()?.to_string();
                Some((cik, name))
            }
        })
        .collect();
    join_all(futs).await.into_iter().flatten().collect()
}

fn rankings_cache_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".longbridge").join("13f_rankings_cache.json"))
}

fn load_rankings_cache(zip_url: &str) -> Option<Vec<RankedFund>> {
    let path = rankings_cache_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    let cache: RankingCache = serde_json::from_str(&text).ok()?;
    (cache.zip_url == zip_url).then_some(cache.rankings)
}

fn save_rankings_cache(zip_url: &str, rankings: &[RankedFund]) {
    let Some(path) = rankings_cache_path() else {
        return;
    };
    let cache = RankingCache {
        zip_url: zip_url.to_string(),
        rankings: rankings.to_vec(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&cache) {
        let _ = std::fs::write(path, json);
    }
}

pub async fn cmd_investors_list(top: usize, format: &OutputFormat) -> Result<()> {
    let client = sec_client();
    if matches!(format, OutputFormat::Pretty) {
        eprintln!("Fetching SEC 13F institutional investor rankings...");
    }
    let zip_url = match fetch_latest_dataset_url(&client).await {
        Ok(u) => u,
        Err(e) => {
            if matches!(format, OutputFormat::Pretty) {
                eprintln!("Warning: could not fetch rankings ({e}). Showing built-in shortcuts.");
            }
            show_builtin_investors(format);
            return Ok(());
        }
    };
    let rankings = if let Some(cached) = load_rankings_cache(&zip_url) {
        if matches!(format, OutputFormat::Pretty) {
            let zip_name = zip_url.rsplit('/').next().unwrap_or("");
            eprintln!("Using cached rankings ({zip_name}).");
        }
        cached
    } else {
        if matches!(format, OutputFormat::Pretty) {
            eprintln!("Downloading 13F quarterly data set (last 1 MB)...");
        }
        let (chunk, chunk_start) = download_zip_tail(&client, &zip_url).await?;
        let eocd_pos =
            find_eocd(&chunk).ok_or_else(|| anyhow!("EOCD signature not found in ZIP tail"))?;
        let cd_size = read_u32_le(&chunk, eocd_pos + 12) as usize;
        let cd_offset_abs = u64::from(read_u32_le(&chunk, eocd_pos + 16));
        let cd = parse_central_directory(&chunk, chunk_start, cd_offset_abs, cd_size);
        let submission_key = cd
            .keys()
            .find(|k| k.ends_with("SUBMISSION.tsv"))
            .cloned()
            .ok_or_else(|| anyhow!("SUBMISSION.tsv not found in ZIP central directory"))?;
        let summarypage_key = cd
            .keys()
            .find(|k| k.ends_with("SUMMARYPAGE.tsv"))
            .cloned()
            .ok_or_else(|| anyhow!("SUMMARYPAGE.tsv not found in ZIP central directory"))?;
        let (sbs_local, sbs_comp_size, sbs_method) = cd[&submission_key];
        let (smy_local, smy_comp_size, smy_method) = cd[&summarypage_key];
        let submission_text =
            extract_file_from_chunk(&chunk, chunk_start, sbs_local, sbs_comp_size, sbs_method)?;
        let summarypage_text =
            extract_file_from_chunk(&chunk, chunk_start, smy_local, smy_comp_size, smy_method)?;
        let submissions = parse_submission_tsv(&submission_text);
        let values = parse_summarypage_tsv(&summarypage_text);
        // Join accession tables to get per-CIK total AUM (in thousands USD).
        let mut by_cik: HashMap<String, (String, u64)> = HashMap::new();
        for (accession, value) in &values {
            if let Some((cik, period)) = submissions.get(accession) {
                by_cik
                    .entry(cik.clone())
                    .and_modify(|e| e.1 += value)
                    .or_insert_with(|| (period.clone(), *value));
            }
        }
        // Sort by AUM descending; cache top 200 to cover any --top request.
        let mut all_ranked: Vec<(String, String, u64)> = by_cik
            .into_iter()
            .map(|(cik, (period, aum))| (cik, period, aum))
            .collect();
        all_ranked.sort_unstable_by(|a, b| b.2.cmp(&a.2));
        all_ranked.truncate(200);
        let padded_ciks: Vec<String> = all_ranked
            .iter()
            .map(|(cik, _, _)| format!("{cik:0>10}"))
            .collect();
        if matches!(format, OutputFormat::Pretty) {
            eprintln!(
                "Fetching entity names for top {} filers...",
                padded_ciks.len()
            );
        }
        let names = fetch_entity_names(&client, padded_ciks).await;
        let ranked: Vec<RankedFund> = all_ranked
            .into_iter()
            .map(|(cik, period, aum)| {
                let cik_padded = format!("{cik:0>10}");
                let name = names
                    .get(&cik_padded)
                    .cloned()
                    .unwrap_or_else(|| format!("CIK {cik}"));
                RankedFund {
                    cik: cik_padded,
                    name,
                    aum_thousands: aum,
                    period,
                }
            })
            .collect();
        save_rankings_cache(&zip_url, &ranked);
        ranked
    };
    let displayed: Vec<_> = rankings.into_iter().take(top).collect();
    match format {
        OutputFormat::Json => {
            let json: Vec<_> = displayed
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    serde_json::json!({
                        "rank": i + 1,
                        "cik": f.cik,
                        "name": f.name,
                        "aum_usd": f.aum_thousands,
                        "period": f.period,
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&json).unwrap_or_default()
            );
        }
        OutputFormat::Pretty => {
            println!();
            let rows: Vec<Vec<String>> = displayed
                .iter()
                .enumerate()
                .map(|(i, f)| {
                    vec![
                        (i + 1).to_string(),
                        f.name.clone(),
                        format_large_usd(f.aum_thousands),
                        f.period.clone(),
                        f.cik.clone(),
                    ]
                })
                .collect();
            print_table(&["#", "name", "AUM", "period", "cik"], rows, format);
            println!("\nView holdings: longbridge investors <SLUG|CIK>");
            let zip_name = zip_url.rsplit('/').next().unwrap_or("");
            println!("Source: SEC EDGAR 13F — {zip_name}");
        }
    }
    Ok(())
}

fn show_builtin_investors(format: &OutputFormat) {
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
        println!("\nView holdings: longbridge investors <SLUG|CIK>");
        println!("Data source: SEC EDGAR 13F filings (sec.gov)");
    }
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

// Core holdings display logic — shared by slug-based and CIK-based entry points.
// `label` is the header line shown in Pretty mode (e.g. "Warren Buffett — Berkshire Hathaway").
// `firm` is included in the JSON output under the "firm" key.
async fn show_holdings(
    client: &reqwest::Client,
    cik: &str,
    label: &str,
    firm: &str,
    top: usize,
    format: &OutputFormat,
) -> Result<()> {
    if matches!(format, OutputFormat::Pretty) {
        eprintln!("Fetching 13F filings for {label} from SEC EDGAR...");
    }

    let (accession, filing_date, report_date) = fetch_latest_13f(client, cik).await?;

    if matches!(format, OutputFormat::Pretty) {
        eprintln!("Period: {report_date}  (filed: {filing_date})");
    }

    let xml = fetch_infotable_xml(client, cik, &accession).await?;
    let raw_holdings = parse_holdings(&xml)?;

    if raw_holdings.is_empty() {
        return Err(anyhow!("no holdings found in 13F filing {accession}"));
    }

    // Consolidate duplicate CUSIP entries (sub-managers report separately; merge per security).
    let mut seen: HashMap<String, usize> = HashMap::new();
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
                            format!("{:.2}", h.value as f64 / total_value as f64 * 100.0)
                        } else {
                            "0.00".to_string()
                        },
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "investor": label,
                    "firm": firm,
                    "cik": cik,
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
            println!("\n{label} (period: {report_date})\n");
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

pub async fn cmd_investor_holdings(slug: &str, top: usize, format: &OutputFormat) -> Result<()> {
    let investor = INVESTORS.iter().find(|inv| inv.id == slug).ok_or_else(|| {
        let slugs: Vec<&str> = INVESTORS.iter().map(|i| i.id).collect();
        anyhow!(
            "unknown investor slug '{}'. Known slugs:\n  {}\n\nTo look up any 13F filer by CIK, run: longbridge investors <CIK>",
            slug,
            slugs.join("\n  ")
        )
    })?;
    let client = sec_client();
    let label = format!("{} — {}", investor.name, investor.firm);
    show_holdings(&client, investor.cik, &label, investor.firm, top, format).await
}

// View 13F holdings for an arbitrary SEC EDGAR filer by their CIK number.
pub async fn cmd_investor_holdings_by_cik(
    cik_raw: &str,
    top: usize,
    format: &OutputFormat,
) -> Result<()> {
    let cik_num: u64 = cik_raw
        .parse()
        .map_err(|_| anyhow!("invalid CIK number: '{cik_raw}' — CIK must be numeric"))?;
    let cik = format!("{cik_num:010}");
    let client = sec_client();

    // Resolve entity name from EDGAR submissions before fetching holdings.
    let sub_url = format!("https://data.sec.gov/submissions/CIK{cik}.json");
    let resp = client.get(&sub_url).send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!(
            "CIK {cik} not found on SEC EDGAR (HTTP {}). \
             Use `longbridge investors --search <name>` to find the correct CIK.",
            resp.status()
        ));
    }
    let data: serde_json::Value = resp.json().await?;
    let entity_name = data["name"].as_str().unwrap_or("Unknown Fund").to_string();

    show_holdings(&client, &cik, &entity_name, &entity_name, top, format).await
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
