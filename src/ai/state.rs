//! Chat state for the `longbridge ai` TUI.
//!
//! Modeled on grok-build's `xai-chat-state`: a single state snapshot mutated by
//! a stream of typed [`ChatEvent`]s. The turn task produces events; the view
//! renders the snapshot. Keeping mutation in one `apply` method (rather than
//! scattered across the UI loop) is what lets the view stay a pure function of
//! state.

use longbridge::agent::Reference;
use serde_json::Value;

use crate::openapi::chats::TokenUsage;

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
    /// A finished reasoning phase, folded to how long it took until opened.
    Thinking,
}

/// How a tool call ended, for the transcript's tool lines.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ToolStatus {
    Running = 0,
    Ok = 1,
    Failed = 2,
}

/// A finished reasoning phase: how long it ran, and whether the reader has
/// opened it.
///
/// The reasoning itself stays in the message's `text`. It is kept rather than
/// discarded — how an answer was arrived at is part of the record — but folded
/// by default, because at full length it buries the answer it produced.
pub struct ThinkingBlock {
    pub secs: u64,
    pub expanded: bool,
}

pub struct Message {
    pub role: Role,
    pub text: String,
    /// Set only on [`Role::Tool`] lines.
    pub tool: Option<ToolStatus>,
    /// Set only on [`Role::Thinking`] lines.
    pub thinking: Option<ThinkingBlock>,
    /// The round's token usage, on the [`Role::Assistant`] line that answered
    /// it. Absent for every other line, and for answers that consumed no
    /// tokens — see [`TokenUsage`].
    pub token_usage: Option<TokenUsage>,
}

impl Message {
    /// A plain transcript line from `role`.
    pub fn new(role: Role, text: String) -> Self {
        Self {
            role,
            text,
            tool: None,
            thinking: None,
            token_usage: None,
        }
    }

    /// A tool line naming the tool and how its call is going.
    pub fn tool(name: String, status: ToolStatus) -> Self {
        Self {
            role: Role::Tool,
            text: name,
            tool: Some(status),
            thinking: None,
            token_usage: None,
        }
    }

    /// A finished reasoning phase, folded until the reader opens it.
    pub fn thinking(reasoning: String, secs: u64) -> Self {
        Self {
            role: Role::Thinking,
            text: reasoning,
            tool: None,
            thinking: Some(ThinkingBlock {
                secs,
                expanded: false,
            }),
            token_usage: None,
        }
    }

