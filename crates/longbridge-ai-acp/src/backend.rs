use async_trait::async_trait;
use futures::stream::BoxStream;
use std::path::Path;

pub type BackendError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Events understood by the protocol adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentEvent<Session> {
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
        session: Session,
        questions: Vec<String>,
    },
    Finished(Session),
}

/// Provider-neutral seam used by both the CLI and an embedded desktop client.
#[async_trait]
pub trait AgentBackend: Send + Sync + 'static {
    type Session: Clone + Default + Send + Sync + 'static;

    async fn prompt(
        &self,
        session: Self::Session,
        prompt: String,
        cwd: &Path,
    ) -> Result<BoxStream<'static, Result<AgentEvent<Self::Session>, BackendError>>, BackendError>;
}
