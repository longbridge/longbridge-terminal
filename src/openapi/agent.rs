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
    tool_call_id: String,
    questions: Vec<String>,
}

fn answers_for(
    pending: &PendingInteraction,
    input: &str,
) -> Result<longbridge::agent::AnswersByToolCall, BackendError> {
    let answers = if pending.questions.len() == 1 {
        [(pending.questions[0].clone(), input.to_owned())]
            .into_iter()
            .collect()
    } else if let Ok(object) =
        serde_json::from_str::<std::collections::HashMap<String, String>>(input)
    {
        let missing = pending
            .questions
            .iter()
            .filter(|question| !object.contains_key(*question))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(
                format!("answers JSON is missing questions: {}", missing.join(", ")).into(),
            );
        }
        object
    } else {
        let lines = input.lines().map(str::trim).collect::<Vec<_>>();
        if lines.len() != pending.questions.len() {
            return Err(format!(
                "this agent needs {} answers; send one answer per line or a JSON object keyed by question",
                pending.questions.len()
            )
            .into());
        }
        pending
            .questions
            .iter()
            .cloned()
            .zip(lines.into_iter().map(str::to_owned))
            .collect()
    };

    Ok([(pending.tool_call_id.clone(), answers)]
        .into_iter()
        .collect())
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
                        Some(Ok(AgentEvent::Thought(message.text)))
                    }
                    Ok(ConversationStreamEvent::Message(message)) => {
                        Some(Ok(AgentEvent::Text(message.text)))
                    }
                    Ok(ConversationStreamEvent::NodeToolUseStarted(tool)) => {
                        Some(Ok(AgentEvent::ToolStarted {
                            id: tool.tool_use_id,
                            title: tool.tool_name,
                            raw_input: serde_json::from_str(&tool.tool_args).ok(),
                        }))
                    }
                    Ok(ConversationStreamEvent::NodeToolUseFinished(tool)) => {
                        let output = tool
                            .outputs
                            .data
                            .or_else(|| tool.outputs.text.map(serde_json::Value::String));
                        Some(Ok(AgentEvent::ToolFinished {
                            id: tool.tool_use_id,
                            title: tool.tool_name,
                            success: tool.status == "succeeded",
                            raw_output: output,
                        }))
                    }
                    Ok(ConversationStreamEvent::HumanInteractionRequired(response)) => {
                        let interrupt = response.interrupt?;
                        let questions = interrupt
                            .questions
                            .into_iter()
                            .map(|question| question.question)
                            .collect::<Vec<_>>();
                        let state = OpenApiAgentSession {
                            conversation_id: Some(response.chat_uid),
                            parent_message_id: Some(response.message_id),
                            pending_interaction: Some(PendingInteraction {
                                tool_call_id: interrupt.tool_call_id,
                                questions: questions.clone(),
                            }),
                        };
                        Some(Ok(AgentEvent::NeedsInput {
                            session: state,
                            questions,
                        }))
                    }
                    Ok(ConversationStreamEvent::WorkflowFinished(response)) => {
                        Some(Ok(AgentEvent::Finished(OpenApiAgentSession {
                            conversation_id: Some(response.chat_uid),
                            parent_message_id: Some(response.message_id),
                            pending_interaction: None,
                        })))
                    }
                    Ok(_) => None,
                    Err(error) => Some(Err(Box::new(error) as BackendError)),
                }
            })
            .boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_question_accepts_free_form_answer() {
        let pending = PendingInteraction {
            tool_call_id: "tool-1".into(),
            questions: vec!["Continue?".into()],
        };
        let answers = answers_for(&pending, "yes").expect("answers");
        assert_eq!(answers["tool-1"]["Continue?"], "yes");
    }

    #[test]
    fn multiple_questions_accept_one_line_each() {
        let pending = PendingInteraction {
            tool_call_id: "tool-1".into(),
            questions: vec!["Market?".into(), "Period?".into()],
        };
        let answers = answers_for(&pending, "US\n1 month").expect("answers");
        assert_eq!(answers["tool-1"]["Market?"], "US");
        assert_eq!(answers["tool-1"]["Period?"], "1 month");
    }
}
