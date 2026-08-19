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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    User,
    Assistant,
    System,
    /// A turn-level error rendered as semantic red text.
    Alert,
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
    /// The server reported the turn failed (via `chat_finished`/`workflow_finished`),
    /// even though the stream itself ended cleanly. Recorded so the turn is
    /// finalized as an error rather than a clean completion.
    TurnError(String),
    /// End-of-turn metadata (source references / suggested follow-ups) rendered
    /// as interactive chips rather than folded into the answer text.
    Meta {
        references: Vec<Reference>,
        further: Vec<String>,
    },
    /// The turn ended. Finalizes the streamed answer into a message.
    TurnFinished { error: Option<String> },
}

/// Where the turn in flight began. `None` between turns.
///
/// Carried for analytics, which needs both halves: the timestamp to measure how
/// long a turn took, and the transcript index to tell *this* turn's tool calls
/// apart from every call the conversation has made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TurnStart {
    /// When the prompt went out. A monotonic instant rather than a wall clock:
    /// the value is only ever used as a duration, and a clock adjustment
    /// mid-turn would otherwise report a negative or wildly inflated one.
    pub at: std::time::Instant,
    /// Index into [`ChatState::messages`] the turn starts at.
    pub from: usize,
}

/// The full chat state the view renders.
#[derive(Default)]
pub struct ChatState {
    pub agent_uid: String,
    /// Changes whenever `/new` starts a fresh conversation. Runtime events carry
    /// the generation they originated in so late events from an aborted turn can
    /// never be applied to its successor.
    pub generation: u64,
    pub messages: Vec<Message>,
    /// The assistant answer accumulating during the active turn.
    pub streaming: Option<String>,
    pub status: String,
    pub busy: bool,
    /// Lines scrolled up from the bottom (0 = pinned to the latest).
    pub scroll: u16,
    /// Server-generated conversation title, shown in History when present.
    pub title: Option<String>,
    /// Longbridge conversation id, kept for the life of the conversation.
    pub chat_uid: Option<String>,
    /// The message id of the turn in flight — what a resume addresses.
    pub message_id: Option<String>,
    /// The message a *new* turn is parented to: the last one that completed.
    ///
    /// Kept apart from [`Self::message_id`] because a turn that failed or is
    /// paused awaiting answers cannot be built on. Parenting to it left every
    /// later message failing too, so one unanswerable interrupt bricked the whole
    /// conversation.
    pub parent_message_id: Option<String>,
    /// Set when the last turn ended asking a question; the next prompt answers.
    pub pending_interrupt: Option<Value>,
    /// A server-reported error for the active turn, carried out-of-band.
    ///
    /// The stream ends `Ok` even when the server says the turn failed (the error
    /// rides a `chat_finished`/`workflow_finished` event, not a transport error),
    /// so without this a failed turn would look clean and become the parent of the
    /// next — the very thing that bricks a conversation. Consumed by
    /// [`Self::finish_turn`].
    pub turn_error: Option<String>,
    /// Prompts typed while a turn was running, sent one at a time as it frees up.
    ///
    /// A reader who has thought of the next question should not have to hold it in
    /// their head until the answer lands — and typing it used to start a second,
    /// concurrent turn on the same conversation.
    pub queued: Vec<String>,
    /// Tools that failed during the active turn.
    pub tool_failures: Vec<String>,
    /// Source references from the latest completed turn (rendered as chips).
    pub references: Vec<Reference>,
    /// Suggested follow-up questions from the latest turn (click to send).
    pub further: Vec<String>,
    /// Where the turn in flight began, for analytics. See [`TurnStart`].
    pub turn_started: Option<TurnStart>,
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
                // Recorded before the prompt is pushed, so `from` points at the
                // prompt itself and the turn's own tool lines are everything
                // after it — not the whole conversation's.
                self.turn_started = Some(TurnStart {
                    at: std::time::Instant::now(),
                    from: self.messages.len(),
                });
                self.messages.push(Message::new(Role::User, text));
                self.scroll = 0;
                self.busy = true;
                self.streaming = Some(String::new());
                self.tool_failures.clear();
                self.references.clear();
                self.further.clear();
                self.turn_error = None;
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
            ChatEvent::Interrupt(interrupt) => {
                if let Some(message_id) = interrupt.get("message_id") {
                    let id = match message_id {
                        Value::String(id) => Some(id.clone()),
                        Value::Number(id) => Some(id.to_string()),
                        _ => None,
                    };
                    if let Some(id) = id.filter(|id| !id.is_empty() && id != "0") {
                        self.message_id = Some(id);
                    }
                }
                self.pending_interrupt = Some(interrupt);
            }
            // Keep the first cause: a later teardown event may repeat it or arrive
            // blank, and the first one is the real reason.
            ChatEvent::TurnError(err) => {
                if self.turn_error.is_none() && !err.trim().is_empty() {
                    self.turn_error = Some(err);
                }
            }
            ChatEvent::Meta {
                references,
                further,
            } => {
                self.references = references;
                self.further = further;
            }
            ChatEvent::TurnFinished { error } => {
                // A transport error takes precedence, but a server-reported one is
                // just as fatal — fold it in so the turn is finalized as failed.
                let error = error.or_else(|| self.turn_error.take());
                self.turn_error = None;
                self.finish_turn(error);
            }
        }
    }

    fn finish_turn(&mut self, error: Option<String>) {
        let error_free = error.is_none();
        let produced = self
            .streaming
            .take()
            .filter(|t| !t.trim().is_empty())
            .map(|text| {
                self.messages.push(Message::new(Role::Assistant, text));
            })
            .is_some();
        if let Some(err) = error {
            self.messages.push(Message::new(Role::Alert, err));
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
        // A turn only becomes the parent of the next one if it actually finished:
        // an errored or paused message is not something the server can continue
        // from, and treating it as the parent is what bricked the conversation.
        if error_free && self.pending_interrupt.is_none() {
            self.parent_message_id = self.message_id.clone();
        }
        if !error_free {
            // A pause that ended in an error cannot be resumed either, so the next
            // message must not try to answer it.
            self.pending_interrupt = None;
        }
        self.busy = false;
        self.status.clear();
    }

    /// Reset to a fresh conversation, keeping the agent but dropping all
    /// messages and conversation identity. Used by the "new chat" action.
    pub fn reset(&mut self, welcome: String) {
        self.generation = self.generation.wrapping_add(1);
        self.messages = vec![Message::new(Role::System, welcome)];
        self.streaming = None;
        self.status.clear();
        self.busy = false;
        self.scroll = 0;
        self.title = None;
        self.chat_uid = None;
        self.message_id = None;
        self.parent_message_id = None;
        self.pending_interrupt = None;
        self.turn_error = None;
        self.queued.clear();
        self.tool_failures.clear();
        self.references.clear();
        self.further.clear();
        self.turn_started = None;
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
        // Cancelling means stop, not "stop this one and start the next".
        self.queued.clear();
        // The aborted run cannot be resumed, so a question it raised is not
        // answerable — leaving it set would misroute the next prompt as an answer
        // to a run the server has already dropped.
        self.pending_interrupt = None;
        self.turn_error = None;
        // Cleared last, and only here: callers that report the cancelled turn
        // read it first, so anything that clears it earlier loses the duration.
        self.turn_started = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{ChatEvent, ChatState, Role, ToolStatus};

    fn state() -> ChatState {
        ChatState::new("chatbot".into(), "welcome".into())
    }

    #[test]
    fn cancelling_folds_the_partial_answer_and_clears_the_queue() {
        let mut s = state();
        s.apply(ChatEvent::UserPrompt("first".into()));
        s.apply(ChatEvent::Delta("partial".into()));
        s.queued.push("queued next".into());
        s.cancel("(cancelled)");
        assert!(!s.busy);
        // The partial answer is kept, with the cancel note appended.
        let last = &s.messages.last().unwrap().text;
        assert!(last.contains("partial") && last.contains("(cancelled)"));
        // Cancelling means stop, not "run the queue".
        assert!(s.queued.is_empty());
    }

    /// Cancelling a turn that had raised a question drops the interrupt: the
    /// aborted run cannot be resumed, so the next prompt must go out fresh rather
    /// than as an answer to a run the server already let go.
    #[test]
    fn cancelling_drops_a_pending_interrupt() {
        let mut s = state();
        s.apply(ChatEvent::UserPrompt("first".into()));
        s.apply(ChatEvent::Interrupt(
            serde_json::json!({ "tool_call_id": "tc1" }),
        ));
        s.cancel("(cancelled)");
        assert!(s.pending_interrupt.is_none());
    }

    /// A server-reported error (delivered out-of-band, over a stream that ends
    /// cleanly) finalizes the turn as failed: it prints an error line and does not
    /// advance the parent.
    #[test]
    fn a_server_error_finalizes_the_turn_as_failed() {
        let mut s = state();
        s.apply(ChatEvent::UserPrompt("q".into()));
        s.apply(ChatEvent::TurnStarted {
            chat_uid: "c1".into(),
            message_id: "m1".into(),
        });
        s.apply(ChatEvent::TurnError("rate limited".into()));
        s.apply(ChatEvent::TurnFinished { error: None });
        assert_eq!(s.parent_message_id, None, "a failed turn is not a parent");
        assert!(
            s.messages.last().unwrap().text.contains("rate limited"),
            "the failure is reported to the reader"
        );
        assert_eq!(s.messages.last().unwrap().role, Role::Alert);
        assert!(!s.messages.last().unwrap().text.contains("[error]"));
        assert!(
            s.turn_error.is_none(),
            "the error is consumed, not left to leak"
        );
    }

    #[test]
    fn only_a_clean_finish_becomes_the_next_turns_parent() {
        // A finish carrying an error must not be parented on: doing so bricked the
        // whole conversation. Only an error-free, un-paused turn advances it.
        let mut s = state();
        s.apply(ChatEvent::UserPrompt("q".into()));
        s.apply(ChatEvent::TurnStarted {
            chat_uid: "c1".into(),
            message_id: "m1".into(),
        });
        s.apply(ChatEvent::TurnFinished {
            error: Some("boom".into()),
        });
        assert_eq!(s.parent_message_id, None, "an errored turn is not a parent");

        let mut s = state();
        s.apply(ChatEvent::UserPrompt("q".into()));
        s.apply(ChatEvent::TurnStarted {
            chat_uid: "c1".into(),
            message_id: "m2".into(),
        });
        s.apply(ChatEvent::Delta("ok".into()));
        s.apply(ChatEvent::TurnFinished { error: None });
        assert_eq!(s.parent_message_id.as_deref(), Some("m2"));
    }

    #[test]
    fn a_title_event_sets_the_conversation_title() {
        let mut s = state();
        s.apply(ChatEvent::Title("Tesla outlook".into()));
        assert_eq!(s.title.as_deref(), Some("Tesla outlook"));
        // A blank title does not overwrite a real one.
        s.apply(ChatEvent::Title("   ".into()));
        assert_eq!(s.title.as_deref(), Some("Tesla outlook"));
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

    #[test]
    fn an_interrupts_message_id_is_authoritative_for_continuation() {
        let mut s = state();
        s.message_id = Some("old".into());
        s.apply(ChatEvent::Interrupt(serde_json::json!({
            "message_id": 42,
            "interactions": []
        })));
        assert_eq!(s.message_id.as_deref(), Some("42"));
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
