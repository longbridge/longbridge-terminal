//! `agent chats`: list the account's chats (conversations) across agents.
//! `agent chat-detail`: fetch one chat's detail, including its messages.

use anyhow::{Context, Result};

use super::render::strip_control_chars;
use crate::cli::output::{fmt_unix_ts, print_json_value, print_table};
use crate::cli::OutputFormat;

/// `agent chats` — GET /v1/ai/chats.
pub async fn cmd_chats(
    page: u32,
    count: u32,
    exclude_agent_uids: Option<String>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    if verbose {
        eprintln!("* GET /v1/ai/chats");
    }
    let mut opts = longbridge::agent::GetChatsOptions::new()
        .page(page as i32)
        .limit(count as i32);
    if let Some(exclude) = exclude_agent_uids {
        opts = opts.exclude_agent_uids(exclude);
    }
    let resp = crate::openapi::global_rate_limiter()
        .execute("agent_chats", || {
            let opts = opts.clone();
            Box::pin(async move { crate::openapi::agent().chats(opts).await })
        })
        .await
        .context("Failed to list AI chats")?;

    match format {
        OutputFormat::Json => print_json_value(&serde_json::to_value(&resp)?, format),
        OutputFormat::Pretty => {
            let rows = resp
                .chats
                .iter()
                .map(|c| {
                    vec![
                        strip_control_chars(&c.uid),
                        strip_control_chars(&c.name),
                        strip_control_chars(&c.agent_name),
                        fmt_unix_ts(c.updated_at),
                    ]
                })
                .collect();
            print_table(&["UID", "NAME", "AGENT", "UPDATED_AT"], rows, format);
        }
    }
    Ok(())
}

/// `agent chat-detail <CHAT_UID>` — GET /v1/ai/chats/{chat_uid}.
pub async fn cmd_chat_detail(chat_uid: String, format: &OutputFormat, verbose: bool) -> Result<()> {
    if verbose {
        eprintln!("* GET /v1/ai/chats/{chat_uid}");
    }
    let resp = crate::openapi::global_rate_limiter()
        .execute("agent_chat_detail", || {
            let chat_uid = chat_uid.clone();
            Box::pin(async move { crate::openapi::agent().chat(chat_uid).await })
        })
        .await
        .context("Failed to get AI chat detail")?;

    match format {
        OutputFormat::Json => print_json_value(&serde_json::to_value(&resp)?, format),
        OutputFormat::Pretty => {
            eprintln!(
                "chat {} — {}",
                strip_control_chars(&resp.chat.uid),
                strip_control_chars(&resp.chat.name)
            );
            let rows = resp
                .messages
                .iter()
                .map(|m| {
                    // Join the text chunks into a single, single-line preview.
                    let text: String = m
                        .chunks
                        .iter()
                        .map(|ch| ch.content.as_str())
                        .collect::<Vec<_>>()
                        .join(" ");
                    vec![
                        m.id.to_string(),
                        strip_control_chars(&m.sender),
                        strip_control_chars(&text),
                        fmt_unix_ts(m.created_at),
                    ]
                })
                .collect();
            print_table(&["ID", "SENDER", "CONTENT", "CREATED_AT"], rows, format);
        }
    }
    Ok(())
}
