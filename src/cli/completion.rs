use clap::CommandFactory;
use clap_complete::{generate, Shell};

use crate::cli::Cli;

pub fn cmd_completion(shell: Shell) {
    let mut cmd = Cli::command();
    generate(shell, &mut cmd, "longbridge", &mut std::io::stdout());
}
