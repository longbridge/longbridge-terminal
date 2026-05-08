use anyhow::Result;
use serde_json::{Map, Value};

use super::api::http_get;
use super::output::print_table;
use super::OutputFormat;
use crate::utils::text::strip_html;

fn fmt_ts(v: &serde_json::Value) -> String {
    let ts = match v {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    };
    ts.map_or_else(|| val_str(v), crate::utils::datetime::format_timestamp)
}

fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
}

fn val_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "-".to_string(),
        other => other.to_string(),
    }
}

fn transform_news_item(item: &Value) -> Value {
    let keep: &[&str] = &["id", "title", "source_name"];
    let mut obj = Map::new();
    if let Some(map) = item.as_object() {
        for (k, v) in map {
            if k == "publish_at_timestamp" {
                obj.insert("time".to_string(), Value::String(fmt_ts(v)));
            } else if k == "description" {
                let excerpt: String = strip_html(&val_str(v)).chars().take(80).collect();
                obj.insert("excerpt".to_string(), Value::String(excerpt));
            } else if keep.contains(&k.as_str()) {
                if k == "title" {
                    obj.insert(k.clone(), Value::String(strip_html(&val_str(v))));
                } else {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }
    }
    if let Some(id) = obj.get("id").and_then(Value::as_str) {
        let url = format!("https://longbridge.com/news/{id}.md");
        obj.insert("url".to_string(), Value::String(url));
    }
    Value::Object(obj)
}

fn transform_topic_item(item: &Value) -> Value {
    let keep: &[&str] = &[
        "id",
        "title",
        "comments_count",
        "likes_count",
        "creator_name",
        "creator_id",
    ];
    let mut obj = Map::new();
    if let Some(map) = item.as_object() {
        for (k, v) in map {
            if k == "created_at_timestamp" {
                obj.insert("time".to_string(), Value::String(fmt_ts(v)));
            } else if k == "description" {
                let excerpt: String = strip_html(&val_str(v)).chars().take(80).collect();
                obj.insert("excerpt".to_string(), Value::String(excerpt));
            } else if keep.contains(&k.as_str()) {
                if k == "title" {
                    obj.insert(k.clone(), Value::String(strip_html(&val_str(v))));
                } else {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }
    }
    if let Some(id) = obj.get("id").and_then(Value::as_str) {
        let url = format!("https://longbridge.com/topics/{id}.md");
        obj.insert("url".to_string(), Value::String(url));
    }
    Value::Object(obj)
}

// ── search ────────────────────────────────────────────────────────────────────

/// Search news or community topics.
pub async fn cmd_search(
    keyword: String,
    tab: &str,
    limit: usize,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    match tab {
        "news" => {
            let data = http_get("/v1/search/news", &[("k", keyword.as_str())], verbose).await?;
            match format {
                OutputFormat::Json => {
                    if let Some(list) = data["news_list"].as_array() {
                        let items: Vec<Value> =
                            list.iter().take(limit).map(transform_news_item).collect();
                        print_json(&Value::Array(items));
                    } else {
                        print_json(&data);
                    }
                }
                OutputFormat::Pretty => {
                    if let Some(list) = data["news_list"].as_array() {
                        let items: Vec<&Value> = list.iter().take(limit).collect();
                        if items.is_empty() {
                            println!("No news results.");
                            return Ok(());
                        }
                        let headers = ["id", "title", "time"];
                        let rows: Vec<Vec<String>> = items
                            .iter()
                            .map(|item| {
                                vec![
                                    val_str(&item["id"]),
                                    strip_html(&val_str(&item["title"])),
                                    fmt_ts(&item["publish_at_timestamp"]),
                                ]
                            })
                            .collect();
                        print_table(&headers, rows, &OutputFormat::Pretty);
                    } else {
                        print_json(&data);
                    }
                }
            }
        }
        "topics" => {
            let data = http_get("/v1/search/topics", &[("k", keyword.as_str())], verbose).await?;
            match format {
                OutputFormat::Json => {
                    if let Some(list) = data["topic_list"].as_array() {
                        let items: Vec<Value> =
                            list.iter().take(limit).map(transform_topic_item).collect();
                        print_json(&Value::Array(items));
                    } else {
                        print_json(&data);
                    }
                }
                OutputFormat::Pretty => {
                    if let Some(list) = data["topic_list"].as_array() {
                        let items: Vec<&Value> = list.iter().take(limit).collect();
                        if items.is_empty() {
                            println!("No topic results.");
                            return Ok(());
                        }
                        let headers = ["id", "author", "time", "excerpt"];
                        let rows: Vec<Vec<String>> = items
                            .iter()
                            .map(|item| {
                                let excerpt: String = strip_html(&val_str(&item["description"]))
                                    .chars()
                                    .take(60)
                                    .collect();
                                vec![
                                    val_str(&item["id"]),
                                    val_str(&item["creator_name"]),
                                    fmt_ts(&item["created_at_timestamp"]),
                                    excerpt,
                                ]
                            })
                            .collect();
                        print_table(&headers, rows, &OutputFormat::Pretty);
                    } else {
                        print_json(&data);
                    }
                }
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}
