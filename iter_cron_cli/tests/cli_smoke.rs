//! Smoke tests for the `iter-cron` binary.
//!
//! These shell out to the freshly-built binary (`CARGO_BIN_EXE_iter_cron`)
//! so the full clap surface, banner emission, and exit-code mapping are
//! exercised end-to-end. They keep the trigger-CLI output contract pinned
//! against regressions: a typo in the `started`/`stopped` banner, a
//! missing `--quiet` branch, or an exit-code drift would surface here
//! before it surfaced in user reports.

use std::process::{Command, Stdio};

fn iter_cron_bin() -> &'static str {
    env!("CARGO_BIN_EXE_iter-cron")
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(iter_cron_bin())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("spawning iter-cron")
}

/// Positive path: `--at-startup --max-signals 1` emits one signal at
/// launch, the counting wrapper trips the cancellation token, and the
/// trigger exits cleanly with exit code 0. The `started` and `stopped`
/// banner lines must appear on stderr — and only on stderr — and stdout
/// must be empty.
#[test]
fn at_startup_with_max_signals_one_emits_then_exits_cleanly() {
    let out = run(&[
        "--queue-url",
        "memory://",
        "--schedule",
        "0 0 1 1 *",
        "--at-startup",
        "--max-signals",
        "1",
    ]);
    assert!(
        out.status.success(),
        "iter-cron exit={:?}\nstderr=\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stdout.is_empty(),
        "iter-cron must reserve stdout; got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Pin the banner shape declared in `iter_cron_cli/src/main.rs`. A
    // future refactor that drops the `instance=...` prefix or rewords
    // `started`/`stopped` is a contract change and must update this test
    // alongside `docs/cli-output-contract.md`.
    assert!(
        stderr.contains("iter-cron: started ("),
        "stderr missing started banner; got:\n{stderr}"
    );
    assert!(
        stderr.contains("iter-cron: stopped ("),
        "stderr missing stopped banner; got:\n{stderr}"
    );
    assert!(
        stderr.contains("published=1"),
        "stopped banner should report published=1; got:\n{stderr}"
    );
}

/// `--quiet` suppresses both banner lines on the same positive path.
#[test]
fn quiet_suppresses_banners() {
    let out = run(&[
        "--queue-url",
        "memory://",
        "--schedule",
        "0 0 1 1 *",
        "--at-startup",
        "--max-signals",
        "1",
        "--quiet",
    ]);
    assert!(
        out.status.success(),
        "stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("iter-cron: started"),
        "--quiet must suppress started banner; got:\n{stderr}"
    );
    assert!(
        !stderr.contains("iter-cron: stopped"),
        "--quiet must suppress stopped banner; got:\n{stderr}"
    );
}

/// Negative path: an invalid cron expression maps to `USER_INPUT (1)`
/// via `CronTriggerError::InvalidExpression`. The error chain must be
/// rendered through `crate::error::print_error` (a one-line `Error:`
/// headline + indented `caused by:` chain), not the Rust runtime's
/// `Debug` formatter.
#[test]
fn invalid_schedule_exits_user_input() {
    let out = run(&["--queue-url", "memory://", "--schedule", "this is not cron"]);
    assert!(
        !out.status.success(),
        "should fail on invalid cron expression"
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "USER_INPUT (1) expected; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Error: "),
        "stderr must carry an 'Error: ' headline; got:\n{stderr}"
    );
    // Narrow Debug-formatting guard: catches a regression where
    // `print_error` is bypassed and the runtime renders the variant via
    // its `Debug` impl. Tied to the same contract checked by
    // `error_output_is_diagnostic_not_debug` in `iter_cli/tests/contract.rs`.
    assert!(
        !stderr.contains("InvalidExpression("),
        "stderr looks like Debug output; got:\n{stderr}"
    );
}

/// Negative path: an unsupported `--queue-url` scheme maps to
/// `RUNTIME (2)` — trigger binaries classify a bad queue URL as a
/// runtime failure, unlike `iter enqueue`, which treats it as user
/// input (1).
#[test]
fn invalid_queue_url_scheme_exits_runtime() {
    let out = run(&["--queue-url", "ftp://nope", "--schedule", "0 0 1 1 *"]);
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

/// `--help` renders without erroring and includes the documented
/// `EXAMPLES` section (P11).
#[test]
fn help_includes_examples_section() {
    let out = run(&["--help"]);
    assert!(out.status.success(), "iter-cron --help must succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("EXAMPLES:"),
        "--help should advertise EXAMPLES; got:\n{stdout}"
    );
}
