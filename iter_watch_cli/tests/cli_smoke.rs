//! Smoke tests for the `iter-watch` binary.
//!
//! Mirrors `iter_cron_cli/tests/cli_smoke.rs` and
//! `iter_files_cli/tests/cli_smoke.rs`. The positive (started/stopped
//! banner pair) path is covered by those two binaries — the local
//! `cli_eprintln!` macro and `error::run_main` are exercised end-to-end
//! there. Here we focus on the watch-specific preflight check
//! (`WatchedDirMissing`) and the help surface, since reproducing a real
//! fs-event positive path deterministically requires more orchestration
//! than is healthy for a smoke test.

use std::process::{Command, Stdio};

fn iter_watch_bin() -> &'static str {
    env!("CARGO_BIN_EXE_iter-watch")
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(iter_watch_bin())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("spawning iter-watch")
}

/// Negative path: a missing `--dir` is rejected synchronously by
/// `WatchCliError::WatchedDirMissing` (`USER_INPUT`) before any banner
/// or queue work runs. The error chain must render through
/// `crate::error::print_error`, not the runtime's `Debug` formatter.
#[test]
fn missing_dir_exits_user_input_before_banner() {
    let out = run(&[
        "--queue-url",
        "memory://",
        "--dir",
        "/tmp/iter-does-not-exist",
    ]);
    assert!(!out.status.success(), "should fail on missing --dir");
    assert_eq!(
        out.status.code(),
        Some(1),
        "USER_INPUT (1) expected; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // P2: stdout is reserved for data. An early preflight error must not
    // leak any bytes onto stdout, even at the cost of skipping a banner.
    assert!(
        out.stdout.is_empty(),
        "iter-watch must not write stdout on early error; got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Error: "),
        "stderr must carry an 'Error: ' headline; got:\n{stderr}"
    );
    assert!(
        stderr.contains("watched directory does not exist"),
        "diagnostic must name the failure cause; got:\n{stderr}"
    );
    // The preflight check runs before the started banner; a regression
    // that moves the dir-existence check after the banner would let
    // operators see "iter-watch: started ..." for a doomed run, which
    // is misleading.
    assert!(
        !stderr.contains("iter-watch: started"),
        "started banner must not print before the dir-existence check; got:\n{stderr}"
    );
    assert!(
        !stderr.contains("WatchedDirMissing"),
        "stderr looks like Debug output; got:\n{stderr}"
    );
}

/// Negative path with `--quiet`: same exit code, same Error headline.
/// `--quiet` only suppresses *banners*, not error output (P7 — quiet is
/// for scripts, not for hiding diagnostics).
#[test]
fn quiet_does_not_suppress_errors() {
    let out = run(&[
        "--queue-url",
        "memory://",
        "--dir",
        "/tmp/iter-does-not-exist",
        "--quiet",
    ]);
    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(1));
    assert!(
        out.stdout.is_empty(),
        "iter-watch --quiet must not write stdout on early error; got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Error: "),
        "--quiet must not silence errors; got:\n{stderr}"
    );
}

/// `--help` renders without erroring and includes the documented
/// `EXAMPLES` section (P11).
#[test]
fn help_includes_examples_section() {
    let out = run(&["--help"]);
    assert!(out.status.success(), "iter-watch --help must succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("EXAMPLES:"), "got:\n{stdout}");
}

/// Negative path: an unsupported `--queue-url` scheme maps to
/// `RUNTIME (2)` — trigger binaries classify a bad queue URL as a
/// runtime failure, unlike `iter enqueue`, which treats it as user
/// input (1).
#[test]
fn invalid_queue_url_scheme_exits_runtime() {
    let out = run(&["--queue-url", "ftp://nope", "--dir", "."]);
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