    /// Attach a round's token usage to an assistant line, dropping an empty
    /// one so it never shows as a "0 tokens" placeholder.
    #[must_use]
    pub fn with_token_usage(mut self, usage: Option<TokenUsage>) -> Self {
        self.token_usage = usage.filter(|u| !u.is_empty());
        self
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
    /// An incremental chunk of the agent's reasoning for the turn in flight.
    ThinkingDelta(String),
    /// The reasoning phase ended; the live block collapses to a summary line.
    ThinkingFinished,
    /// The turn's cumulative token usage so far. Overwrites the last value
    /// (never accumulates), so a dropped or replayed frame keeps the count
    /// correct.
    TokenUsage(TokenUsage),
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
    /// The connection carrying the turn died and is being re-established.
    ///
    /// The run continues on the server, and re-attaching to it replays this
    /// turn from the beginning, so everything the dead connection produced is
    /// discarded rather than left for the replay to duplicate.
    StreamInterrupted,
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

/// The agent's reasoning for the turn in flight, and when it started.
///
/// Held apart from [`ChatState::messages`] because it is live-only: it is what
/// the reader watches during the wait, and once the answer lands it collapses to
/// a single [`Role::Thinking`] line rather than staying in the transcript at
/// full length, where it would bury the answer it belongs to.
pub struct Thinking {
    pub text: String,
    pub started: std::time::Instant,
}

impl Default for Thinking {
    fn default() -> Self {
        Self {
            text: String::new(),
            started: std::time::Instant::now(),
        }
    }
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
    /// The reasoning accumulating during the active turn. See [`Thinking`].
    pub thinking: Option<Thinking>,
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
    /// Prompts typed while a turn was running, sent together as it frees up.
    ///
    /// A reader who has thought of the next question should not have to hold it in
    /// their head until the answer lands — and typing it used to start a second,
    /// concurrent turn on the same conversation. Drained by [`Self::take_queued`].
    pub queued: Vec<String>,
    /// Tools that failed during the active turn.
    pub tool_failures: Vec<String>,
    /// Source references from the latest completed turn (rendered as chips).
    pub references: Vec<Reference>,
    /// Suggested follow-up questions from the latest turn (click to send).
    pub further: Vec<String>,
    /// Where the turn in flight began, for analytics. See [`TurnStart`].
    pub turn_started: Option<TurnStart>,
    /// The active turn's cumulative token usage, carried until the answer is
    /// finalized and then attached to that assistant message. Overwritten by
    /// each frame, so the value is always the round's running total.
    pub token_usage: Option<TokenUsage>,
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
                self.thinking = None;
                self.tool_failures.clear();
                self.references.clear();
                self.further.clear();
                self.turn_error = None;
                self.token_usage = None;
            }
            ChatEvent::TurnStarted {
                chat_uid,
                message_id,
            } => {
                self.chat_uid = Some(chat_uid);
                self.message_id = Some(message_id);
            }
            ChatEvent::Delta(text) => {
                // Answer text is the end of the reasoning, whether or not the
                // stream said so. Only real text: an empty delta would retire a
                // block the agent is still writing, splitting one reasoning phase
                // into two.
                if !text.is_empty() {
                    self.collapse_thinking();
                }
                self.streaming
                    .get_or_insert_with(String::new)
                    .push_str(&text);
            }
            ChatEvent::Status(status) => self.status = status,
            ChatEvent::ThinkingDelta(text) => {
                self.thinking.get_or_insert_with(Thinking::default).text += &text;
            }
            ChatEvent::ThinkingFinished => self.collapse_thinking(),
            // Cumulative for the round, so replace rather than add.
            ChatEvent::TokenUsage(usage) => self.token_usage = Some(usage),
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
            ChatEvent::StreamInterrupted => {
                // Drop this turn's transcript back to the prompt that started
                // it: the replay re-emits its tool lines, and keeping the
                // originals would show every tool twice.
                if let Some(start) = self.turn_started {
                    self.messages.truncate(start.from + 1);
                }
                self.streaming = Some(String::new());
                self.thinking = None;
                self.tool_failures.clear();
                self.references.clear();
                self.further.clear();
                self.turn_error = None;
                // The replay re-emits this round's cumulative token frames.
                self.token_usage = None;
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
        // Before the answer is committed, so a turn whose reasoning never got an
        // explicit `thinking_finished` still leaves its summary above the answer
        // rather than below it.
        self.collapse_thinking();
        self.settle_running_tools();
        let error_free = error.is_none();
        // Taken whether or not an answer was produced, so a turn that streamed
        // no text never carries its usage into the next turn.
        let usage = self.token_usage.take();
        let produced = self
            .streaming
            .take()
            .filter(|t| !t.trim().is_empty())
            .map(|text| {
                self.messages
                    .push(Message::new(Role::Assistant, text).with_token_usage(usage));
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

    /// Stop any tool line still claiming to be in flight.
    ///
    /// The turn is over, so nothing is running — whatever the stream did or did
    /// not say. Work delegated to a subagent reports its completion through an
    /// event family this client does not render, which left those rows spinning
    /// for the rest of the session under a finished, complete answer. A failure
    /// is always reported explicitly (and recorded in [`Self::tool_failures`]),
    /// so a call that never reported one is taken as having completed.
    fn settle_running_tools(&mut self) {
        for m in &mut self.messages {
            if m.tool == Some(ToolStatus::Running) {
                m.tool = Some(ToolStatus::Ok);
            }
        }
    }

    /// Retire the live reasoning block into the transcript, folded to one line
    /// saying how long it ran. The reasoning is kept, not dropped — it is part of
    /// how the answer was reached — but it opens only when the reader asks.
    ///
    /// Reasoning that produced nothing leaves no trace: an empty block would be a
    /// row claiming the agent thought when it did not.
    fn collapse_thinking(&mut self) {
        let Some(thinking) = self.thinking.take() else {
            return;
        };
        if thinking.text.trim().is_empty() {
            return;
        }
        let secs = thinking.started.elapsed().as_secs();
        self.messages.push(Message::thinking(thinking.text, secs));
    }

    /// Reset to a fresh conversation, keeping the agent but dropping all
    /// messages and conversation identity. Used by the "new chat" action.
    pub fn reset(&mut self, welcome: String) {
        self.generation = self.generation.wrapping_add(1);
        self.messages = vec![Message::new(Role::System, welcome)];
        self.streaming = None;
        self.thinking = None;
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
        self.token_usage = None;
    }

    /// Take everything queued as a single prompt, or `None` if nothing waits.
    ///
    /// Prompts typed while one answer was streaming are one follow-up thought, not
    /// a backlog of separate turns: sending them together lets the agent plan once
    /// with the whole picture, instead of answering the first and then discovering
    /// the rest one turn at a time.
    pub fn take_queued(&mut self) -> Option<String> {
        if self.queued.is_empty() {
            return None;
        }
        Some(std::mem::take(&mut self.queued).join("\n\n"))
    }

    /// Cancel the active turn, folding any partial answer into the transcript.
    pub fn cancel(&mut self, cancelled_label: &str) {
        // First, so the summary lands above the partial answer — and so a turn
        // cancelled mid-reasoning does not leave its live block on screen with
        // nothing left to finish it.
        self.collapse_thinking();
        self.settle_running_tools();
        let usage = self.token_usage.take();
        if let Some(mut text) = self.streaming.take() {
            if text.trim().is_empty() {
                text = cancelled_label.to_string();
            } else {
                text.push('\n');
                text.push_str(cancelled_label);
            }
            self.messages
                .push(Message::new(Role::Assistant, text).with_token_usage(usage));
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

    /// The reasoning is live while the turn runs — that is the whole point of
    /// carrying it — and collapses to one line once the answer starts, so it
    /// cannot bury the answer it produced.
    #[test]
    fn reasoning_streams_live_and_collapses_when_the_answer_starts() {
        let mut s = state();
        s.apply(ChatEvent::UserPrompt("how is TSLA?".into()));
        s.apply(ChatEvent::ThinkingDelta("Let".into()));
        s.apply(ChatEvent::ThinkingDelta(" me think".into()));
        assert_eq!(
            s.thinking.as_ref().map(|t| t.text.as_str()),
            Some("Let me think"),
            "the reasoning is what the reader watches during the wait"
        );
        s.apply(ChatEvent::Delta("TSLA is".into()));
        assert!(s.thinking.is_none(), "the answer starting ends the block");
        assert_eq!(
            s.messages.last().map(|m| m.role),
            Some(Role::Thinking),
            "one line stays behind, above the answer"
        );
    }

    /// A `thinking_finished` before any answer text collapses it just the same.
    #[test]
    fn an_explicit_end_of_reasoning_collapses_it() {
        let mut s = state();
        s.apply(ChatEvent::UserPrompt("how is TSLA?".into()));
        s.apply(ChatEvent::ThinkingDelta("weighing the data".into()));
        s.apply(ChatEvent::ThinkingFinished);
        assert!(s.thinking.is_none());
        assert_eq!(s.messages.last().map(|m| m.role), Some(Role::Thinking));
    }

    /// A turn that never reasoned must not claim it did.
    #[test]
    fn a_turn_without_reasoning_leaves_no_line() {
        let mut s = state();
        s.apply(ChatEvent::UserPrompt("hi".into()));
        s.apply(ChatEvent::Delta("hello".into()));
        s.apply(ChatEvent::TurnFinished { error: None });
        assert!(!s.messages.iter().any(|m| m.role == Role::Thinking));
    }

    /// Reasoning belongs to the turn that produced it: a new prompt starts empty
    /// rather than continuing the previous turn's block.
    #[test]
    fn a_new_prompt_starts_its_own_reasoning() {
        let mut s = state();
        s.apply(ChatEvent::UserPrompt("first".into()));
        s.apply(ChatEvent::ThinkingDelta("about the first".into()));
        s.apply(ChatEvent::UserPrompt("second".into()));
        assert!(s.thinking.is_none());
    }

    /// Work the agent delegates to a subagent reports its completion through an
    /// event family this client does not render, so those rows never resolved and
    /// span for the rest of the session under a finished answer. The turn ending
    /// is proof enough that nothing is still in flight.
    #[test]
    fn a_finished_turn_leaves_no_tool_claiming_to_be_running() {
        for outcome in [None, Some("upstream unavailable".to_string())] {
            let mut s = state();
            s.apply(ChatEvent::UserPrompt("analyse QCOM and SNDK".into()));
            s.apply(ChatEvent::ToolStarted("Get Ticker Region".into()));
            s.apply(ChatEvent::ToolFinished {
                name: "Get Ticker Region".into(),
                ok: true,
            });
            // Started by a subagent, never reported finished.
            s.apply(ChatEvent::ToolStarted("Get Candlestick Data".into()));
            s.apply(ChatEvent::Delta("QCOM and SNDK diverge.".into()));
            s.apply(ChatEvent::TurnFinished { error: outcome });
            assert!(
                !s.messages
                    .iter()
                    .any(|m| m.tool == Some(ToolStatus::Running)),
                "nothing runs after the turn is over"
            );
        }
    }

    /// Cancelling is the same: the run is gone, so nothing it started is still
    /// going.
    #[test]
    fn cancelling_leaves_no_tool_claiming_to_be_running() {
        let mut s = state();
        s.apply(ChatEvent::UserPrompt("analyse QCOM".into()));
        s.apply(ChatEvent::ToolStarted("Get Candlestick Data".into()));
        s.cancel("(cancelled)");
        assert!(!s
            .messages
            .iter()
            .any(|m| m.tool == Some(ToolStatus::Running)));
    }

    /// Prompts lined up during one answer go out as a single turn: the reader was
    /// reading one reply when they typed them, so they belong together.
    #[test]
    fn everything_queued_goes_out_as_one_prompt() {
        let mut s = state();
        assert!(
            s.take_queued().is_none(),
            "nothing waiting, nothing to send"
        );
        s.queued.push("那 NVDA 呢？".into());
        s.queued.push("顺便看看 AMD".into());
        assert_eq!(
            s.take_queued(),
            Some("那 NVDA 呢？\n\n顺便看看 AMD".to_string())
        );
        assert!(s.queued.is_empty(), "and the queue is drained, not re-sent");
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

    /// A turn cancelled while the agent was still reasoning leaves nothing live:
    /// the block has no stream left to finish it, so it would sit on screen for
    /// the rest of the session.
    #[test]
    fn cancelling_retires_the_live_reasoning() {
        let mut s = state();
        s.apply(ChatEvent::UserPrompt("first".into()));
        s.apply(ChatEvent::ThinkingDelta("halfway through a thought".into()));
        s.cancel("(cancelled)");
        assert!(s.thinking.is_none());
        assert!(s.messages.iter().any(|m| m.role == Role::Thinking));
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

    /// The round's token total lands on the assistant message it belongs to,
    /// and a fresh prompt starts the next round's count from nothing.
    #[test]
    fn token_usage_attaches_to_the_answer_and_resets_next_turn() {
        use crate::openapi::chats::TokenUsage;
        let mut s = state();
        s.apply(ChatEvent::UserPrompt("how is TSLA?".into()));
        s.apply(ChatEvent::TokenUsage(TokenUsage {
            prompt_tokens: 1200,
            completion_tokens: 80,
            total_tokens: 1280,
        }));
        // A later cumulative frame overwrites the earlier one.
        s.apply(ChatEvent::TokenUsage(TokenUsage {
            prompt_tokens: 2600,
            completion_tokens: 210,
            total_tokens: 2810,
        }));
        s.apply(ChatEvent::Delta("TSLA is up.".into()));
        s.apply(ChatEvent::TurnFinished { error: None });
        let answer = s.messages.last().unwrap();
        assert_eq!(answer.role, Role::Assistant);
        assert_eq!(answer.token_usage.unwrap().total_tokens, 2810);
        // The next turn does not inherit the previous round's total.
        s.apply(ChatEvent::UserPrompt("and NVDA?".into()));
        assert!(s.token_usage.is_none());
    }

    /// A round that consumed no tokens (a cache hit) leaves no footer to render.
    #[test]
    fn an_answer_without_usage_carries_none() {
        let mut s = state();
        s.apply(ChatEvent::UserPrompt("hi".into()));
        s.apply(ChatEvent::Delta("hello".into()));
        s.apply(ChatEvent::TurnFinished { error: None });
        assert!(s.messages.last().unwrap().token_usage.is_none());
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
