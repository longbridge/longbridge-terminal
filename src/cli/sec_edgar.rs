//! SEC EDGAR N-PORT holdings retrieval for US ETFs.
//!
//! For a US-listed ETF, this module resolves the fund's CIK and series id from
//! the public mutual-fund ticker map, locates the latest `NPORT-P` filing, and
//! parses the full portfolio holdings out of the filing's `primary_doc.xml`.
//!
//! All requests use a contact-identifying `User-Agent` as required by SEC fair
//! access policy. The ticker map is cached on disk for 7 days.

use anyhow::{anyhow, Result};
use roxmltree::Document;
use serde::Serialize;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

// SEC EDGAR fair-access policy requires a declared automated-tool User-Agent
// that includes contact info. SEC's WAF rejects UAs containing a parenthesized
// URL, so we use the documented "Organization Application contact-email" form.
const SEC_UA: &str = concat!(
    "Longbridge longbridge-terminal/",
    env!("CARGO_PKG_VERSION"),
    " support@longbridge.com"
);

// Mutual-fund / ETF ticker map. ~28k rows, refreshed roughly daily upstream.
const TICKER_MAP_URL: &str = "https://www.sec.gov/files/company_tickers_mf.json";

// On-disk cache time-to-live for the ticker map.
const TICKER_MAP_TTL: Duration = Duration::from_hours(7 * 24);

/// A single portfolio holding parsed from an N-PORT filing.
#[derive(Debug, Clone, Serialize)]
pub struct Holding {
    pub name: String,
    pub lei: Option<String>,
    pub cusip: Option<String>,
    pub isin: Option<String>,
    /// Number of shares / units held (`balance`).
    pub shares: Option<f64>,
    /// Market value in USD (`valUSD`).
    pub value_usd: Option<f64>,
    /// Percentage of the fund's net assets (`pctVal`), e.g. `7.564` means 7.564%.
    pub weight: Option<f64>,
}

/// Full set of holdings for an ETF, parsed from a single N-PORT filing.
#[derive(Debug, Clone, Serialize)]
pub struct EtfHoldings {
    /// Series / fund name from the filing (`seriesName`).
    pub series_name: String,
    /// Reporting period end date (`repPdDate`), e.g. `2026-03-31`.
    pub report_period: String,
    /// EDGAR filing date of the N-PORT report, e.g. `2026-05-28`.
    pub filed_date: String,
    pub holdings: Vec<Holding>,
}

fn sec_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(SEC_UA)
        .timeout(Duration::from_mins(1))
        .build()
        .expect("failed to build HTTP client")
}

/// Strip the `.US` / `.HK` market suffix and uppercase the ticker.
fn normalize_ticker(symbol: &str) -> String {
    match symbol.rfind('.') {
        Some(i) => symbol[..i].to_uppercase(),
        None => symbol.to_uppercase(),
    }
}

fn cache_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| {
        h.join(".longbridge")
            .join("sec")
            .join("company_tickers_mf.json")
    })
}

/// Return the cached ticker-map bytes when present and younger than the TTL.
fn read_fresh_cache(path: &PathBuf) -> Option<Vec<u8>> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let age = SystemTime::now().duration_since(modified).ok()?;
    if age <= TICKER_MAP_TTL {
        std::fs::read(path).ok()
    } else {
        None
    }
}

/// Fetch the mutual-fund ticker map, preferring a fresh on-disk cache.
async fn load_ticker_map(client: &reqwest::Client) -> Result<Vec<u8>> {
    let path = cache_path();
    if let Some(p) = &path {
        if let Some(bytes) = read_fresh_cache(p) {
            return Ok(bytes);
        }
    }
    let resp = client.get(TICKER_MAP_URL).send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!("SEC ticker map returned HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await?.to_vec();
    if let Some(p) = &path {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(p, &bytes);
    }
    Ok(bytes)
}

/// Resolve `(cik, series_id)` for a ticker from the mutual-fund ticker map.
///
/// The map is shaped as `{"fields":["cik","seriesId","classId","symbol"],
/// "data":[[1100663,"S000004310","C000012040","IVV"], ...]}`.
fn resolve_series(bytes: &[u8], ticker: &str) -> Option<(u64, String)> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let fields = v.get("fields")?.as_array()?;
    let idx = |name: &str| fields.iter().position(|f| f.as_str() == Some(name));
    let cik_i = idx("cik")?;
    let series_i = idx("seriesId")?;
    let symbol_i = idx("symbol")?;
    let data = v.get("data")?.as_array()?;
    for row in data {
        let row = row.as_array()?;
        let sym = row.get(symbol_i).and_then(serde_json::Value::as_str);
        if sym.map(str::to_uppercase).as_deref() == Some(ticker) {
            let cik = row.get(cik_i).and_then(serde_json::Value::as_u64)?;
            let series = row.get(series_i).and_then(serde_json::Value::as_str)?;
            return Some((cik, series.to_string()));
        }
    }
    None
}

