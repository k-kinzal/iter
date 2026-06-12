//! Smoke tests for the `iter-command` binary.
//!
//! These shell out to the freshly-built binary (`CARGO_BIN_EXE_iter-command`)
//! so the full clap surface and exit-code mapping are exercised end-to-end.

use std::process::{Command, Stdio};

fn iter_command_bin() -> &'static str {
    env!("CARGO_BIN_EXE_iter-command")
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(iter_command_bin())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("spawning iter-command")
}

/// Negative path: an unsupported `--queue-url` scheme maps to
/// `RUNTIME (2)` — trigger binaries classify a bad queue URL as a
/// runtime failure, unlike `iter enqueue`, which treats it as user
/// input (1).
#[test]
fn invalid_queue_url_scheme_exits_runtime() {
    let out = run(&["--queue-url", "ftp://nope", "--run", "true"]);
    assert!(
        !out.status.success(),
        "should fail on unsupported queue url scheme"
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "RUNTIME (2) expected; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Error: "),
        "stderr must carry an 'Error: ' headline; got:\n{stderr}"
    );
}
