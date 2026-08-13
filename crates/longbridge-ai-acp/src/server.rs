use crate::{AgentBackend, AgentEvent, AgentSession};
use agent_client_protocol::schema::{
    v1::{
        AgentCapabilities, CancelNotification, ContentBlock, ContentChunk, Implementation,
        InitializeRequest, InitializeResponse, NewSessionRequest, NewSessionResponse,
        PromptRequest, PromptResponse, SessionId, SessionNotification, SessionUpdate, StopReason,
        TextContent, ToolCall, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
    },
    ProtocolVersion,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, Responder, Stdio};
use futures::StreamExt;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use uuid::Uuid;

struct SessionRecord {
    cwd: std::path::PathBuf,
    state: AgentSession,
    cancel: tokio::sync::watch::Sender<u64>,
}

type Sessions = Arc<RwLock<HashMap<SessionId, SessionRecord>>>;

fn flatten_prompt(blocks: &[ContentBlock]) -> agent_client_protocol::Result<String> {
    blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text(text) => Ok(text.text.clone()),
            ContentBlock::ResourceLink(resource) => {
                Ok(format!("{}: {}", resource.name, resource.uri))
            }
            _ => Err(agent_client_protocol::Error::invalid_params()),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("\n"))
}

/// Build an ACP agent component around any backend.
pub fn acp_agent(
    backend: impl AgentBackend,
) -> impl agent_client_protocol::component::ConnectTo<Client> {
    let backend = Arc::new(backend);
    let sessions: Sessions = Arc::new(RwLock::new(HashMap::new()));

    let new_sessions = Arc::clone(&sessions);
    let prompt_sessions = Arc::clone(&sessions);
    let cancel_sessions = Arc::clone(&sessions);
    Agent
        .builder()
        .name("longbridge-ai")
        .on_receive_request(
            async move |request: InitializeRequest, responder, _connection| {
                if request.protocol_version != ProtocolVersion::V1 {
                    return responder.respond_with_error(
                        agent_client_protocol::Error::invalid_params()
                            .data("only ACP protocol version 1 is supported"),
                    );
                }
                responder.respond(
                    InitializeResponse::new(ProtocolVersion::V1)
                        .agent_capabilities(AgentCapabilities::new())
                        .agent_info(Implementation::new(
                            "Longbridge AI",
                            env!("CARGO_PKG_VERSION"),
                        )),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: NewSessionRequest,
                        responder: Responder<NewSessionResponse>,
                        _connection| {
                let id = SessionId::new(Uuid::new_v4().to_string());
                new_sessions.write().await.insert(
                    id.clone(),
                    SessionRecord {
                        cwd: request.cwd,
                        state: AgentSession::default(),
                        cancel: tokio::sync::watch::channel(0).0,
                    },
                );
                responder.respond(NewSessionResponse::new(id))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: CancelNotification, _connection| {
                if let Some(session) = cancel_sessions.read().await.get(&notification.session_id) {
                    let next = *session.cancel.borrow() + 1;
                    let _ = session.cancel.send(next);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: PromptRequest,
                        responder: Responder<PromptResponse>,
                        connection: ConnectionTo<Client>| {
                let prompt = match flatten_prompt(&request.prompt) {
                    Ok(prompt) => prompt,
                    Err(error) => return responder.respond_with_error(error),
                };
                let prompt_sessions = Arc::clone(&prompt_sessions);
                let backend = Arc::clone(&backend);
                let task_connection = connection.clone();
                connection.spawn(async move {
                    let (cwd, state, mut cancelled) = {
                        let sessions = prompt_sessions.read().await;
                        let record = sessions
                            .get(&request.session_id)
                            .ok_or_else(agent_client_protocol::Error::invalid_params)?;
                        (
                            record.cwd.clone(),
                            record.state.clone(),
                            record.cancel.subscribe(),
                        )
                    };
                    let mut events =
                        backend.prompt(state, prompt, &cwd).await.map_err(|error| {
                            agent_client_protocol::Error::internal_error().data(error.to_string())
                        })?;

                    let mut stop_reason = StopReason::EndTurn;
                    loop {
                        let event = tokio::select! {
                            changed = cancelled.changed() => {
                                if changed.is_ok() {
                                    stop_reason = StopReason::Cancelled;
                                }
                                break;
                            }
                            event = events.next() => event,
                        };
                        let Some(event) = event else { break };
                        match event.map_err(|error| {
                            agent_client_protocol::Error::internal_error().data(error.to_string())
                        })? {
                            AgentEvent::Text(text) => {
                                task_connection.send_notification(SessionNotification::new(
                                    request.session_id.clone(),
                                    SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                        ContentBlock::Text(TextContent::new(text)),
                                    )),
                                ))?;
                            }
                            AgentEvent::Thought(text) => {
                                task_connection.send_notification(SessionNotification::new(
                                    request.session_id.clone(),
                                    SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                                        ContentBlock::Text(TextContent::new(text)),
                                    )),
                                ))?;
                            }
                            AgentEvent::ToolStarted {
                                id,
                                title,
                                raw_input,
                            } => {
                                task_connection.send_notification(SessionNotification::new(
                                    request.session_id.clone(),
                                    SessionUpdate::ToolCall(
                                        ToolCall::new(id, title)
                                            .status(ToolCallStatus::InProgress)
                                            .raw_input(raw_input),
                                    ),
                                ))?;
                            }
                            AgentEvent::ToolFinished {
                                id,
                                title,
                                success,
                                raw_output,
                            } => {
                                task_connection.send_notification(SessionNotification::new(
                                    request.session_id.clone(),
                                    SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                                        id,
                                        ToolCallUpdateFields::new()
                                            .title(title)
                                            .status(if success {
                                                ToolCallStatus::Completed
                                            } else {
                                                ToolCallStatus::Failed
                                            })
                                            .raw_output(raw_output),
                                    )),
                                ))?;
                            }
                            AgentEvent::NeedsInput { session, questions } => {
                                prompt_sessions
                                    .write()
                                    .await
                                    .get_mut(&request.session_id)
                                    .expect("session exists")
                                    .state = session;
                                let text = questions
                                    .iter()
                                    .enumerate()
                                    .map(|(index, question)| format!("{}. {question}", index + 1))
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                task_connection.send_notification(SessionNotification::new(
                                    request.session_id.clone(),
                                    SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                        ContentBlock::Text(TextContent::new(text)),
                                    )),
                                ))?;
                            }
                            AgentEvent::Finished(state) => {
                                prompt_sessions
                                    .write()
                                    .await
                                    .get_mut(&request.session_id)
                                    .expect("session exists")
                                    .state = state;
                            }
                        }
                    }
                    responder.respond(PromptResponse::new(stop_reason))
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
}

/// Serve a backend over newline-delimited ACP JSON-RPC on stdin/stdout.
pub async fn serve_stdio(backend: impl AgentBackend) -> agent_client_protocol::Result<()> {
    use agent_client_protocol::component::ConnectTo;
    acp_agent(backend).connect_to(Stdio::new()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentEvent, BackendError};
    use agent_client_protocol::schema::v1::{ImageContent, ResourceLink};
    use async_trait::async_trait;
    use futures::{stream, stream::BoxStream};
    use std::{path::Path, sync::Mutex};

    #[derive(Default)]
    struct MockBackend {
        seen: Mutex<Vec<AgentSession>>,
    }

    struct SlowBackend;

    #[async_trait]
    impl AgentBackend for SlowBackend {
        async fn prompt(
            &self,
            _session: AgentSession,
            _prompt: String,
            _cwd: &Path,
        ) -> Result<BoxStream<'static, Result<AgentEvent, BackendError>>, BackendError> {
            Ok(Box::pin(stream::pending()))
        }
    }

    #[async_trait]
    impl AgentBackend for MockBackend {
        async fn prompt(
            &self,
            session: AgentSession,
            prompt: String,
            _cwd: &Path,
        ) -> Result<BoxStream<'static, Result<AgentEvent, BackendError>>, BackendError> {
            self.seen.lock().expect("mutex").push(session);
            Ok(Box::pin(stream::iter([
                Ok(AgentEvent::Thought("checking".into())),
                Ok(AgentEvent::Text(format!("answer: {prompt}"))),
                Ok(AgentEvent::Finished(AgentSession {
                    conversation_id: Some("chat-1".into()),
                    parent_message_id: Some("message-1".into()),
                    pending_interaction: None,
                })),
            ])))
        }
    }

    #[tokio::test]
    async fn exposes_streaming_acp_session() {
        let updates = Arc::new(Mutex::new(Vec::new()));
        let received = Arc::clone(&updates);
        let client = agent_client_protocol::Client
            .builder()
            .on_receive_notification(
                async move |notification: SessionNotification, _cx| {
                    received.lock().expect("mutex").push(notification.update);
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            );

        client
            .connect_with(acp_agent(MockBackend::default()), async |connection| {
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let session = connection
                    .send_request(NewSessionRequest::new(std::path::PathBuf::from("/tmp")))
                    .block_task()
                    .await?;
                connection
                    .send_request(PromptRequest::new(
                        session.session_id,
                        vec![ContentBlock::Text(TextContent::new("hello"))],
                    ))
                    .block_task()
                    .await?;
                Ok(())
            })
            .await
            .expect("ACP exchange");

        let updates = updates.lock().expect("mutex");
        assert!(matches!(
            updates.first(),
            Some(SessionUpdate::AgentThoughtChunk(_))
        ));
        assert!(matches!(
            updates.get(1),
            Some(SessionUpdate::AgentMessageChunk(_))
        ));
    }

    #[tokio::test]
    async fn cancel_stops_an_active_prompt() {
        let response = agent_client_protocol::Client
            .builder()
            .connect_with(acp_agent(SlowBackend), async |connection| {
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let session_id = connection
                    .send_request(NewSessionRequest::new(std::path::PathBuf::from("/tmp")))
                    .block_task()
                    .await?
                    .session_id;
                let prompt = connection.send_request(PromptRequest::new(
                    session_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("wait"))],
                ));
                tokio::task::yield_now().await;
                connection.send_notification(CancelNotification::new(session_id))?;
                prompt.block_task().await
            })
            .await
            .expect("cancelled prompt response");

        assert_eq!(response.stop_reason, StopReason::Cancelled);
    }

    #[test]
    fn prompt_flattener_preserves_text_and_resource_links() {
        let prompt = flatten_prompt(&[
            ContentBlock::Text(TextContent::new("review this")),
            ContentBlock::ResourceLink(ResourceLink::new("proposal", "file:///tmp/proposal.md")),
        ])
        .expect("supported prompt");

        assert_eq!(prompt, "review this\nproposal: file:///tmp/proposal.md");
    }

    #[test]
    fn prompt_flattener_rejects_unadvertised_media() {
        let error = flatten_prompt(&[ContentBlock::Image(ImageContent::new(
            "base64",
            "image/png",
        ))]);

        assert!(error.is_err());
    }
}
