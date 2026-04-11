//! Integration tests for `iter enqueue`.
//!
//! These tests shell out to the freshly-built `iter` binary
//! (`CARGO_BIN_EXE_iter`) so the full clap surface is exercised end-to-end.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn iter_bin() -> &'static str {
    env!("CARGO_BIN_EXE_iter")
}

fn run_iter(cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(iter_bin())
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("spawning iter binary")
}

fn write(path: &Path, contents: &str) {
    std::fs::write(path, contents).expect("writing fixture");
}

fn count_pending(queue_root: &Path) -> usize {
    let pending = queue_root.join("pending");
    if !pending.exists() {
        return 0;
    }
    std::fs::read_dir(&pending).expect("read pending").count()
}

#[test]
fn enqueue_via_iterfile_writes_pending_file() {
    let dir = TempDir::new().expect("tempdir");
    let queue_path = dir.path().join("q");
    let iterfile = dir.path().join("Iterfile");

    write(
        &iterfile,
        &format!(
            r#"queue file {{ path = "{}" }}
workspace local {{ base = "." }}
agent claude {{
    mode = print
    command = "claude"
}}
runner {{
    continue_on_error = false
    behavior = wait
}}
prompt "{{{{metadata.prompt}}}}"
"#,
            queue_path.display()
        ),
    );

    let out = run_iter(
        dir.path(),
        &["enqueue", "-f", "Iterfile", "-m", "prompt=hello"],
    );
    assert!(
        out.status.success(),
        "enqueue exit={:?}\nstderr=\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let id = stdout.trim();
    assert!(
        !id.is_empty(),
        "expected signal id on stdout, got {stdout:?}"
    );

    assert_eq!(count_pending(&queue_path), 1, "exactly one pending file");
}

#[test]
fn enqueue_via_queue_url_writes_pending_file() {
    let dir = TempDir::new().expect("tempdir");
    let queue_path = dir.path().join("qu");
    let url = format!("file://{}", queue_path.display());

    let out = run_iter(
        dir.path(),
        &[
            "enqueue",
            "--queue-url",
            &url,
            "-m",
            "prompt=via-url",
            "-m",
            "tag=critical-bit",
        ],
    );
    assert!(
        out.status.success(),
        "enqueue exit={:?}\nstderr=\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(count_pending(&queue_path), 1);
}

#[test]
fn enqueue_via_compose_single_queue_works() {
    let dir = TempDir::new().expect("tempdir");
    let queue_path = dir.path().join("q-compose");
    let compose = dir.path().join("compose.iter");
    let iterfile = dir.path().join("Iterfile");

    write(
        &compose,
        &format!(
            r#"queue main file {{ path = "{}" }}

service worker {{
    build = "./Iterfile"
}}
"#,
            queue_path.display()
        ),
    );
    write(
        &iterfile,
        r#"workspace local { base = "." }
agent claude {
    mode = print
    command = "claude"
}
runner {
    continue_on_error = false
    behavior = wait
}
prompt "stub"
"#,
    );

    let out = run_iter(
        dir.path(),
        &["enqueue", "-f", "compose.iter", "-m", "prompt=hi"],
    );
    assert!(
        out.status.success(),
        "stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(count_pending(&queue_path), 1);
}

#[test]
fn enqueue_via_compose_multi_queue_requires_queue_flag() {
    let dir = TempDir::new().expect("tempdir");
    let q_a = dir.path().join("qa");
    let q_b = dir.path().join("qb");
    let compose = dir.path().join("compose.iter");
    let iterfile = dir.path().join("Iterfile");

    write(
        &compose,
        &format!(
            r#"queue alpha file {{ path = "{}" }}
queue beta file {{ path = "{}" }}

service worker_a {{
    build = "./Iterfile"
    queue = alpha
}}
service worker_b {{
    build = "./Iterfile"
    queue = beta
}}
"#,
            q_a.display(),
            q_b.display()
        ),
    );
    write(
        &iterfile,
        r#"workspace local { base = "." }
agent claude {
    mode = print
    command = "claude"
}
runner {
    continue_on_error = false
    behavior = wait
}
prompt "stub"
"#,
    );

    let out = run_iter(
        dir.path(),
        &["enqueue", "-f", "compose.iter", "-m", "prompt=hi"],
    );
    assert!(!out.status.success(), "expected failure on ambiguous queue");
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Display-only assertion: the variant-name arm of an OR would let a
    // Debug-formatting regression (`AmbiguousQueue { .. }`) pass here while
    // `error_output_is_diagnostic_not_debug` in `tests/contract.rs` is busy
    // catching the same regression elsewhere. Keep the two contracts in step.
    assert!(stderr.contains("multiple queues"), "stderr was: {stderr}");

    let out_named = run_iter(
        dir.path(),
        &[
            "enqueue",
            "-f",
            "compose.iter",
            "--queue",
            "beta",
            "-m",
            "prompt=ok",
        ],
    );
    assert!(
        out_named.status.success(),
        "with --queue beta, stderr=\n{}",
        String::from_utf8_lossy(&out_named.stderr)
    );
    assert_eq!(count_pending(&q_b), 1);
    assert_eq!(count_pending(&q_a), 0);
}

#[test]
fn enqueue_unknown_queue_name_errors() {
    let dir = TempDir::new().expect("tempdir");
    let q_a = dir.path().join("qa");
    let compose = dir.path().join("compose.iter");
    let iterfile = dir.path().join("Iterfile");

    write(
        &compose,
        &format!(
            r#"queue alpha file {{ path = "{}" }}

service worker {{
    build = "./Iterfile"
}}
"#,
            q_a.display()
        ),
    );
    write(
        &iterfile,
        r#"workspace local { base = "." }
agent claude {
    mode = print
    command = "claude"
}
runner {
    continue_on_error = false
    behavior = wait
}
prompt "stub"
"#,
    );

    let out = run_iter(
        dir.path(),
        &[
            "enqueue",
            "-f",
            "compose.iter",
            "--queue",
            "nope",
            "-m",
            "prompt=x",
        ],
    );
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("nope"), "stderr was: {stderr}");
}

#[test]
fn enqueue_invalid_metadata_separator_errors() {
    let dir = TempDir::new().expect("tempdir");
    let queue_path = dir.path().join("q");
    let url = format!("file://{}", queue_path.display());

    let out = run_iter(
        dir.path(),
        &["enqueue", "--queue-url", &url, "-m", "no-equals"],
    );
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("KEY=VALUE"), "stderr was: {stderr}");
}

#[test]
fn enqueue_invalid_metadata_key_errors() {
    let dir = TempDir::new().expect("tempdir");
    let queue_path = dir.path().join("q");
    let url = format!("file://{}", queue_path.display());

    let out = run_iter(
        dir.path(),
        &["enqueue", "--queue-url", &url, "-m", "not a key=v"],
    );
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("invalid metadata key"),
        "stderr was: {stderr}"
    );
}

