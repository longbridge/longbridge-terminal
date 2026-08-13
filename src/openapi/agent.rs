use async_trait::async_trait;
use futures::{stream::BoxStream, StreamExt};
use longbridge::agent::ConversationStreamEvent;
use longbridge_ai_acp::{AgentBackend, AgentEvent, BackendError};
use std::path::Path;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OpenApiAgentSession {
    conversation_id: Option<String>,
    parent_message_id: Option<String>,
    pending_interaction: Option<PendingInteraction>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingInteraction {
    groups: Vec<PendingQuestionGroup>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingQuestionGroup {
    answer_key: String,
    questions: Vec<PendingQuestion>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingQuestion {
    text: String,
    options: Vec<PendingOption>,
    multi_select: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingOption {
    label: String,
    description: String,
}

fn answers_for(
    pending: &PendingInteraction,
    input: &str,
) -> Result<longbridge::agent::AnswersByToolCall, BackendError> {
    if pending.groups.len() != 1 {
        return serde_json::from_str(input).map_err(|error| {
            format!("multiple interactions require answers_by_tool_call JSON: {error}").into()
        });
    }
    let group = &pending.groups[0];
    if group.questions.is_empty() {
        return Ok([(
            group.answer_key.clone(),
            [("authorized".to_string(), input.trim().to_string())]
                .into_iter()
                .collect(),
        )]
        .into_iter()
        .collect());
    }
    let answers = if group.questions.len() == 1 {
        [(
            group.questions[0].text.clone(),
            normalize_answer(&group.questions[0], input)?,
        )]
        .into_iter()
        .collect()
    } else if let Ok(object) =
        serde_json::from_str::<std::collections::HashMap<String, String>>(input)
    {
        let missing = group
            .questions
            .iter()
            .filter(|question| !object.contains_key(&question.text))
            .map(|question| question.text.clone())
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(
                format!("answers JSON is missing questions: {}", missing.join(", ")).into(),
            );
        }
        object
    } else {
        let lines = input.lines().map(str::trim).collect::<Vec<_>>();
        if lines.len() != group.questions.len() {
            return Err(format!(
                "this agent needs {} answers; send one answer per line or a JSON object keyed by question",
                group.questions.len()
            )
            .into());
        }
        group
            .questions
            .iter()
            .map(|question| question.text.clone())
            .zip(
                group
                    .questions
                    .iter()
                    .zip(lines)
                    .map(|(question, answer)| normalize_answer(question, answer))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter(),
            )
            .collect()
    };

    Ok([(group.answer_key.clone(), answers)].into_iter().collect())
}

fn normalize_answer(question: &PendingQuestion, input: &str) -> Result<String, BackendError> {
    let input = input.trim();
    if question.options.is_empty() {
        return Ok(input.to_owned());
    }
    let selections: Vec<String> = if question.multi_select && input.starts_with('[') {
        serde_json::from_str(input)
            .map_err(|error| format!("invalid multi-select JSON array: {error}"))?
    } else if question.multi_select {
        input
            .split(',')
            .map(|value| value.trim().to_owned())
            .collect()
    } else {
        vec![input.to_owned()]
    };
    Ok(selections
        .into_iter()
        .map(|selection| {
            selection
                .parse::<usize>()
                .ok()
                .and_then(|index| index.checked_sub(1))
                .and_then(|index| question.options.get(index))
                .or_else(|| {
                    question
                        .options
                        .iter()
                        .find(|option| option.label == selection)
                })
                .map_or(selection, |option| option.description.clone())
        })
        .collect::<Vec<_>>()
        .join(", "))
}

#[derive(Clone)]
pub struct OpenApiAgent {
    context: longbridge::AgentContext,
    agent_id: String,
}

impl OpenApiAgent {
    pub fn new(context: longbridge::AgentContext, agent_id: impl Into<String>) -> Self {
        Self {
            context,
            agent_id: agent_id.into(),
        }
    }
}

#[async_trait]
impl AgentBackend for OpenApiAgent {
    type Session = OpenApiAgentSession;

    async fn prompt(
        &self,
        session: Self::Session,
        prompt: String,
        _cwd: &Path,
    ) -> Result<BoxStream<'static, Result<AgentEvent<Self::Session>, BackendError>>, BackendError>
    {
        let stream = if let Some(pending) = &session.pending_interaction {
            self.context
                .continue_conversation_streamed(
                    self.agent_id.clone(),
                    session
                        .conversation_id
                        .clone()
                        .ok_or("missing conversation id")?,
                    session
                        .parent_message_id
                        .clone()
                        .ok_or("missing message id")?,
                    answers_for(pending, &prompt)?,
                )
                .await?
                .boxed()
        } else {
            self.context
                .conversation_streamed(
                    self.agent_id.clone(),
                    prompt,
                    session.conversation_id,
                    session.parent_message_id,
                )
                .await?
                .boxed()
        };

        Ok(stream
            .filter_map(|event| async move {
                match event {
                    Ok(ConversationStreamEvent::Message(message))
                        if message.message_type == "think" =>
                    {
                        let metadata = event_metadata("message", &message);
                        Some(Ok(AgentEvent::Content {
                            text: message.text,
                            thought: true,
                            metadata,
                        }))
                    }
                    Ok(ConversationStreamEvent::Message(message)) => {
                        let metadata = event_metadata("message", &message);
                        Some(Ok(AgentEvent::Content {
                            text: message.text,
                            thought: false,
                            metadata,
                        }))
                    }
                    Ok(ConversationStreamEvent::NodeToolUseStarted(tool)) => {
                        let metadata = event_metadata("node_tool_use_started", &tool);
                        Some(Ok(AgentEvent::ToolStartedRich {
                            id: tool.tool_use_id.clone(),
                            title: tool.tool_name.clone(),
                            raw_input: serde_json::from_str(&tool.tool_args).ok(),
                            metadata,
                        }))
                    }
                    Ok(ConversationStreamEvent::NodeToolUseFinished(tool)) => {
                        let metadata = event_metadata("node_tool_use_finished", &tool);
                        let output = tool
                            .outputs
                            .data
                            .or_else(|| tool.outputs.text.map(serde_json::Value::String))
                            .or_else(|| {
                                (!tool.error.trim().is_empty())
                                    .then(|| serde_json::Value::String(tool.error.clone()))
                            });
                        Some(Ok(AgentEvent::ToolFinishedRich {
                            id: tool.tool_use_id,
                            title: tool.tool_name,
                            success: tool.status == "succeeded",
                            raw_output: output,
                            metadata,
                        }))
                    }
                    Ok(ConversationStreamEvent::HumanInteractionRequired(response)) => {
                        let interrupt = response.interrupt?;
                        let metadata = serde_json::to_value(&interrupt).ok();
                        let interaction_groups = if interrupt.interactions.is_empty() {
                            vec![(
                                interrupt.tool_call_id.clone(),
                                String::new(),
                                String::new(),
                                interrupt.questions,
                            )]
                        } else {
                            interrupt
                                .interactions
                                .into_iter()
                                .map(|interaction| {
                                    let key = if interaction.interrupt_id.is_empty() {
                                        interaction.tool_call_id
                                    } else {
                                        interaction.interrupt_id
                                    };
                                    (
                                        key,
                                        interaction.interaction_type,
                                        interaction.tool_name,
                                        interaction.questions,
                                    )
                                })
                                .collect()
                        };
                        if let Some((answer_key, _, tool_name, _)) = interaction_groups
                            .iter()
                            .find(|(_, interaction_type, _, _)| interaction_type == "authorization")
                        {
                            let state = OpenApiAgentSession {
                                conversation_id: Some(response.chat_uid),
                                parent_message_id: Some(response.message_id),
                                pending_interaction: Some(PendingInteraction {
                                    groups: vec![PendingQuestionGroup {
                                        answer_key: answer_key.clone(),
                                        questions: Vec::new(),
                                    }],
                                }),
                            };
                            return Some(Ok(AgentEvent::PermissionRequired {
                                session: state,
                                tool_call_id: interrupt.tool_call_id,
                                title: if tool_name.is_empty() {
                                    "Access your Longbridge account".to_string()
                                } else {
                                    tool_name.clone()
                                },
                                metadata,
                            }));
                        }
                        if interaction_groups.iter().any(|(_, interaction_type, _, _)| {
                            interaction_type == "trade_password"
                        }) {
                            let state = OpenApiAgentSession {
                                conversation_id: Some(response.chat_uid),
                                parent_message_id: Some(response.message_id),
                                pending_interaction: None,
                            };
                            return Some(Ok(AgentEvent::Notice {
                                session: state,
                                text: "This operation requires trade-password verification in an official Longbridge client. ACP cannot collect or verify a trade password.".to_string(),
                                metadata,
                            }));
                        }
                        if interaction_groups.iter().any(|(_, interaction_type, _, _)| {
                            interaction_type == "data_authorization"
                        }) {
                            let state = OpenApiAgentSession {
                                conversation_id: Some(response.chat_uid),
                                parent_message_id: Some(response.message_id),
                                pending_interaction: None,
                            };
                            return Some(Ok(AgentEvent::Notice {
                                session: state,
                                text: "This operation requires signing a securities-data authorization in an official Longbridge client. ACP cannot complete that external authorization flow.".to_string(),
                                metadata,
                            }));
                        }
                        let mut groups = Vec::new();
                        let mut questions = Vec::new();
                        for (answer_key, _, _, source_questions) in interaction_groups {
                            let mut pending_questions = Vec::new();
                            questions.extend(source_questions.into_iter().map(|question| {
                                let options = question
                                    .options
                                    .iter()
                                    .map(|option| PendingOption {
                                        label: option.label.clone(),
                                        description: option.description.clone(),
                                    })
                                    .collect::<Vec<_>>();
                                pending_questions.push(PendingQuestion {
                                    text: question.question.clone(),
                                    options: options.clone(),
                                    multi_select: question.multi_select,
                                });
                                render_question(&question.question, &options, question.multi_select)
                            }));
                            groups.push(PendingQuestionGroup {
                                answer_key,
                                questions: pending_questions,
                            });
                        }
                        let state = OpenApiAgentSession {
                            conversation_id: Some(response.chat_uid),
                            parent_message_id: Some(response.message_id),
                            pending_interaction: Some(PendingInteraction { groups }),
                        };
                        Some(Ok(AgentEvent::NeedsInput {
                            session: state,
                            questions,
                            metadata,
                        }))
                    }
                    Ok(ConversationStreamEvent::WorkflowFinished(response)) => {
                        let metadata = serde_json::json!({
                            "status": response.status,
                            "error_code": response.error.as_ref().map_or(0, |error| error.code),
                            "error_message": response.error.as_ref().map_or("", |error| error.message.as_str()),
                            "elapsed_time": response.elapsed_time,
                            "outputs": {
                                "answer": response.answer,
                                "references": response.references,
                                "further_questions": response.further_questions,
                            },
                            "chat_uid": response.chat_uid,
                            "message_id": response.message_id,
                        });
                        Some(Ok(AgentEvent::Completed {
                            session: OpenApiAgentSession {
                                conversation_id: metadata
                                    .get("chat_uid")
                                    .and_then(serde_json::Value::as_str)
                                    .map(str::to_owned),
                                parent_message_id: metadata
                                    .get("message_id")
                                    .and_then(serde_json::Value::as_str)
                                    .map(str::to_owned),
                                pending_interaction: None,
                            },
                            metadata,
                        }))
                    }
                    Ok(ConversationStreamEvent::ChatStarted(payload)) => Some(Ok(native_event(
                        "chat_started",
                        serde_json::to_value(payload).ok()?,
                    ))),
                    Ok(ConversationStreamEvent::WorkflowStarted(payload)) => Some(Ok(
                        native_event("workflow_started", serde_json::to_value(payload).ok()?),
                    )),
                    Ok(ConversationStreamEvent::Ping) => {
                        Some(Ok(native_event("ping", serde_json::Value::Null)))
                    }
                    Ok(ConversationStreamEvent::ThinkingStarted(payload)) => Some(Ok(
                        native_event("thinking_started", serde_json::to_value(payload).ok()?),
                    )),
                    Ok(ConversationStreamEvent::ThinkingFinished(payload)) => Some(Ok(
                        native_event("thinking_finished", serde_json::to_value(payload).ok()?),
                    )),
                    Ok(ConversationStreamEvent::SubagentStarted(payload)) => {
                        let metadata = event_metadata("subagent_started", &payload);
                        let title = if payload.goal.is_empty() {
                            "Subagent".to_string()
                        } else {
                            payload.goal.clone()
                        };
                        Some(Ok(AgentEvent::ToolStartedRich {
                            id: payload.tool_use_id,
                            title,
                            raw_input: Some(serde_json::json!({ "prompt": payload.prompt })),
                            metadata,
                        }))
                    }
                    Ok(ConversationStreamEvent::SubagentProgress(payload)) => Some(Ok(
                        native_event("subagent_progress", serde_json::to_value(payload).ok()?),
                    )),
                    Ok(ConversationStreamEvent::SubagentFinished(payload)) => {
                        let metadata = event_metadata("subagent_finished", &payload);
                        let title = payload
                            .outputs
                            .goal
                            .clone()
                            .filter(|goal| !goal.is_empty())
                            .unwrap_or_else(|| "Subagent".to_string());
                        Some(Ok(AgentEvent::ToolFinishedRich {
                            id: payload.tool_use_id,
                            title,
                            success: payload.status == "succeeded",
                            raw_output: serde_json::to_value(payload.outputs).ok(),
                            metadata,
                        }))
                    }
                    Ok(ConversationStreamEvent::AgentToolStarted(payload)) => {
                        let metadata = event_metadata("agent_tool_started", &payload);
                        let title = if payload.title.is_empty() {
                            payload.agent_tool_name.clone()
                        } else {
                            payload.title.clone()
                        };
                        Some(Ok(AgentEvent::ToolStartedRich {
                            id: payload.tool_use_id,
                            title,
                            raw_input: serde_json::from_str(&payload.tool_args).ok(),
                            metadata,
                        }))
                    }
                    Ok(ConversationStreamEvent::AgentToolProgress(payload)) => Some(Ok(
                        native_event("agent_tool_progress", serde_json::to_value(payload).ok()?),
                    )),
                    Ok(ConversationStreamEvent::AgentToolFinished(payload)) => {
                        let metadata = event_metadata("agent_tool_finished", &payload);
                        Some(Ok(AgentEvent::ToolFinishedRich {
                            id: payload.tool_use_id,
                            title: payload.agent_tool_name,
                            success: payload.status == "succeeded",
                            raw_output: payload.outputs,
                            metadata,
                        }))
                    }
                    Ok(ConversationStreamEvent::QueryMasked(payload)) => Some(Ok(native_event(
                        "query_masked",
                        serde_json::to_value(payload).ok()?,
                    ))),
                    Ok(ConversationStreamEvent::PlanChanged(payload)) => Some(Ok(native_event(
                        "plan_changed",
                        serde_json::to_value(payload).ok()?,
                    ))),
                    Ok(ConversationStreamEvent::ContextCompressStarted(payload)) => {
                        Some(Ok(native_event(
                            "context_compress_started",
                            serde_json::to_value(payload).ok()?,
                        )))
                    }
                    Ok(ConversationStreamEvent::ContextCompressFinished(payload)) => {
                        Some(Ok(native_event(
                            "context_compress_finished",
                            serde_json::to_value(payload).ok()?,
                        )))
                    }
                    Ok(ConversationStreamEvent::ChatFinished(payload)) => Some(Ok(native_event(
                        "chat_finished",
                        serde_json::to_value(payload).ok()?,
                    ))),
                    Ok(ConversationStreamEvent::ChatTitleUpdated(payload)) => Some(Ok(
                        native_event("chat_title_updated", serde_json::to_value(payload).ok()?),
                    )),
                    Ok(ConversationStreamEvent::Other { event, data }) => {
                        Some(Ok(native_event(&event, data)))
                    }
                    Err(error) => Some(Err(Box::new(error) as BackendError)),
                }
            })
            .boxed())
    }
}

fn native_event(event: &str, data: serde_json::Value) -> AgentEvent<OpenApiAgentSession> {
    AgentEvent::Extension {
        namespace: "longbridge.ai/event".to_string(),
        event: event.to_string(),
        data,
    }
}

fn event_metadata<T: serde::Serialize>(event: &str, data: &T) -> serde_json::Value {
    serde_json::json!({
        "event": event,
        "data": serde_json::to_value(data).unwrap_or(serde_json::Value::Null),
    })
}

fn render_question(question: &str, options: &[PendingOption], multi_select: bool) -> String {
    if options.is_empty() {
        return question.to_owned();
    }
    format!(
        "{question}\n{}{}",
        options
            .iter()
            .enumerate()
            .map(|(index, option)| match option.label.is_empty() {
                true => format!("{}. {}", index + 1, option.description),
                false => format!("{}. {} — {}", index + 1, option.label, option.description),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        if multi_select {
            "\nMultiple selections are allowed; reply with option numbers separated by commas."
        } else {
            ""
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_without_questions_uses_authorized_answer() {
        let pending = PendingInteraction {
            groups: vec![PendingQuestionGroup {
                answer_key: "authorization-1".into(),
                questions: vec![],
            }],
        };
        assert_eq!(
            answers_for(&pending, "true").unwrap()["authorization-1"]["authorized"],
            "true"
        );
    }

    #[test]
    fn option_label_is_resolved_to_description_for_continue_api() {
        let pending = PendingInteraction {
            groups: vec![PendingQuestionGroup {
                answer_key: "call-1".into(),
                questions: vec![PendingQuestion {
                    text: "你今天想从哪个方向开始看？".into(),
                    options: vec![PendingOption {
                        label: "分析近期涨跌".into(),
                        description: "查你关注标的近期的涨跌表现和异动原因".into(),
                    }],
                    multi_select: false,
                }],
            }],
        };
        assert_eq!(
            answers_for(&pending, "分析近期涨跌").unwrap()["call-1"]["你今天想从哪个方向开始看？"],
            "查你关注标的近期的涨跌表现和异动原因"
        );
    }

    #[test]
    fn question_answer_keeps_the_original_question_as_continue_key() {
        let pending = PendingInteraction {
            groups: vec![PendingQuestionGroup {
                answer_key: "ask-1".into(),
                questions: vec![PendingQuestion {
                    text: "Which market?".into(),
                    options: vec![],
                    multi_select: false,
                }],
            }],
        };
        assert_eq!(
            answers_for(&pending, "US").unwrap()["ask-1"]["Which market?"],
            "US"
        );
    }
    #[test]
    fn one_question_accepts_free_form_answer() {
        let pending = PendingInteraction {
            groups: vec![PendingQuestionGroup {
                answer_key: "tool-1".into(),
                questions: vec![PendingQuestion {
                    text: "Continue?".into(),
                    options: vec![],
                    multi_select: false,
                }],
            }],
        };
        let answers = answers_for(&pending, "yes").expect("answers");
        assert_eq!(answers["tool-1"]["Continue?"], "yes");
    }

    #[test]
    fn multiple_questions_accept_one_line_each() {
        let pending = PendingInteraction {
            groups: vec![PendingQuestionGroup {
                answer_key: "tool-1".into(),
                questions: vec![
                    PendingQuestion {
                        text: "Market?".into(),
                        options: vec![],
                        multi_select: false,
                    },
                    PendingQuestion {
                        text: "Period?".into(),
                        options: vec![],
                        multi_select: false,
                    },
                ],
            }],
        };
        let answers = answers_for(&pending, "US\n1 month").expect("answers");
        assert_eq!(answers["tool-1"]["Market?"], "US");
        assert_eq!(answers["tool-1"]["Period?"], "1 month");
    }
}
