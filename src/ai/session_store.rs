//! History for `longbridge ai` chats, backed by the account's server-side
//! conversations.
//!
//! Rather than a local file store, this reads the shared `/v1/ai/chats`
//! endpoints (via [`crate::openapi::chats`]) so History shows the same
//! conversations as the web and other clients, and resuming one continues the
//! server's thread. Both operations are network calls; the TUI runs them off
//! the event loop and delivers the results over a channel.

use super::state::{ChatState, Message, Role};

/// A one-line summary for the History list.
pub struct SessionSummary {
    /// Server chat uid.
    pub id: String,
    /// Seconds since the Unix epoch; used for ordering and the age subtitle.
    pub updated_at: u64,
    pub title: String,
    pub agent: String,
}

/// A fully loaded conversation, ready to restore into [`ChatState`].
pub struct LoadedChat {
    pub agent_uid: String,
    pub chat_uid: String,
    pub message_id: Option<String>,
    pub title: Option<String>,
    pub messages: Vec<Message>,
}

/// List the account's chats, newest first. `None` means the request failed
/// (as opposed to an account with no conversations).
pub async fn list_summaries() -> Option<Vec<SessionSummary>> {
    let resp = crate::openapi::chats::list_chats(1, 50, None).await.ok()?;
    let mut sessions: Vec<SessionSummary> = resp
        .chats
        .into_iter()
        .map(|c| SessionSummary {
            title: if c.name.trim().is_empty() {
                rust_i18n::t!("Ai.UntitledSession").to_string()
            } else {
                c.name
            },
            // An agent is shown by name only — its uid is an internal handle
            // that never surfaces. So an unnamed default agent falls back to
            // the product name, and any other unnamed one to nothing at all.
            agent: if !c.agent_name.is_empty() {
                c.agent_name
            } else if c.agent_uid == crate::cli::agent::DEFAULT_AGENT_UID {
                rust_i18n::t!("Ai.Assistant").to_string()
            } else {
                String::new()
            },
            updated_at: u64::try_from(c.updated_at).unwrap_or(0),
            id: c.uid,
        })
        .collect();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
    Some(sessions)
}

/// Load one chat's full message history for resuming.
pub async fn load_detail(uid: &str) -> Option<LoadedChat> {
    let detail = crate::openapi::chats::chat_detail(uid).await.ok()?;
    let agent_uid = detail
        .messages
        .iter()
        .map(|m| m.agent_uid.clone())
        .find(|a| !a.is_empty())
        .unwrap_or_default();
    let message_id = detail
        .messages
        .last()
        .filter(|m| m.id != 0)
        .map(|m| m.id.to_string());
    let messages = detail
        .messages
        .iter()
        .filter_map(|m| {
            let text = m.text();
            if text.trim().is_empty() {
                return None;
            }
            let role = if m.sender == "user" {
                Role::User
            } else {
                Role::Assistant
            };
            Some(Message::new(role, text))
        })
        .collect();
    let title = (!detail.chat.name.trim().is_empty()).then(|| detail.chat.name.clone());
    Some(LoadedChat {
        agent_uid,
        chat_uid: uid.to_string(),
        message_id,
        title,
        messages,
    })
}

/// Restore a loaded conversation into `state`, ready for follow-ups.
pub fn restore(loaded: LoadedChat, state: &mut ChatState) {
    if !loaded.agent_uid.is_empty() {
        state.agent_uid = loaded.agent_uid;
    }
    state.title = loaded.title;
    state.chat_uid = Some(loaded.chat_uid);
    state.message_id = loaded.message_id;
    state.pending_interrupt = None;
    state.scroll = 0;
    state.references.clear();
    state.further.clear();
    state.messages = loaded.messages;
}
