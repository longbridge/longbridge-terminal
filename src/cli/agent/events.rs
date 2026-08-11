//! SSE event model for agent conversations.
//!
//! Wire format: `event:message\ndata:{"event":"<type>","data":{...}}\n\n`.
//! The outer SSE event name is always `message`; dispatch happens on the
//! inner `event` field. Unknown inner events map to `AgentEvent::Unknown`
//! so new server-side event types never break the CLI.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    ChatStarted {
        chat_uid: String,
        message_id: String,
    },
    AnswerDelta {
        text: String,
    },
    ProcessDelta {
        text: String,
    },
    ThinkingStarted,
    ThinkingFinished,
    ToolUseStarted {
        tool_name: String,
    },
    ToolUseFinished {
        tool_name: String,
        status: String,
    },
    WorkflowFinished {
        status: String,
        outputs: Value,
        elapsed_time: Option<f64>,
        error_message: String,
    },
    HumanInteractionRequired {
        interrupt: Value,
    },
    ChatFinished {
        error_message: String,
    },
    Unknown {
        event: String,
    },
}

/// Render a JSON id that may be a number or a string as a plain string.
fn id_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn str_field(data: &Value, key: &str) -> String {
    data.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Parse one SSE `data:` payload. Returns `None` for unparseable JSON.
pub fn parse_data_line(payload: &str) -> Option<AgentEvent> {
    let frame: Value = serde_json::from_str(payload).ok()?;
    let event = frame
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let data = frame.get("data").cloned().unwrap_or(Value::Null);
    let ev = match event {
        "chat_started" => AgentEvent::ChatStarted {
            chat_uid: str_field(&data, "chat_uid"),
            message_id: data.get("message_id").map(id_string).unwrap_or_default(),
        },
        "message" => {
            let text = str_field(&data, "text");
            match data.get("type").and_then(Value::as_str) {
                Some("answer") => AgentEvent::AnswerDelta { text },
                _ => AgentEvent::ProcessDelta { text },
            }
        }
        "thinking_started" => AgentEvent::ThinkingStarted,
        "thinking_finished" => AgentEvent::ThinkingFinished,
        "node_tool_use_started" => AgentEvent::ToolUseStarted {
            tool_name: str_field(&data, "tool_name"),
        },
        "node_tool_use_finished" => AgentEvent::ToolUseFinished {
            tool_name: str_field(&data, "tool_name"),
            status: str_field(&data, "status"),
        },
        "workflow_finished" => AgentEvent::WorkflowFinished {
            status: str_field(&data, "status"),
            outputs: data.get("outputs").cloned().unwrap_or(Value::Null),
            elapsed_time: data.get("elapsed_time").and_then(Value::as_f64),
            error_message: str_field(&data, "error_message"),
        },
        "human_interaction_required" => AgentEvent::HumanInteractionRequired { interrupt: data },
        "chat_finished" => AgentEvent::ChatFinished {
            error_message: str_field(&data, "error_message"),
        },
        other => AgentEvent::Unknown {
            event: other.to_string(),
        },
    };
    Some(ev)
}

/// Accumulates raw response bytes and yields complete SSE `data:` payloads.
///
/// Network chunks can split a frame anywhere, including mid-UTF-8-codepoint,
/// so buffering happens at the byte level and lines are only decoded once a
/// `\n` is seen.
#[derive(Default)]
pub struct SseLineBuffer {
    buf: Vec<u8>,
}

impl SseLineBuffer {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut payloads = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);
            if let Some(rest) = line.strip_prefix("data:") {
                payloads.push(rest.trim_start().to_string());
            }
        }
        payloads
    }
}

/// Structured widget extracted from the answer markdown.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind")]
pub enum Widget {
    #[serde(rename = "vis-chart")]
    VisChart { spec: Value },
    #[serde(rename = "x-widget")]
    XWidget { src: String },
}

/// Final result of one conversation round, assembled from the SSE stream.
#[derive(Debug, Default, serde::Serialize)]
pub struct ChatOutcome {
    pub chat_uid: String,
    pub message_id: String,
    pub status: String,
    pub answer: String,
    pub widgets: Vec<Widget>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub further_questions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interrupt: Option<Value>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error_message: String,
}

/// Folds `AgentEvent`s into a `ChatOutcome`.
#[derive(Default)]
pub struct ChatAggregator {
    outcome: ChatOutcome,
}

