use longbridge_ai_acp::{
    with_initialized_session, DenyPermissions, DesktopSession, ExternalAgent, ExternalAgentConfig,
};
use std::sync::Arc;

#[tokio::test]
async fn cli_negotiates_acp_over_real_stdio() {
    let handshake = with_initialized_session(
        ExternalAgent::new(cli_config()),
        std::env::temp_dir(),
        Arc::new(DenyPermissions),
        async |handshake, _session| Ok(handshake),
    )
    .await
    .expect("CLI ACP stdio handshake");

    assert_eq!(
        handshake.implementation.expect("agent info").name,
        "Longbridge AI"
    );
}

#[tokio::test]
async fn desktop_session_owns_external_agent_lifecycle() {
    let session = DesktopSession::connect(
        ExternalAgent::new(cli_config()),
        std::env::temp_dir(),
        Arc::new(DenyPermissions),
    )
    .await
    .expect("external desktop session");

    assert_eq!(
        session
            .handshake()
            .implementation
            .as_ref()
            .expect("agent info")
            .name,
        "Longbridge AI"
    );
    session.shutdown().await;
}

fn cli_config() -> ExternalAgentConfig {
    ExternalAgentConfig::new(env!("CARGO_BIN_EXE_longbridge"))
        .arg("acp")
        .env("LONGBRIDGE_APP_KEY", "test-app-key")
        .env("LONGBRIDGE_APP_SECRET", "test-app-secret")
        .env("LONGBRIDGE_ACCESS_TOKEN", "test-access-token")
        .env("LONGBRIDGE_REGION", "global")
}
