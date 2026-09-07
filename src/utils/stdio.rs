//! Printing that survives a reader walking away.
//!
//! `longbridge kline 700.HK --count 1000 | head -1` leaves the CLI writing into
//! a pipe nobody is reading. Rust ignores `SIGPIPE` for the whole process, so
//! that write fails with `EPIPE` rather than killing us — and `println!` turns
//! the failure into a panic. Under this crate's `panic = "abort"` release
//! profile that panic is a `SIGABRT` and a core dump, for what is an ordinary
//! way to end a pipeline.
//!
//! The `print!` / `println!` / `eprint!` / `eprintln!` macros defined at the
//! crate root shadow the prelude ones and route every call site here, where a
//! closed reader ends the run quietly instead.
//!
//! Restoring `SIGPIPE` to `SIG_DFL` is the other common fix, and was rejected:
//! this binary holds long-lived WebSocket and HTTP connections, and `std`'s
//! vectored socket writes go through `writev` without `MSG_NOSIGNAL`, so the
//! default disposition would trade a stdout panic for the process being killed
//! mid-session by a peer reset.

use std::io::{self, Write};

/// Exit code for a run whose output was cut short by its reader.
///
/// Zero, not the 141 a `SIGPIPE` death would produce: `| head` is a normal way
/// to look at the front of a long result, and reporting failure there would
/// break `set -o pipefail` scripts that did nothing wrong.
const PIPE_CLOSED_EXIT_CODE: i32 = 0;

/// Write `args` to stdout, appending a newline when `newline`.
///
/// A closed stdout ends the run: stdout carries the answer the command was
/// asked for, and there is nowhere left to put the rest of it.
pub fn write_stdout(args: std::fmt::Arguments<'_>, newline: bool) {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let result = write(&mut out, args, newline);
    // Released before the exit below, so the flush of stdout on the way out is
    // not re-entering a lock this thread still holds.
    drop(out);

    match result {
        Ok(()) => {}
        Err(e) if is_broken_pipe(&e) => std::process::exit(PIPE_CLOSED_EXIT_CODE),
        // A genuine fault — a full disk, a bad file descriptor. Panicking is
        // what the std macros do, and keeps a real I/O failure loud.
        Err(e) => panic!("failed printing to stdout: {e}"),
    }
}

/// Write `args` to stderr, appending a newline when `newline`.
///
/// A closed stderr is dropped rather than fatal: stderr carries diagnostics
/// beside the real output, so losing the reader of the commentary is no reason
/// to truncate an answer that stdout is still delivering. When both are the
/// same closed pipe — `longbridge ... 2>&1 | head -1` — the next stdout write
/// ends the run anyway.
pub fn write_stderr(args: std::fmt::Arguments<'_>, newline: bool) {
    let stderr = io::stderr();
    let mut err = stderr.lock();
    let result = write(&mut err, args, newline);
    drop(err);

    match result {
        Ok(()) => {}
        Err(e) if is_broken_pipe(&e) => {}
        Err(e) => panic!("failed printing to stderr: {e}"),
    }
}

fn is_broken_pipe(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::BrokenPipe
}

fn write(out: &mut impl Write, args: std::fmt::Arguments<'_>, newline: bool) -> io::Result<()> {
    out.write_fmt(args)?;
    if newline {
        out.write_all(b"\n")?;
    }
    Ok(())
}
