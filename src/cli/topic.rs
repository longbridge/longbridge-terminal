use anyhow::Result;
use longbridge::content::{
    CreateReplyOptions, CreateTopicOptions, ListTopicRepliesOptions, MyTopicsOptions, OwnedTopic,
    TopicReply,
};
use regex::Regex;

use super::{output::print_table, OutputFormat};
use crate::utils::datetime::format_datetime;

/// Format topic content by replacing `[st]ST/MARKET/SYMBOL#...[/st]` tags with ticker symbols like `TSLA.US`.
pub fn format_topic_contents(text: &str) -> String {
    let mut result = String::new();
    let mut remaining = text;
    while let Some(start) = remaining.find("[st]") {
        result.push_str(&remaining[..start]);
        let after_open = &remaining[start + 4..];
        let Some(end) = after_open.find("[/st]") else {
            result.push_str("[st]");
            result.push_str(after_open);
            return result;
        };
        let inner = &after_open[..end];
        // Format: ST/MARKET/SYMBOL#DisplayName or ST/MARKET/SYMBOL
        let code = inner.split('#').next().unwrap_or(inner);
        let parts: Vec<&str> = code.split('/').collect();
        if parts.len() >= 3 {
            let ticker = format!("{}.{}", parts[2].to_uppercase(), parts[1].to_uppercase());
            result.push_str(&ticker);
        } else {
            result.push_str(inner);
        }
        remaining = &after_open[end + 5..];
    }
    result.push_str(remaining);
    result
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
    let opts = MyTopicsOptions {
        page: Some(page),
        size: Some(size),
        topic_type: post_type,
    };
    let items = crate::openapi::content().my_topics(opts).await?;

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
            let desc = format_topic_contents(&item.description);
            let display = if item.title.is_empty() {
                desc.clone()
            } else {
                item.title.clone()
            };
            vec![
                item.id.clone(),
                display,
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

fn topic_reply_to_json(item: &TopicReply) -> serde_json::Value {
    serde_json::json!({
        "id": item.id,
        "topic_id": item.topic_id,
        "body": item.body,
        "reply_to_id": item.reply_to_id,
        "author": {
            "member_id": item.author.member_id,
            "name": item.author.name,
        },
        "likes_count": item.likes_count,
        "comments_count": item.comments_count,
        "created_at": item.created_at.unix_timestamp(),
    })
}

/// Get full details of a topic by ID via the `OpenAPI` (`GET /v1/content/topics/{id}`).
pub async fn cmd_topic_detail_api(id: String, format: &OutputFormat) -> Result<()> {
    let item = crate::openapi::content().topic_detail(id).await?;

    if matches!(format, OutputFormat::Json) {
        println!(
            "{}",
            serde_json::to_string_pretty(&item).unwrap_or_default()
        );
        return Ok(());
    }

    println!("ID:       {}", item.id);
    println!("Type:     {}", item.topic_type);
    if !item.title.is_empty() {
        println!("Title:    {}", item.title);
    }
    if !item.tickers.is_empty() {
        println!("Tickers:  {}", item.tickers.join(", "));
    }
    if !item.hashtags.is_empty() {
        println!("Tags:     {}", item.hashtags.join(", "));
    }
    println!(
        "Stats:    {} likes  {} comments  {} views",
        item.likes_count, item.comments_count, item.views_count
    );
    println!("Created:  {}", format_datetime(item.created_at));
    println!("URL:      {}", item.detail_url);
    let content = if item.topic_type == "post" {
        item.description.clone()
    } else {
        item.body.clone()
    };

    if !content.is_empty() {
        println!("\n{}", format_topic_contents(&content));
    }
    Ok(())
}

/// List replies for a topic.
pub async fn cmd_topic_replies(
    topic_id: String,
    page: i32,
    size: i32,
    format: &OutputFormat,
) -> Result<()> {
    let opts = ListTopicRepliesOptions {
        page: Some(page),
        size: Some(size),
    };
    let items = crate::openapi::content()
        .list_topic_replies(topic_id, opts)
        .await?;

    if items.is_empty() {
        println!("No replies found.");
        return Ok(());
    }

    if matches!(format, OutputFormat::Json) {
        let records: Vec<_> = items.iter().map(topic_reply_to_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&records).unwrap_or_default()
        );
        return Ok(());
    }

    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            println!("{}", "-".repeat(60));
            println!();
        }
        println!("ID:      {}", item.id);
        println!("Author:  {}", item.author.name);
        if item.reply_to_id != "0" {
            println!("Reply to: {}", item.reply_to_id);
        }
        println!(
            "Stats:   {} likes  {} replies",
            item.likes_count, item.comments_count
        );
        println!("Created: {}", format_datetime(item.created_at));
        let body = format_topic_contents(&item.body);
        if !body.is_empty() {
            println!("\n{body}");
        }
        println!();
    }
    Ok(())
}

/// Post a reply to a topic.
pub async fn cmd_create_reply(
    topic_id: String,
    body: String,
    reply_to_id: Option<String>,
    format: &OutputFormat,
) -> Result<()> {
    let opts = CreateReplyOptions { body, reply_to_id };
    let reply = crate::openapi::content()
        .create_topic_reply(topic_id, opts)
        .await?;

    if matches!(format, OutputFormat::Json) {
        println!(
            "{}",
            serde_json::to_string_pretty(&topic_reply_to_json(&reply)).unwrap_or_default()
        );
        return Ok(());
    }

    println!("Reply created successfully.");
    println!("  ID: {}", reply.id);
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

#[cfg(test)]
mod tests {
    use super::format_topic_contents;

    #[test]
    fn test_single_stock_tag() {
        assert_eq!(
            format_topic_contents("[st]ST/US/TSLA#Tesla.US[/st]"),
            "TSLA.US"
        );
    }

    #[test]
    fn test_stock_tag_with_surrounding_text() {
        assert_eq!(
            format_topic_contents("Bullish on [st]ST/US/TSLA#Tesla.US[/st] today"),
            "Bullish on TSLA.US today"
        );
    }

    #[test]
    fn test_multiple_stock_tags() {
        assert_eq!(
            format_topic_contents("[st]ST/HK/700#Tencent.HK[/st] and [st]ST/US/AAPL#Apple.US[/st]"),
            "700.HK and AAPL.US"
        );
    }

    #[test]
    fn test_no_stock_tags() {
        assert_eq!(
            format_topic_contents("Plain text with no tags"),
            "Plain text with no tags"
        );
    }

    #[test]
    fn test_uppercase_output() {
        assert_eq!(
            format_topic_contents("[st]st/us/tsla#Tesla.US[/st]"),
            "TSLA.US"
        );
    }
}
