use clap::CommandFactory;
use clap_complete::{generate, Shell};

use crate::cli::Cli;

pub fn cmd_completion(shell: Shell) {
    let mut cmd = Cli::command();
    generate(shell, &mut cmd, "longport", &mut std::io::stdout());
}

pub(crate) fn schema_for_path(path: &[String]) -> Option<super::schema::ResponseSchema> {
    (path == ["completion"]).then(|| super::schema::text("Shell completion script"))
}