impl ChatAggregator {
    pub fn push(&mut self, ev: &AgentEvent) {
        match ev {
            AgentEvent::ChatStarted {
                chat_uid,
                message_id,
            } => {
                self.outcome.chat_uid.clone_from(chat_uid);
                self.outcome.message_id.clone_from(message_id);
            }
            AgentEvent::AnswerDelta { text } => self.outcome.answer.push_str(text),
            AgentEvent::HumanInteractionRequired { interrupt } => {
                self.outcome.status = "interrupted".to_string();
                self.outcome.interrupt = Some(interrupt.clone());
            }
            AgentEvent::WorkflowFinished {
                status,
                outputs,
                elapsed_time,
                error_message,
            } => {
                // An interrupt verdict must not be overwritten by the
                // workflow teardown that follows it.
                if self.outcome.status.is_empty() {
                    self.outcome.status.clone_from(status);
                }
                self.outcome.elapsed_time = *elapsed_time;
                self.outcome.error_message.clone_from(error_message);
                if let Some(refs) = outputs.get("references").and_then(Value::as_array) {
                    self.outcome.references.clone_from(refs);
                }
                if let Some(qs) = outputs.get("further_questions").and_then(Value::as_array) {
                    self.outcome.further_questions = qs
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToString::to_string)
                        .collect();
                }
            }
            AgentEvent::ChatFinished { error_message } => {
                if !error_message.is_empty() && self.outcome.error_message.is_empty() {
                    self.outcome.error_message.clone_from(error_message);
                }
            }
            _ => {}
        }
    }

    pub fn finish(mut self) -> ChatOutcome {
        if self.outcome.status.is_empty() {
            self.outcome.status = "unknown".to_string();
        }
        self.outcome.widgets = extract_widgets(&self.outcome.answer);
        self.outcome
    }
}

