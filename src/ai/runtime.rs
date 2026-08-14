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

/// Build the conversation request for the current input: a fresh chat, a
/// follow-up, or an answer to a pending clarifying question.
pub fn build_request(state: &ChatState, query: String) -> ConversationRequest {
    match (
        state.pending_interrupt.as_ref(),
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
            parent_message_id: state.message_id.clone(),
        },
    }
}

/// Spawn the turn: stream the conversation, mapping each SDK event to
/// [`ChatEvent`]s pushed onto `tx`. The returned handle is aborted on cancel
/// (dropping the SSE stream). The closure captures only the `Send` sender, so
/// the task is `Send` for `tokio::spawn`.
pub fn spawn_turn(req: ConversationRequest, tx: UnboundedSender<ChatEvent>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut forward = |ev: AgentEvent| {
            for chat_event in map_agent_event(&ev) {
                let _ = tx.send(chat_event);
            }
        };
        let result = stream_conversation(req, false, &mut forward).await;
        let _ = tx.send(ChatEvent::TurnFinished {
            error: result.err().map(|e| e.to_string()),
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
        AgentEvent::ToolUseStarted { tool_name } => vec![ChatEvent::Status(
            t!("Agent.CallingTool", name = tool_name).to_string(),
        )],
        AgentEvent::ToolUseFinished { tool_name, status } => {
            let mut out = vec![ChatEvent::Status(t!("Agent.Generating").to_string())];
            if tool_failed(status) {
                out.push(ChatEvent::ToolFailed(tool_name.clone()));
            }
            out
        }
        AgentEvent::WorkflowFinished {
            references,
            further_questions,
            ..
        } => {
            if references.is_empty() && further_questions.is_empty() {
                Vec::new()
            } else {
                vec![ChatEvent::Meta {
                    references: references.clone(),
                    further: further_questions.clone(),
                }]
            }
        }
        AgentEvent::HumanInteractionRequired { interrupt } => vec![
            ChatEvent::Delta(interrupt_text(interrupt)),
            ChatEvent::Interrupt(interrupt.clone()),
        ],
        AgentEvent::ChatFinished { error_message } if !error_message.is_empty() => {
            vec![ChatEvent::Delta(format!("\n[error] {error_message}"))]
        }
        AgentEvent::ThinkingFinished
        | AgentEvent::ChatFinished { .. }
        | AgentEvent::Unknown { .. } => Vec::new(),
    }
}

fn tool_failed(status: &str) -> bool {
    let s = status.to_ascii_lowercase();
    s.contains("fail") || s.contains("error") || s == "rejected"
}

/// Map one free-text answer onto the interrupt's tool-call questions.
fn build_answers(interrupt: &Value, answer: &str) -> longbridge::agent::AnswersByToolCall {
    let mut by_tool_call: longbridge::agent::AnswersByToolCall = HashMap::new();
    let tool_call_id = interrupt
        .get("tool_call_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let inner = by_tool_call.entry(tool_call_id).or_default();
    if let Some(questions) = interrupt.get("questions").and_then(Value::as_array) {
        for q in questions {
            if let Some(question) = q.get("question").and_then(Value::as_str) {
                inner.insert(question.to_string(), answer.to_string());
            }
        }
    }
    by_tool_call
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

fn interrupt_text(interrupt: &Value) -> String {
    let mut out = format!("\n{}", t!("Agent.Interrupted"));
    if let Some(questions) = interrupt.get("questions").and_then(Value::as_array) {
        for q in questions {
            if let Some(text) = q.get("question").and_then(Value::as_str) {
                out.push_str("\n- ");
                out.push_str(text);
            }
        }
    }
    out
}
