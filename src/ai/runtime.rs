//! Agent runtime seam for the `longbridge ai` TUI.
//!
//! Modeled on grok-build's `xai-grok-shell`: turns a user prompt into a
//! streaming Longbridge AI conversation and translates the SDK's stream into
//! [`ChatEvent`]s the state layer understands. The Longbridge AI model runs
//! server-side (it orchestrates its own tools), so this is a thin translation
//! layer over [`stream_conversation`] — there is no local tool execution.

use std::collections::HashMap;

use rust_i18n::t;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use super::state::{ChatEvent, ChatState};
use crate::cli::agent::client::{stream_conversation, ConversationRequest};
use crate::cli::agent::events::AgentEvent;

/// A chat event tagged with the conversation generation that spawned it.
///
/// Aborting a task cannot retract events it already queued.  The generation lets
/// the TUI discard those events after `/new` has moved on to another conversation.
pub struct TurnEvent {
    pub generation: u64,
    pub event: ChatEvent,
}

/// Build the conversation request for the current input: a fresh chat, a
/// follow-up, or an answer to a pending clarifying question.
///
/// Resuming a paused run needs a question to key the answer under, so an
/// interrupt that named none cannot be answered — the request would carry an
/// empty answer map and the run would stay paused, which is how an interrupted
/// turn ended up looking like a dead conversation. That case falls through to a
/// follow-up in the same conversation instead: the agent gets the reply, just as
/// a new message rather than as a resumption.
pub fn build_request(state: &ChatState, query: String) -> ConversationRequest {
    match (
        state.pending_interrupt.as_ref().filter(is_answerable),
        &state.chat_uid,
        &state.message_id,
    ) {
        (Some(interrupt), Some(chat_uid), Some(message_id)) => ConversationRequest::Continue {
            agent_uid: state.agent_uid.clone(),
            chat_uid: chat_uid.clone(),
            message_id: message_id.clone(),
            answers: build_answers(interrupt, &query),
        },
        _ => ConversationRequest::New {
            agent_uid: state.agent_uid.clone(),
            query,
            chat_uid: state.chat_uid.clone(),
            // The last message that *completed*, not the one that just failed or
            // paused — the server cannot build on either.
            parent_message_id: state.parent_message_id.clone(),
        },
    }
}

/// Spawn the turn: stream the conversation, mapping each SDK event to
/// [`ChatEvent`]s pushed onto `tx`. The returned handle is aborted on cancel
/// (dropping the SSE stream). The closure captures only the `Send` sender, so
/// the task is `Send` for `tokio::spawn`.
pub fn spawn_turn(
    req: ConversationRequest,
    generation: u64,
    tx: UnboundedSender<TurnEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut forward = |ev: AgentEvent| {
            for chat_event in map_agent_event(&ev) {
                let _ = tx.send(TurnEvent {
                    generation,
                    event: chat_event,
                });
            }
        };
        let result = stream_conversation(req, false, &mut forward).await;
        let _ = tx.send(TurnEvent {
            generation,
            event: ChatEvent::TurnFinished {
                error: result.err().map(|e| e.to_string()),
            },
        });
    })
}

