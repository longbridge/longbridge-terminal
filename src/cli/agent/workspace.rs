//! `agent workspaces`: list the AI workspaces that hold the account's agents.

use anyhow::Result;
use serde_json::json;

use super::client::{AgentApi, LbAgentApi, WorkspaceInfo};
use crate::cli::output::{fmt_unix_ts, print_json_value, print_table};
use crate::cli::OutputFormat;

pub async fn cmd_workspaces(format: &OutputFormat, verbose: bool) -> Result<()> {
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

/// Build the pretty-table rows for `agent workspaces`. `id` and `name` are
/// server-supplied and printed verbatim, so they are stripped of control
/// characters first (JSON output stays raw — serde escapes it safely).
fn workspace_rows(workspaces: &[WorkspaceInfo]) -> Vec<Vec<String>> {
    use super::render::strip_control_chars;
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
