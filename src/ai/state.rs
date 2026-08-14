//! Chat state for the `longbridge ai` TUI.
//!
//! Modeled on grok-build's `xai-chat-state`: a single state snapshot mutated by
//! a stream of typed [`ChatEvent`]s. The turn task produces events; the view
//! renders the snapshot. Keeping mutation in one `apply` method (rather than
//! scattered across the UI loop) is what lets the view stay a pure function of
//! state.

use longbridge::agent::Reference;
use serde_json::Value;

/// Who authored a transcript line.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    System,
    /// A tool the agent called, recorded so the answer can be traced back to
    /// the data it was built from.
    Tool,
}

/// How a tool call ended, for the transcript's tool lines.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolStatus {
    Running,
    Ok,
    Failed,
}

pub struct Message {
    pub role: Role,
    pub text: String,
    /// Set only on [`Role::Tool`] lines.
    pub tool: Option<ToolStatus>,
}

impl Message {
    /// A plain transcript line from `role`.
    pub fn new(role: Role, text: String) -> Self {
        Self {
            role,
            text,
            tool: None,
        }
    }

    /// A tool line naming the tool and how its call is going.
    pub fn tool(name: String, status: ToolStatus) -> Self {
        Self {
            role: Role::Tool,
            text: name,
            tool: Some(status),
        }
    }
}

/// A typed mutation of [`ChatState`], emitted by the running turn (see
/// `runtime::map_agent_event`) or by user input.
pub enum ChatEvent {
    /// The user submitted a prompt; append it and enter the busy state.
    UserPrompt(String),
    /// Conversation identity from the first stream event, used to thread
    /// follow-ups.
    TurnStarted {
        chat_uid: String,
        message_id: String,
    },
    /// An incremental chunk of the assistant's answer.
    Delta(String),
    /// A transient status line (thinking, calling a tool, generating).
    Status(String),
    /// The agent started calling a tool; appends a running tool line.
    ToolStarted(String),
    /// A tool finished; resolves its line and records failures so an empty turn
    /// can explain itself.
    ToolFinished { name: String, ok: bool },
    /// The server auto-generated a title for this conversation.
    Title(String),
    /// The agent paused to ask the user something; the next prompt answers it.
    Interrupt(Value),
    /// End-of-turn metadata (source references / suggested follow-ups) rendered
    /// as interactive chips rather than folded into the answer text.
    Meta {
        references: Vec<Reference>,
        further: Vec<String>,
    },
    /// The turn ended. Finalizes the streamed answer into a message.
    TurnFinished { error: Option<String> },
}

/// The full chat state the view renders.
#[derive(Default)]
pub struct ChatState {
    pub agent_uid: String,
    pub messages: Vec<Message>,
    /// The assistant answer accumulating during the active turn.
    pub streaming: Option<String>,
    pub status: String,
    pub busy: bool,
    /// Lines scrolled up from the bottom (0 = pinned to the latest).
    pub scroll: u16,
    /// Server-generated conversation title, shown in History when present.
    pub title: Option<String>,
    /// Longbridge conversation IDs of the latest turn (for follow-ups).
    pub chat_uid: Option<String>,
    pub message_id: Option<String>,
    /// Set when the last turn ended asking a question; the next prompt answers.
    pub pending_interrupt: Option<Value>,
    /// Tools that failed during the active turn.
    pub tool_failures: Vec<String>,
    /// Source references from the latest completed turn (rendered as chips).
    pub references: Vec<Reference>,
    /// Suggested follow-up questions from the latest turn (click to send).
    pub further: Vec<String>,
}

impl ChatState {
    pub fn new(agent_uid: String, welcome: String) -> Self {
        Self {
            agent_uid,
            messages: vec![Message::new(Role::System, welcome)],
            ..Self::default()
        }
    }