/// Translate one Longbridge AI stream event into zero or more chat events.
fn map_agent_event(ev: &AgentEvent) -> Vec<ChatEvent> {
    match ev {
        AgentEvent::ChatStarted {
            chat_uid,
            message_id,
        } => vec![ChatEvent::TurnStarted {
            chat_uid: chat_uid.clone(),
            message_id: message_id.clone(),
        }],
        AgentEvent::AnswerDelta { text } => vec![
            ChatEvent::Delta(text.clone()),
            ChatEvent::Status(t!("Agent.Generating").to_string()),
        ],
        AgentEvent::ThinkingStarted => vec![ChatEvent::Status(t!("Agent.Thinking").to_string())],
        // A tool call goes to the transcript as well as the status line: the
        // status line is overwritten by the next event, and which data an answer
        // was built from is worth keeping.
        AgentEvent::ToolUseStarted { tool_name } => vec![
            ChatEvent::ToolStarted(tool_name.clone()),
            ChatEvent::Status(t!("Agent.CallingTool", name = tool_name).to_string()),
        ],
        AgentEvent::ToolUseFinished {
            tool_name,
            status,
            error,
        } => {
            let mut events = vec![
                ChatEvent::ToolFinished {
                    name: tool_name.clone(),
                    ok: !tool_failed(status),
                },
                ChatEvent::Status(t!("Agent.Generating").to_string()),
            ];
            if tool_failed(status) && is_trade_password_error(error) {
                events.push(ChatEvent::TurnError(display_error(error)));
            }
            events
        }
        AgentEvent::WorkflowFinished {
            references,
            further_questions,
            error_message,
            ..
        } => {
            let mut events = Vec::new();
            if !(references.is_empty() && further_questions.is_empty()) {
                events.push(ChatEvent::Meta {
                    references: references.clone(),
                    further: further_questions.clone(),
                });
            }
            // A workflow that ends with an error is a failed turn: carry the cause
            // so the turn is finalized as such, not silently dropped.
            if !error_message.is_empty() {
                events.push(ChatEvent::TurnError(display_error(error_message)));
            }
            events
        }
        AgentEvent::HumanInteractionRequired { interrupt } => vec![
            ChatEvent::Delta(interrupt_text(interrupt)),
            ChatEvent::Interrupt(interrupt.clone()),
        ],
        // A non-empty error here means the server failed the turn. Carry it as a
        // turn error rather than appending it to the answer text: folded into the
        // answer it looked like a clean completion, so the next turn was parented
        // on a failed message and the whole conversation bricked.
        AgentEvent::ChatFinished { error_message } if !error_message.is_empty() => {
            vec![ChatEvent::TurnError(display_error(error_message))]
        }
        AgentEvent::ChatTitleUpdated { title } => vec![ChatEvent::Title(title.clone())],
        AgentEvent::ThinkingFinished
        | AgentEvent::ChatFinished { .. }
        | AgentEvent::Unknown { .. } => Vec::new(),
    }
}

fn display_error(error: &str) -> String {
    if is_trade_password_error(error) {
        t!("Agent.TradePasswordUnsupported").to_string()
    } else {
        error.to_string()
    }
}

fn is_trade_password_error(error: &str) -> bool {
    error
        .to_ascii_lowercase()
        .replace(['-', '_'], " ")
        .contains("trade password")
}

fn tool_failed(status: &str) -> bool {
    let s = status.to_ascii_lowercase();
    s.contains("fail") || s.contains("error") || s == "rejected"
}

/// Map one free-text answer onto the interrupt's tool-call questions.
fn build_answers(interrupt: &Value, answer: &str) -> longbridge::agent::AnswersByToolCall {
    let mut by_tool_call: longbridge::agent::AnswersByToolCall = HashMap::new();
    for interaction in interrupt_interactions(interrupt) {
        let Some(interrupt_id) = interaction_id(interaction) else {
            continue;
        };
        let inner = by_tool_call.entry(interrupt_id.to_string()).or_default();
        if let Some(questions) = interaction.get("questions").and_then(Value::as_array) {
            for q in questions {
                if let Some(question) = q.get("question").and_then(Value::as_str) {
                    inner.insert(question.to_string(), answer.to_string());
                }
            }
        }
        match interaction.get("type").and_then(Value::as_str) {
            Some("authorization") => {
                inner.insert("authorized".into(), confirmation_value(answer).into());
            }
            Some("connector_reauth") => {
                inner.insert("acknowledged".into(), confirmation_value(answer).into());
            }
            _ => {}
        }
    }
    by_tool_call
}

fn confirmation_value(answer: &str) -> &'static str {
    let answer = answer.trim().to_ascii_lowercase();
    if matches!(
        answer.as_str(),
        "decline" | "deny" | "no" | "false" | "拒绝" | "拒絕"
    ) {
        "false"
    } else {
        "true"
    }
}

/// Normalize the live `interactions[]` protocol while retaining compatibility
/// with older streams that placed one interaction directly in `data`.
pub fn interrupt_interactions(interrupt: &Value) -> Vec<&Value> {
    interrupt
        .get("interactions")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
        .map_or_else(|| vec![interrupt], |items| items.iter().collect())
}

