use std::process::Command;

struct CliOutput {
    status: i32,
    stdout: String,
    stderr: String,
}

fn run(args: &[&str]) -> CliOutput {
    let output = Command::new(env!("CARGO_BIN_EXE_longport"))
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run longport {args:?}: {e}"));

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
    assert!(out.stdout.contains("Usage: longport auth"));
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
        "no response schema available for \"longport\""
    );
}

#[test]
fn root_help_does_not_offer_tui_command() {
    let out = run(&["--help"]);

    assert_eq!(out.status, 0, "stderr: {}", out.stderr);
    assert!(
        !out.stdout.contains("longport tui"),
        "help still advertises disabled TUI command:\n{}",
        out.stdout
    );
    assert!(
        !out.stdout
            .contains("Launch the interactive full-screen TUI"),
        "help still describes disabled TUI command:\n{}",
        out.stdout
    );
}
