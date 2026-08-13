# longbridge-ai-acp

Embeddable Agent Client Protocol bridge for Longbridge AI. It supports both
sides needed by Longbridge products:

- expose a Longbridge AI agent to Zed and other ACP clients;
- connect a native Rust desktop UI to external ACP agents such as Codex and
  Claude through the official ACP SDK.

The crate does not read CLI credentials. A desktop app supplies its own SDK
configuration and API endpoint:

```rust,no_run
use longbridge_ai_acp::LongbridgeAgent;

# fn example(oauth: longbridge::oauth::OAuth) {
let config = longbridge::Config::from_oauth(oauth);
let agent = LongbridgeAgent::from_api(
    config,
    "https://openapi.longbridge.com",
    "ag_7d3f9b2c",
).expect("valid API endpoint");
// Pass `agent` directly to your in-process ACP client or `serve_stdio`.
# }
```

For an existing host-owned configuration, use
`LongbridgeAgent::from_config`. For an external subprocess agent, construct an
`ExternalAgentConfig` and connect `ExternalAgent` to an
`agent_client_protocol::Client`.

Desktop applications normally use `with_session` for an embedded Longbridge
agent and `with_external_session` for a subprocess agent. Both yield one
persistent `ActiveSession`, so the UI can send multiple prompts without
recreating the conversation. Implement `ClientDelegate` to route permission
requests into the product's own confirmation UI; `DenyPermissions` is provided
for read-only/chat-only hosts.

Use `with_initialized_session` when the UI also needs the negotiated protocol
version, capabilities, and implementation metadata. `ExternalAgentKind::Codex`
and `ExternalAgentKind::Claude` produce launch configurations for the separately
installed `codex-acp` and `claude-agent-acp` adapters. The native provider CLIs
do not expose ACP themselves.

For a UI model that must outlive one callback, use `DesktopSession`. It owns the
ACP connection and, for external agents, the subprocess. The UI sends commands
and receives protocol events without retaining SDK lifetimes:

```rust,no_run
use longbridge_ai_acp::{
    acp_agent, DenyPermissions, DesktopSession, DesktopSessionEvent,
    LongbridgeAgent,
};
use std::sync::Arc;

# async fn example(agent: LongbridgeAgent) -> Result<(), Box<dyn std::error::Error>> {
let mut session = DesktopSession::connect(
    acp_agent(agent),
    std::env::current_dir()?,
    Arc::new(DenyPermissions),
).await?;

session.prompt("Summarize my portfolio risk").await?;
while let Some(event) = session.next_event().await {
    match event {
        DesktopSessionEvent::Update(update) => println!("{update:?}"),
        DesktopSessionEvent::TurnFinished(_) => break,
        DesktopSessionEvent::Failed(error) => return Err(error.into()),
    }
}
session.shutdown().await;
# Ok(())
# }
```

`DesktopSession::cancel` and `shutdown` are safe UI lifecycle operations. A
second prompt before `TurnFinished` returns `SessionControlError::Busy`.
