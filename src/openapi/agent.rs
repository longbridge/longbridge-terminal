use async_trait::async_trait;
use futures::{stream, stream::BoxStream, StreamExt};
use longbridge::agent::ConversationStreamEvent;
use longbridge_ai_acp::{
    AgentBackend, AgentEvent, AgentPlanEntry, AgentSessionInfo, AgentSessionPage, BackendError,
    LoadedAgentSession,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct OpenApiAgentSession {
    #[serde(default)]
    acp_session_id: Option<String>,
    conversation_id: Option<String>,
    parent_message_id: Option<String>,
    pending_interaction: Option<PendingInteraction>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct PendingInteraction {
    groups: Vec<PendingQuestionGroup>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct PendingQuestionGroup {
    answer_key: String,
    questions: Vec<PendingQuestion>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct PendingQuestion {
    text: String,
    options: Vec<PendingOption>,
    multi_select: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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
                    .collect::<Result<Vec<_>, _>>()?,
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

/// ACP backend used before the user completes `longbridge auth login`.
pub struct AuthenticationRequiredAgent {
    history: Arc<Mutex<SessionHistory>>,
    history_path: Option<Arc<PathBuf>>,
}

impl AuthenticationRequiredAgent {
    pub fn new(agent_id: &str) -> Self {
        let history_path = session_history_path(agent_id).map(Arc::new);
        let history = history_path
            .as_deref()
            .map_or_else(SessionHistory::default, |path| SessionHistory::load(path));
        Self {
            history: Arc::new(Mutex::new(history)),
            history_path,
        }
    }

    fn persist_history(&self, history: &SessionHistory) {
        persist_session_history(self.history_path.as_deref(), history);
    }
}

#[async_trait]
impl AgentBackend for AuthenticationRequiredAgent {
    type Session = OpenApiAgentSession;
    const SESSION_HISTORY: bool = true;

    fn new_session(&self, session_id: &str, cwd: &Path) -> Self::Session {
        let state = OpenApiAgentSession {
            acp_session_id: Some(session_id.to_owned()),
            ..Default::default()
        };
        if let Ok(mut history) = self.history.lock() {
            history.upsert(StoredSession {
                session_id: session_id.to_owned(),
                cwd: cwd.to_path_buf(),
                title: None,
                state: state.clone(),
                events: Vec::new(),
            });
            self.persist_history(&history);
        }
        state
    }

    async fn list_sessions(
        &self,
        cwd: Option<&Path>,
        cursor: Option<&str>,
    ) -> Result<AgentSessionPage, BackendError> {
        Ok(self
            .history
            .lock()
            .map_err(|_| std::io::Error::other("session history lock is poisoned"))?
            .list(cwd, cursor))
    }

    async fn load_session(
        &self,
        session_id: &str,
        _cwd: &Path,
    ) -> Result<LoadedAgentSession<Self::Session>, BackendError> {
        let stored = self
            .history
            .lock()
            .map_err(|_| std::io::Error::other("session history lock is poisoned"))?
            .get(session_id)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "session not found")
            })?;
        Ok(LoadedAgentSession {
            state: stored.state,
            history: stream::iter(stored.events.into_iter().map(Ok)).boxed(),
        })
    }

    async fn prompt(
        &self,
        _session: Self::Session,
        _prompt: String,
        _cwd: &Path,
    ) -> Result<BoxStream<'static, Result<AgentEvent<Self::Session>, BackendError>>, BackendError>
    {
        Ok(stream::iter([Ok(AgentEvent::Text(
            rust_i18n::t!("ACP.LoginRequired").to_string(),
        ))])
        .boxed())
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredSession {
    session_id: String,
    cwd: PathBuf,
    title: Option<String>,
    state: OpenApiAgentSession,
    events: Vec<AgentEvent<OpenApiAgentSession>>,
}

#[derive(Default, Serialize, Deserialize)]
struct SessionHistory {
    #[serde(default)]
    sessions: Vec<StoredSession>,
}

impl SessionHistory {
    const PAGE_SIZE: usize = 50;
    const MAX_SESSIONS: usize = 200;

    fn list(&self, cwd: Option<&Path>, cursor: Option<&str>) -> AgentSessionPage {
        let offset = cursor
            .and_then(|cursor| cursor.parse::<usize>().ok())
            .unwrap_or_default();
        let matching = self
            .sessions
            .iter()
            .rev()
            .filter(|session| cwd.is_none_or(|cwd| session.cwd == cwd))
            .skip(offset)
            .take(Self::PAGE_SIZE + 1)
            .collect::<Vec<_>>();
        let has_more = matching.len() > Self::PAGE_SIZE;
        let sessions = matching
            .into_iter()
            .take(Self::PAGE_SIZE)
            .map(|session| AgentSessionInfo {
                session_id: session.session_id.clone(),
                cwd: session.cwd.clone(),
                title: session.title.clone(),
                updated_at: None,
            })
            .collect::<Vec<_>>();
        AgentSessionPage {
            next_cursor: has_more.then(|| (offset + sessions.len()).to_string()),
            sessions,
        }
    }

    fn get(&self, session_id: &str) -> Option<StoredSession> {
        self.sessions
            .iter()
            .find(|session| session.session_id == session_id)
            .cloned()
    }

    fn upsert(&mut self, session: StoredSession) {
        self.sessions
            .retain(|existing| existing.session_id != session.session_id);
        self.sessions.push(session);
        if self.sessions.len() > Self::MAX_SESSIONS {
            self.sessions
                .drain(..self.sessions.len() - Self::MAX_SESSIONS);
        }
    }

    fn load(path: &Path) -> Self {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self, path: &Path) -> Result<(), BackendError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
        }
        #[cfg(windows)]
        if path.exists() {
            fs::remove_file(path)?;
        }
        fs::rename(temporary, path)?;
        Ok(())
    }
}

fn session_history_path(agent_id: &str) -> Option<PathBuf> {
    let digest = Sha256::digest(agent_id.as_bytes());
    let key = digest[..12]
        .iter()
        .fold(String::with_capacity(24), |mut key, byte| {
            let _ = write!(key, "{byte:02x}");
            key
        });
    dirs::home_dir().map(|home| {
        home.join(".longbridge")
            .join("acp")
            .join(format!("sessions-{key}.json"))
    })
}

impl OpenApiAgent {
    pub fn new(context: longbridge::AgentContext, agent_id: impl Into<String>) -> Self {
        Self {
            context,
            agent_id: agent_id.into(),
        }
    }
}

fn persist_session_history(path: Option<&PathBuf>, history: &SessionHistory) {
    if let Some(path) = path {
        if let Err(error) = history.save(path) {
            tracing::warn!(
                target: "longbridge::acp",
                %error,
                path = %path.display(),
                "failed to persist ACP session history"
            );
        }
    }
}

#[async_trait]
impl AgentBackend for OpenApiAgent {
    type Session = OpenApiAgentSession;
    const SESSION_HISTORY: bool = true;

    fn new_session(&self, session_id: &str, _cwd: &Path) -> Self::Session {
        OpenApiAgentSession {
            acp_session_id: Some(session_id.to_owned()),
            ..Default::default()
        }
    }

    async fn list_sessions(
        &self,
        _cwd: Option<&Path>,
        cursor: Option<&str>,
    ) -> Result<AgentSessionPage, BackendError> {
        const PAGE_SIZE: u32 = 50;
        // Server chats are account-scoped, not per-directory, so the `cwd`
        // filter is ignored — every chat is returned. The cursor is the
        // 1-based page number.
        let page = cursor
            .and_then(|cursor| cursor.parse::<u32>().ok())
            .unwrap_or(1)
            .max(1);
        // `session/list` must never break the ACP flow: an ACP client treats a
        // failed listing as fatal and then cannot even start a new session. If
        // the chats API is unreachable (network, auth, or not yet deployed to
        // this environment), degrade to an empty page instead of erroring.
        let resp = match crate::openapi::chats::list_chats(page, PAGE_SIZE, None).await {
            Ok(resp) => resp,
            Err(error) => {
                tracing::warn!(
                    target: "longbridge::acp",
                    %error,
                    "failed to list chats for session/list; returning an empty page"
                );
                return Ok(AgentSessionPage {
                    sessions: Vec::new(),
                    next_cursor: None,
                });
            }
        };
        let has_more = resp.chats.len() as u32 >= PAGE_SIZE;
        let sessions = resp
            .chats
            .into_iter()
            .map(|chat| AgentSessionInfo {
                session_id: chat.uid,
                cwd: PathBuf::new(),
                title: (!chat.name.is_empty()).then_some(chat.name),
                updated_at: (chat.updated_at > 0).then(|| chat.updated_at.to_string()),
            })
            .collect();
        Ok(AgentSessionPage {
            sessions,
            next_cursor: has_more.then(|| (page + 1).to_string()),
        })
    }

    async fn load_session(
        &self,
        session_id: &str,
        _cwd: &Path,
    ) -> Result<LoadedAgentSession<Self::Session>, BackendError> {
        // Like `session/list`, `session/load` must not break the ACP flow: a
        // client that resumes a remembered session treats a failed load as
        // fatal. If the chat can't be fetched (network, auth, not deployed to
        // this environment, or an id that isn't a server chat), resume with an
        // empty history so the user can keep chatting.
        // The client remembers a session by its ACP id. For a session created
        // via `session/new` that id is a local UUID, and the server chat it maps
        // to is only learned after the first prompt — so translate through the
        // local id index. A session resumed from `session/list` already carries
        // the server chat uid, which has no index entry and is used as-is.
        let chat_uid = resolve_chat_uid(session_id).unwrap_or_else(|| session_id.to_owned());
        let detail = match crate::openapi::chats::chat_detail(&chat_uid).await {
            Ok(detail) => detail,
            Err(error) => {
                tracing::warn!(
                    target: "longbridge::acp",
                    %error,
                    session_id,
                    "failed to load chat detail; resuming with empty history"
                );
                return Ok(LoadedAgentSession {
                    state: OpenApiAgentSession {
                        acp_session_id: Some(session_id.to_owned()),
                        ..Default::default()
                    },
                    history: stream::iter(Vec::new()).boxed(),
                });
            }
        };
        // Resume the server-side chat: the next prompt continues this
        // conversation, appended after its most recent *completed* round. The
        // parent must be the last assistant message that actually produced an
        // answer — not simply the last message, nor the last assistant message.
        // A failed/aborted round leaves an empty assistant message (e.g. status
        // 5, no text); resuming with that (or with a trailing unanswered `user`
        // turn) as the parent makes the continue API keep failing with 400000.
        record_chat_mapping(session_id, &detail.chat.uid);
        let parent_message_id = detail
            .messages
            .iter()
            .rfind(|message| message.sender == "assistant" && !message.text().is_empty())
            .map(|message| message.id.to_string());
        let state = OpenApiAgentSession {
            acp_session_id: Some(session_id.to_owned()),
            conversation_id: Some(detail.chat.uid.clone()),
            parent_message_id,
            ..Default::default()
        };
        // Replay each stored message as a history event: a user message becomes
        // `UserText`, anything else `Text`. Empty messages are skipped.
        let events: Vec<Result<AgentEvent<Self::Session>, BackendError>> = detail
            .messages
            .into_iter()
            .filter_map(|message| {
                let text = message.text();
                if text.is_empty() {
                    return None;
                }
                Some(Ok(if message.sender == "user" {
                    AgentEvent::UserText(text)
                } else {
                    AgentEvent::Text(text)
                }))
            })
            .collect();
        Ok(LoadedAgentSession {
            state,
            history: stream::iter(events).boxed(),
        })
    }

    async fn prompt(
        &self,
        session: Self::Session,
        prompt: String,
        _cwd: &Path,
    ) -> Result<BoxStream<'static, Result<AgentEvent<Self::Session>, BackendError>>, BackendError>
    {
        let acp_session_id = session.acp_session_id.clone();
        let started = if let Some(pending) = &session.pending_interaction {
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
                .await
                .map(StreamExt::boxed)
        } else {
            // Silently retry once if the conversation workflow fails with the
            // transient 400000 ("Something went wrong. Please try again.").
            conversation_stream_with_retry(
                self.context.clone(),
                self.agent_id.clone(),
                prompt,
                session.conversation_id,
                session.parent_message_id,
            )
            .await
        };
        // A turn that fails to start (e.g. the conversation API returns an auth
        // error or times out) must not fail the ACP turn or connection: surface
        // the error to the user as a notice and end the turn gracefully. The ACP
        // server also guards against this, but degrading here yields a cleaner
        // message and keeps the session usable.
        let stream = match started {
            Ok(stream) => stream,
            Err(error) => {
                let notice = AgentEvent::Notice {
                    session: OpenApiAgentSession {
                        acp_session_id: acp_session_id.clone(),
                        ..Default::default()
                    },
                    text: format!("This request could not be completed: {error}"),
                    metadata: None,
                };
                return Ok(stream::iter(vec![Ok(notice)]).boxed());
            }
        };

        Ok(stream
            .filter_map(move |event| {
                let acp_session_id = acp_session_id.clone();
                async move {
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
                                acp_session_id: acp_session_id.clone(),
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
                                acp_session_id: acp_session_id.clone(),
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
                                acp_session_id: acp_session_id.clone(),
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
                            acp_session_id: acp_session_id.clone(),
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
                                acp_session_id: acp_session_id.clone(),
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
                    Ok(ConversationStreamEvent::ChatStarted(payload)) => {
                        // First time we learn this session's server chat uid:
                        // bind the ACP session id to it so a later `session/load`
                        // (which arrives with the ACP id) can find the chat.
                        if let Some(acp) = acp_session_id.as_deref() {
                            record_chat_mapping(acp, &payload.chat_uid);
                        }
                        Some(Ok(native_event(
                            "chat_started",
                            serde_json::to_value(payload).ok()?,
                        )))
                    }
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
                    Ok(ConversationStreamEvent::SubagentProgress(payload)) => {
                        let metadata = event_metadata("subagent_progress", &payload);
                        let title = if payload.subagent_tool_name.is_empty() {
                            "Subagent".to_string()
                        } else {
                            payload.subagent_tool_name.clone()
                        };
                        Some(Ok(AgentEvent::ToolProgressRich {
                            id: payload.parent_tool_call_id.clone(),
                            title,
                            raw_output: serde_json::to_value(payload).ok(),
                            metadata,
                        }))
                    }
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
                    Ok(ConversationStreamEvent::AgentToolProgress(payload)) => {
                        let metadata = event_metadata("agent_tool_progress", &payload);
                        let title = if payload.inner_tool_name.is_empty() {
                            payload.agent_tool_name.clone()
                        } else {
                            payload.inner_tool_name.clone()
                        };
                        Some(Ok(AgentEvent::ToolProgressRich {
                            id: payload.parent_tool_call_id.clone(),
                            title,
                            raw_output: serde_json::to_value(payload).ok(),
                            metadata,
                        }))
                    }
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
                    Ok(ConversationStreamEvent::PlanChanged(payload)) => {
                        let metadata = event_metadata("plan_changed", &payload);
                        Some(Ok(AgentEvent::Plan {
                            entries: plan_entries(payload.outputs.as_ref()),
                            metadata,
                        }))
                    }
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
                    Ok(ConversationStreamEvent::ChatTitleUpdated(payload)) => {
                        let metadata = event_metadata("chat_title_updated", &payload);
                        Some(Ok(AgentEvent::SessionTitle {
                            title: payload.title,
                            metadata,
                        }))
                    }
                    Ok(ConversationStreamEvent::Other { event, data }) => {
                        Some(Ok(native_event(&event, data)))
                    }
                    Err(error) => Some(Err(Box::new(error) as BackendError)),
                }
                }
            })
            .boxed())
    }
}

/// Path of the local ACP session-id → server chat-uid index.
///
/// Only this id pointer is persisted locally; chat history itself is always
/// fetched from the API. A session created via `session/new` is remembered by
/// the client under a local UUID, while the server stores its conversation
/// under a chat uid that is only known after the first prompt — this index
/// bridges the two so the session can be resumed.
fn chat_index_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| {
        home.join(".longbridge")
            .join("acp")
            .join("chat-index.json")
    })
}

