use crate::{AgentBackend, AgentEvent, RichContent};
use agent_client_protocol::schema::{
    v1::{
        AgentCapabilities, CancelNotification, ContentBlock, ContentChunk, Implementation,
        InitializeRequest, InitializeResponse, NewSessionRequest, NewSessionResponse,
        PermissionOption, PermissionOptionKind, PromptRequest, PromptResponse,
        RequestPermissionOutcome, RequestPermissionRequest, SessionId, SessionNotification,
        SessionUpdate, StopReason, TextContent, ToolCall, ToolCallStatus, ToolCallUpdate,
        ToolCallUpdateFields,
    },
    ProtocolVersion,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, Responder, Stdio};
use futures::StreamExt;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;
use uuid::Uuid;

const BACKEND_INACTIVITY_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(1);

struct SessionRecord<BackendSession> {
    cwd: std::path::PathBuf,
    state: BackendSession,
    cancel: tokio::sync::watch::Sender<u64>,
}

type Sessions<BackendSession> = Arc<RwLock<HashMap<SessionId, SessionRecord<BackendSession>>>>;

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
pub fn acp_agent<B: AgentBackend>(
    backend: B,
) -> impl agent_client_protocol::component::ConnectTo<Client> {
    let backend = Arc::new(backend);
    let sessions: Sessions<B::Session> = Arc::new(RwLock::new(HashMap::new()));

    let new_sessions = Arc::clone(&sessions);
    let prompt_sessions = Arc::clone(&sessions);
    let cancel_sessions = Arc::clone(&sessions);
    Agent
        .builder()
        .name("longbridge-ai")
        .on_receive_request(
            async move |request: InitializeRequest, responder, _connection| {
                tracing::info!(
                    target: "longbridge_ai_acp::protocol",
                    protocol_version = ?request.protocol_version,
                    "ACP initialize request received"
                );
                if request.protocol_version != ProtocolVersion::V1 {
                    tracing::warn!(
                        target: "longbridge_ai_acp::protocol",
                        protocol_version = ?request.protocol_version,
                        "ACP initialize request rejected"
                    );
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
                tracing::info!(
                    target: "longbridge_ai_acp::protocol",
                    session_id = %id.0,
                    cwd = %request.cwd.display(),
                    "ACP session created"
                );
                new_sessions.write().await.insert(
                    id.clone(),
                    SessionRecord {
                        cwd: request.cwd,
                        state: B::Session::default(),
                        cancel: tokio::sync::watch::channel(0).0,
                    },
                );
                responder.respond(NewSessionResponse::new(id))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: CancelNotification, _connection| {
                tracing::info!(
                    target: "longbridge_ai_acp::protocol",
                    session_id = %notification.session_id.0,
                    "ACP cancel notification received"
                );
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
                    Err(error) => {
                        tracing::warn!(
                            target: "longbridge_ai_acp::protocol",
                            session_id = %request.session_id.0,
                            blocks = request.prompt.len(),
                            "ACP prompt contains an unsupported content block"
                        );
                        return responder.respond_with_error(error);
                    }
                };
                tracing::info!(
                    target: "longbridge_ai_acp::protocol",
                    session_id = %request.session_id.0,
                    blocks = request.prompt.len(),
                    prompt_chars = prompt.chars().count(),
                    "ACP prompt request received"
                );
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
                    let mut events = backend.prompt(state, prompt, &cwd).await.map_err(|error| {
                        tracing::error!(
                            target: "longbridge_ai_acp::protocol",
                            session_id = %request.session_id.0,
                            error = %error,
                            "ACP backend rejected prompt"
                        );
                        agent_client_protocol::Error::internal_error().data(error.to_string())
                    })?;

                    let mut stop_reason = StopReason::EndTurn;
                    let mut rich_text = RichTextFilter::new(request.session_id.0.as_ref());
                    let mut active_tools = HashMap::<String, String>::new();
                    loop {
                        let event = tokio::select! {
                            changed = cancelled.changed() => {
                                if changed.is_ok() {
                                    stop_reason = StopReason::Cancelled;
                                }
                                break;
                            }
                            event = events.next() => event,
                            () = tokio::time::sleep(BACKEND_INACTIVITY_TIMEOUT) => {
                                tracing::warn!(target: "longbridge_ai_acp::protocol", session_id = %request.session_id.0, active_tools = active_tools.len(), timeout_seconds = BACKEND_INACTIVITY_TIMEOUT.as_secs(), "ACP backend event stream timed out");
                                for (id, title) in active_tools.drain() {
                                    task_connection.send_notification(SessionNotification::new(
                                        request.session_id.clone(),
                                        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                                            id,
                                            ToolCallUpdateFields::new()
                                                .title(title)
                                                .status(ToolCallStatus::Failed)
                                                .raw_output(serde_json::json!({ "error": "Backend stopped sending events for 60 seconds" })),
                                        )),
                                    ))?;
                                }
                                send_agent_text(&task_connection, &request.session_id, "\nThe backend stopped responding for 60 seconds. The current operation was timed out so this ACP turn can finish.\n".to_string())?;
                                break;
                            }
                        };
                        let Some(event) = event else {
                            tracing::debug!(
                                target: "longbridge_ai_acp::protocol",
                                session_id = %request.session_id.0,
                                "ACP backend event stream ended"
                            );
                            break;
                        };
                        match event.map_err(|error| {
                            tracing::error!(
                                target: "longbridge_ai_acp::protocol",
                                session_id = %request.session_id.0,
                                error = %error,
                                "ACP backend event stream failed"
                            );
                            agent_client_protocol::Error::internal_error().data(error.to_string())
                        })? {
                            AgentEvent::Text(text) => {
                                tracing::debug!(target: "longbridge_ai_acp::protocol", session_id = %request.session_id.0, chars = text.chars().count(), update = "agent_message_chunk", "ACP session update sent");
                                send_filtered_text(
                                    &task_connection,
                                    &request.session_id,
                                    rich_text.push(&text),
                                    None,
                                )?;
                            }
                            AgentEvent::Thought(text) => {
                                tracing::debug!(target: "longbridge_ai_acp::protocol", session_id = %request.session_id.0, chars = text.chars().count(), update = "agent_thought_chunk", "ACP session update sent");
                                task_connection.send_notification(SessionNotification::new(
                                    request.session_id.clone(),
                                    SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                                        ContentBlock::Text(TextContent::new(text)),
                                    )),
                                ))?;
                            }
                            AgentEvent::Content {
                                text,
                                thought,
                                metadata,
                            } => {
                                tracing::debug!(target: "longbridge_ai_acp::protocol", session_id = %request.session_id.0, chars = text.chars().count(), thought, update = "content_chunk", "ACP session update sent");
                                if !thought {
                                    task_connection.send_notification(SessionNotification::new(
                                        request.session_id.clone(),
                                        SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                            ContentBlock::Text(
                                                TextContent::new("")
                                                    .meta(longbridge_meta(metadata.clone())),
                                            ),
                                        )),
                                    ))?;
                                    send_filtered_text(
                                        &task_connection,
                                        &request.session_id,
                                        rich_text.push(&text),
                                        Some(&standard_fallback_meta()),
                                    )?;
                                    continue;
                                }
                                let content = ContentChunk::new(ContentBlock::Text(
                                    TextContent::new(text).meta(longbridge_meta(metadata)),
                                ));
                                let update = if thought {
                                    SessionUpdate::AgentThoughtChunk(content)
                                } else {
                                    SessionUpdate::AgentMessageChunk(content)
                                };
                                task_connection.send_notification(SessionNotification::new(
                                    request.session_id.clone(),
                                    update,
                                ))?;
                            }
                            AgentEvent::ToolStarted {
                                id,
                                title,
                                raw_input,
                            } => {
                                active_tools.insert(id.clone(), title.clone());
                                tracing::debug!(target: "longbridge_ai_acp::protocol", session_id = %request.session_id.0, tool_call_id = %id, title = %title, update = "tool_call", "ACP session update sent");
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
                                active_tools.remove(&id);
                                tracing::debug!(target: "longbridge_ai_acp::protocol", session_id = %request.session_id.0, tool_call_id = %id, title = %title, success, output = ?raw_output, update = "tool_call_update", "ACP session update sent");
                                let failure = (!success)
                                    .then(|| tool_failure_message(&title, raw_output.as_ref()));
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
                                if let Some(failure) = failure {
                                    send_agent_text(
                                        &task_connection,
                                        &request.session_id,
                                        failure,
                                    )?;
                                }
                            }
                            AgentEvent::ToolStartedRich {
                                id,
                                title,
                                raw_input,
                                metadata,
                            } => {
                                active_tools.insert(id.clone(), title.clone());
                                tracing::debug!(target: "longbridge_ai_acp::protocol", session_id = %request.session_id.0, tool_call_id = %id, title = %title, update = "tool_call", rich = true, "ACP session update sent");
                                task_connection.send_notification(SessionNotification::new(
                                    request.session_id.clone(),
                                    SessionUpdate::ToolCall(
                                        ToolCall::new(id, title)
                                            .status(ToolCallStatus::InProgress)
                                            .raw_input(raw_input)
                                            .meta(longbridge_meta(metadata)),
                                    ),
                                ))?;
                            }
                            AgentEvent::ToolFinishedRich {
                                id,
                                title,
                                success,
                                raw_output,
                                metadata,
                            } => {
                                active_tools.remove(&id);
                                tracing::debug!(target: "longbridge_ai_acp::protocol", session_id = %request.session_id.0, tool_call_id = %id, title = %title, success, output = ?raw_output, update = "tool_call_update", rich = true, "ACP session update sent");
                                let failure = (!success)
                                    .then(|| tool_failure_message(&title, raw_output.as_ref()));
                                task_connection.send_notification(SessionNotification::new(
                                    request.session_id.clone(),
                                    SessionUpdate::ToolCallUpdate(
                                        ToolCallUpdate::new(
                                            id,
                                            ToolCallUpdateFields::new()
                                                .title(title)
                                                .status(if success {
                                                    ToolCallStatus::Completed
                                                } else {
                                                    ToolCallStatus::Failed
                                                })
                                                .raw_output(raw_output),
                                        )
                                        .meta(longbridge_meta(metadata)),
                                    ),
                                ))?;
                                if let Some(failure) = failure {
                                    send_agent_text(
                                        &task_connection,
                                        &request.session_id,
                                        failure,
                                    )?;
                                }
                            }
                            AgentEvent::NeedsInput {
                                session,
                                questions,
                                metadata,
                            } => {
                                tracing::info!(target: "longbridge_ai_acp::protocol", session_id = %request.session_id.0, questions = questions.len(), "ACP backend is waiting for user input");
                                prompt_sessions
                                    .write()
                                    .await
                                    .get_mut(&request.session_id)
                                    .expect("session exists")
                                    .state = session;
                                if let Some(data) = metadata {
                                    let mut meta = serde_json::Map::new();
                                    meta.insert(
                                        "longbridge.ai/event".to_string(),
                                        serde_json::json!({
                                            "event": "human_interaction_required",
                                            "data": data,
                                        }),
                                    );
                                    task_connection.send_notification(SessionNotification::new(
                                        request.session_id.clone(),
                                        SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                            ContentBlock::Text(TextContent::new("").meta(meta)),
                                        )),
                                    ))?;
                                }
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
                                // A human-interaction event is terminal for this
                                // backend turn. The next ACP prompt is the user's
                                // answer and resumes the persisted session state.
                                break;
                            }
                            AgentEvent::PermissionRequired {
                                session,
                                tool_call_id,
                                title,
                                metadata,
                            } => {
                                tracing::info!(target: "longbridge_ai_acp::protocol", session_id = %request.session_id.0, tool_call_id = %tool_call_id, "requesting ACP tool permission");
                                let tool_call = ToolCallUpdate::new(
                                    tool_call_id.clone(),
                                    ToolCallUpdateFields::new()
                                        .title(title)
                                        .status(ToolCallStatus::Pending),
                                );
                                let permission = RequestPermissionRequest::new(
                                    request.session_id.clone(),
                                    tool_call,
                                    vec![
                                        PermissionOption::new(
                                            "allow",
                                            "Allow",
                                            PermissionOptionKind::AllowOnce,
                                        ),
                                        PermissionOption::new(
                                            "deny",
                                            "Deny",
                                            PermissionOptionKind::RejectOnce,
                                        ),
                                    ],
                                )
                                .meta(metadata.map(longbridge_meta));
                                let response = task_connection
                                    .send_request(permission)
                                    .block_task()
                                    .await?;
                                let authorized = matches!(
                                    response.outcome,
                                    RequestPermissionOutcome::Selected(selected)
                                        if selected.option_id.0.as_ref() == "allow"
                                );
                                events = backend
                                    .prompt(session, authorized.to_string(), &cwd)
                                    .await
                                    .map_err(|error| {
                                        tracing::error!(target: "longbridge_ai_acp::protocol", session_id = %request.session_id.0, error = %error, "ACP backend failed to resume after permission");
                                        agent_client_protocol::Error::internal_error()
                                            .data(error.to_string())
                                    })?;
                            }
                            AgentEvent::Notice {
                                session,
                                text,
                                metadata,
                            } => {
                                prompt_sessions
                                    .write()
                                    .await
                                    .get_mut(&request.session_id)
                                    .expect("session exists")
                                    .state = session;
                                let content = ContentChunk::new(ContentBlock::Text(
                                    TextContent::new(text).meta(longbridge_meta(
                                        metadata.unwrap_or(serde_json::Value::Null),
                                    )),
                                ));
                                task_connection.send_notification(SessionNotification::new(
                                    request.session_id.clone(),
                                    SessionUpdate::AgentMessageChunk(content),
                                ))?;
                                break;
                            }
                            AgentEvent::RichContent(content) => {
                                tracing::debug!(target: "longbridge_ai_acp::protocol", session_id = %request.session_id.0, content_id = %content.content_id, kind = ?content.kind, update = "rich_content", "ACP rich content sent");
                                for chunk in content.to_acp_chunks() {
                                    task_connection.send_notification(SessionNotification::new(
                                        request.session_id.clone(),
                                        SessionUpdate::AgentMessageChunk(chunk),
                                    ))?;
                                }
                            }
                            AgentEvent::Extension {
                                namespace,
                                event,
                                data,
                            } => {
                                tracing::debug!(target: "longbridge_ai_acp::protocol", session_id = %request.session_id.0, namespace = %namespace, event = %event, update = "extension", "ACP session update sent");
                                let mut meta = serde_json::Map::new();
                                meta.insert(
                                    namespace,
                                    serde_json::json!({ "event": event, "data": data }),
                                );
                                task_connection.send_notification(SessionNotification::new(
                                    request.session_id.clone(),
                                    SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                        ContentBlock::Text(TextContent::new("").meta(meta)),
                                    )),
                                ))?;
                            }
                            AgentEvent::Finished(state) => {
                                tracing::debug!(target: "longbridge_ai_acp::protocol", session_id = %request.session_id.0, "ACP backend state persisted");
                                prompt_sessions
                                    .write()
                                    .await
                                    .get_mut(&request.session_id)
                                    .expect("session exists")
                                    .state = state;
                            }
                            AgentEvent::Completed { session, metadata } => {
                                tracing::info!(target: "longbridge_ai_acp::protocol", session_id = %request.session_id.0, "ACP backend workflow completed");
                                prompt_sessions
                                    .write()
                                    .await
                                    .get_mut(&request.session_id)
                                    .expect("session exists")
                                    .state = session;
                                let mut meta = serde_json::Map::new();
                                meta.insert(
                                    "longbridge.ai/event".to_string(),
                                    serde_json::json!({
                                        "event": "workflow_finished",
                                        "data": metadata,
                                    }),
                                );
                                task_connection.send_notification(SessionNotification::new(
                                    request.session_id.clone(),
                                    SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                        ContentBlock::Text(TextContent::new("").meta(meta)),
                                    )),
                                ))?;
                            }
                        }
                    }
                    send_filtered_text(
                        &task_connection,
                        &request.session_id,
                        rich_text.finish(),
                        None,
                    )?;
                    tracing::info!(
                        target: "longbridge_ai_acp::protocol",
                        session_id = %request.session_id.0,
                        stop_reason = ?stop_reason,
                        "ACP prompt response sent"
                    );
                    responder.respond(PromptResponse::new(stop_reason))
                })?;
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
}

