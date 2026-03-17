use anyhow::{bail, Result};
use longbridge::httpclient::{Json, Method};
use serde::Deserialize;
use time::OffsetDateTime;

use super::{output::print_table, OutputFormat};

const NEWS_DETAIL_BASE: &str = "https://longbridge.com/news";

/// Format a Unix timestamp as "YYYY-MM-DD HH:MM", falling back to the raw number on error.
fn format_timestamp(ts: i64) -> String {
    OffsetDateTime::from_unix_timestamp(ts).map_or_else(
        |_| ts.to_string(),
        |dt| {
            format!(
                "{}-{:02}-{:02} {:02}:{:02}",
                dt.year(),
                dt.month() as u8,
                dt.day(),
                dt.hour(),
                dt.minute()
            )
        },
    )
}

/// Return `s` truncated to `max` chars with a trailing `…`, or the original if it fits.
fn truncate_display(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        format!("{}…", s.chars().take(max).collect::<String>())
    } else {
        s.to_owned()
    }
}

#[derive(Debug, Deserialize)]
struct NewsItem {
    id: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(deserialize_with = "deserialize_str_or_i64")]
    published_at: i64,
    comments_count: i64,
    likes_count: i64,
}

fn deserialize_str_or_i64<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};

    struct StrOrI64;
    impl Visitor<'_> for StrOrI64 {
        type Value = i64;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("i64 or string containing i64")
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<i64, E> {
            Ok(v)
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<i64, E> {
            i64::try_from(v).map_err(de::Error::custom)
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<i64, E> {
            v.parse().map_err(de::Error::custom)
        }
    }
    deserializer.deserialize_any(StrOrI64)
}

