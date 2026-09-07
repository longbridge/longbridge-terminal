//! A reader that stops early — `longbridge ... | head -1` — must end the run
//! quietly. Rust ignores `SIGPIPE`, so the failed write surfaces as `EPIPE`,
//! and the release profile builds with `panic = "abort"`: left unhandled, an
//! everyday pipeline produced `SIGABRT` and a core dump.

use std::io::Read;
use std::process::{Command, Stdio};

/// `completion zsh` is the subject because it needs no credentials and emits a
/// few hundred KiB — far past the pipe buffer, so the child is still writing
/// when the reader goes away, which is what makes the closed pipe observable.
#[test]
fn closing_stdout_early_exits_quietly() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_longbridge"))
        .args(["completion", "zsh"])
        // Pins the access point, which skips the startup probe and keeps the
        // run off the network.
        .env("LONGBRIDGE_REGION", "global")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn longbridge");

    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut first = [0u8; 1];
    stdout.read_exact(&mut first).expect("read first byte");
    // Closing the read end is what `head` does once it has what it asked for.
    drop(stdout);

    let output = child.wait_with_output().expect("wait for longbridge");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !stderr.contains("panicked"),
        "panicked on a closed stdout: {stderr}"
    );
    // `None` here means death by signal — the abort this guards against.
    assert_eq!(output.status.code(), Some(0), "stderr: {stderr}");
}
