//! Business analytics for `longbridge ai`.
//!
//! Kept out of [`super::tui`] because the reporting is incidental to the view:
//! every function here is one call at a decision point, and gathering them makes
//! the whole event surface readable in one screen.
//!
//! # What is not reported
//!
//! No question text, no answer text, no tool arguments. A question to this agent
//! names the symbols and positions the reader is asking about, which is exactly
//! the shape of thing analytics has no business carrying. Lengths and counts go
//! out; content does not.

use crate::analytics::{event, track};

use super::state::{ChatState, Role};

/// How a turn ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The agent answered.
    Ok,
    /// The stream itself failed — the request never completed.
    StreamError,
    /// The server failed the turn, having accepted it.
    ServerError,
    /// The reader stopped it, or walked away by starting a new conversation.
    Cancelled,
}

impl Outcome {
    /// `result` as reported. Two error kinds share one result so completion rate
    /// is a single filter, with `error_kind` telling them apart.
    const fn result(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::StreamError | Self::ServerError => "error",
            Self::Cancelled => "cancelled",
        }
    }

    const fn error_kind(self) -> Option<&'static str> {
        match self {
            Self::StreamError => Some("stream"),
            Self::ServerError => Some("server"),
            Self::Ok | Self::Cancelled => None,
        }
    }
}

/// The chat was opened. Reported from `main`, which knows whether a token was
/// found before the view is built.
pub fn launch(agent: &str, signed_in: bool) {
    track(
        event::AI_LAUNCH,
        serde_json::json!({ "agent": agent, "signed_in": signed_in }),
    );
}

/// A prompt went out. `query_len` is a character count — never the text.
pub fn turn_start(state: &ChatState, query_len: usize) {
    track(
        event::AI_TURN_START,
        serde_json::json!({
            "agent": state.agent_uid,
            // Whether this is the opening question of a conversation. A first
            // turn has no history to build on and behaves differently enough
            // that mixing the two hides both.
            "is_first_turn": !state.messages.iter().any(|m| m.role == Role::User),
            "query_len": query_len,
        }),
    );
}

/// A turn ended, however it ended.
///
/// Reported for cancelled and abandoned turns too: a start with no end would
/// hang in the warehouse forever, and no completion rate could be derived from a
/// population where an unknown share of turns simply stop being mentioned.
///
/// Call **after** the state has applied the finishing event, so the answer has
/// been folded into the transcript and its length is final — but read the
/// outcome **before**, because applying it clears the server error that
/// distinguishes one failure from the other.
pub fn turn_finish(state: &ChatState, outcome: Outcome) {
    let Some(started) = state.turn_started else {
        // No start recorded means no turn was in flight — a cancel key pressed
        // on an idle chat, most often. Reporting an end without a beginning
        // would inflate the turn count with turns that never happened.
        return;
    };
    let shape = TurnShape::of(state);

    track(
        event::AI_TURN_FINISH,
        serde_json::json!({
            "agent": state.agent_uid,
            "result": outcome.result(),
            "error_kind": outcome.error_kind(),
            "duration_ms": started.at.elapsed().as_millis() as u64,
            // Distinct tools, and how many calls in total: the agent may call
            // one tool repeatedly, and "used 3 tools" and "made 11 calls" are
            // different facts about the same turn.
            "tools": shape.tools,
            "tool_calls": shape.tool_calls,
            "tools_failed": state.tool_failures,
            "answer_len": shape.answer_len,
            "has_references": !state.references.is_empty(),
            "has_further_questions": !state.further.is_empty(),
        }),
    );
}

/// What one turn's transcript amounts to.
///
/// Split out of [`turn_finish`] to be testable: the reporting call itself has no
/// return value and reaches the network, so without this the claim that these
/// numbers describe *this* turn rather than the whole conversation could not be
/// checked by anything.
#[derive(Debug, Default, PartialEq, Eq)]
struct TurnShape<'a> {
    /// Distinct tool names, in the order first called.
    tools: Vec<&'a str>,
    /// Every call, including repeats of the same tool.
    tool_calls: usize,
    /// Characters in the answer this turn produced.
    answer_len: usize,
}