    /// Apply one event, mutating the snapshot. This is the single place chat
    /// state changes.
    pub fn apply(&mut self, event: ChatEvent) {
        match event {
            ChatEvent::UserPrompt(text) => {
                self.messages.push(Message::new(Role::User, text));
                self.scroll = 0;
                self.busy = true;
                self.streaming = Some(String::new());
                self.tool_failures.clear();
                self.references.clear();
                self.further.clear();
            }
            ChatEvent::TurnStarted {
                chat_uid,
                message_id,
            } => {
                self.chat_uid = Some(chat_uid);
                self.message_id = Some(message_id);
            }
            ChatEvent::Delta(text) => {
                self.streaming
                    .get_or_insert_with(String::new)
                    .push_str(&text);
            }
            ChatEvent::Status(status) => self.status = status,
            // A tool call is committed to the transcript rather than only
            // flashing through the status line: for a finance agent, which data
            // an answer was built from is part of the answer.
            ChatEvent::ToolStarted(name) => {
                self.messages.push(Message::tool(name, ToolStatus::Running));
            }
            ChatEvent::ToolFinished { name, ok } => {
                // Resolve the most recent running line for this tool. The agent
                // may call the same tool more than once in a turn, so match on
                // name and take the latest still-running one.
                if let Some(msg) = self.messages.iter_mut().rev().find(|m| {
                    m.role == Role::Tool && m.text == name && m.tool == Some(ToolStatus::Running)
                }) {
                    msg.tool = Some(if ok {
                        ToolStatus::Ok
                    } else {
                        ToolStatus::Failed
                    });
                } else {
                    // No start was seen (a reconnect mid-turn, say); still record
                    // that the tool ran rather than dropping it.
                    let status = if ok {
                        ToolStatus::Ok
                    } else {
                        ToolStatus::Failed
                    };
                    self.messages.push(Message::tool(name.clone(), status));
                }
                if !ok {
                    self.tool_failures.push(name);
                }
            }
            ChatEvent::Title(title) => {
                if !title.trim().is_empty() {
                    self.title = Some(title);
                }
            }
            ChatEvent::Interrupt(interrupt) => self.pending_interrupt = Some(interrupt),
            ChatEvent::Meta {
                references,
                further,
            } => {
                self.references = references;
                self.further = further;
            }
            ChatEvent::TurnFinished { error } => self.finish_turn(error),
        }
    }

    fn finish_turn(&mut self, error: Option<String>) {
        let produced = self
            .streaming
            .take()
            .filter(|t| !t.trim().is_empty())
            .map(|text| {
                self.messages.push(Message::new(Role::Assistant, text));
            })
            .is_some();
        if let Some(err) = error {
            self.messages
                .push(Message::new(Role::System, format!("[error] {err}")));
        } else if !produced {
            // A turn that streamed no answer text — usually because every tool
            // the agent tried failed (e.g. account tools on a paper account).
            let note = if self.tool_failures.is_empty() {
                rust_i18n::t!("Ai.NoAnswer").to_string()
            } else {
                rust_i18n::t!(
                    "Ai.NoAnswerToolsFailed",
                    tools = self.tool_failures.join(", ")
                )
                .to_string()
            };
            self.messages.push(Message::new(Role::System, note));
        }
        self.busy = false;
        self.status.clear();
    }

    /// Reset to a fresh conversation, keeping the agent but dropping all
    /// messages and conversation identity. Used by the "new chat" action.
    pub fn reset(&mut self, welcome: String) {
        self.messages = vec![Message::new(Role::System, welcome)];
        self.streaming = None;
        self.status.clear();
        self.busy = false;
        self.scroll = 0;
        self.title = None;
        self.chat_uid = None;
        self.message_id = None;
        self.pending_interrupt = None;
        self.tool_failures.clear();
        self.references.clear();
        self.further.clear();
    }