/// Scan answer markdown for embedded widgets. The answer text itself is
/// left untouched; this is a read-only extraction.
pub fn extract_widgets(answer: &str) -> Vec<Widget> {
    let mut widgets = Vec::new();
    // Fenced ```vis-chart blocks
    let mut rest = answer;
    while let Some(start) = rest.find("```vis-chart") {
        let after = &rest[start + "```vis-chart".len()..];
        let Some(end) = after.find("```") else { break };
        if let Ok(spec) = serde_json::from_str::<Value>(after[..end].trim()) {
            widgets.push(Widget::VisChart { spec });
        }
        rest = &after[end + 3..];
    }
    // <x-widget src="..."> tags
    let mut rest = answer;
    while let Some(start) = rest.find("<x-widget") {
        let after = &rest[start..];
        let Some(tag_end) = after.find('>') else {
            break;
        };
        let tag = &after[..tag_end];
        if let Some(src_start) = tag.find("src=\"") {
            let src_rest = &tag[src_start + 5..];
            if let Some(src_end) = src_rest.find('"') {
                widgets.push(Widget::XWidget {
                    src: src_rest[..src_end].to_string(),
                });
            }
        }
        rest = &after[tag_end..];
    }
    widgets
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/agent_sse_stream.txt");
    const GOLDEN_ANSWER: &str = include_str!("../../../tests/fixtures/agent_sse_answer.md");

    fn fixture_events() -> Vec<AgentEvent> {
        let mut buf = SseLineBuffer::default();
        let mut events = Vec::new();
        for payload in buf.push(FIXTURE.as_bytes()) {
            if let Some(ev) = parse_data_line(&payload) {
                events.push(ev);
            }
        }
        events
    }

    #[test]
    fn full_fixture_parses_without_panic() {
        let events = fixture_events();
        assert!(
            events.len() > 500,
            "expected 500+ events, got {}",
            events.len()
        );
    }

    #[test]
    fn chat_started_extracts_ids() {
        let events = fixture_events();
        let Some(AgentEvent::ChatStarted {
            chat_uid,
            message_id,
        }) = events.first()
        else {
            panic!("first event must be ChatStarted, got {:?}", events.first());
        };
        assert_eq!(chat_uid, "hfn2l0ccrmv1u");
        // message_id is numeric in the wire format; we stringify it
        assert_eq!(message_id, "13025051");
    }

    #[test]
    fn answer_deltas_concatenate_to_golden() {
        let answer: String = fixture_events()
            .iter()
            .filter_map(|e| match e {
                AgentEvent::AnswerDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(answer, GOLDEN_ANSWER);
    }

    #[test]
    fn unknown_events_are_tolerated() {
        // `ping` and `chat_title_updated` are real but undocumented events
        let events = fixture_events();
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::Unknown { event } if event == "ping"
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::Unknown { event } if event == "chat_title_updated"
        )));
    }

    #[test]
    fn workflow_finished_extracts_outputs() {
        let events = fixture_events();
        let Some(AgentEvent::WorkflowFinished {
            status,
            outputs,
            elapsed_time,
            ..
        }) = events
            .iter()
            .find(|e| matches!(e, AgentEvent::WorkflowFinished { .. }))
        else {
            panic!("no WorkflowFinished in fixture");
        };
        assert_eq!(status, "succeeded");
        assert!(elapsed_time.unwrap() > 100.0);
        assert!(outputs.get("further_questions").is_some());
        assert!(outputs.get("references").is_some());
        // The fixture was sanitized: the giant LLM context must not be present
        assert!(outputs.get("context").is_none());
    }

    #[test]
    fn tool_events_extract_names() {
        let events = fixture_events();
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolUseStarted { tool_name } if tool_name == "获取证券 K 线数据"
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolUseFinished { status, .. } if status == "succeeded"
        )));
    }

    #[test]
    fn line_buffer_handles_split_chunks() {
        let mut buf = SseLineBuffer::default();
        let frame = "event:message\ndata:{\"event\":\"thinking_started\",\"data\":{}}\n\n";
        let (a, b) = frame.as_bytes().split_at(20); // split mid-line
        assert!(buf.push(a).is_empty());
        let payloads = buf.push(b);
        assert_eq!(payloads.len(), 1);
        assert_eq!(
            parse_data_line(&payloads[0]),
            Some(AgentEvent::ThinkingStarted)
        );
    }

    #[test]
    fn human_interaction_required_maps_to_interrupt() {
        let payload = r#"{"event":"human_interaction_required","data":{"tool_call_id":"call_abc","questions":[{"question":"Which period?","choices":["1w","1m"]}]}}"#;
        let Some(AgentEvent::HumanInteractionRequired { interrupt }) = parse_data_line(payload)
        else {
            panic!("expected HumanInteractionRequired");
        };
        assert_eq!(interrupt["tool_call_id"], "call_abc");
    }

    #[test]
    fn unparseable_payload_returns_none() {
        assert_eq!(parse_data_line("not json"), None);
    }

    #[test]
    fn aggregator_assembles_outcome_from_fixture() {
        let mut agg = ChatAggregator::default();
        for ev in fixture_events() {
            agg.push(&ev);
        }
        let outcome = agg.finish();
        assert_eq!(outcome.chat_uid, "hfn2l0ccrmv1u");
        assert_eq!(outcome.message_id, "13025051");
        assert_eq!(outcome.status, "succeeded");
        assert_eq!(outcome.answer, GOLDEN_ANSWER);
        assert_eq!(outcome.references.len(), 2);
        assert_eq!(outcome.further_questions.len(), 3);
        assert!(outcome.elapsed_time.unwrap() > 100.0);
        assert!(outcome.interrupt.is_none());
        // one vis-chart + one x-widget in the recorded answer
        assert_eq!(outcome.widgets.len(), 2);
    }

    #[test]
    fn aggregator_marks_interrupted() {
        let mut agg = ChatAggregator::default();
        agg.push(
            &parse_data_line(r#"{"event":"chat_started","data":{"chat_uid":"c1","message_id":7}}"#)
                .unwrap(),
        );
        agg.push(&parse_data_line(r#"{"event":"human_interaction_required","data":{"tool_call_id":"call_a","questions":[{"question":"Which market?"}]}}"#).unwrap());
        let outcome = agg.finish();
        assert_eq!(outcome.status, "interrupted");
        assert_eq!(outcome.interrupt.unwrap()["tool_call_id"], "call_a");
    }

    #[test]
    fn extract_widgets_finds_vis_chart_and_x_widget() {
        let md = "before\n```vis-chart\n{\"type\":\"column\",\"data\":[]}\n```\nmid\n<x-widget src=\"widget://quote/security/detail?symbol=TSLA.US&time_range=1\"></x-widget>\nafter";
        let widgets = extract_widgets(md);
        assert_eq!(widgets.len(), 2);
        assert!(matches!(&widgets[0], Widget::VisChart { spec } if spec["type"] == "column"));
        assert!(matches!(&widgets[1], Widget::XWidget { src } if src.contains("symbol=TSLA.US")));
    }

    #[test]
    fn extract_widgets_skips_malformed_chart_json() {
        let md = "```vis-chart\nnot json\n```";
        assert!(extract_widgets(md).is_empty());
    }

    #[test]
    fn widget_serializes_with_kind_tag() {
        let w = Widget::XWidget {
            src: "widget://x".into(),
        };
        let json = serde_json::to_value(&w).unwrap();
        assert_eq!(json["kind"], "x-widget");
        assert_eq!(json["src"], "widget://x");
    }
}