fn send_agent_text(
    connection: &ConnectionTo<Client>,
    session_id: &SessionId,
    text: String,
) -> agent_client_protocol::Result<()> {
    connection.send_notification(SessionNotification::new(
        session_id.clone(),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        )))),
    ))
}

enum FilteredContent {
    Text(String),
    Rich(RichContent),
}

struct RichTextFilter {
    buffer: String,
    content_id_prefix: String,
    next_chart: usize,
}

impl RichTextFilter {
    fn new(content_id_prefix: &str) -> Self {
        Self {
            buffer: String::new(),
            content_id_prefix: content_id_prefix.to_owned(),
            next_chart: 1,
        }
    }

    fn push(&mut self, text: &str) -> Vec<FilteredContent> {
        const MARKER: &str = "```vis-chart";
        self.buffer.push_str(text);
        let mut output = Vec::new();
        loop {
            let Some(open) = self.buffer.find(MARKER) else {
                let retained = marker_suffix_len(&self.buffer, MARKER);
                let emit_len = self.buffer.len() - retained;
                if emit_len > 0 {
                    output.push(FilteredContent::Text(
                        self.buffer.drain(..emit_len).collect(),
                    ));
                }
                break;
            };
            if open > 0 {
                output.push(FilteredContent::Text(self.buffer.drain(..open).collect()));
            }
            let after_marker = MARKER.len();
            let Some(body_start) = self.buffer[after_marker..]
                .strip_prefix("\r\n")
                .map(|_| after_marker + 2)
                .or_else(|| {
                    self.buffer[after_marker..]
                        .strip_prefix('\n')
                        .map(|_| after_marker + 1)
                })
            else {
                break;
            };
            let Some(relative_close) = self.buffer[body_start..].find("```") else {
                break;
            };
            let close = body_start + relative_close;
            let end = close + 3;
            let complete: String = self.buffer.drain(..end).collect();
            let body = &complete[body_start..close];
            match serde_json::from_str(body.trim()).ok().and_then(|data| {
                RichContent::chart(
                    format!("{}:chart-{}", self.content_id_prefix, self.next_chart),
                    data,
                )
                .ok()
            }) {
                Some(chart) => {
                    self.next_chart += 1;
                    output.push(FilteredContent::Rich(chart));
                }
                None => output.push(FilteredContent::Text(complete)),
            }
        }
        output
    }