    /// Cancel the active turn, folding any partial answer into the transcript.
    pub fn cancel(&mut self, cancelled_label: &str) {
        if let Some(mut text) = self.streaming.take() {
            if text.trim().is_empty() {
                text = cancelled_label.to_string();
            } else {
                text.push('\n');
                text.push_str(cancelled_label);
            }
            self.messages.push(Message::new(Role::Assistant, text));
        }
        self.busy = false;
        self.status.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{ChatEvent, ChatState, Role, ToolStatus};

    fn state() -> ChatState {
        ChatState::new("chatbot".into(), "welcome".into())
    }

    #[test]
    fn a_prompt_then_answer_becomes_two_messages() {
        let mut s = state();
        s.apply(ChatEvent::UserPrompt("hi".into()));
        assert!(s.busy);
        s.apply(ChatEvent::Delta("hel".into()));
        s.apply(ChatEvent::Delta("lo".into()));
        s.apply(ChatEvent::TurnFinished { error: None });
        assert!(!s.busy);
        // welcome(system) + user + assistant
        assert_eq!(s.messages.len(), 3);
        assert!(matches!(s.messages[1].role, Role::User));
        assert_eq!(s.messages[2].text, "hello");
    }

    #[test]
    fn an_empty_turn_with_failed_tools_explains_itself() {
        let mut s = state();
        s.apply(ChatEvent::UserPrompt("my positions?".into()));
        s.apply(ChatEvent::ToolFinished {
            name: "Get Account Info".into(),
            ok: false,
        });
        s.apply(ChatEvent::TurnFinished { error: None });
        let last = &s.messages.last().unwrap().text;
        assert!(last.contains("Get Account Info"));
    }

    #[test]
    fn turn_started_captures_follow_up_ids() {
        let mut s = state();
        s.apply(ChatEvent::TurnStarted {
            chat_uid: "c1".into(),
            message_id: "m1".into(),
        });
        assert_eq!(s.chat_uid.as_deref(), Some("c1"));
        assert_eq!(s.message_id.as_deref(), Some("m1"));
    }

    /// Tool calls used to exist only as a status string that the next event
    /// overwrote, so a turn that read six data sources left no trace of it.
    #[test]
    fn tool_calls_are_recorded_in_the_transcript() {
        let mut s = state();
        s.apply(ChatEvent::UserPrompt("what is NVDA doing?".into()));
        s.apply(ChatEvent::ToolStarted("Get Quote".into()));
        s.apply(ChatEvent::ToolStarted("Get Capital Flow".into()));
        s.apply(ChatEvent::ToolFinished {
            name: "Get Quote".into(),
            ok: true,
        });
        s.apply(ChatEvent::ToolFinished {
            name: "Get Capital Flow".into(),
            ok: false,
        });
        s.apply(ChatEvent::Delta("NVDA closed at 182.40".into()));
        s.apply(ChatEvent::TurnFinished { error: None });

        let tools: Vec<(&str, Option<ToolStatus>)> = s
            .messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| (m.text.as_str(), m.tool))
            .collect();
        assert_eq!(
            tools,
            vec![
                ("Get Quote", Some(ToolStatus::Ok)),
                ("Get Capital Flow", Some(ToolStatus::Failed)),
            ],
            "both calls should survive the turn with their outcome"
        );
        // The answer still lands, after the tools that produced it.
        assert!(s.messages.last().is_some_and(|m| m.role == Role::Assistant));
    }

    /// The same tool called twice resolves the right line each time, rather than
    /// the first call swallowing the second's outcome.
    #[test]
    fn repeated_tool_calls_resolve_independently() {
        let mut s = state();
        s.apply(ChatEvent::UserPrompt("compare".into()));
        s.apply(ChatEvent::ToolStarted("Get Quote".into()));
        s.apply(ChatEvent::ToolFinished {
            name: "Get Quote".into(),
            ok: true,
        });
        s.apply(ChatEvent::ToolStarted("Get Quote".into()));
        s.apply(ChatEvent::ToolFinished {
            name: "Get Quote".into(),
            ok: false,
        });
        let statuses: Vec<Option<ToolStatus>> = s
            .messages
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| m.tool)
            .collect();
        assert_eq!(
            statuses,
            vec![Some(ToolStatus::Ok), Some(ToolStatus::Failed)]
        );
    }

    /// A finish with no matching start still records the call.
    #[test]
    fn an_unpaired_tool_finish_is_still_recorded() {
        let mut s = state();
        s.apply(ChatEvent::ToolFinished {
            name: "Get Quote".into(),
            ok: true,
        });
        assert!(s
            .messages
            .iter()
            .any(|m| m.role == Role::Tool && m.tool == Some(ToolStatus::Ok)));
    }
}
