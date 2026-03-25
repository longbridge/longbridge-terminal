use anyhow::Result;
use longbridge::content::{CreateTopicOptions, ListMyTopicsOptions, OwnedTopic};
use regex::Regex;
use time::OffsetDateTime;

use super::{output::print_table, OutputFormat};

fn format_datetime(dt: OffsetDateTime) -> String {
    format!(
        "{}-{:02}-{:02} {:02}:{:02}",
        dt.year(),
        dt.month() as u8,
        dt.day(),
        dt.hour(),
        dt.minute()
    )
}

fn owned_topic_to_json(item: &OwnedTopic) -> serde_json::Value {
    serde_json::json!({
        "id": item.id,
        "title": item.title,
        "topic_type": item.topic_type,
        "tickers": item.tickers,
        "hashtags": item.hashtags,
        "likes_count": item.likes_count,
        "comments_count": item.comments_count,
        "views_count": item.views_count,
        "shares_count": item.shares_count,
        "license": item.license,
        "url": format!("https://longbridge.com/topics/{}", item.id),
        "created_at": item.created_at.unix_timestamp(),
        "updated_at": item.updated_at.unix_timestamp(),
    })
}

/// List topics created by the authenticated user.
pub async fn cmd_topics_mine(
    page: i32,
    size: i32,
    post_type: Option<String>,
    format: &OutputFormat,
) -> Result<()> {
    let opts = ListMyTopicsOptions {
        page: Some(page),
        size: Some(size),
        topic_type: post_type,
    };
    let items = crate::openapi::content().topics_mine(opts).await?;

    if items.is_empty() {
        println!("No topics found.");
        return Ok(());
    }

    if matches!(format, OutputFormat::Json) {
        let records: Vec<_> = items.iter().map(owned_topic_to_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&records).unwrap_or_default()
        );
        return Ok(());
    }

    let headers = &[
        "id",
        "title/excerpt",
        "type",
        "created_at",
        "likes",
        "comments",
        "views",
    ];
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
                display.clone(),
                item.topic_type.clone(),
                format_datetime(item.created_at),
                item.likes_count.to_string(),
                item.comments_count.to_string(),
                item.views_count.to_string(),
            ]
        })
        .collect();

    print_table(headers, rows, format);
    Ok(())
}

/// Returns true if the text contains Markdown or HTML that won't render in a plain-text post.
fn has_rich_markup(text: &str) -> bool {
    // Markdown
    if text.contains("**")
        || text.contains("##")
        || text.contains("| ")
        || text.contains("```")
        || text.contains("- [")
        || text.contains("![")
    {
        return true;
    }
    // HTML tags: <tag>, <tag attr>, </tag>
    Regex::new(r"<.+?>").is_ok_and(|re| re.is_match(text))
}

/// Publish a new community discussion topic.
pub async fn cmd_create_topic(
    title: Option<String>,
    body: String,
    post_type: Option<String>,
    tickers: Vec<String>,
    format: &OutputFormat,
) -> Result<()> {
    let is_post = post_type.as_deref().unwrap_or("post") == "post";
    if is_post && has_rich_markup(&body) {
        eprintln!(
            "Warning: --type post is plain text only. Markdown and HTML (**, ##, <b>, etc.) \
             will appear as literal characters. Use --type article for rich formatting."
        );
    }

    let opts = CreateTopicOptions {
        title: title.unwrap_or_default(),
        body,
        topic_type: post_type,
        tickers: if tickers.is_empty() {
            None
        } else {
            Some(tickers)
        },
        hashtags: None,
        license: None,
    };
    let id = crate::openapi::content().create_topic(opts).await?;

    if matches!(format, OutputFormat::Json) {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "id": id })).unwrap_or_default()
        );
        return Ok(());
    }

    println!("Topic created successfully.");
    println!("  ID:   {id}");
    println!("  URL:  https://longbridge.com/topics/{id}");
    Ok(())
}
