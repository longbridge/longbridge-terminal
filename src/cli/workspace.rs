//! AI workspace commands (list workspaces for the current account).

use anyhow::Result;
use serde_json::json;

use super::agent::client::{AgentApi, LbAgentApi, WorkspaceInfo};
use super::output::{fmt_unix_ts, print_json_value, print_table};
use super::{OutputFormat, WorkspaceCmd};

pub async fn cmd_workspace(
    cmd: Option<WorkspaceCmd>,
    format: &OutputFormat,
    verbose: bool,
) -> Result<()> {
    match cmd {
        // Bare `longbridge workspace`: show what the group offers rather than
        // guessing a subcommand. See `exit_with_subcommand_help`.
        None => crate::cli::exit_with_subcommand_help("workspace"),
        Some(WorkspaceCmd::List) => cmd_list(format, verbose).await,
    }
}

async fn cmd_list(format: &OutputFormat, verbose: bool) -> Result<()> {
    let api = LbAgentApi { verbose };
    let workspaces = api.list_workspaces().await?;
    match format {
        OutputFormat::Json => {
            print_json_value(&json!({ "workspaces": workspaces }), format);
        }
        OutputFormat::Pretty => {
            let rows = workspace_rows(&workspaces);
            print_table(&["ID", "NAME", "CREATED_AT", "UPDATED_AT"], rows, format);
        }
    }
    Ok(())
}

/// Build the pretty-table rows for `workspace list`. `id` and `name` are
/// server-supplied and printed verbatim, so they are stripped of control
/// characters first (JSON output stays raw — serde escapes it safely).
fn workspace_rows(workspaces: &[WorkspaceInfo]) -> Vec<Vec<String>> {
    use super::agent::render::strip_control_chars;
    workspaces
        .iter()
        .map(|w| {
            vec![
                strip_control_chars(&w.id),
                strip_control_chars(&w.name),
                fmt_unix_ts(w.created_at),
                fmt_unix_ts(w.updated_at),
            ]
        })
        .collect()
}

pub(crate) fn schema_for_path(path: &[String]) -> Option<super::schema::ResponseSchema> {
    use super::schema::object;

    match path.join(" ").as_str() {
        "workspace" | "workspace list" => Some(object(
            "AI workspaces for the current account",
            &["workspaces"],
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{workspace_rows, WorkspaceInfo};

    #[test]
    fn workspace_rows_strip_control_chars() {
        let rows = workspace_rows(&[WorkspaceInfo {
            id: "33\x1b[31m".into(),
            name: "Long\x1b]0;pwn\x07bridge".into(),
            created_at: 0,
            updated_at: 0,
        }]);
        assert_eq!(rows.len(), 1);
        let joined = rows[0].join("|");
        assert!(!joined.contains('\x1b'), "ESC survived: {joined:?}");
        assert!(!joined.contains('\x07'), "BEL survived: {joined:?}");
        assert!(joined.contains("33") && joined.contains("bridge"));
    }
}
