use longbridge_ai_acp::{with_initialized_session, DenyPermissions, ExternalAgentConfig};
use std::sync::Arc;

#[tokio::test]
async fn cli_negotiates_acp_over_real_stdio() {
    let config = ExternalAgentConfig::new(env!("CARGO_BIN_EXE_longbridge"))
        .args(["acp", "--agent-id", "integration-test"])
        .env("LONGBRIDGE_APP_KEY", "test-app-key")
        .env("LONGBRIDGE_APP_SECRET", "test-app-secret")
        .env("LONGBRIDGE_ACCESS_TOKEN", "test-access-token")
        .env("LONGBRIDGE_REGION", "global");

    let handshake = with_initialized_session(
        longbridge_ai_acp::ExternalAgent::new(config),
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