/// Locate the latest `NPORT-P` filing for a series via the EDGAR Atom feed.
/// Returns `(filing_href, filing_date)`.
async fn latest_nport_filing(
    client: &reqwest::Client,
    series_id: &str,
) -> Result<(String, String)> {
    let url = format!(
        "https://www.sec.gov/cgi-bin/browse-edgar?action=getcompany&CIK={series_id}\
         &type=NPORT-P&dateb=&owner=include&count=5&output=atom"
    );
    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!("SEC EDGAR returned HTTP {}", resp.status()));
    }
    let xml = resp.text().await?;
    let doc = Document::parse(&xml).map_err(|e| anyhow!("failed to parse EDGAR Atom feed: {e}"))?;

    let href = doc
        .descendants()
        .find(|n| n.has_tag_name("filing-href"))
        .and_then(|n| n.text())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("no NPORT-P filing found for series {series_id}"))?
        .to_string();
    let date = doc
        .descendants()
        .find(|n| n.has_tag_name("filing-date"))
        .and_then(|n| n.text())
        .map_or_else(String::new, |s| s.trim().to_string());
    Ok((href, date))
}

/// Build the `primary_doc.xml` URL from an index page href by replacing the
/// trailing `*-index.htm` segment.
fn primary_doc_url(filing_href: &str) -> String {
    match filing_href.rfind('/') {
        Some(i) => format!("{}/primary_doc.xml", &filing_href[..i]),
        None => filing_href.to_string(),
    }
}

fn parse_f64(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        t.parse().ok()
    }
}

