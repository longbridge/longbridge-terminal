use anyhow::Result;

pub fn cmd_init(invite_code: &str) -> Result<()> {
    crate::auth::save_invite_code(invite_code)?;
    println!("Initialized successfully.");
    Ok(())
}
