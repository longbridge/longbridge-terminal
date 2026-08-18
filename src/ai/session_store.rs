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
    /// The last message the server can build on — see [`continuable_parent`].
    pub parent_message_id: Option<String>,
    pub title: Option<String>,
    pub messages: Vec<Message>,
    /// HITL payload carried by the last paused message, if any.
    pub pending_interrupt: Option<serde_json::Value>,
}

/// List the account's chats, newest first. `None` means the request failed
/// (as opposed to an account with no conversations).
pub async fn list_summaries() -> Option<Vec<SessionSummary>> {
    if !crate::openapi::is_ready() {
        return None;
    }
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
    if !crate::openapi::is_ready() {
        return None;
    }
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
    let parent_message_id = continuable_parent(&detail.messages);
    let pending_interrupt = pending_interrupt(&detail.messages);
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
        parent_message_id,
        title,
        messages,
        pending_interrupt,
    })
}

/// Recover the pending HITL payload stored in an interrupted history message.
/// Web clients rebuild their confirmation card from this `interrupt` chunk;
/// doing the same keeps Allow / Decline available after reopening a session.
fn pending_interrupt(messages: &[crate::openapi::chats::ChatMessage]) -> Option<serde_json::Value> {
    const INTERRUPTED: i32 = 5;
    // A confirmation is resumable only while it is the current end state of
    // the conversation. Searching backwards resurrects an older confirmation
    // after newer turns have already run, then submits its interrupt id against
    // the newest message id.
    let message = messages
        .last()
        .filter(|message| message.id != 0 && message.status == INTERRUPTED)?;
    message
        .chunks
        .iter()
        .rev()
        .filter(|chunk| chunk.chunk_type == "interrupt")
        .filter_map(|chunk| serde_json::from_str::<serde_json::Value>(&chunk.content).ok())
        .find(|payload| {
            payload.get("status").and_then(serde_json::Value::as_str) != Some("completed")
                && payload
                    .get("interactions")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|items| !items.is_empty())
        })
}

/// The last message a new turn can be parented on: the most recent one the
/// server marked *finished*.
///
/// A message left waiting for the reader to answer a clarifying question, or one
/// that errored, is not a valid parent — the server rejects the request with a bare
/// "Something went wrong. Please try again.". That is what bricked a conversation
/// the reader quit while it was asking something: on resume every later message was
/// parented on the paused one and failed, forever. Branching from the last finished
/// message instead continues the conversation (verified against the API), and the
/// question is simply re-asked if the agent still needs it.
fn continuable_parent(messages: &[crate::openapi::chats::ChatMessage]) -> Option<String> {
    /// The server's message status for a run that completed. The other observed
    /// values are 4 (failed, with `error_code` set) and 5 (interrupted, waiting for
    /// human input); neither can be continued from.
    const FINISHED: i32 = 1;
    messages
        .iter()
        .rev()
        .find(|m| m.id != 0 && m.status == FINISHED)
        .map(|m| m.id.to_string())
}

/// Restore a loaded conversation into `state`, ready for follow-ups.
pub fn restore(loaded: LoadedChat, state: &mut ChatState) {
    if !loaded.agent_uid.is_empty() {
        state.agent_uid = loaded.agent_uid;
    }
    state.title = loaded.title;
    state.chat_uid = Some(loaded.chat_uid);
    state.message_id = loaded.message_id;
    // Not necessarily the last message: a conversation can end paused or failed,
    // and neither is something the server will build on.
    state.parent_message_id = loaded.parent_message_id;
    state.pending_interrupt = loaded.pending_interrupt;
    state.turn_error = None;
    state.scroll = 0;
    state.references.clear();
    state.further.clear();
    // Drop any transient turn state so a restored conversation never inherits a
    // half-streamed answer, a stale status/spinner, or queued prompts.
    state.streaming = None;
    state.status.clear();
    state.busy = false;
    state.queued.clear();
    state.tool_failures.clear();
    state.messages = loaded.messages;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openapi::chats::ChatMessage;

    fn msg(id: i64, status: i32) -> ChatMessage {
        ChatMessage {
            id,
            status,
            ..ChatMessage::default()
        }
    }

    /// The bug this exists for: a conversation the reader quit while the agent was
    /// asking them something ends on a paused message, and parenting the next turn
    /// on it fails with a bare "Something went wrong" — every time, forever.
    #[test]
    fn a_paused_last_message_is_not_a_parent() {
        // 1 finished, 5 interrupted.
        let messages = [msg(10, 1), msg(11, 1), msg(12, 5)];
        assert_eq!(continuable_parent(&messages).as_deref(), Some("11"));
    }

    #[test]
    fn a_failed_last_message_is_not_a_parent_either() {
        let messages = [msg(10, 1), msg(11, 4)];
        assert_eq!(continuable_parent(&messages).as_deref(), Some("10"));
    }

    #[test]
    fn an_ordinary_conversation_continues_from_its_last_message() {
        let messages = [msg(10, 1), msg(11, 1)];
        assert_eq!(continuable_parent(&messages).as_deref(), Some("11"));
    }

    /// Nothing to build on: the request goes out without a parent rather than with
    /// one the server will reject.
    #[test]
    fn nothing_continuable_means_no_parent() {
        assert_eq!(continuable_parent(&[]), None);
        assert_eq!(continuable_parent(&[msg(10, 5)]), None);
        // An id of 0 is a placeholder, not a message.
        assert_eq!(continuable_parent(&[msg(0, 1)]), None);
    }

    #[test]
    fn a_paused_messages_interrupt_is_restored_from_history() {
        let mut paused = msg(12, 5);
        paused.chunks.push(crate::openapi::chats::ChatMessageChunk {
            chunk_type: "interrupt".into(),
            content: serde_json::json!({
                "status": "pending",
                "interactions": [{
                    "interrupt_id": "authorize_watchlist",
                    "type": "authorization",
                    "tool_display_name": "Read watchlist"
                }]
            })
            .to_string(),
            ..Default::default()
        });

        let interrupt = pending_interrupt(&[paused]).expect("pending interrupt");
        assert_eq!(
            interrupt["interactions"][0]["interrupt_id"],
            "authorize_watchlist"
        );
    }

    #[test]
    fn an_old_pause_is_not_restored_after_a_newer_message() {
        let mut paused = msg(12, 5);
        paused.chunks.push(crate::openapi::chats::ChatMessageChunk {
            chunk_type: "interrupt".into(),
            content: serde_json::json!({
                "status": "pending",
                "interactions": [{
                    "interrupt_id": "old_watchlist_authorization",
                    "type": "authorization"
                }]
            })
            .to_string(),
            ..Default::default()
        });

        assert!(pending_interrupt(&[paused, msg(13, 1)]).is_none());
    }

    #[test]
    fn restore_keeps_a_pending_confirmation_answerable() {
        let interrupt = serde_json::json!({
            "interactions": [{
                "interrupt_id": "authorize_watchlist",
                "type": "authorization"
            }]
        });
        let loaded = LoadedChat {
            agent_uid: "agent".into(),
            chat_uid: "chat".into(),
            message_id: Some("12".into()),
            parent_message_id: Some("11".into()),
            title: None,
            messages: Vec::new(),
            pending_interrupt: Some(interrupt.clone()),
        };
        let mut state = ChatState::default();
        restore(loaded, &mut state);
        assert_eq!(state.pending_interrupt, Some(interrupt));
    }
}
