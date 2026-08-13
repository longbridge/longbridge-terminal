use longbridge_ai_acp::{
    with_initialized_session, DenyPermissions, ExternalAgent, ExternalAgentConfig,
};
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let command = args.next().ok_or("usage: acp_probe <command> [args...]")?;
    let config =
        ExternalAgentConfig::new(command).args(args.map(|arg| arg.to_string_lossy().into_owned()));
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let handshake = with_initialized_session(
                ExternalAgent::new(config),
                std::env::current_dir()?,
                Arc::new(DenyPermissions),
                async |handshake, _session| Ok(handshake),
            )
            .await?;
            println!(
                "ACP {:?}: {}",
                handshake.protocol_version,
                handshake
                    .implementation
                    .map_or_else(|| "unknown agent".to_owned(), |info| info.name)
            );
            Ok::<_, Box<dyn std::error::Error>>(())
        })
}