fn load_chat_index() -> std::collections::HashMap<String, String> {
    chat_index_path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Translate an ACP session id to the server chat uid it was bound to, if any.
fn resolve_chat_uid(acp_session_id: &str) -> Option<String> {
    load_chat_index().remove(acp_session_id)
}

/// Bind an ACP session id to its server chat uid (best effort; failures to
/// persist only cost the ability to resume history, never the live turn).
fn record_chat_mapping(acp_session_id: &str, chat_uid: &str) {
    if acp_session_id.is_empty() || chat_uid.is_empty() || acp_session_id == chat_uid {
        return;
    }
    let Some(path) = chat_index_path() else {
        return;
    };
    let mut index = load_chat_index();
    if index.get(acp_session_id).map(String::as_str) == Some(chat_uid) {
        return;
    }
    index.insert(acp_session_id.to_owned(), chat_uid.to_owned());
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(&index) {
        let _ = std::fs::write(&path, bytes);
    }
}

/// Start a conversation stream, silently retrying once if the workflow fails
/// with the transient `400000` ("Something went wrong. Please try again.").
///
/// The retry is spliced in at the raw-event level: the failed `workflow_finished`
/// is swallowed and a fresh attempt's events continue the same ACP turn, so the
/// caller's event mapping never sees the transient failure. Only the *initial*
/// start error propagates (so the caller can degrade); a failure to start the
/// retry is surfaced as a stream error instead.
async fn conversation_stream_with_retry(
    context: longbridge::AgentContext,
    agent_id: String,
    query: String,
    chat_uid: Option<String>,
    parent_message_id: Option<String>,
) -> Result<BoxStream<'static, Result<ConversationStreamEvent, longbridge::Error>>, longbridge::Error>
{
    const RETRYABLE_CODE: i32 = 400_000;

    struct State {
        context: longbridge::AgentContext,
        stream: BoxStream<'static, Result<ConversationStreamEvent, longbridge::Error>>,
        /// The params to restart once; `None` after the retry is spent.
        retry: Option<(String, String, Option<String>, Option<String>)>,
    }

    let first = context
        .conversation_streamed(
            agent_id.clone(),
            query.clone(),
            chat_uid.clone(),
            parent_message_id.clone(),
        )
        .await?;

    let state = State {
        context,
        stream: first.boxed(),
        retry: Some((agent_id, query, chat_uid, parent_message_id)),
    };

    Ok(stream::unfold(state, |mut state| async move {
        loop {
            match state.stream.next().await {
                Some(Ok(ConversationStreamEvent::WorkflowFinished(response)))
                    if state.retry.is_some()
                        && response
                            .error
                            .as_ref()
                            .is_some_and(|error| error.code == RETRYABLE_CODE) =>
                {
                    let (agent_id, query, chat_uid, parent_message_id) =
                        state.retry.take().expect("retry budget checked above");
                    tracing::warn!(
                        target: "longbridge::acp",
                        code = RETRYABLE_CODE,
                        "conversation workflow failed transiently; retrying once"
                    );
                    match state
                        .context
                        .conversation_streamed(agent_id, query, chat_uid, parent_message_id)
                        .await
                    {
                        Ok(retry_stream) => state.stream = retry_stream.boxed(),
                        Err(error) => return Some((Err(error), state)),
                    }
                }
                Some(item) => return Some((item, state)),
                None => return None,
            }
        }
    })
    .boxed())
}

fn plan_entries(outputs: Option<&serde_json::Value>) -> Vec<AgentPlanEntry> {
    let Some(outputs) = outputs else {
        return Vec::new();
    };
    let parsed;
    let outputs = if let Some(value) = outputs.as_str() {
        parsed = serde_json::from_str(value).unwrap_or(serde_json::Value::Null);
        &parsed
    } else {
        outputs
    };
    let todos = outputs
        .get("todos")
        .or_else(|| outputs.get("plan"))
        .and_then(serde_json::Value::as_array)
        .or_else(|| outputs.as_array());
    todos
        .into_iter()
        .flatten()
        .filter_map(|todo| {
            let content = todo
                .get("content")
                .or_else(|| todo.get("title"))
                .and_then(serde_json::Value::as_str)?;
            Some(AgentPlanEntry {
                content: content.to_string(),
                priority: todo
                    .get("priority")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("medium")
                    .to_string(),
                status: todo
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("pending")
                    .to_string(),
            })
        })
        .collect()
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
            .map(|(index, option)| {
                if option.label.is_empty() {
                    format!("{}. {}", index + 1, option.description)
                } else {
                    format!("{}. {} — {}", index + 1, option.label, option.description)
                }
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

    #[test]
    fn plan_outputs_accept_object_and_json_string_shapes() {
        let object = serde_json::json!({
            "todos": [
                { "content": "Inspect", "priority": "high", "status": "in_progress" },
                { "title": "Report", "status": "completed" }
            ]
        });
        let entries = plan_entries(Some(&object));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].content, "Inspect");
        assert_eq!(entries[0].priority, "high");
        assert_eq!(entries[1].content, "Report");
        assert_eq!(entries[1].priority, "medium");

        let string = serde_json::Value::String(object.to_string());
        assert_eq!(plan_entries(Some(&string)), entries);
    }

    #[test]
    fn new_backend_session_retains_the_acp_session_id() {
        let state = OpenApiAgentSession {
            acp_session_id: Some("acp-session-1".into()),
            ..Default::default()
        };

        assert_eq!(state.acp_session_id.as_deref(), Some("acp-session-1"));
    }

}
