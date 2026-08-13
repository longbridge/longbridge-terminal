use agent_client_protocol::schema::v1::{
    AgentCapabilities, Implementation, InitializeRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, AcpAgentConfig, ActiveSession, Agent, Client, ConnectTo};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;

/// Launch configuration for an external ACP agent such as Codex or Claude.
pub type ExternalAgentConfig = AcpAgentConfig;

/// Official ACP subprocess component. The external agent keeps ownership of
/// its provider authentication, model selection, and native configuration.
pub type ExternalAgent = AcpAgent;

/// ACP adapters supported by Longbridge desktop products.
///
/// The native `codex` and `claude` CLIs do not themselves expose ACP. Hosts
/// launch the separately installed adapters and keep provider authentication in
/// those adapters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalAgentKind {
    Codex,
    Claude,
}

impl ExternalAgentKind {
    /// Build a launch configuration using the adapter's conventional executable
    /// name. A product may replace this with an absolute, verified path.
    #[must_use]
    pub fn config(self) -> ExternalAgentConfig {
        ExternalAgentConfig::new(match self {
            Self::Codex => "codex-acp",
            Self::Claude => "claude-agent-acp",
        })
    }
}

/// Host callbacks that require desktop UI policy or user interaction.
#[async_trait]
pub trait ClientDelegate: Send + Sync + 'static {
    async fn request_permission(
        &self,
        request: RequestPermissionRequest,
    ) -> RequestPermissionResponse;
}

/// Conservative delegate for read-only/chat-only hosts.
pub struct DenyPermissions;

/// Metadata negotiated before a desktop chat session is created.
///
/// Desktop products can use this value to select UI affordances without
/// depending on provider-specific configuration for Longbridge, Codex, or
/// Claude.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentHandshake {
    pub protocol_version: ProtocolVersion,
    pub capabilities: AgentCapabilities,
    pub implementation: Option<Implementation>,
}

#[async_trait]
impl ClientDelegate for DenyPermissions {
    async fn request_permission(
        &self,
        _request: RequestPermissionRequest,
    ) -> RequestPermissionResponse {
        RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled)
    }
}

/// Run one persistent ACP session against any agent transport.
///
/// Passing [`crate::acp_agent`] keeps Longbridge AI fully in-process. Passing
/// [`ExternalAgent`] launches Codex, Claude, or another ACP subprocess. The
/// callback receives an [`ActiveSession`] and may send any number of prompts.
pub async fn with_session<R>(
    agent: impl ConnectTo<Client> + 'static,
    cwd: impl Into<PathBuf>,
    delegate: Arc<dyn ClientDelegate>,
    operation: impl for<'runner> AsyncFnOnce(
        ActiveSession<'runner, Agent>,
    ) -> Result<R, agent_client_protocol::Error>,
) -> Result<R, agent_client_protocol::Error> {
    with_initialized_session(agent, cwd, delegate, async move |_handshake, session| {
        operation(session).await
    })
    .await
}

/// Initialize an ACP agent, expose its negotiated capabilities, and run one
/// persistent session.
pub async fn with_initialized_session<R>(
    agent: impl ConnectTo<Client> + 'static,
    cwd: impl Into<PathBuf>,
    delegate: Arc<dyn ClientDelegate>,
    operation: impl for<'runner> AsyncFnOnce(
        AgentHandshake,
        ActiveSession<'runner, Agent>,
    ) -> Result<R, agent_client_protocol::Error>,
) -> Result<R, agent_client_protocol::Error> {
    let delegate_for_permissions = Arc::clone(&delegate);
    Client
        .builder()
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| {
                responder.respond(delegate_for_permissions.request_permission(request).await)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, async move |connection| {
            let response = connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let handshake = AgentHandshake {
                protocol_version: response.protocol_version,
                capabilities: response.agent_capabilities,
                implementation: response.agent_info,
            };
            connection
                .build_session(cwd.into())
                .block_task()
                .run_until(async move |session| operation(handshake, session).await)
                .await
        })
        .await
}

/// Launch an external ACP process and run a persistent desktop chat session.
pub async fn with_external_session<R>(
    config: ExternalAgentConfig,
    cwd: impl Into<PathBuf>,
    delegate: Arc<dyn ClientDelegate>,
    operation: impl for<'runner> AsyncFnOnce(
        ActiveSession<'runner, Agent>,
    ) -> Result<R, agent_client_protocol::Error>,
) -> Result<R, agent_client_protocol::Error> {
    with_session(ExternalAgent::new(config), cwd, delegate, operation).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{acp_agent, AgentBackend, AgentEvent, BackendError};
    use futures::{stream, stream::BoxStream};
    use std::path::Path;

    struct Echo;

    #[async_trait]
    impl AgentBackend for Echo {
        type Session = ();

        async fn prompt(
            &self,
            _session: (),
            prompt: String,
            _cwd: &Path,
        ) -> Result<BoxStream<'static, Result<AgentEvent<()>, BackendError>>, BackendError>
        {
            Ok(Box::pin(stream::iter([
                Ok(AgentEvent::Text(format!("echo: {prompt}"))),
                Ok(AgentEvent::Finished(())),
            ])))
        }
    }

    #[tokio::test]
    async fn desktop_client_runs_embedded_agent_without_subprocess() {
        let answer = with_session(
            acp_agent(Echo),
            "/tmp",
            Arc::new(DenyPermissions),
            async |mut session| {
                session.send_prompt("hello")?;
                session.read_to_string().await
            },
        )
        .await
        .expect("embedded ACP session");

        assert_eq!(answer, "echo: hello");
    }

    #[tokio::test]
    async fn desktop_client_receives_negotiated_agent_metadata() {
        let implementation = with_initialized_session(
            acp_agent(Echo),
            "/tmp",
            Arc::new(DenyPermissions),
            async |handshake, _session| Ok(handshake.implementation),
        )
        .await
        .expect("embedded ACP handshake")
        .expect("agent implementation");

        assert_eq!(implementation.name, "Longbridge AI");
    }

    #[test]
    fn external_agent_presets_use_acp_adapters() {
        assert_eq!(
            ExternalAgentKind::Codex.config().command(),
            Path::new("codex-acp")
        );
        assert_eq!(
            ExternalAgentKind::Claude.config().command(),
            Path::new("claude-agent-acp")
        );
    }
}
