use crate::{AgentBackend, AgentEvent, AgentSession, BackendError, PendingInteraction};
use async_trait::async_trait;
use futures::{stream::BoxStream, StreamExt};
use longbridge::agent::ConversationStreamEvent;
use std::{path::Path, sync::Arc};

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

/// Adapter for one published Longbridge AI agent.
#[derive(Clone)]
pub struct LongbridgeAgent {
    context: longbridge::AgentContext,
    agent_id: String,
}

impl LongbridgeAgent {
    #[must_use]
    pub fn new(context: longbridge::AgentContext, agent_id: impl Into<String>) -> Self {
        Self {
            context,
            agent_id: agent_id.into(),
        }
    }

    /// Construct an agent for an explicit API endpoint.
    ///
    /// The caller owns authentication: build `config` with either
    /// `longbridge::Config::from_oauth` (desktop login) or
    /// `longbridge::Config::from_apikey` (`OpenAPI` credentials). No CLI token
    /// storage or process-global state is consulted.
    pub fn from_api(
        mut config: longbridge::Config,
        api_url: impl Into<String>,
        agent_id: impl Into<String>,
    ) -> Result<Self, BackendError> {
        let api_url = api_url.into();
        let parsed = url::Url::parse(&api_url)?;
        let secure = parsed.scheme() == "https";
        let local_development = cfg!(debug_assertions)
            && parsed.scheme() == "http"
            && parsed
                .host_str()
                .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"));
        if !secure && !local_development {
            return Err("Longbridge AI API URL must use HTTPS (HTTP is allowed only for local debug endpoints)".into());
        }
        config.set_http_url(api_url);
        Ok(Self::new(
            longbridge::AgentContext::new(Arc::new(config)),
            agent_id,
        ))
    }

    /// Construct from a fully configured SDK value supplied by the host app.
    #[must_use]
    pub fn from_config(config: Arc<longbridge::Config>, agent_id: impl Into<String>) -> Self {
        Self::new(longbridge::AgentContext::new(config), agent_id)
    }

    #[must_use]
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }
}

#[async_trait]
impl AgentBackend for LongbridgeAgent {
    async fn prompt(
        &self,
        session: AgentSession,
        prompt: String,
        _cwd: &Path,
    ) -> Result<BoxStream<'static, Result<AgentEvent, BackendError>>, BackendError> {
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
                        let state = AgentSession {
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
                        Some(Ok(AgentEvent::Finished(AgentSession {
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

    #[test]
    fn production_api_requires_https() {
        let config = longbridge::Config::from_apikey("key", "secret", "token");
        assert!(LongbridgeAgent::from_api(config, "http://example.com", "agent").is_err());
    }
}
