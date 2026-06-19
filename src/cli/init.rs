use anyhow::Result;

pub fn cmd_init(invite_code: &str) -> Result<()> {
    crate::auth::save_invite_code(invite_code)?;
    println!("Initialized successfully.");
    Ok(())
}

pub(crate) fn schema_for_path(path: &[String]) -> Option<super::schema::ResponseSchema> {
    (path == ["init"]).then(|| super::schema::text("Invite-code initialization status message"))
}
