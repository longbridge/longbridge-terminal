//! ACP integration for Longbridge AI.
//!
//! The crate deliberately contains no CLI globals. Desktop applications can
//! construct [`LongbridgeAgent`] from their own `OpenAPI` configuration and run
//! it in-process, while the `longbridge` binary exposes the same component over
//! ACP on stdio.

mod backend;
mod client;
mod longbridge;
mod server;

pub use agent_client_protocol as acp;
pub use backend::{AgentBackend, AgentEvent, AgentSession, BackendError, PendingInteraction};
pub use client::{
    with_external_session, with_initialized_session, with_session, AgentHandshake, ClientDelegate,
    DenyPermissions, ExternalAgent, ExternalAgentConfig, ExternalAgentKind,
};
pub use longbridge::LongbridgeAgent;
pub use server::{acp_agent, serve_stdio};
