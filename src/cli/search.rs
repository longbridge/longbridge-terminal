use anyhow::Result;
use serde_json::{json, Value};

use super::api::{http_get, http_post};
use super::output::print_table;
use super::OutputFormat;

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
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

// ── search ────────────────────────────────────────────────────────────────────

/// Search across securities, news, community, help, and more.
pub async fn cmd_search(
    keyword: String,
    tab: &str,
    market: Option<&str>,
    product: Option<&str>,
    limit: usize,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    match tab {
        "market" => {
            let mut params: Vec<(&str, &str)> = vec![("k", keyword.as_str())];
            if let Some(m) = market {
                params.push(("market", m));
            }
            if let Some(p) = product {
                params.push(("product", p));
            }
            let data = http_get("/v4/search", &params, verbose).await?;
            match format {
                OutputFormat::Json => print_json(&data),
                OutputFormat::Pretty => {
                    if let Some(list) = data["product_list"].as_array() {
                        let items: Vec<&Value> = list.iter().take(limit).collect();
                        if items.is_empty() {
                            println!("No results.");
                            return Ok(());
                        }
                        let headers = ["symbol", "name", "market", "type"];
                        let rows: Vec<Vec<String>> = items
                            .iter()
                            .map(|item| {
                                vec![
                                    val_str(&item["code"]),
                                    val_str(&item["name"]),
                                    val_str(&item["market"]),
                                    val_str(&item["type"]),
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
        "news" => {
            let data = http_get("/v1/news_search", &[("k", keyword.as_str())], verbose).await?;
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
                                    val_str(&item["title"]),
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
        "posts" => {
            let body = json!({ "k": keyword });
            let data = http_post("/v1/search/social_topics", body, verbose).await?;
            match format {
                OutputFormat::Json => print_json(&data),
                OutputFormat::Pretty => {
                    if let Some(list) = data["topic_list"].as_array() {
                        let items: Vec<&Value> = list.iter().take(limit).collect();
                        if items.is_empty() {
                            println!("No post results.");
                            return Ok(());
                        }
                        let headers = ["id", "author", "excerpt"];
                        let rows: Vec<Vec<String>> = items
                            .iter()
                            .map(|item| {
                                let excerpt: String =
                                    val_str(&item["content"]).chars().take(60).collect();
                                vec![val_str(&item["id"]), val_str(&item["author_name"]), excerpt]
                            })
                            .collect();
                        print_table(&headers, rows, &OutputFormat::Pretty);
                    } else {
                        print_json(&data);
                    }
                }
            }
        }
        "hashtags" => {
            let body = json!({ "k": keyword });
            let data = http_post("/v2/search/hashtag", body, verbose).await?;
            match format {
                OutputFormat::Json => print_json(&data),
                OutputFormat::Pretty => {
                    if let Some(list) = data["hashtag_list"].as_array() {
                        let items: Vec<&Value> = list.iter().take(limit).collect();
                        if items.is_empty() {
                            println!("No hashtag results.");
                            return Ok(());
                        }
                        let headers = ["id", "name", "topic_count"];
                        let rows: Vec<Vec<String>> = items
                            .iter()
                            .map(|item| {
                                vec![
                                    val_str(&item["id"]),
                                    val_str(&item["name"]),
                                    val_str(&item["topic_count"]),
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
        "help" => {
            let limit_str = limit.to_string();
            let body = json!({ "k": keyword, "limit": limit_str });
            let data = http_post("/v1/helpcenter/search_main", body, verbose).await?;
            match format {
                OutputFormat::Json => print_json(&data),
                OutputFormat::Pretty => {
                    if let Some(list) = data["help_topic_list"].as_array() {
                        if list.is_empty() {
                            println!("No help results.");
                            return Ok(());
                        }
                        let headers = ["id", "title"];
                        let rows: Vec<Vec<String>> = list
                            .iter()
                            .take(limit)
                            .map(|item| vec![val_str(&item["id"]), val_str(&item["title"])])
                            .collect();
                        print_table(&headers, rows, &OutputFormat::Pretty);
                    } else {
                        print_json(&data);
                    }
                }
            }
        }
        "share-lists" => {
            let body = json!({ "k": keyword, "id": "" });
            let data = http_post("/v1/search/share_lists", body, verbose).await?;
            match format {
                OutputFormat::Json => print_json(&data),
                OutputFormat::Pretty => {
                    if let Some(list) = data["share_list"].as_array() {
                        let items: Vec<&Value> = list.iter().take(limit).collect();
                        if items.is_empty() {
                            println!("No share-list results.");
                            return Ok(());
                        }
                        let headers = ["id", "name", "author", "stock_count"];
                        let rows: Vec<Vec<String>> = items
                            .iter()
                            .map(|item| {
                                vec![
                                    val_str(&item["id"]),
                                    val_str(&item["name"]),
                                    val_str(&item["author_name"]),
                                    val_str(&item["stock_count"]),
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
        "users" => {
            let body = json!({ "k": keyword, "id": "" });
            let data = http_post("/v1/search/social_users", body, verbose).await?;
            match format {
                OutputFormat::Json => print_json(&data),
                OutputFormat::Pretty => {
                    if let Some(list) = data["user_list"].as_array() {
                        let items: Vec<&Value> = list.iter().take(limit).collect();
                        if items.is_empty() {
                            println!("No user results.");
                            return Ok(());
                        }
                        let headers = ["id", "name", "followers"];
                        let rows: Vec<Vec<String>> = items
                            .iter()
                            .map(|item| {
                                vec![
                                    val_str(&item["id"]),
                                    val_str(&item["name"]),
                                    val_str(&item["followers_count"]),
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
        "institutions" => {
            let body = json!({ "k": keyword });
            let data = http_post("/v1/search/social_institutions", body, verbose).await?;
            match format {
                OutputFormat::Json => print_json(&data),
                OutputFormat::Pretty => {
                    if let Some(list) = data["institution_list"].as_array() {
                        let items: Vec<&Value> = list.iter().take(limit).collect();
                        if items.is_empty() {
                            println!("No institution results.");
                            return Ok(());
                        }
                        let headers = ["id", "name", "followers"];
                        let rows: Vec<Vec<String>> = items
                            .iter()
                            .map(|item| {
                                vec![
                                    val_str(&item["id"]),
                                    val_str(&item["name"]),
                                    val_str(&item["followers_count"]),
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

// ── search hot words ──────────────────────────────────────────────────────────

/// Fetch hot search words.
pub async fn cmd_search_hot(format: &OutputFormat, verbose: bool) -> Result<()> {
    let data = http_get("/search/gethotwords", &[], verbose).await?;
    match format {
        OutputFormat::Json => print_json(&data),
        OutputFormat::Pretty => {
            // Try common response shapes for hot words
            let list = data["hot_words"]
                .as_array()
                .or_else(|| data["list"].as_array())
                .or_else(|| data["words"].as_array());
            if let Some(words) = list {
                if words.is_empty() {
                    println!("No hot words.");
                    return Ok(());
                }
                for (i, w) in words.iter().enumerate() {
                    let word = if w.is_string() {
                        val_str(w)
                    } else {
                        val_str(&w["word"])
                    };
                    println!("{}. {word}", i + 1);
                }
            } else {
                print_json(&data);
            }
        }
    }
    Ok(())
}