impl<'a> TurnShape<'a> {
    /// Measure the turn in flight. Empty when no turn has started.
    fn of(state: &'a ChatState) -> Self {
        let Some(started) = state.turn_started else {
            return Self::default();
        };
        // Only this turn's slice: `messages` accumulates for the whole
        // conversation, so measuring all of it would report the running total on
        // every turn and make each one look busier than the last.
        let turn = state.messages.get(started.from..).unwrap_or_default();
        let mut tools: Vec<&str> = Vec::new();
        for message in turn.iter().filter(|m| m.role == Role::Tool) {
            if !tools.contains(&message.text.as_str()) {
                tools.push(&message.text);
            }
        }
        Self {
            tools,
            tool_calls: turn.iter().filter(|m| m.role == Role::Tool).count(),
            answer_len: turn
                .iter()
                .rev()
                .find(|m| m.role == Role::Assistant)
                .map_or(0, |m| m.text.chars().count()),
        }
    }
}

/// A fresh conversation was started.
pub fn session_new() {
    track(event::AI_SESSION_NEW, serde_json::json!({}));
}

/// A conversation was reopened from history.
pub fn session_resume() {
    track(event::AI_SESSION_RESUME, serde_json::json!({}));
}

/// The agent was switched.
pub fn agent_switch(from: &str, to: &str) {
    track(
        event::AI_AGENT_SWITCH,
        serde_json::json!({ "from": from, "to": to }),
    );
}

/// The agent stopped to ask the reader something.
pub fn interrupt(question_count: usize) {
    track(
        event::AI_INTERRUPT,
        serde_json::json!({ "question_count": question_count }),
    );
}

/// How that question ended. `answered` is false when the reader left it instead
/// — the interesting half, since an interrupt nobody answers is a dead end in
/// the flow rather than a feature being used.
pub fn interrupt_answered(answered: bool) {
    track(
        event::AI_INTERRUPT_ANSWERED,
        serde_json::json!({ "outcome": if answered { "answered" } else { "abandoned" } }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::state::ChatEvent;

    /// Two complete turns, the second calling a different tool than the first.
    fn two_turns() -> ChatState {
        let mut state = ChatState::default();
        for (prompt, tool, answer) in [
            ("first", "Get Quote", "short"),
            ("second", "Get News", "a much longer answer"),
        ] {
            state.apply(ChatEvent::UserPrompt(prompt.into()));
            state.apply(ChatEvent::ToolStarted(tool.into()));
            state.apply(ChatEvent::ToolFinished {
                name: tool.into(),
                ok: true,
            });
            state.apply(ChatEvent::Delta(answer.into()));
            state.apply(ChatEvent::TurnFinished { error: None });
        }
        state
    }

    /// The numbers describe the turn that just ended, not the conversation.
    /// Measured over all of `messages` instead, every turn would inherit every
    /// earlier turn's tools and each one would look busier than the last.
    #[test]
    fn a_turn_is_measured_without_its_predecessors() {
        let state = two_turns();
        let shape = TurnShape::of(&state);
        assert_eq!(shape.tools, ["Get News"], "the first turn's tool leaked in");
        assert_eq!(shape.tool_calls, 1);
        assert_eq!(shape.answer_len, "a much longer answer".chars().count());
    }

    /// The same tool called twice is one tool and two calls — they answer
    /// different questions and collapsing them loses the retry.
    #[test]
    fn a_repeated_tool_counts_once_by_name_and_twice_by_call() {
        let mut state = ChatState::default();
        state.apply(ChatEvent::UserPrompt("q".into()));
        for _ in 0..2 {
            state.apply(ChatEvent::ToolStarted("Get Quote".into()));
            state.apply(ChatEvent::ToolFinished {
                name: "Get Quote".into(),
                ok: true,
            });
        }
        let shape = TurnShape::of(&state);
        assert_eq!(shape.tools, ["Get Quote"]);
        assert_eq!(shape.tool_calls, 2);
    }

    /// An idle chat measures as nothing, so a stray cancel cannot invent a turn.
    #[test]
    fn an_idle_chat_has_no_shape() {
        assert_eq!(TurnShape::of(&ChatState::default()), TurnShape::default());
    }

    /// Both failure kinds report one `result`, so completion rate stays a single
    /// filter; `error_kind` is what tells them apart.
    #[test]
    fn the_two_failures_share_a_result_but_not_a_kind() {
        assert_eq!(Outcome::StreamError.result(), "error");
        assert_eq!(Outcome::ServerError.result(), "error");
        assert_ne!(
            Outcome::StreamError.error_kind(),
            Outcome::ServerError.error_kind()
        );
    }

    /// A success carries no error kind — an empty string here would land in the
    /// warehouse as a distinct value and every breakdown would grow a phantom
    /// bucket.
    #[test]
    fn a_clean_turn_has_no_error_kind() {
        assert_eq!(Outcome::Ok.error_kind(), None);
        assert_eq!(Outcome::Cancelled.error_kind(), None);
    }
}