    fn finish(&mut self) -> Vec<FilteredContent> {
        if self.buffer.is_empty() {
            Vec::new()
        } else {
            vec![FilteredContent::Text(std::mem::take(&mut self.buffer))]
        }
    }
}

fn marker_suffix_len(value: &str, marker: &str) -> usize {
    (1..marker.len())
        .rev()
        .find(|length| value.ends_with(&marker[..*length]))
        .unwrap_or(0)
}

fn standard_fallback_meta() -> serde_json::Map<String, serde_json::Value> {
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        "longbridge.ai/standard-fallback".to_owned(),
        serde_json::json!(true),
    );
    metadata
}

fn send_filtered_text(
    connection: &ConnectionTo<Client>,
    session_id: &SessionId,
    content: Vec<FilteredContent>,
    text_metadata: Option<&serde_json::Map<String, serde_json::Value>>,
) -> agent_client_protocol::Result<()> {
    for item in content {
        match item {
            FilteredContent::Text(text) if !text.is_empty() => {
                let text = match &text_metadata {
                    Some(metadata) => TextContent::new(text).meta((*metadata).clone()),
                    None => TextContent::new(text),
                };
                connection.send_notification(SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(text))),
                ))?;
            }
            FilteredContent::Rich(rich) => {
                for chunk in rich.to_acp_chunks() {
                    connection.send_notification(SessionNotification::new(
                        session_id.clone(),
                        SessionUpdate::AgentMessageChunk(chunk),
                    ))?;
                }
            }
            FilteredContent::Text(_) => {}
        }
    }
    Ok(())
}

