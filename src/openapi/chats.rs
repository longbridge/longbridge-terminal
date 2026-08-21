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
    /// The message's visible text: only the `text`-type chunks joined together.
    ///
    /// A message can also carry `process` (thinking) and `tool_use` chunks whose
    /// `content` is internal JSON, not display text; those are excluded here so
    /// the answer is just the answer. The reasoning is read back separately, by
    /// [`Self::reasoning`].
    pub fn text(&self) -> String {
        self.chunks
            .iter()
            .filter(|c| c.chunk_type == "text")
            .map(|c| c.content.as_str())
            .collect::<Vec<_>>()
            .join("")
    }

    /// The reasoning the agent recorded for this message.
    ///
    /// Lives in the `process` chunks as `{"message": "…"}`. A message can hold
    /// several — the agent thinks, answers a little, thinks again — and they are
    /// joined in order, the same flattening [`Self::text`] already applies to the
    /// answer itself.
    pub fn reasoning(&self) -> String {
        self.chunks
            .iter()
            .filter(|c| c.chunk_type == "process")
            .filter_map(|c| serde_json::from_str::<serde_json::Value>(&c.content).ok())
            .filter_map(|v| {
                v.get("message")
                    .and_then(serde_json::Value::as_str)
                    .map(crate::utils::text::strip_control_chars)
            })
            .filter(|m| !m.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// True while the server is still generating this message.
    ///
    /// `status` is the shared message-status enum: `0` generating, `1`
    /// completed, `2` stopped by the user, `3` exception, `4` blocked by the
    /// guard rails, `5` paused for human input. Anything but `0` is terminal —
    /// the run is over and this record is the whole of it.
    pub fn is_generating(&self) -> bool {
        self.status == 0
    }

    /// One entry of the `extends` list, parsed from its JSON-string `value`.
    ///
    /// `extends` is a list of `{key, value, created_at}` whose `value` is itself
    /// a JSON document encoded as a string. It holds the end-of-turn metadata
    /// that the SSE stream delivers as typed events, persisted for anyone
    /// reading the message back afterwards.
    fn extends_entry(&self, key: &str) -> Option<serde_json::Value> {
        self.extends
            .as_array()?
            .iter()
            .find(|e| e.get("key").and_then(serde_json::Value::as_str) == Some(key))?
            .get("value")
            .and_then(serde_json::Value::as_str)
            .and_then(|raw| serde_json::from_str(raw).ok())
    }

    /// The sources the answer cites, as persisted in `extends`.
    pub fn references(&self) -> Vec<longbridge::agent::Reference> {
        self.extends_entry("references")
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }

    /// The follow-up questions the agent suggested, as persisted in `extends`.
    pub fn further_questions(&self) -> Vec<String> {
        self.extends_entry("further_questions")
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::{ChatMessage, ChatMessageChunk};

    fn chunk(kind: &str, content: &str) -> ChatMessageChunk {
        ChatMessageChunk {
            chunk_type: kind.into(),
            content: content.into(),
            ..Default::default()
        }
    }

    /// A stored message keeps its reasoning in `process` chunks, and the answer
    /// in `text` ones. Reading a chat back has to tell them apart, or the
    /// reasoning is either lost or spliced into the answer.
    #[test]
    fn reasoning_and_answer_are_read_back_separately() {
        let m = ChatMessage {
            sender: "assistant".into(),
            thinking_seconds: 4,
            chunks: vec![
                chunk("process", r#"{"message":"Weighing the data."}"#),
                chunk("text", "NVDA is up."),
                chunk("tool_use", r#"{"status":"succeeded"}"#),
                chunk("process", r#"{"message":"One more check."}"#),
                chunk("text", " Details follow."),
            ],
            ..Default::default()
        };
        assert_eq!(m.text(), "NVDA is up. Details follow.");
        assert_eq!(m.reasoning(), "Weighing the data.\n\nOne more check.");
    }

    /// A message that never reasoned must not claim it did, and a `process`
    /// chunk we cannot read is skipped rather than shown as raw JSON.
    #[test]
    fn unreadable_or_absent_reasoning_yields_nothing() {
        let none = ChatMessage {
            chunks: vec![chunk("text", "hi")],
            ..Default::default()
        };
        assert!(none.reasoning().is_empty());
        let broken = ChatMessage {
            chunks: vec![
                chunk("process", "not json"),
                chunk("process", r#"{"other":"field"}"#),
                chunk("process", r#"{"message":"   "}"#),
            ],
            ..Default::default()
        };
        assert!(broken.reasoning().is_empty());
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
