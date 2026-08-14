//! `agent chats`: list the account's chats (conversations) across agents.
//! `agent chat-detail`: fetch one chat's detail, including its messages.
//!
//! Both are thin renderers over the shared [`crate::openapi::chats`] client.

use anyhow::Result;

use crate::cli::output::{fmt_unix_ts, print_json_value, print_table};
use crate::cli::OutputFormat;
use crate::utils::text::strip_control_chars;

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
    let resp = crate::openapi::chats::list_chats(page, count, exclude_agent_uids).await?;

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

/// `agent chat-detail <CHAT_UID>` — `GET /v1/ai/chats/{chat_uid}`.
pub async fn cmd_chat_detail(chat_uid: String, format: &OutputFormat, verbose: bool) -> Result<()> {
    if verbose {
        eprintln!("* GET /v1/ai/chats/{chat_uid}");
    }
    let resp = crate::openapi::chats::chat_detail(&chat_uid).await?;

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
                    vec![
                        m.id.to_string(),
                        strip_control_chars(&m.sender),
                        strip_control_chars(&m.text()),
                        fmt_unix_ts(m.created_at),
                    ]
                })
                .collect();
            print_table(&["ID", "SENDER", "CONTENT", "CREATED_AT"], rows, format);
        }
    }
    Ok(())
}
