use anyhow::{bail, Result};

use super::{output::print_table, OutputFormat};
use crate::utils::datetime::format_datetime;

const NEWS_DETAIL_BASE: &str = "https://longbridge.com/news";

/// Return `s` truncated to `max` chars with a trailing `…`, or the original if it fits.
fn truncate_display(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        format!("{}…", s.chars().take(max).collect::<String>())
    } else {
        s.to_owned()
    }
}

/// Fetch news articles for a symbol.
pub async fn cmd_news(symbol: String, count: usize, format: &OutputFormat) -> Result<()> {
    let items = crate::openapi::content().news(&symbol).await?;

    if items.is_empty() {
        println!("No news found for {symbol}.");
        return Ok(());
    }

    let items: Vec<_> = items.into_iter().take(count).collect();

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
                    "url": item.url,
                    "published_at": item.published_at.unix_timestamp(),
                    "likes_count": item.likes_count,
                    "comments_count": item.comments_count,
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
                format_datetime(item.published_at),
                item.likes_count.to_string(),
                item.comments_count.to_string(),
            ]
        })
        .collect();

    print_table(headers, rows, format);
    Ok(())
}

/// Fetch regulatory filings for a symbol.
pub async fn cmd_filings(symbol: String, count: usize, format: &OutputFormat) -> Result<()> {
    let items = crate::openapi::quote().filings(&symbol).await?;

    if items.is_empty() {
        println!("No filings found for {symbol}.");
        return Ok(());
    }

    let items: Vec<_> = items.into_iter().take(count).collect();

    if matches!(format, OutputFormat::Json) {
        let records: Vec<serde_json::Value> = items
            .iter()
            .map(|item| {
                serde_json::json!({
                    "id": item.id,
                    "title": item.title,
                    "description": item.description,
                    "file_name": item.file_name,
                    "publish_at": item.published_at.unix_timestamp(),
                    "file_count": item.file_urls.len(),
                    "file_urls": item.file_urls,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&records).unwrap_or_default()
        );
        return Ok(());
    }

    let headers = &["id", "title", "file_name", "files", "publish_at"];
    let rows = items
        .iter()
        .map(|item| {
            vec![
                item.id.clone(),
                truncate_display(&item.title, 60),
                item.file_name.clone(),
                item.file_urls.len().to_string(),
                format_datetime(item.published_at),
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
/// Markdown. TXT files are printed as-is. Returns an error for HTTP failures
/// (e.g. 403 from SEC EDGAR) or unsupported formats (e.g. PDF).
pub async fn cmd_filing_detail(
    symbol: String,
    id: String,
    list_files: bool,
    file_index: usize,
) -> Result<()> {
    let items = crate::openapi::quote().filings(&symbol).await?;

    let filing = items
        .into_iter()
        .find(|item| item.id == id)
        .ok_or_else(|| anyhow::anyhow!("Filing '{id}' not found"))?;

    if list_files {
        for (i, url) in filing.file_urls.iter().enumerate() {
            println!("{i}: {url}");
        }
        println!("\n> Usage: longbridge filing-detail {symbol} {id} --file-index <N>");
        return Ok(());
    }

    let url = filing
        .file_urls
        .get(file_index)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "File index {file_index} out of range (filing has {} file(s))",
                filing.file_urls.len()
            )
        })?
        .clone();

    let total_files = filing.file_urls.len();

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
        let status = file_resp.status();
        return Err(anyhow::anyhow!("failed to fetch {url} (HTTP {status})"));
    }

    let content_type = file_resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    let path = url.split('?').next().unwrap_or(&url);
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let ext = ext.as_deref().unwrap_or("");
    let is_text = content_type.contains("html")
        || content_type.contains("text/plain")
        || ext == "html"
        || ext == "htm"
        || ext == "xml"
        || ext == "txt";

    if !is_text {
        return Err(anyhow::anyhow!(
            "unsupported format for {url} (content-type: {content_type})"
        ));
    }

    let body = file_resp.text().await?;

    let is_html = content_type.contains("html") || ext == "html" || ext == "htm" || ext == "xml";

    let output = if is_html {
        sec2md::convert(&body)
    } else {
        body
    };

    print!("{output}");

    println!("\n---\nSource: {url}");
    if total_files > 1 && file_index == 0 {
        println!(
            "Note: this filing has {total_files} files. Use --file-index N (0..{}) to fetch others.",
            total_files - 1
        );
    }

    Ok(())
}

/// Fetch community discussion topics for a symbol.
pub async fn cmd_topics(symbol: String, count: usize, format: &OutputFormat) -> Result<()> {
    let items = crate::openapi::content().topics(&symbol).await?;

    if items.is_empty() {
        println!("No topics found for {symbol}.");
        return Ok(());
    }

    let items: Vec<_> = items.into_iter().take(count).collect();

    if matches!(format, OutputFormat::Json) {
        let records: Vec<serde_json::Value> = items
            .iter()
            .map(|item| {
                serde_json::json!({
                    "id": item.id,
                    "title": item.title,
                    "description": crate::cli::topic::format_topic_contents(&item.description),
                    "url": item.url,
                    "published_at": item.published_at.unix_timestamp(),
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
                format_datetime(item.published_at),
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
