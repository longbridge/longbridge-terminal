use anyhow::Result;

pub fn cmd_init(channel_key: &str) -> Result<()> {
    crate::auth::save_channel(channel_key)?;
    println!("Initialized successfully.");
    Ok(())
}