pub fn interaction_id(interaction: &Value) -> Option<&str> {
    interaction
        .get("interrupt_id")
        .or_else(|| interaction.get("tool_call_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
}

pub fn interaction_questions(interaction: &Value) -> Vec<&Value> {
    interaction
        .get("questions")
        .and_then(Value::as_array)
        .map(|questions| {
            questions
                .iter()
                .filter(|q| {
                    q.get("question")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.is_empty())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Wrap per-question answers into the SDK's `{tool_call_id: {question: answer}}`
/// shape for resuming an interrupted conversation.
pub fn answers_by_tool_call(
    tool_call_id: &str,
    answers: &HashMap<String, String>,
) -> longbridge::agent::AnswersByToolCall {
    let mut by_tool_call: longbridge::agent::AnswersByToolCall = HashMap::new();
    by_tool_call.insert(tool_call_id.to_string(), answers.clone());
    by_tool_call
}

/// Whether the interrupt can be answered by resuming the paused run.
///
/// The resume body is `{tool_call_id: {question: answer}}`, so both a tool call
/// id and at least one question text are required. Without them there is nothing
/// to key an answer under.
pub fn is_answerable(interrupt: &&Value) -> bool {
    if interrupt_interactions(interrupt).iter().any(|interaction| {
        matches!(
            interaction.get("type").and_then(Value::as_str),
            Some("trade_password" | "connector_reauth" | "openapi_reauth" | "data_authorization")
        )
    }) {
        return false;
    }
    interrupt_interactions(interrupt).iter().any(|interaction| {
        interaction_id(interaction).is_some()
            && (!interaction_questions(interaction).is_empty()
                || matches!(
                    interaction.get("type").and_then(Value::as_str),
                    Some("authorization" | "connector_reauth")
                ))
    })
}

/// The interrupt's question texts, in order.
fn interrupt_questions(interrupt: &Value) -> Vec<&str> {
    interrupt_interactions(interrupt)
        .into_iter()
        .flat_map(interaction_questions)
        .filter_map(|q| q.get("question").and_then(Value::as_str))
        .collect()
}

fn interrupt_text(interrupt: &Value) -> String {
    let mut out = format!("\n{}", t!("Agent.Interrupted"));
    let questions = interrupt_questions(interrupt);
    for text in &questions {
        out.push_str("\n- ");
        out.push_str(text);
    }
    // An interrupt with nothing answerable in it used to render as a bare header
    // and no way forward. Say what happened and that the conversation continues,
    // and log the payload — the shape is the server's, and this is the only
    // record of one we could not read.
    let confirmations: Vec<&str> = interrupt_interactions(interrupt)
        .into_iter()
        .filter(|interaction| {
            interaction.get("type").and_then(Value::as_str) == Some("authorization")
        })
        .filter_map(|interaction| {
            interaction
                .get("tool_display_name")
                .or_else(|| interaction.get("tool_name"))
                .and_then(Value::as_str)
        })
        .collect();
    for name in &confirmations {
        out.push_str("\n- ");
        out.push_str(name);
    }
    let trade_password_unsupported = interrupt_interactions(interrupt).iter().any(|interaction| {
        interaction.get("type").and_then(Value::as_str) == Some("trade_password")
    });
    let external_authorization_unsupported =
        interrupt_interactions(interrupt).iter().any(|interaction| {
            matches!(
                interaction.get("type").and_then(Value::as_str),
                Some("connector_reauth" | "openapi_reauth" | "data_authorization")
            )
        });
    if trade_password_unsupported {
        out.push_str("\n\n");
        out.push_str(&t!("Agent.TradePasswordUnsupported"));
    }
    if external_authorization_unsupported {
        out.push_str("\n\n");
        out.push_str(&t!("Agent.ExternalAuthorizationUnsupported"));
    }
    if questions.is_empty()
        && confirmations.is_empty()
        && !trade_password_unsupported
        && !external_authorization_unsupported
    {
        tracing::warn!(
            interrupt = %interrupt,
            "interrupt carried no answerable question"
        );
        out.push_str("\n\n");
        out.push_str(&t!("Agent.InterruptedUnreadable"));
    } else if !trade_password_unsupported && !external_authorization_unsupported {
        out.push_str("\n\n");
        out.push_str(&t!("Agent.InterruptedAnswerHint"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::state::ChatEvent;

    #[test]
    fn answer_deltas_map_to_a_delta_plus_a_status() {
        let evs = map_agent_event(&AgentEvent::AnswerDelta { text: "hi".into() });
        assert!(matches!(evs[0], ChatEvent::Delta(ref t) if t == "hi"));
        assert!(matches!(evs[1], ChatEvent::Status(_)));
    }

    #[test]
    fn a_tool_start_is_recorded_and_shown() {
        let evs = map_agent_event(&AgentEvent::ToolUseStarted {
            tool_name: "get_quote".into(),
        });
        assert!(matches!(evs[0], ChatEvent::ToolStarted(ref n) if n == "get_quote"));
        assert!(matches!(evs[1], ChatEvent::Status(_)));
    }

    #[test]
    fn a_tool_finish_carries_its_outcome() {
        let ok = map_agent_event(&AgentEvent::ToolUseFinished {
            tool_name: "get_quote".into(),
            status: "succeeded".into(),
            error: String::new(),
        });
        assert!(matches!(ok[0], ChatEvent::ToolFinished { ok: true, .. }));
        let bad = map_agent_event(&AgentEvent::ToolUseFinished {
            tool_name: "get_quote".into(),
            status: "failed".into(),
            error: "upstream unavailable".into(),
        });
        assert!(matches!(bad[0], ChatEvent::ToolFinished { ok: false, .. }));
    }

    #[test]
    fn a_tool_trade_password_failure_marks_the_turn_unsupported() {
        let events = map_agent_event(&AgentEvent::ToolUseFinished {
            tool_name: "Get Enhanced RMS Account Info".into(),
            status: "failed".into(),
            error: "MCP tool trade password validation (trade password required for mcp tool)"
                .into(),
        });
        assert!(events.iter().any(|event| matches!(
            event,
            ChatEvent::TurnError(text)
                if text == t!("Agent.TradePasswordUnsupported").as_ref()
        )));
    }

    #[test]
    fn a_title_event_maps_through() {
        let evs = map_agent_event(&AgentEvent::ChatTitleUpdated {
            title: "Tesla".into(),
        });
        assert!(matches!(evs.as_slice(), [ChatEvent::Title(t)] if t == "Tesla"));
    }

    #[test]
    fn a_workflow_with_no_meta_maps_to_nothing() {
        let evs = map_agent_event(&AgentEvent::WorkflowFinished {
            status: "succeeded".into(),
            references: vec![],
            further_questions: vec![],
            elapsed_time: None,
            error_message: String::new(),
        });
        assert!(evs.is_empty());
    }

    #[test]
    fn an_interrupt_shows_the_question_and_records_it() {
        let interrupt = json!({"tool_call_id": "tc", "questions": [{"question": "Which market?"}]});
        let evs = map_agent_event(&AgentEvent::HumanInteractionRequired {
            interrupt: interrupt.clone(),
        });
        // The question is written into the transcript, and the interrupt is kept
        // so the next prompt can answer it.
        assert!(matches!(&evs[0], ChatEvent::Delta(t) if t.contains("Which market?")));
        assert!(matches!(&evs[1], ChatEvent::Interrupt(i) if *i == interrupt));
    }

    #[test]
    fn a_chat_error_becomes_a_turn_error_but_an_empty_one_is_dropped() {
        let with = map_agent_event(&AgentEvent::ChatFinished {
            error_message: "rate limited".into(),
        });
        assert!(matches!(&with[0], ChatEvent::TurnError(t) if t == "rate limited"));
        let without = map_agent_event(&AgentEvent::ChatFinished {
            error_message: String::new(),
        });
        assert!(without.is_empty());
    }

    #[test]
    fn a_workflow_error_becomes_a_turn_error() {
        let evs = map_agent_event(&AgentEvent::WorkflowFinished {
            status: "failed".into(),
            references: vec![],
            further_questions: vec![],
            elapsed_time: None,
            error_message: "workflow blew up".into(),
        });
        assert!(matches!(&evs[0], ChatEvent::TurnError(t) if t == "workflow blew up"));
    }

    #[test]
    fn a_trade_password_error_explains_that_terminal_cannot_complete_the_flow() {
        for ev in [
            AgentEvent::WorkflowFinished {
                status: "failed".into(),
                references: vec![],
                further_questions: vec![],
                elapsed_time: None,
                error_message: "trade password required for mcp tool".into(),
            },
            AgentEvent::ChatFinished {
                error_message: "trade_password validation failed".into(),
            },
        ] {
            let events = map_agent_event(&ev);
            assert!(matches!(
                &events[0],
                ChatEvent::TurnError(text)
                    if text == t!("Agent.TradePasswordUnsupported").as_ref()
            ));
        }
    }

    /// A turn the server reports as failed — over a stream that itself ends `Ok` —
    /// must not become the parent of the next turn, or the failure propagates to
    /// every later message and bricks the conversation.
    #[test]
    fn a_server_reported_error_does_not_become_the_parent() {
        use crate::ai::state::ChatEvent;
        let mut state = ChatState::new("chatbot".into(), "welcome".into());
        // One good turn establishes a parent.
        state.apply(ChatEvent::UserPrompt("hi".into()));
        state.apply(ChatEvent::TurnStarted {
            chat_uid: "c1".into(),
            message_id: "m1".into(),
        });
        state.apply(ChatEvent::Delta("hello".into()));
        state.apply(ChatEvent::TurnFinished { error: None });
        assert_eq!(state.parent_message_id.as_deref(), Some("m1"));
        // Then a turn the server fails via chat_finished, while the stream ends Ok
        // (so TurnFinished carries no transport error).
        state.apply(ChatEvent::UserPrompt("and now?".into()));
        state.apply(ChatEvent::TurnStarted {
            chat_uid: "c1".into(),
            message_id: "m2".into(),
        });
        state.apply(ChatEvent::TurnError("Something went wrong".into()));
        state.apply(ChatEvent::TurnFinished { error: None });
        assert_eq!(
            state.parent_message_id.as_deref(),
            Some("m1"),
            "the parent stays at the last message that actually completed"
        );
    }

    use crate::ai::state::ChatState;
    use serde_json::json;

    fn interrupted(interrupt: Value) -> ChatState {
        let mut state = ChatState::new("chatbot".into(), "welcome".into());
        state.chat_uid = Some("c1".into());
        state.message_id = Some("m1".into());
        state.parent_message_id = Some("m0".into());
        state.pending_interrupt = Some(interrupt);
        state
    }

    /// A clarifying question is answered by resuming the paused run, with the
    /// answer keyed by the question the agent asked.
    #[test]
    fn an_answerable_interrupt_resumes_the_paused_run() {
        let state = interrupted(json!({
            "tool_call_id": "tc1",
            "questions": [{ "question": "Which market?" }]
        }));
        match build_request(&state, "HK".into()) {
            ConversationRequest::Continue {
                message_id,
                answers,
                ..
            } => {
                assert_eq!(message_id, "m1");
                assert_eq!(answers["tc1"]["Which market?"], "HK");
            }
            ConversationRequest::New { .. } => panic!("expected a resume, got a new turn"),
        }
    }

    /// The live HITL protocol nests interactions and addresses answers by
    /// `interrupt_id` (not the tool call id). This is the shape used when a
    /// watchlist tool asks the user for permission.
    #[test]
    fn a_nested_authorization_interrupt_resumes_with_its_interrupt_id() {
        let state = interrupted(json!({
            "message_id": 11_820_498,
            "interactions": [{
                "tool_call_id": "call_watchlist",
                "interrupt_id": "authorize_watchlist",
                "type": "authorization",
                "tool_display_name": "Read watchlist",
                "questions": []
            }]
        }));
        match build_request(&state, "Allow".into()) {
            ConversationRequest::Continue { answers, .. } => {
                assert_eq!(answers["authorize_watchlist"]["authorized"], "true");
                assert!(!answers.contains_key("call_watchlist"));
            }
            ConversationRequest::New { .. } => panic!("expected a resume, got a new turn"),
        }
    }

    #[test]
    fn trade_password_interrupt_explains_that_terminal_cannot_continue_it() {
        let interrupt = json!({
            "interactions": [{
                "interrupt_id": "trade_password",
                "type": "trade_password",
                "tool_display_name": "Account details"
            }]
        });
        assert!(!is_answerable(&&interrupt));
        assert!(interrupt_text(&interrupt).contains(t!("Agent.TradePasswordUnsupported").as_ref()));
    }

    #[test]
    fn external_authorization_interrupts_explain_that_terminal_cannot_continue_them() {
        for kind in ["connector_reauth", "openapi_reauth", "data_authorization"] {
            let interrupt = json!({
                "interactions": [{
                    "interrupt_id": "external_auth",
                    "type": kind
                }]
            });
            assert!(!is_answerable(&&interrupt));
            assert!(interrupt_text(&interrupt)
                .contains(t!("Agent.ExternalAuthorizationUnsupported").as_ref()));
        }
    }

    /// An interrupt with nothing to key an answer under would resume with an
    /// empty answer map and leave the run paused — a dead conversation. The reply
    /// goes through as a follow-up instead.
    #[test]
    fn an_unanswerable_interrupt_falls_back_to_a_follow_up() {
        for interrupt in [
            json!({ "tool_call_id": "tc1", "questions": [] }),
            json!({ "tool_call_id": "tc1" }),
            json!({ "tool_call_id": "tc1", "questions": [{ "options": [] }] }),
            json!({ "questions": [{ "question": "Which market?" }] }),
        ] {
            let state = interrupted(interrupt.clone());
            match build_request(&state, "HK".into()) {
                ConversationRequest::New {
                    query,
                    chat_uid,
                    parent_message_id,
                    ..
                } => {
                    assert_eq!(query, "HK");
                    // Same conversation: the agent still sees the reply in context.
                    assert_eq!(chat_uid.as_deref(), Some("c1"));
                    // Parented to the last message that completed — not to the
                    // paused one, which the server cannot build on.
                    assert_eq!(parent_message_id.as_deref(), Some("m0"));
                }
                ConversationRequest::Continue { .. } => {
                    panic!("expected a follow-up for {interrupt}, got a resume")
                }
            }
        }
    }

    /// An unreadable interrupt has to say so: a bare header with no question and
    /// no instruction is where the conversation looked dead.
    #[test]
    fn an_unreadable_interrupt_says_what_to_do() {
        let text = interrupt_text(&json!({ "tool_call_id": "tc1" }));
        assert!(text.contains(t!("Agent.InterruptedUnreadable").as_ref()));
        let text = interrupt_text(&json!({
            "tool_call_id": "tc1",
            "questions": [{ "question": "Which market?" }]
        }));
        assert!(text.contains("Which market?"));
        assert!(text.contains(t!("Agent.InterruptedAnswerHint").as_ref()));
    }

    /// A failed turn must not become the parent of the next one, or every later
    /// message fails the same way and the conversation is bricked.
    #[test]
    fn a_failed_turn_does_not_become_the_parent() {
        use crate::ai::state::ChatEvent;
        let mut state = ChatState::new("chatbot".into(), "welcome".into());
        // One good turn.
        state.apply(ChatEvent::UserPrompt("hi".into()));
        state.apply(ChatEvent::TurnStarted {
            chat_uid: "c1".into(),
            message_id: "m1".into(),
        });
        state.apply(ChatEvent::Delta("hello".into()));
        state.apply(ChatEvent::TurnFinished { error: None });
        assert_eq!(state.parent_message_id.as_deref(), Some("m1"));
        // Then one that fails.
        state.apply(ChatEvent::UserPrompt("and now?".into()));
        state.apply(ChatEvent::TurnStarted {
            chat_uid: "c1".into(),
            message_id: "m2".into(),
        });
        state.apply(ChatEvent::TurnFinished {
            error: Some("Something went wrong".into()),
        });
        assert_eq!(
            state.parent_message_id.as_deref(),
            Some("m1"),
            "the parent stays at the last message that completed"
        );
        match build_request(&state, "retry".into()) {
            ConversationRequest::New {
                parent_message_id, ..
            } => assert_eq!(parent_message_id.as_deref(), Some("m1")),
            ConversationRequest::Continue { .. } => panic!("expected a follow-up"),
        }
    }

    /// A paused turn is the same: until it is resumed it cannot be built on.
    #[test]
    fn a_paused_turn_does_not_become_the_parent() {
        use crate::ai::state::ChatEvent;
        let mut state = ChatState::new("chatbot".into(), "welcome".into());
        state.apply(ChatEvent::UserPrompt("hi".into()));
        state.apply(ChatEvent::TurnStarted {
            chat_uid: "c1".into(),
            message_id: "m1".into(),
        });
        state.apply(ChatEvent::Interrupt(json!({ "tool_call_id": "tc1" })));
        state.apply(ChatEvent::TurnFinished { error: None });
        assert_eq!(state.parent_message_id, None);
        // The paused id is still what a resume would address.
        assert_eq!(state.message_id.as_deref(), Some("m1"));
    }
}
