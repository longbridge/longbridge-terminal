use longbridge_ai_acp::{
    with_initialized_session, DenyPermissions, DesktopSession, ExternalAgent, ExternalAgentConfig,
};
use std::sync::Arc;
use tokio::{
    io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader},
    process::Command,
};

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

#[tokio::test]
async fn unauthenticated_cli_advertises_terminal_login_during_initialize() {
    let home = tempfile::tempdir().expect("temporary home");
    let mut child = Command::new(env!("CARGO_BIN_EXE_longbridge"))
        .arg("acp")
        .env_clear()
        .env("HOME", home.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn CLI");
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientInfo": { "name": "test", "version": "1.0.0" },
            "clientCapabilities": { "terminal": true }
        }
    });
    let mut stdin = child.stdin.take().expect("child stdin");
    stdin
        .write_all(format!("{request}\n").as_bytes())
        .await
        .expect("write initialize");
    let mut response = String::new();
    let mut stdout = BufReader::new(child.stdout.take().expect("child stdout"));
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        stdout.read_line(&mut response),
    )
    .await
    .expect("initialize response timeout")
    .expect("read initialize response");
    let response: serde_json::Value = serde_json::from_str(&response).expect("JSON-RPC response");

    assert_eq!(response["result"]["authMethods"][0]["type"], "terminal");
    assert_eq!(response["result"]["agentCapabilities"]["loadSession"], true);
    assert_eq!(
        response["result"]["authMethods"][0]["args"],
        serde_json::json!(["auth", "login"])
    );
    child.kill().await.expect("stop CLI");
}

fn cli_config() -> ExternalAgentConfig {
    ExternalAgentConfig::new(env!("CARGO_BIN_EXE_longbridge"))
        .arg("acp")
        .env("HOME", "/nonexistent/longbridge-acp-test-home")
}