/// Fetch news articles for a symbol: GET /v1/content/{symbol}/news
pub async fn cmd_news(symbol: String, count: usize, format: &OutputFormat) -> Result<()> {
    // SDK unwraps the {"code":0,"data":{...}} envelope; Response maps to the data field.
    #[derive(Debug, Deserialize)]
    struct Response {
        items: Vec<NewsItem>,
    }

    let path = format!("/v1/content/{symbol}/news");
    let resp = crate::openapi::http_client()
        .request(Method::GET, path)
        .response::<Json<Response>>()
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let items = resp.0.items;
    if items.is_empty() {
        println!("No news found for {symbol}.");
        return Ok(());
    }

    let items: Vec<&NewsItem> = items.iter().take(count).collect();

    if matches!(format, OutputFormat::Json) {
        let records: Vec<serde_json::Value> = items
            .iter()
            .map(|item| {
                let title = if item.title.is_empty() {
                    truncate_display(&item.description, 70)
                } else {
                    item.title.clone()
                };
                serde_json::json!({
                    "id": item.id,
                    "title": title,
                    "published_at": item.published_at,
                    "likes_count": item.likes_count,
                    "comments_count": item.comments_count,
                    "url": format!("{NEWS_DETAIL_BASE}/{}.md", item.id),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&records).unwrap_or_default()
        );
        return Ok(());
    }

    let headers = &["id", "title", "published_at", "likes", "comments"];
    let rows = items
        .iter()
        .map(|item| {
            let display = if item.title.is_empty() {
                &item.description
            } else {
                &item.title
            };
            vec![
                item.id.clone(),
                truncate_display(display, 70),
                format_timestamp(item.published_at),
                item.likes_count.to_string(),
                item.comments_count.to_string(),
            ]
        })
        .collect();

    print_table(headers, rows, format);
    Ok(())
}

/// Fetch regulatory filings for a symbol: GET /v1/quote/filings?symbol=AAPL.US
pub async fn cmd_filings(symbol: String, count: usize, format: &OutputFormat) -> Result<()> {
    use serde::Serialize;

    #[derive(Debug, Deserialize)]
    struct FilingItem {
        id: String,
        title: String,
        description: String,
        file_name: String,
        #[serde(deserialize_with = "deserialize_str_or_i64")]
        publish_at: i64,
    }

    #[derive(Debug, Deserialize)]
    struct Response {
        items: Vec<FilingItem>,
    }

    #[derive(Debug, Serialize)]
    struct Query {
        symbol: String,
    }

    let resp = crate::openapi::http_client()
        .request(Method::GET, "/v1/quote/filings")
        .query_params(Query {
            symbol: symbol.clone(),
        })
        .response::<Json<Response>>()
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let items = resp.0.items;
    if items.is_empty() {
        println!("No filings found for {symbol}.");
        return Ok(());
    }

    let items: Vec<&FilingItem> = items.iter().take(count).collect();

    if matches!(format, OutputFormat::Json) {
        let records: Vec<serde_json::Value> = items
            .iter()
            .map(|item| {
                serde_json::json!({
                    "id": item.id,
                    "title": item.title,
                    "description": item.description,
                    "file_name": item.file_name,
                    "publish_at": item.publish_at,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&records).unwrap_or_default()
        );
        return Ok(());
    }

    let headers = &["id", "title", "file_name", "publish_at"];
    let rows = items
        .iter()
        .map(|item| {
            vec![
                item.id.clone(),
                truncate_display(&item.title, 60),
                item.file_name.clone(),
                format_timestamp(item.publish_at),
            ]
        })
        .collect();

    print_table(headers, rows, format);
    Ok(())
}

const FILING_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/146.0.0.0 Safari/537.36";

/// Fetch and convert a regulatory filing to Markdown (HTML/TXT only).
///
/// Calls the filings list API to resolve the download URL for the given id,
/// then fetches the document with browser-like headers and converts HTML to
/// Markdown. TXT files are printed as-is. Unsupported formats (PDF, etc.) or
/// HTTP errors (e.g. 403 from SEC EDGAR) fall back to printing the raw URL.
pub async fn cmd_filing_detail(symbol: String, id: String) -> Result<()> {
    use serde::Serialize;

    #[derive(Debug, Deserialize)]
    struct FilingItem {
        id: String,
        file_urls: Vec<String>,
    }

    #[derive(Debug, Deserialize)]
    struct Response {
        items: Vec<FilingItem>,
    }

    #[derive(Debug, Serialize)]
    struct Query {
        symbol: String,
    }

    let resp = crate::openapi::http_client()
        .request(Method::GET, "/v1/quote/filings")
        .query_params(Query { symbol })
        .response::<Json<Response>>()
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let filing = resp
        .0
        .items
        .into_iter()
        .find(|item| item.id == id)
        .ok_or_else(|| anyhow::anyhow!("Filing '{id}' not found"))?;

    let url = filing
        .file_urls
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No download URL for filing '{id}'"))?;

    let client = reqwest::Client::new();
    let file_resp = client
        .get(&url)
        .header("User-Agent", FILING_UA)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Accept-Encoding", "gzip, deflate, br")
        .header("Cache-Control", "max-age=0")
        .header("Connection", "keep-alive")
        .header("Upgrade-Insecure-Requests", "1")
        .header("Sec-Fetch-Dest", "document")
        .header("Sec-Fetch-Mode", "navigate")
        .header("Sec-Fetch-Site", "none")
        .header("Sec-Fetch-User", "?1")
        .send()
        .await?;

    if !file_resp.status().is_success() {
        // Fall back to the raw URL (e.g. 403 from SEC EDGAR) so the caller can handle it.
        println!("{url}");
        return Ok(());
    }

    let content_type = file_resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    // Use URL path (before query string) for extension detection.
    let path = url.split('?').next().unwrap_or(&url);
    let is_text = content_type.contains("html")
        || content_type.contains("text/plain")
        || path.ends_with(".html")
        || path.ends_with(".htm")
        || path.ends_with(".txt");

    if !is_text {
        // Unsupported format (e.g. PDF): return the URL so the caller can handle it.
        println!("{url}");
        return Ok(());
    }

    let body = file_resp.text().await?;

    let is_html = content_type.contains("html")
        || path.ends_with(".html")
        || path.ends_with(".htm");

    let output = if is_html {
        sec2md::convert(&body)
    } else {
        body
    };

    print!("{output}");

    // Append the source URL so the caller can reference or verify the original document.
    println!("\n---\nSource: {url}");

    Ok(())
}

/// Fetch community discussion topics for a symbol: GET /v1/content/{symbol}/topics
pub async fn cmd_topics(symbol: String, count: usize, format: &OutputFormat) -> Result<()> {
    #[derive(Debug, Deserialize)]
    struct TopicItem {
        id: String,
        title: String,
        description: String,
        url: String,
        #[serde(deserialize_with = "deserialize_str_or_i64")]
        published_at: i64,
        comments_count: i64,
        likes_count: i64,
        shares_count: i64,
    }

    #[derive(Debug, Deserialize)]
    struct Response {
        items: Vec<TopicItem>,
    }

    let path = format!("/v1/content/{symbol}/topics");
    let resp = crate::openapi::http_client()
        .request(Method::GET, path)
        .response::<Json<Response>>()
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let items = resp.0.items;
    if items.is_empty() {
        println!("No topics found for {symbol}.");
        return Ok(());
    }

    let items: Vec<&TopicItem> = items.iter().take(count).collect();

    if matches!(format, OutputFormat::Json) {
        let records: Vec<serde_json::Value> = items
            .iter()
            .map(|item| {
                serde_json::json!({
                    "id": item.id,
                    "title": item.title,
                    "description": item.description,
                    "url": item.url,
                    "published_at": item.published_at,
                    "likes_count": item.likes_count,
                    "comments_count": item.comments_count,
                    "shares_count": item.shares_count,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&records).unwrap_or_default()
        );
        return Ok(());
    }

    let headers = &["id", "title", "published_at", "likes", "comments", "shares"];
    let rows = items
        .iter()
        .map(|item| {
            let display = if item.title.is_empty() {
                &item.description
            } else {
                &item.title
            };
            vec![
                item.id.clone(),
                truncate_display(display, 60),
                format_timestamp(item.published_at),
                item.likes_count.to_string(),
                item.comments_count.to_string(),
                item.shares_count.to_string(),
            ]
        })
        .collect();

    print_table(headers, rows, format);
    Ok(())
}

/// Fetch full topic content as Markdown: GET <https://longbridge.com/topics/{id}.md>
pub async fn cmd_topic_detail(id: String) -> Result<()> {
    let url = format!("https://longbridge.com/topics/{id}.md");
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("User-Agent", "Mozilla/5.0")
        .send()
        .await?;

    if !resp.status().is_success() {
        bail!("Failed to fetch topic detail: HTTP {}", resp.status());
    }

    let content = resp.text().await?;
    print!("{content}");
    Ok(())
}

/// Fetch full news article as Markdown: GET <https://longbridge.com/news/{id}.md>
pub async fn cmd_news_detail(id: String) -> Result<()> {
    let url = format!("{NEWS_DETAIL_BASE}/{id}.md");
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("User-Agent", "Mozilla/5.0")
        .send()
        .await?;

    if !resp.status().is_success() {
        bail!("Failed to fetch news detail: HTTP {}", resp.status());
    }

    let content = resp.text().await?;
    print!("{content}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    // Helper to drive `deserialize_str_or_i64` through a JSON value.
    fn deser(json: &str) -> serde_json::Result<i64> {
        #[derive(Deserialize)]
        struct Wrapper(#[serde(deserialize_with = "deserialize_str_or_i64")] i64);
        serde_json::from_str::<Wrapper>(json).map(|w| w.0)
    }

    // ── deserialize_str_or_i64 ───────────────────────────────────────────────

    #[test]
    fn deser_integer_literal() {
        assert_eq!(deser("1700000000").unwrap(), 1_700_000_000);
    }

    #[test]
    fn deser_negative_integer() {
        assert_eq!(deser("-1").unwrap(), -1);
    }

    #[test]
    fn deser_string_integer() {
        assert_eq!(deser(r#""1700000000""#).unwrap(), 1_700_000_000);
    }

    #[test]
    fn deser_string_negative() {
        assert_eq!(deser(r#""-42""#).unwrap(), -42);
    }

    #[test]
    fn deser_string_invalid_errors() {
        assert!(deser(r#""not-a-number""#).is_err());
    }

    #[test]
    fn deser_u64_too_large_errors() {
        // u64::MAX cannot fit in i64.
        let json = u64::MAX.to_string();
        assert!(deser(&json).is_err());
    }

    // ── format_timestamp ────────────────────────────────────────────────────

    #[test]
    fn format_known_timestamp() {
        // Unix 0 == 1970-01-01 00:00 UTC
        assert_eq!(format_timestamp(0), "1970-01-01 00:00");
    }

    #[test]
    fn format_realistic_timestamp() {
        // 2024-01-15 07:50 UTC  →  1705305000
        assert_eq!(format_timestamp(1_705_305_000), "2024-01-15 07:50");
    }

    #[test]
    fn format_invalid_timestamp_falls_back_to_raw() {
        // Values far outside the valid range produce an error from time::OffsetDateTime.
        let ts = i64::MAX;
        assert_eq!(format_timestamp(ts), ts.to_string());
    }

    // ── truncate_display ────────────────────────────────────────────────────

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate_display("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact_length_unchanged() {
        let s = "a".repeat(10);
        assert_eq!(truncate_display(&s, 10), s);
    }

    #[test]
    fn truncate_long_string_adds_ellipsis() {
        let s = "a".repeat(11);
        let result = truncate_display(&s, 10);
        assert!(result.ends_with('…'));
        assert_eq!(result.chars().count(), 11); // 10 chars + ellipsis
    }

    #[test]
    fn truncate_multibyte_chars() {
        // Each Chinese character is one char but multiple bytes.
        let s = "中文测试内容标题超过限制的长度"; // 14 chars
        let result = truncate_display(s, 5);
        assert!(result.starts_with("中文测试内"));
        assert!(result.ends_with('…'));
    }

    #[test]
    fn truncate_empty_string_unchanged() {
        assert_eq!(truncate_display("", 5), "");
    }
}
