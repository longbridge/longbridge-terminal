use async_trait::async_trait;
use futures::stream::BoxStream;
use std::path::Path;

pub type BackendError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Opaque backend state retained for one ACP session.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentSession {
    pub conversation_id: Option<String>,
    pub parent_message_id: Option<String>,
    pub pending_interaction: Option<PendingInteraction>,
}

/// Information required to resume an interrupted Longbridge conversation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingInteraction {
    pub tool_call_id: String,
    pub questions: Vec<String>,
}

/// Events understood by the protocol adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentEvent {
    Text(String),
    Thought(String),
    ToolStarted {
        id: String,
        title: String,
        raw_input: Option<serde_json::Value>,
    },
    ToolFinished {
        id: String,
        title: String,
        success: bool,
        raw_output: Option<serde_json::Value>,
    },
    NeedsInput {
        session: AgentSession,
        questions: Vec<String>,
    },
    Finished(AgentSession),
}

/// Provider-neutral seam used by both the CLI and an embedded desktop client.
#[async_trait]
pub trait AgentBackend: Send + Sync + 'static {
    async fn prompt(
        &self,
        session: AgentSession,
        prompt: String,
        cwd: &Path,
    ) -> Result<BoxStream<'static, Result<AgentEvent, BackendError>>, BackendError>;
}