/// Parse holdings out of an N-PORT `primary_doc.xml` document.
fn parse_nport(xml: &str) -> Result<EtfHoldings> {
    let doc = Document::parse(xml).map_err(|e| anyhow!("failed to parse N-PORT XML: {e}"))?;

    let gen_info = doc.descendants().find(|n| n.has_tag_name("genInfo"));
    let child_text = |parent: &roxmltree::Node, tag: &str| -> String {
        parent
            .descendants()
            .find(|n| n.has_tag_name(tag))
            .and_then(|n| n.text())
            .unwrap_or("")
            .trim()
            .to_string()
    };

    let (series_name, report_period) = match gen_info {
        Some(g) => (child_text(&g, "seriesName"), child_text(&g, "repPdDate")),
        None => (String::new(), String::new()),
    };

    let mut holdings = Vec::new();
    for sec in doc.descendants().filter(|n| n.has_tag_name("invstOrSec")) {
        // Direct-child lookup keeps nested blocks (e.g. derivative legs that
        // also carry <name>) from polluting the top-level fields.
        let field = |tag: &str| -> Option<String> {
            sec.children()
                .find(|n| n.has_tag_name(tag))
                .and_then(|n| n.text())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };

        let name = field("name").unwrap_or_default();
        let lei = field("lei");
        let cusip = field("cusip");

        // <isin> may carry the value as a `value="US..."` attribute or as text.
        let isin = sec
            .children()
            .find(|n| n.has_tag_name("isin"))
            .and_then(|n| {
                n.attribute("value")
                    .map(str::to_string)
                    .or_else(|| n.text().map(|t| t.trim().to_string()))
            })
            .filter(|s| !s.is_empty());

        let shares = field("balance").and_then(|s| parse_f64(&s));
        let value_usd = field("valUSD").and_then(|s| parse_f64(&s));
        let weight = field("pctVal").and_then(|s| parse_f64(&s));

        // Skip rows with no identifying name at all.
        if name.is_empty() && cusip.is_none() && isin.is_none() {
            continue;
        }

        holdings.push(Holding {
            name,
            lei,
            cusip,
            isin,
            shares,
            value_usd,
            weight,
        });
    }

    // Sort by weight descending; rows with a missing weight sink to the bottom.
    holdings.sort_by(|a, b| {
        let aw = a.weight.unwrap_or(f64::NEG_INFINITY);
        let bw = b.weight.unwrap_or(f64::NEG_INFINITY);
        bw.partial_cmp(&aw).unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(EtfHoldings {
        series_name,
        report_period,
        filed_date: String::new(),
        holdings,
    })
}

/// Fetch full ETF holdings from SEC EDGAR for a US ticker symbol.
///
/// `symbol` may carry a market suffix (e.g. `IVV.US`); it is normalized
/// internally. Returns an error if the ticker is not present in the mutual-fund
/// map (e.g. UIT-structured funds like SPY) or if any request/parse step fails.
pub async fn fetch_etf_holdings(symbol: &str) -> Result<EtfHoldings> {
    let ticker = normalize_ticker(symbol);
    let client = sec_client();

    let map = load_ticker_map(&client).await?;
    let (_cik, series_id) = resolve_series(&map, &ticker)
        .ok_or_else(|| anyhow!("ticker '{ticker}' not found in SEC mutual-fund map"))?;

    let (filing_href, filed_date) = latest_nport_filing(&client, &series_id).await?;
    let doc_url = primary_doc_url(&filing_href);

    let resp = client.get(&doc_url).send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!(
            "SEC N-PORT document returned HTTP {} for {doc_url}",
            resp.status()
        ));
    }
    let xml = resp.text().await?;
    let mut parsed = parse_nport(&xml)?;
    parsed.filed_date = filed_date;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
    <edgarSubmission>
      <formData>
        <genInfo>
          <seriesName>iShares Core S&amp;P 500 ETF</seriesName>
          <repPdDate>2026-03-31</repPdDate>
        </genInfo>
        <invstOrSecs>
          <invstOrSec>
            <name>Apple Inc</name>
            <lei>HWUPKR0MPOU8FGXBT394</lei>
            <cusip>037833100</cusip>
            <isin value="US0378331005"/>
            <balance>100.5</balance>
            <valUSD>20000.25</valUSD>
            <pctVal>6.123</pctVal>
          </invstOrSec>
          <invstOrSec>
            <name>NVIDIA Corp</name>
            <cusip>67066G104</cusip>
            <isin>US67066G1040</isin>
            <balance>200</balance>
            <valUSD>30000</valUSD>
            <pctVal>7.564</pctVal>
          </invstOrSec>
          <invstOrSec>
            <name>Some Cash Position</name>
            <balance>5</balance>
          </invstOrSec>
        </invstOrSecs>
      </formData>
    </edgarSubmission>
    "#;

    #[test]
    fn parses_and_sorts_by_weight() {
        let h = parse_nport(SAMPLE).unwrap();
        assert_eq!(h.series_name, "iShares Core S&P 500 ETF");
        assert_eq!(h.report_period, "2026-03-31");
        assert_eq!(h.holdings.len(), 3);

        // NVIDIA (7.564) sorts above Apple (6.123); the weightless row sinks last.
        assert_eq!(h.holdings[0].name, "NVIDIA Corp");
        assert_eq!(h.holdings[0].weight, Some(7.564));
        assert_eq!(h.holdings[1].name, "Apple Inc");
        assert_eq!(h.holdings[2].name, "Some Cash Position");
        assert_eq!(h.holdings[2].weight, None);
    }

    #[test]
    fn isin_attribute_and_text_forms() {
        let h = parse_nport(SAMPLE).unwrap();
        let apple = h.holdings.iter().find(|x| x.name == "Apple Inc").unwrap();
        assert_eq!(apple.isin.as_deref(), Some("US0378331005"));
        assert_eq!(apple.cusip.as_deref(), Some("037833100"));
        let nvda = h.holdings.iter().find(|x| x.name == "NVIDIA Corp").unwrap();
        assert_eq!(nvda.isin.as_deref(), Some("US67066G1040"));
    }

    #[test]
    fn resolve_series_matches_symbol() {
        let map = br#"{"fields":["cik","seriesId","classId","symbol"],
            "data":[[1100663,"S000004310","C000012040","IVV"],
                    [1067839,"S000101292","C000300000","QQQ"]]}"#;
        assert_eq!(
            resolve_series(map, "IVV"),
            Some((1_100_663, "S000004310".to_string()))
        );
        assert_eq!(
            resolve_series(map, "QQQ"),
            Some((1_067_839, "S000101292".to_string()))
        );
        assert_eq!(resolve_series(map, "SPY"), None);
    }

    #[test]
    fn primary_doc_url_replaces_index_segment() {
        let href = "https://www.sec.gov/Archives/edgar/data/1100663/000207169126012459/0002071691-26-012459-index.htm";
        assert_eq!(
            primary_doc_url(href),
            "https://www.sec.gov/Archives/edgar/data/1100663/000207169126012459/primary_doc.xml"
        );
    }
}
