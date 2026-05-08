use anyhow::Result;
use serde_json::Value;

use super::api::http_get;
use super::output::print_table;
use super::OutputFormat;
use crate::utils::text::strip_html;

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
                OutputFormat::Json => print_json(&data),
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
                                    val_str(&item["publish_at"]),
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
                OutputFormat::Json => print_json(&data),
                OutputFormat::Pretty => {
                    if let Some(list) = data["topic_list"].as_array() {
                        let items: Vec<&Value> = list.iter().take(limit).collect();
                        if items.is_empty() {
                            println!("No topic results.");
                            return Ok(());
                        }
                        let headers = ["id", "author", "excerpt"];
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