#[test]
fn enqueue_unsupported_url_scheme_errors() {
    let dir = TempDir::new().expect("tempdir");

    let out = run_iter(
        dir.path(),
        &["enqueue", "--queue-url", "kafka://nope", "-m", "prompt=hi"],
    );
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unsupported queue url"),
        "stderr was: {stderr}"
    );
}

#[test]
fn enqueue_no_source_errors() {
    let dir = TempDir::new().expect("tempdir");

    let out = run_iter(dir.path(), &["enqueue", "-m", "prompt=x"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no queue source"), "stderr was: {stderr}");
}

#[test]
fn enqueue_missing_iterfile_errors() {
    let dir = TempDir::new().expect("tempdir");

    let out = run_iter(
        dir.path(),
        &["enqueue", "-f", "does-not-exist", "-m", "prompt=x"],
    );
    assert!(!out.status.success());
    // USER_INPUT (1) — a missing file the user named is a user mistake, not
    // a parse / config error (CONFIG 64) and not a runtime fault (RUNTIME 2).
    // Ties into the cross-subcommand contract pinned by
    // `compose_subcommands_agree_on_missing_file_exit_code` in tests/contract.rs.
    assert_eq!(
        out.status.code(),
        Some(1),
        "USER_INPUT (1) expected; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn enqueue_priority_high_accepted() {
    let dir = TempDir::new().expect("tempdir");
    let queue_path = dir.path().join("q");
    let url = format!("file://{}", queue_path.display());

    let out = run_iter(
        dir.path(),
        &[
            "enqueue",
            "--queue-url",
            &url,
            "-m",
            "prompt=urgent",
            "--priority",
            "high",
        ],
    );
    assert!(
        out.status.success(),
        "stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(count_pending(&queue_path), 1);

    let entry = std::fs::read_dir(queue_path.join("pending"))
        .expect("read pending")
        .next()
        .expect("at least one entry")
        .expect("entry ok");
    let name = entry.file_name();
    let name_str = name.to_string_lossy();
    // Filename layout: {255-priority:03}-... ; HIGH=75 -> 255-75 = 180.
    assert!(
        name_str.starts_with("180-"),
        "expected priority-encoded prefix 180-, got {name_str}"
    );
}
