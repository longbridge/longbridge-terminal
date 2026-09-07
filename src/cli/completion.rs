use clap::CommandFactory;
use clap_complete::{generate, Shell};

use crate::cli::Cli;

pub fn cmd_completion(shell: Shell) {
    let mut cmd = Cli::command();
    // Generated into memory and printed, rather than written straight to
    // stdout: `clap_complete` unwraps its own write errors, so `longbridge
    // completion zsh | head -1` aborted inside the generator before the
    // crate's broken-pipe handling could see it. A completion script is a few
    // hundred KiB, so buffering it costs nothing.
    let mut script = Vec::new();
    generate(shell, &mut cmd, "longbridge", &mut script);
    print!("{}", String::from_utf8_lossy(&script));
}

pub(crate) fn schema_for_path(path: &[String]) -> Option<super::schema::ResponseSchema> {
    (path == ["completion"]).then(|| super::schema::text("Shell completion script"))
}
