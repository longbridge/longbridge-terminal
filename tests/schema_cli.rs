use std::process::Command;

struct CliOutput {
    status: i32,
    stdout: String,
    stderr: String,
}

fn run(args: &[&str]) -> CliOutput {
    let output = Command::new(env!("CARGO_BIN_EXE_longbridge"))
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run longbridge {args:?}: {e}"));

    CliOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn schema(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout).expect("schema stdout should be valid JSON")
}

#[test]
fn quote_schema_prints_json_schema_without_auth() {
    let out = run(&["quote", "--schema"]);

    assert_eq!(out.status, 0, "stderr: {}", out.stderr);
    assert!(out.stderr.is_empty(), "stderr: {}", out.stderr);
    let schema = schema(&out.stdout);
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(schema["type"], "array");
    assert_eq!(schema["items"]["type"], "object");
    assert_eq!(schema["items"]["properties"]["symbol"]["type"], "string");
    assert_eq!(schema["items"]["properties"]["last"]["type"], "string");
    assert_eq!(
        schema["items"]["properties"]["pre_market"]["type"],
        "object"
    );
}

#[test]
fn depth_schema_does_not_require_symbol() {
    let out = run(&["depth", "--schema"]);

    assert_eq!(out.status, 0, "stderr: {}", out.stderr);
    assert!(out.stderr.is_empty(), "stderr: {}", out.stderr);
    let schema = schema(&out.stdout);
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["asks"]["type"], "array");
    assert_eq!(schema["properties"]["asks"]["items"]["type"], "object");
    assert_eq!(schema["properties"]["bids"]["type"], "array");
}

#[test]
fn nested_command_schema_uses_nested_response_shape() {
    let out = run(&["kline", "history", "--schema"]);

    assert_eq!(out.status, 0, "stderr: {}", out.stderr);
    assert!(out.stderr.is_empty(), "stderr: {}", out.stderr);
    let schema = schema(&out.stdout);
    assert_eq!(schema["type"], "array");
    assert_eq!(schema["items"]["properties"]["timestamp"]["type"], "string");
    assert_eq!(schema["items"]["properties"]["close"]["type"], "string");
}

#[test]
fn command_group_schema_prints_help() {
    let out = run(&["auth", "--schema"]);

    assert_eq!(out.status, 0, "stderr: {}", out.stderr);
    assert!(out.stderr.is_empty(), "stderr: {}", out.stderr);
    assert!(out.stdout.contains("Usage: longbridge auth"));
    assert!(out.stdout.contains("Commands:"));
}

#[test]
fn operational_leaf_command_has_schema_too() {
    let out = run(&["check", "--schema"]);

    assert_eq!(out.status, 0, "stderr: {}", out.stderr);
    assert!(out.stderr.is_empty(), "stderr: {}", out.stderr);
    let schema = schema(&out.stdout);
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["status"]["type"], "string");
}

#[test]
fn root_schema_reports_no_response_schema() {
    let out = run(&["--schema"]);

    assert_eq!(out.status, 1);
    assert!(out.stdout.is_empty());
    let err: serde_json::Value =
        serde_json::from_str(&out.stderr).expect("structured schema error");
    assert_eq!(
        err["error"],
        "no response schema available for \"longbridge\""
    );
}

// ── A2A command surface: behavior that only shows up in a real process ──────
//
// These spawn the binary because the properties under test are process-level:
// the exit status, which stream output lands on, and the fact that no auth or
// network happens first. A unit test calling the function directly cannot see
// any of that — it bypasses `main`'s preflight entirely.

/// Run with a HOME that holds no credentials, so the process is genuinely
/// logged out however the machine running the suite is configured.
fn run_logged_out(args: &[&str]) -> CliOutput {
    let empty_home = std::env::temp_dir().join(format!("lb-test-home-{}", std::process::id()));
    std::fs::create_dir_all(&empty_home).expect("create empty HOME");
    let output = Command::new(env!("CARGO_BIN_EXE_longbridge"))
        .args(args)
        .env("HOME", &empty_home)
        .output()
        .unwrap_or_else(|e| panic!("run longbridge {args:?}: {e}"));
    let _ = std::fs::remove_dir_all(&empty_home);
    CliOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

#[test]
fn bare_command_groups_print_help_without_authenticating() {
    for group in ["agent", "workspace"] {
        let out = run_logged_out(&[group]);

        assert_eq!(
            out.status, 2,
            "`{group}` should exit 2 like clap does for a missing subcommand; stderr: {}",
            out.stderr
        );
        assert!(
            out.stdout.contains(&format!("Usage: longbridge {group}")),
            "`{group}` should print its help; stdout: {}",
            out.stdout
        );
        // The point of resolving this in `main`: no auth, no network.
        assert!(
            !out.stderr.contains("auth login") && !out.stderr.contains("Authentication"),
            "`{group}` must not reach authentication; stderr: {}",
            out.stderr
        );
    }
}

#[test]
fn a_subcommand_that_needs_auth_still_reports_it() {
    // Guards against the checks above swallowing every failure.
    let out = run_logged_out(&["agent", "list"]);

    assert_ne!(out.status, 0);
    assert!(
        out.stderr.contains("auth login"),
        "expected an auth hint; stderr: {}",
        out.stderr
    );
}

#[test]
fn interactive_with_json_is_rejected_before_any_network() {
    let out = run_logged_out(&[
        "agent",
        "chat",
        "some-uid",
        "hi",
        "--interactive",
        "--format",
        "json",
    ]);

    assert_ne!(out.status, 0);
    assert!(
        out.stderr.contains("pretty"),
        "expected the pretty-only guard; stderr: {}",
        out.stderr
    );
    assert!(
        !out.stderr.contains("auth login"),
        "guard must fire before authentication; stderr: {}",
        out.stderr
    );
    assert!(
        out.stdout.is_empty(),
        "stdout must stay clean: {}",
        out.stdout
    );
}

#[test]
fn skill_document_is_offline_and_alias_matches() {
    let primary = run_logged_out(&["agent", "--skill"]);
    assert_eq!(primary.status, 0, "stderr: {}", primary.stderr);
    assert!(primary.stdout.contains("longbridge agent"));

    let alias = run_logged_out(&["agent", "--skills"]);
    assert_eq!(
        alias.stdout, primary.stdout,
        "--skills must stay a byte-identical alias"
    );

    let help = run(&["agent", "-h"]);
    assert!(
        help.stdout.contains("--skill"),
        "--skill must be advertised"
    );
    assert!(
        !help.stdout.contains("--skills"),
        "the plural alias must stay hidden: {}",
        help.stdout
    );
}
