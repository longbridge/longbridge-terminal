use anyhow::{bail, Result};
use longbridge::httpclient::{Json, Method};
use serde::Deserialize;
use time::OffsetDateTime;

use super::{output::print_table, OutputFormat};

const NEWS_DETAIL_BASE: &str = "https://longbridge.com/news";

#[derive(Debug, Deserialize)]
struct NewsItem {
    id: String,
    title: String,
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
            Ok(v as i64)
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
                serde_json::json!({
                    "id": item.id,
                    "title": item.title,
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
            let dt = OffsetDateTime::from_unix_timestamp(item.published_at)
                .map(|dt| {
                    format!(
                        "{}-{:02}-{:02} {:02}:{:02}",
                        dt.year(),
                        dt.month() as u8,
                        dt.day(),
                        dt.hour(),
                        dt.minute()
                    )
                })
                .unwrap_or_else(|_| item.published_at.to_string());

            let title = if item.title.chars().count() > 70 {
                format!("{}…", item.title.chars().take(70).collect::<String>())
            } else {
                item.title.clone()
            };

            vec![
                item.id.clone(),
                title,
                dt,
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
        file_urls: Vec<String>,
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
                    "file_urls": item.file_urls,
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
            let dt = OffsetDateTime::from_unix_timestamp(item.publish_at)
                .map(|dt| {
                    format!(
                        "{}-{:02}-{:02} {:02}:{:02}",
                        dt.year(),
                        dt.month() as u8,
                        dt.day(),
                        dt.hour(),
                        dt.minute()
                    )
                })
                .unwrap_or_else(|_| item.publish_at.to_string());

            let title = if item.title.chars().count() > 60 {
                format!("{}…", item.title.chars().take(60).collect::<String>())
            } else {
                item.title.clone()
            };

            vec![item.id.clone(), title, item.file_name.clone(), dt]
        })
        .collect();

    print_table(headers, rows, format);
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
            let dt = OffsetDateTime::from_unix_timestamp(item.published_at)
                .map(|dt| {
                    format!(
                        "{}-{:02}-{:02} {:02}:{:02}",
                        dt.year(),
                        dt.month() as u8,
                        dt.day(),
                        dt.hour(),
                        dt.minute()
                    )
                })
                .unwrap_or_else(|_| item.published_at.to_string());

            let display = if item.title.is_empty() {
                &item.description
            } else {
                &item.title
            };
            let title = if display.chars().count() > 60 {
                format!("{}…", display.chars().take(60).collect::<String>())
            } else {
                display.clone()
            };

            vec![
                item.id.clone(),
                title,
                dt,
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