fn tool_failure_message(title: &str, output: Option<&serde_json::Value>) -> String {
    let detail = output.and_then(|output| match output {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Object(fields) => {
            ["error", "message", "detail"].into_iter().find_map(|key| {
                fields
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
        }
        _ => None,
    });
    match detail.filter(|detail| !detail.is_empty()) {
        Some(detail) => format!("\nTool ‘{title}’ failed: {detail}\n"),
        None => format!("\nTool ‘{title}’ failed.\n"),
    }
}

fn longbridge_meta(data: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    let mut meta = serde_json::Map::new();
    meta.insert("longbridge.ai/event".to_string(), data);
    meta
}

/// Serve a backend over newline-delimited ACP JSON-RPC on stdin/stdout.
pub async fn serve_stdio<B: AgentBackend>(backend: B) -> agent_client_protocol::Result<()> {
    use agent_client_protocol::component::ConnectTo;
    tracing::info!(target: "longbridge_ai_acp::protocol", "ACP stdio server started");
    let result = acp_agent(backend).connect_to(Stdio::new()).await;
    match &result {
        Ok(()) => tracing::info!(target: "longbridge_ai_acp::protocol", "ACP stdio server stopped"),
        Err(error) => {
            tracing::error!(target: "longbridge_ai_acp::protocol", %error, "ACP stdio server failed");
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentEvent, BackendError, RichContent, RICH_CONTENT_NAMESPACE};
    use agent_client_protocol::schema::v1::{ImageContent, ResourceLink};
    use async_trait::async_trait;
    use futures::{stream, stream::BoxStream};
    use std::{path::Path, sync::Mutex};

    #[derive(Default)]
    struct MockBackend {
        seen: Mutex<Vec<TestSession>>,
    }

    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    struct TestSession {
        conversation_id: Option<String>,
    }

    struct SlowBackend;

    struct RichBackend;

    struct MarkdownChartBackend;

    #[test]
    fn failed_tool_message_prefers_structured_error_detail() {
        assert_eq!(
            tool_failure_message(
                "Quote",
                Some(&serde_json::json!({ "error": "not entitled" }))
            ),
            "\nTool ‘Quote’ failed: not entitled\n"
        );
        assert_eq!(
            tool_failure_message("Quote", None),
            "\nTool ‘Quote’ failed.\n"
        );
    }

    #[async_trait]
    impl AgentBackend for SlowBackend {
        type Session = ();

        async fn prompt(
            &self,
            _session: (),
            _prompt: String,
            _cwd: &Path,
        ) -> Result<BoxStream<'static, Result<AgentEvent<()>, BackendError>>, BackendError>
        {
            Ok(Box::pin(stream::pending()))
        }
    }

    #[async_trait]
    impl AgentBackend for RichBackend {
        type Session = ();

        async fn prompt(
            &self,
            _session: (),
            _prompt: String,
            _cwd: &Path,
        ) -> Result<BoxStream<'static, Result<AgentEvent<()>, BackendError>>, BackendError>
        {
            let chart = RichContent::chart(
                "chart-1",
                serde_json::json!({
                    "type": "column",
                    "data": [{ "category": "FY2025", "value": 3.79 }]
                }),
            )?;
            Ok(Box::pin(stream::iter([Ok(AgentEvent::RichContent(chart))])))
        }
    }

    #[async_trait]
    impl AgentBackend for MarkdownChartBackend {
        type Session = ();

        async fn prompt(
            &self,
            _session: (),
            _prompt: String,
            _cwd: &Path,
        ) -> Result<BoxStream<'static, Result<AgentEvent<()>, BackendError>>, BackendError>
        {
            Ok(Box::pin(stream::iter([
                Ok(AgentEvent::Text(
                    "Before\n```vis-chart\n{\"type\":\"column\",\"group\":true,\"data\":[".into(),
                )),
                Ok(AgentEvent::Text(
                    concat!(
                        "{\"category\":\"FY2025\",\"value\":3.79,\"group\":\"Profit\"},",
                        "{\"category\":\"FY2025\",\"value\":6.41,\"group\":\"R&D\"}]}",
                        "\n```\nAfter"
                    )
                    .into(),
                )),
            ])))
        }
    }

    #[async_trait]
    impl AgentBackend for MockBackend {
        type Session = TestSession;

        async fn prompt(
            &self,
            session: TestSession,
            prompt: String,
            _cwd: &Path,
        ) -> Result<BoxStream<'static, Result<AgentEvent<TestSession>, BackendError>>, BackendError>
        {
            self.seen.lock().expect("mutex").push(session);
            Ok(Box::pin(stream::iter([
                Ok(AgentEvent::Thought("checking".into())),
                Ok(AgentEvent::Text(format!("answer: {prompt}"))),
                Ok(AgentEvent::Finished(TestSession {
                    conversation_id: Some("chat-1".into()),
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
    async fn rich_content_sends_markdown_fallback_before_svg_preview() {
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
            .connect_with(acp_agent(RichBackend), async |connection| {
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
                        vec![ContentBlock::Text(TextContent::new("chart"))],
                    ))
                    .block_task()
                    .await?;
                Ok(())
            })
            .await
            .expect("ACP exchange");

        let updates = updates.lock().expect("mutex");
        let Some(SessionUpdate::AgentMessageChunk(text_chunk)) = updates.first() else {
            panic!("expected text fallback");
        };
        let ContentBlock::Text(text) = &text_chunk.content else {
            panic!("expected text content");
        };
        assert!(text.text.starts_with('|'));
        assert!(text.text.contains("FY2025"));
        assert!(!text.text.contains("```vis-chart"));
        assert!(text
            .meta
            .as_ref()
            .is_some_and(|meta| meta.contains_key(RICH_CONTENT_NAMESPACE)));
        let Some(SessionUpdate::AgentMessageChunk(image_chunk)) = updates.get(1) else {
            panic!("expected image preview");
        };
        assert!(matches!(image_chunk.content, ContentBlock::Image(_)));
    }

    #[tokio::test]
    async fn complete_streamed_vis_chart_fence_adds_one_svg_preview() {
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
            .connect_with(acp_agent(MarkdownChartBackend), async |connection| {
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
                        vec![ContentBlock::Text(TextContent::new("chart"))],
                    ))
                    .block_task()
                    .await?;
                Ok(())
            })
            .await
            .expect("ACP exchange");

        let updates = updates.lock().expect("mutex");
        assert_eq!(
            updates
                .iter()
                .filter(|update| matches!(
                    update,
                    SessionUpdate::AgentMessageChunk(chunk)
                        if matches!(chunk.content, ContentBlock::Image(_))
                ))
                .count(),
            1
        );
        let visible_text = updates
            .iter()
            .filter_map(|update| match update {
                SessionUpdate::AgentMessageChunk(chunk) => match &chunk.content {
                    ContentBlock::Text(text) => Some(text.text.as_str()),
                    _ => None,
                },
                _ => None,
            })
            .collect::<String>();
        assert!(visible_text.contains("FY2025"));
        assert!(visible_text.contains("Profit"));
        assert!(visible_text.contains("R&D"));
        assert!(!visible_text.contains("```vis-chart"));
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
