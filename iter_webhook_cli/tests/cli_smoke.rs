//! Smoke tests for the `iter-webhook` binary.
//!
//! See `iter_watch_cli/tests/cli_smoke.rs` for the rationale on why this
//! file focuses on negative paths and help: the started/stopped banner
//! pair is already pinned by `iter_cron_cli` and `iter_files_cli`
//! smoke tests through the same shared `cli_eprintln!` macro.

use std::process::{Command, Stdio};

fn iter_webhook_bin() -> &'static str {
    env!("CARGO_BIN_EXE_iter-webhook")
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(iter_webhook_bin())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("spawning iter-webhook")
}

/// Negative path: a non-existent `--secret-file` is rejected before any
/// banner or HTTP work runs. Maps to `USER_INPUT (1)` via the
/// `WebhookCliError::SecretFileRead { source: NotFound, .. }` arm.
#[test]
fn missing_secret_file_exits_user_input_before_banner() {
    let out = run(&[
        "--queue-url",
        "memory://",
        "--secret-file",
        "/tmp/iter-webhook-secret-does-not-exist",
    ]);
    assert!(
        !out.status.success(),
        "should fail on missing --secret-file"
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "USER_INPUT (1) expected; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // P2: secret resolution failure happens before any stdout work, so
    // stdout must be untouched.
    assert!(
        out.stdout.is_empty(),
        "iter-webhook must not write stdout on early error; got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Error: "),
        "stderr must carry an 'Error: ' headline; got:\n{stderr}"
    );
    assert!(
        stderr.contains("reading secret file"),
        "diagnostic must name the failure cause; got:\n{stderr}"
    );
    // Secret resolution runs before the started banner.
    assert!(
        !stderr.contains("iter-webhook: started"),
        "started banner must not print before secret-file resolution; got:\n{stderr}"
    );
    assert!(
        !stderr.contains("SecretFileRead {"),
        "stderr looks like Debug output; got:\n{stderr}"
    );
}

/// Negative path: an unset `--secret-env` variable maps to
/// `WebhookCliError::SecretEnvMissing` (`USER_INPUT`).
#[test]
fn missing_secret_env_exits_user_input() {
    // Use a name that is overwhelmingly unlikely to be set in any
    // reasonable test environment.
    let out = Command::new(iter_webhook_bin())
        .args([
            "--queue-url",
            "memory://",
            "--secret-env",
            "ITER_WEBHOOK_TEST_DEFINITELY_UNSET_VAR_a8f2c0",
        ])
        .stdin(Stdio::null())
        .env_remove("ITER_WEBHOOK_TEST_DEFINITELY_UNSET_VAR_a8f2c0")
        .output()
        .expect("spawning iter-webhook");
    assert!(!out.status.success());
    assert_eq!(out.status.code(), Some(1));
    assert!(
        out.stdout.is_empty(),
        "iter-webhook must not write stdout on early error; got:\n{}",
        String::from_utf8_lossy(&out.stdout)
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
    assert!(out.status.success(), "iter-webhook --help must succeed");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("EXAMPLES:"), "got:\n{stdout}");
}

/// Negative path: an unsupported `--queue-url` scheme maps to
/// `RUNTIME (2)` — trigger binaries classify a bad queue URL as a
/// runtime failure, unlike `iter enqueue`, which treats it as user
/// input (1).
#[test]
fn invalid_queue_url_scheme_exits_runtime() {
    let out = run(&["--queue-url", "ftp://nope"]);
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
