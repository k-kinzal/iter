//! Smoke tests for the `iter-files` binary.
//!
//! Mirrors the structure of `iter_cron_cli/tests/cli_smoke.rs`: each
//! trigger-CLI surface gets the same banner-shape, `--quiet`-suppression,
//! and exit-code-mapping coverage so a typo in any of the four binaries
//! is caught by CI rather than user reports.

use std::io::Write;
use std::process::{Command, Stdio};

use tempfile::TempDir;

fn iter_files_bin() -> &'static str {
    env!("CARGO_BIN_EXE_iter-files")
}

/// Positive path: pipe a single line on stdin with `--max-signals 1`.
/// The trigger publishes one signal and the counting wrapper trips the
/// shutdown token, so the binary exits cleanly with the documented
/// banner pair on stderr.
#[test]
fn stdin_single_line_emits_then_exits_cleanly() {
    let mut child = Command::new(iter_files_bin())
        .args(["--queue-url", "memory://", "--max-signals", "1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn iter-files");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin.write_all(b"hello\n").expect("write stdin");
    }
    let out = child.wait_with_output().expect("wait iter-files");
    assert!(
        out.status.success(),
        "iter-files exit={:?}\nstderr=\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stdout.is_empty(),
        "iter-files must reserve stdout; got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("iter-files: started ("),
        "stderr missing started banner; got:\n{stderr}"
    );
    assert!(
        stderr.contains("iter-files: stopped ("),
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
    let mut child = Command::new(iter_files_bin())
        .args(["--queue-url", "memory://", "--max-signals", "1", "--quiet"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn iter-files");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin.write_all(b"hello\n").expect("write stdin");
    }
    let out = child.wait_with_output().expect("wait iter-files");
    assert!(
        out.status.success(),
        "stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("iter-files: started"),
        "--quiet must suppress started banner; got:\n{stderr}"
    );
    assert!(
        !stderr.contains("iter-files: stopped"),
        "--quiet must suppress stopped banner; got:\n{stderr}"
    );
}

/// Negative path: a malformed `--from` source maps to `USER_INPUT (1)`
/// via `FilesCliError::SourceUnknownForm`.
#[test]
fn unknown_source_form_exits_user_input() {
    let out = Command::new(iter_files_bin())
        .args(["--queue-url", "memory://", "--from", "http://nope"])
        .stdin(Stdio::null())
        .output()
        .expect("spawn iter-files");
    assert!(!out.status.success(), "should fail on unknown --from form");
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
    assert!(
        !stderr.contains("SourceUnknownForm("),
        "stderr looks like Debug output; got:\n{stderr}"
    );
}

/// `--from path:<empty file>` is a clean-exit path that exercises the
/// `path:` parser through real disk I/O. Pinned because the `path:`
/// prefix handling is the only place where `iter-files` does its own
/// argument parsing.
#[test]
fn empty_file_source_drains_to_zero_signals() {
    let dir = TempDir::new().expect("tempdir");
    let empty = dir.path().join("empty.txt");
    std::fs::write(&empty, b"").expect("write empty fixture");
    let out = Command::new(iter_files_bin())
        .args([
            "--queue-url",
            "memory://",
            "--from",
            &format!("path:{}", empty.display()),
        ])
        .stdin(Stdio::null())
        .output()
        .expect("spawn iter-files");
    assert!(
        out.status.success(),
        "exit={:?}; stderr=\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("published=0"), "got:\n{stderr}");
}

/// `--help` renders without erroring and includes the documented
/// `EXAMPLES` section (P11).
#[test]
fn help_includes_examples_section() {
    let out = Command::new(iter_files_bin())
        .args(["--help"])
        .stdin(Stdio::null())
        .output()
        .expect("spawn iter-files --help");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("EXAMPLES:"), "got:\n{stdout}");
}
