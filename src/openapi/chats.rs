//! Shared client for the AI chat-history REST endpoints.
//!
//! `GET /v1/ai/chats` and `GET /v1/ai/chats/{chat_uid}` are **not** part of the
//! language SDKs; they are called here directly through the SDK's signed HTTP
//! client ([`crate::openapi::http_client`]). This module is the single place
//! the ACP session backend and the `agent` CLI commands (and future `ai`
//! commands) share for reading server-side chat history.

use anyhow::{Context, Result};
use longbridge::httpclient::{Json, Method};
use serde::{Deserialize, Serialize};

/// A chat (conversation) with an Agent, as returned by [`list_chats`].
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Chat {
    pub id: i64,
    pub uid: String,
    pub name: String,
    pub agent_id: i64,
    pub agent_name: String,
    pub agent_uid: String,
    pub from_source: String,
    pub has_unread: bool,
    pub created_at: i64,
    pub updated_at: i64,
    /// Agent / permission relation metadata, kept as raw JSON.
    pub chat_relation: serde_json::Value,
}

/// Response for [`list_chats`].
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ChatsResponse {
    pub chats: Vec<Chat>,
}

/// One content chunk of a [`ChatMessage`].
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ChatMessageChunk {
    pub chunk_type: String,
    pub content: String,
    pub index: i32,
    pub started_at: i64,
    pub stopped_at: i64,
}

/// A message within a chat.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ChatMessage {
    pub id: i64,
    pub chat_id: i64,
    pub chat_uid: String,
    pub agent_id: i64,
    pub agent_name: String,
    pub agent_uid: String,
    /// Sender, e.g. `user` or `assistant`.
    pub sender: String,
    pub status: i32,
    pub likes: i32,
    pub parent_message_id: i64,
    pub thinking_seconds: i32,
    pub error_code: i32,
    pub workflow_run_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub chunks: Vec<ChatMessageChunk>,
    /// Extension payload, kept as raw JSON.
    pub extends: serde_json::Value,
}

/// Chat summary carried in the [`ChatDetail`] response.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ChatInfo {
    pub id: i64,
    pub name: String,
    pub uid: String,
}

/// Response for [`chat_detail`].
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ChatDetail {
    pub chat: ChatInfo,
    /// Agent / permission relation metadata, kept as raw JSON.
    pub chat_relation: serde_json::Value,
    pub messages: Vec<ChatMessage>,
}

impl ChatMessage {
    /// Join the message's text chunks into a single string.
    pub fn text(&self) -> String {
        self.chunks
            .iter()
            .map(|c| c.content.as_str())
            .collect::<Vec<_>>()
            .join("")
    }
}

/// `GET /v1/ai/chats` — list the account's chats (conversations) across Agents.
pub async fn list_chats(
    page: u32,
    limit: u32,
    exclude_agent_uids: Option<String>,
) -> Result<ChatsResponse> {
    let resp = crate::openapi::global_rate_limiter()
        .execute("agent_chats", || {
            let exclude_agent_uids = exclude_agent_uids.clone();
            Box::pin(async move {
                #[derive(Serialize)]
                struct Query {
                    page: u32,
                    limit: u32,
                    #[serde(skip_serializing_if = "Option::is_none")]
                    exclude_agent_uids: Option<String>,
                }
                crate::openapi::http_client()
                    .request(Method::GET, "/v1/ai/chats")
                    .query_params(Query {
                        page,
                        limit,
                        exclude_agent_uids,
                    })
                    .response::<Json<ChatsResponse>>()
                    .send()
                    .await
                    .map(|json| json.0)
            })
        })
        .await
        .context("Failed to list AI chats")?;
    Ok(resp)
}

/// `GET /v1/ai/chats/{chat_uid}` — a single chat's detail, including messages.
pub async fn chat_detail(chat_uid: &str) -> Result<ChatDetail> {
    let path = format!("/v1/ai/chats/{chat_uid}");
    let resp = crate::openapi::global_rate_limiter()
        .execute("agent_chat_detail", || {
            let path = path.clone();
            Box::pin(async move {
                crate::openapi::http_client()
                    .request(Method::GET, path)
                    .response::<Json<ChatDetail>>()
                    .send()
                    .await
                    .map(|json| json.0)
            })
        })
        .await
        .context("Failed to get AI chat detail")?;
    Ok(resp)
}
