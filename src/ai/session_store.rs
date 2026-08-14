//! On-disk history for `longbridge ai` chat sessions.
//!
//! A lightweight analog of grok-build's `xai-chat-state::persistence`: each
//! conversation is saved as one JSON file under the user's config directory, so
//! the Sessions view can list past chats and resume them. The `ai` TUI drives
//! the A2A streaming path directly, so it owns this history rather than reusing
//! the ACP backend's store.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::state::{ChatState, Message, Role};

/// One persisted message (role + text), independent of the in-memory `Message`.
#[derive(Serialize, Deserialize)]
struct StoredMessage {
    role: StoredRole,
    text: String,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
enum StoredRole {
    User,
    Assistant,
    System,
}

/// A full saved conversation.
#[derive(Serialize, Deserialize)]
pub struct StoredSession {
    pub id: String,
    /// Seconds since the Unix epoch; used for ordering and display.
    pub updated_at: u64,
    pub agent_uid: String,
    /// Server-generated conversation title, if one was received.
    #[serde(default)]
    pub title: Option<String>,
    pub chat_uid: Option<String>,
    pub message_id: Option<String>,
    messages: Vec<StoredMessage>,
}

/// A one-line summary for the Sessions list.
pub struct SessionSummary {
    pub id: String,
    pub updated_at: u64,
    pub title: String,
    pub agent: String,
}

fn dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("longbridge").join("ai-sessions"))
}

/// List saved sessions, newest first.
#[must_use]
pub fn list() -> Vec<SessionSummary> {
    let Some(dir) = dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut sessions: Vec<SessionSummary> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|bytes| serde_json::from_slice::<StoredSession>(&bytes).ok())
        .map(|s| SessionSummary {
            title: summarize(&s),
            agent: s.agent_uid,
            id: s.id,
            updated_at: s.updated_at,
        })
        .collect();
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    sessions
}

/// Load one session by id.
#[must_use]
pub fn load(id: &str) -> Option<StoredSession> {
    let path = dir()?.join(format!("{}.json", sanitize(id)));
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Persist the current chat state under its session id (creating the directory
/// on first save). A best-effort operation — failures are ignored so the UI
/// never blocks on disk.
pub fn save(id: &str, now: u64, state: &ChatState) {
    let Some(dir) = dir() else { return };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let stored = StoredSession {
        id: id.to_string(),
        updated_at: now,
        agent_uid: state.agent_uid.clone(),
        title: state.title.clone(),
        chat_uid: state.chat_uid.clone(),
        message_id: state.message_id.clone(),
        messages: state
            .messages
            .iter()
            .map(|m| StoredMessage {
                role: match m.role {
                    Role::User => StoredRole::User,
                    Role::Assistant => StoredRole::Assistant,
                    Role::System => StoredRole::System,
                },
                text: m.text.clone(),
            })
            .collect(),
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(&stored) {
        let _ = std::fs::write(dir.join(format!("{}.json", sanitize(id))), bytes);
    }
}

/// Delete one saved session by id. Returns whether a file was removed.
pub fn delete(id: &str) -> bool {
    dir()
        .map(|d| d.join(format!("{}.json", sanitize(id))))
        .is_some_and(|path| std::fs::remove_file(path).is_ok())
}

/// Delete every saved session. Best-effort: unreadable entries are skipped.
pub fn clear() {
    let Some(dir) = dir() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        if entry.path().extension().is_some_and(|x| x == "json") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Restore the persisted messages and conversation IDs into `state`.
pub fn restore(session: StoredSession, state: &mut ChatState) {
    state.agent_uid = session.agent_uid;
    state.title = session.title;
    state.chat_uid = session.chat_uid;
    state.message_id = session.message_id;
    state.pending_interrupt = None;
    state.scroll = 0;
    state.messages = session
        .messages
        .into_iter()
        .map(|m| Message {
            role: match m.role {
                StoredRole::User => Role::User,
                StoredRole::Assistant => Role::Assistant,
                StoredRole::System => Role::System,
            },
            text: m.text,
        })
        .collect();
}

/// The conversation's title: the server-generated one if present, otherwise
/// the first line of the first user message, otherwise a placeholder.
fn summarize(session: &StoredSession) -> String {
    if let Some(title) = session.title.as_ref().filter(|t| !t.trim().is_empty()) {
        return title.clone();
    }
    session
        .messages
        .iter()
        .find(|m| matches!(m.role, StoredRole::User))
        .map(|m| {
            let line = m.text.lines().next().unwrap_or_default();
            line.chars().take(60).collect::<String>()
        })
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| rust_i18n::t!("Ai.UntitledSession").to_string())
}

/// Keep a session id safe as a filename component.
fn sanitize(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
