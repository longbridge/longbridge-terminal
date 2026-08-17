//! Shared client for the news-content REST endpoints.
//!
//! `GET /v1/content/news/{id}` is **not** part of the language SDKs; it is
//! called here directly through the SDK's signed HTTP client
//! ([`crate::openapi::http_client`]), following the same pattern as
//! [`crate::openapi::chats`].

use anyhow::{Context, Result};
use longbridge::httpclient::{Json, Method};
use serde::{Deserialize, Deserializer, Serialize};

/// proto3 JSON encodes `int64` as a string — accept both forms (and `null`,
/// which `#[serde(default)]` alone does not cover). A malformed string is a
/// hard error rather than silently becoming `0`.
fn i64_lenient<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Value {
        Number(i64),
        String(String),
        Null,
    }
    match Value::deserialize(deserializer)? {
        Value::Number(value) => Ok(value),
        Value::String(value) => value.parse().map_err(serde::de::Error::custom),
        Value::Null => Ok(0),
    }
}

/// Author of a [`NewsDetail`] article.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct NewsAuthor {
    #[serde(deserialize_with = "i64_lenient")]
    pub id: i64,
    pub name: String,
    pub avatar: String,
}

/// An image attached to a [`NewsDetail`] article.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct NewsImage {
    pub url: String,
    pub width: i32,
    pub height: i32,
}

/// A full news article, as returned by [`news_detail`].
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct NewsDetail {
    #[serde(deserialize_with = "i64_lenient")]
    pub id: i64,
    pub title: String,
    /// Plain-text excerpt, no HTML tags.
    pub description: String,
    /// Markdown content.
    pub body: String,
    pub url: String,
    pub author: NewsAuthor,
    pub images: Vec<NewsImage>,
    pub comments_count: i32,
    pub likes_count: i32,
    pub shares_count: i32,
    /// Unix timestamp in seconds (UTC).
    #[serde(deserialize_with = "i64_lenient")]
    pub published_at: i64,
    /// `{symbol}.{market}`, e.g. `["AAPL.US", "700.HK"]`.
    pub tickers: Vec<String>,
}

/// `GET /v1/content/news/{id}` — fetch one news article's full detail.
pub async fn news_detail(id: i64) -> Result<NewsDetail> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Response {
        item: NewsDetail,
    }

    let path = format!("/v1/content/news/{id}");
    let resp = crate::openapi::global_rate_limiter()
        .execute("news_detail", || {
            let path = path.clone();
            Box::pin(async move {
                crate::openapi::http_client()
                    .request(Method::GET, path)
                    .response::<Json<Response>>()
                    .send()
                    .await
                    .map(|json| json.0)
            })
        })
        .await
        .context("Failed to get news detail")?;
    Ok(resp.item)
}
