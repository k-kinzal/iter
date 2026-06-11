//! CLI output-contract regression tests.
//!
//! These tests pin the user-visible bytes of each subcommand so a future
//! change cannot quietly drift the surface.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

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

/// Every shell flavour produces a non-empty completion script on stdout.
#[test]
fn completions_emit_non_empty_script_per_shell() {
    let dir = TempDir::new().expect("tempdir");
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let out = run_iter(dir.path(), &["completions", shell]);
        assert!(
            out.status.success(),
            "completions {shell} exit={:?}\nstderr=\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !out.stdout.is_empty(),
            "completions {shell} produced empty stdout"
        );
    }
}

/// Bash completions reference the `complete` builtin so
/// `source <(iter completions bash)` actually wires up.
#[test]
fn bash_completions_reference_complete_builtin() {
    let dir = TempDir::new().expect("tempdir");
    let out = run_iter(dir.path(), &["completions", "bash"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    // `complete -F` is the canonical clap_complete bash binding line; checking
    // both the function-binding flag and the `iter` invocation name guards
    // against a degenerate output where `complete` and `iter` only appear
    // in unrelated comments.
    assert!(
        stdout.contains("complete -F") && stdout.contains("iter"),
        "bash completion missing `complete -F`/iter; stdout starts with:\n{}",
        stdout.lines().take(5).collect::<Vec<_>>().join("\n")
    );
}

/// Error stream is "Error: <message>" + indented `caused by:` chain,
/// not a Debug dump of the error type.
#[test]
fn error_output_is_diagnostic_not_debug() {
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
        stderr.starts_with("Error: "),
        "stderr must start with 'Error: '; got:\n{stderr}"
    );
    // Narrow-scope guard: this asserts the *current* `EnqueueCmdError ::
    // MetadataKey(InvalidKey { .. })` Debug repr does not appear here,
    // not the general "no Debug formatting anywhere" property. A rename
    // of the variants would invalidate these specific substring checks
    // — at that point this test must be updated alongside the rename.
    assert!(
        !stderr.contains("MetadataKey(") && !stderr.contains("InvalidKey {"),
        "stderr looks like Debug output:\n{stderr}"
    );
    // Exit code is USER_INPUT for malformed metadata keys.
    assert_eq!(
        out.status.code(),
        Some(1),
        "USER_INPUT (1) expected for bad metadata key; stderr=\n{stderr}"
    );
}

/// `iter compose up --help` advertises the `--detach` flag, so users see
/// the detached self-fork path documented next to `iter run --detach`.
/// This pins the user-visible surface; if the flag is renamed or hidden
/// the test fails before shipping.
#[test]
fn compose_up_help_advertises_detach_flag() {
    let dir = TempDir::new().expect("tempdir");
    let out = run_iter(dir.path(), &["compose", "up", "--help"]);
    assert!(
        out.status.success(),
        "compose up --help failed; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("--detach"),
        "compose up --help must mention --detach; got:\n{stdout}"
    );
}

/// Typed exit codes — missing iterfile is `USER_INPUT` (1) on **both**
/// `iter run --detach <missing>` and `iter run <missing>` (foreground).
///
/// The two branches route through different error types
/// (`IterCliError::IterfileMissing` for detach via
/// `canonical_iterfile`; `RunCmdError::IterfileMissing` for foreground
/// via `canonical_iterfile_for_run`), and a regression that re-routes
/// either branch through a blanket RUNTIME mapping would have flipped
/// the user-visible exit code from 1 to 2.
#[test]
fn missing_iterfile_uses_user_input_exit_code() {
    let dir = TempDir::new().expect("tempdir");
    for args in [
        &["run", "does-not-exist", "--detach"][..],
        &["run", "does-not-exist"][..],
    ] {
        let out = run_iter(dir.path(), args);
        assert!(!out.status.success(), "{args:?} should fail");
        assert_eq!(
            out.status.code(),
            Some(1),
            "USER_INPUT (1) expected for {args:?}; stderr=\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.starts_with("Error: "),
            "{args:?}: stderr must start with 'Error: '; got:\n{stderr}"
        );
        assert!(
            stderr.contains("iterfile not found"),
            "{args:?}: stderr should name the missing iterfile; got:\n{stderr}"
        );
    }
}

/// `--help` for a top-level subcommand renders without erroring.
#[test]
fn top_level_help_works() {
    let dir = TempDir::new().expect("tempdir");
    for sub in [
        "run",
        "ps",
        "logs",
        "stop",
        "kill",
        "rm",
        "inspect",
        "enqueue",
        "compose",
        "validate",
        "completions",
        "process",
        "signal",
    ] {
        let out = run_iter(dir.path(), &[sub, "--help"]);
        assert!(
            out.status.success(),
            "iter {sub} --help failed (exit={:?})\nstderr=\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !out.stdout.is_empty(),
            "iter {sub} --help produced empty stdout"
        );
    }
}

/// `iter ps -q` on an empty registry succeeds and emits nothing on stdout.
#[test]
fn ps_quiet_on_empty_registry_is_silent() {
    let dir = TempDir::new().expect("tempdir");
    let mut cmd = Command::new(iter_bin());
    cmd.current_dir(dir.path())
        .env("HOME", dir.path())
        .args(["ps", "-q"]);
    let out = cmd.output().expect("spawn iter");
    assert!(
        out.status.success(),
        "iter ps -q exit={:?}\nstderr=\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stdout.is_empty(),
        "iter ps -q on empty registry must produce nothing on stdout; got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// `iter ps` table on an empty registry still emits the header row.
#[test]
fn ps_table_on_empty_registry_renders_header() {
    let dir = TempDir::new().expect("tempdir");
    let mut cmd = Command::new(iter_bin());
    cmd.current_dir(dir.path())
        .env("HOME", dir.path())
        .args(["ps"]);
    let out = cmd.output().expect("spawn iter");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    for col in ["ID", "NAME", "STATUS", "PID", "CREATED", "ITERFILE"] {
        assert!(
            stdout.contains(col),
            "ps table missing column {col}; stdout=\n{stdout}"
        );
    }
}

/// The canonical form `iter process ls` and the alias `iter ps` must
/// produce byte-identical output. They share `PsArgs` but route through
/// separate clap arms — a typo in either dispatch could silently diverge
/// them, and that divergence would only show up in user reports.
#[test]
fn process_ls_and_ps_alias_produce_identical_output() {
    let dir = TempDir::new().expect("tempdir");

    let canonical = Command::new(iter_bin())
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .args(["process", "ls"])
        .output()
        .expect("spawn iter process ls");
    let alias = Command::new(iter_bin())
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .args(["ps"])
        .output()
        .expect("spawn iter ps");

    assert!(canonical.status.success());
    assert!(alias.status.success());
    assert_eq!(
        canonical.stdout, alias.stdout,
        "process ls and ps stdout must be byte-identical"
    );
}

/// Same alias-equivalence guard for `iter process ls -q` vs `iter ps -q`.
#[test]
fn process_ls_quiet_matches_ps_quiet() {
    let dir = TempDir::new().expect("tempdir");

    let canonical = Command::new(iter_bin())
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .args(["process", "ls", "-q"])
        .output()
        .expect("spawn");
    let alias = Command::new(iter_bin())
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .args(["ps", "-q"])
        .output()
        .expect("spawn");

    assert!(canonical.status.success());
    assert!(alias.status.success());
    assert_eq!(canonical.stdout, alias.stdout);
}

/// `iter ps --format json` on an empty registry must produce empty stdout
/// (NDJSON of zero records). A regression that emits `[]`, `null`, or
/// the string `"empty"` would silently break `jq -s '.'` consumers.
#[test]
fn ps_format_json_on_empty_registry_is_empty() {
    let dir = TempDir::new().expect("tempdir");
    let out = Command::new(iter_bin())
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .args(["ps", "--format", "json"])
        .output()
        .expect("spawn");
    assert!(out.status.success());
    assert!(
        out.stdout.is_empty(),
        "ps --format json on empty registry must be empty; got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// `iter compose validate --format json` round-trips through `serde_json`
/// — proving the documented JSON envelope is actually parseable JSON, not
/// pretty-printed text that happens to look JSON-shaped.
#[test]
fn compose_validate_json_is_valid_json() {
    let dir = TempDir::new().expect("tempdir");
    let queue_path = dir.path().join("q");
    let compose = dir.path().join("compose.iter");
    std::fs::write(
        &compose,
        format!(
            "queue main file {{ path = \"{}\" }}\n\
             service api {{\n\
                 build = \"./Iterfile\"\n\
                 queue = main\n\
             }}\n",
            queue_path.display()
        ),
    )
    .expect("write compose");
    let iterfile = dir.path().join("Iterfile");
    std::fs::write(
        &iterfile,
        "workspace local { base = \".\" }\n\
         agent claude {\n  mode = print\n  command = \"claude\"\n}\n\
         runner {\n  agent = claude\n  workspace = local\n  continue_on_error = false\n  behavior = wait\n  prompt = \"hi\"\n}\n",
    )
    .expect("write iterfile");

    let out = Command::new(iter_bin())
        .current_dir(dir.path())
        .args(["compose", "validate", "--format", "json"])
        .output()
        .expect("spawn");

    // The fixture is written inline above and is fully under this test's
    // control. If `compose validate` fails, either the fixture or the
    // parser regressed — both are real failures, not "skip" conditions.
    // Silently returning here would let the JSON-validity contract rot
    // unobserved.
    assert!(
        out.status.success(),
        "compose validate fixture failed; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("compose validate --format json produced invalid JSON: {e}; stdout=\n{stdout}")
    });
    assert!(
        parsed.is_object(),
        "compose validate JSON envelope must be an object; got: {parsed}"
    );

    // The contract documents a single-line compact JSON envelope, not the
    // multi-line `serde_json::to_string_pretty` shape. Pin the byte-level
    // shape so a future switch back to pretty-printing is caught by the
    // suite rather than only by Codex review.
    let body = stdout.trim_end_matches('\n');
    assert!(
        !body.contains('\n'),
        "compose validate --format json envelope must be single-line compact JSON; got:\n{stdout}"
    );
    assert!(
        stdout.ends_with('\n'),
        "compose validate --format json must terminate with a single LF; got bytes: {:?}",
        stdout.as_bytes()
    );
}

/// `iter compose validate -f missing.iter` and `iter compose config -f missing.iter`
/// must agree with `iter enqueue -f missing.iter` and `iter validate missing.iter`
/// on the `USER_INPUT (1)` exit code. The shared `compose_error_exit_code`
/// helper makes that consistency a single-source-of-truth concern, and this
/// test pins the cross-subcommand contract so a future regression that
/// reintroduces a blanket `CONFIG (64)` mapping fails loudly.
///
/// `compose ls` (runtime listing) does **not** take `-f`; it scans the
/// registry and returns success on an empty registry, so it is excluded
/// from this contract.
#[test]
fn compose_subcommands_agree_on_missing_file_exit_code() {
    let dir = TempDir::new().expect("tempdir");
    for args in [
        &["compose", "validate", "-f", "missing.iter"][..],
        &["compose", "config", "-f", "missing.iter"][..],
        &["compose", "up", "-f", "missing.iter"][..],
    ] {
        let out = run_iter(dir.path(), args);
        assert!(!out.status.success(), "{args:?} should fail");
        assert_eq!(
            out.status.code(),
            Some(1),
            "USER_INPUT (1) expected for {args:?}; stderr=\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// `iter validate --format json` against a missing path returns an error
/// with `USER_INPUT` exit code. Verifies the JSON path doesn't accidentally
/// emit an empty success envelope when the file isn't there.
#[test]
fn validate_json_missing_path_errors() {
    let dir = TempDir::new().expect("tempdir");
    let out = run_iter(
        dir.path(),
        &["validate", "does-not-exist.iter", "--format", "json"],
    );
    assert!(!out.status.success(), "missing path must fail");
    assert_eq!(
        out.status.code(),
        Some(1),
        "USER_INPUT (1) expected; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Completion scripts for every shell are not just non-empty — they are
/// actually shell script bytes, not Rust panic output. A regression where
/// `clap_complete::generate` panics inside the generator would surface
/// here as a non-zero exit code; a regression where the output gets
/// truncated or replaced by an error message would show up as a missing
/// shell-specific keyword.
#[test]
fn completions_contain_shell_specific_markers() {
    let dir = TempDir::new().expect("tempdir");
    let bash = run_iter(dir.path(), &["completions", "bash"]);
    let stdout_bash = String::from_utf8_lossy(&bash.stdout);
    assert!(
        stdout_bash.contains("complete -F"),
        "bash completion missing `complete -F` binding"
    );

    let zsh = run_iter(dir.path(), &["completions", "zsh"]);
    let stdout_zsh = String::from_utf8_lossy(&zsh.stdout);
    assert!(
        stdout_zsh.contains("#compdef") || stdout_zsh.contains("compdef"),
        "zsh completion missing compdef directive"
    );

    let fish = run_iter(dir.path(), &["completions", "fish"]);
    let stdout_fish = String::from_utf8_lossy(&fish.stdout);
    assert!(
        stdout_fish.contains("complete -c iter"),
        "fish completion missing `complete -c iter`"
    );
}

/// P11: every user-facing subcommand surfaces an `EXAMPLES` section in
/// `--help`. The aliases (`logs`, `stop`, `kill`, `rm`, `inspect`,
/// `enqueue`) are part of the contract, so they must carry their own
/// EXAMPLES even though their canonical forms (`process logs`, …) also
/// carry one. A user reading `iter logs --help` should not have to know
/// the canonical name to learn how the command is used.
#[test]
fn every_alias_help_has_examples_section() {
    let dir = TempDir::new().expect("tempdir");
    for sub in [
        "logs", "stop", "kill", "rm", "inspect", "enqueue", "ps", "run",
    ] {
        let out = run_iter(dir.path(), &[sub, "--help"]);
        assert!(
            out.status.success(),
            "iter {sub} --help must succeed; stderr=\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("EXAMPLES:"),
            "iter {sub} --help must include EXAMPLES; got:\n{stdout}"
        );
    }
}

/// Same expectation for the canonical resource × verb forms.
#[test]
fn every_canonical_subcommand_help_has_examples_section() {
    let dir = TempDir::new().expect("tempdir");
    let cases: &[&[&str]] = &[
        &["process", "ls", "--help"],
        &["process", "inspect", "--help"],
        &["process", "logs", "--help"],
        &["process", "run", "--help"],
        &["process", "stop", "--help"],
        &["process", "kill", "--help"],
        &["process", "rm", "--help"],
        &["signal", "push", "--help"],
        &["compose", "up", "--help"],
        &["compose", "ls", "--help"],
        &["compose", "validate", "--help"],
        &["completions", "--help"],
        &["validate", "--help"],
    ];
    for args in cases {
        let out = run_iter(dir.path(), args);
        let label = args.join(" ");
        assert!(
            out.status.success(),
            "iter {label} must succeed; stderr=\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("EXAMPLES:"),
            "iter {label} must include EXAMPLES; got:\n{stdout}"
        );
    }
}

/// `iter inspect` is JSON-only (P8): the `--format` flag is deliberately
/// not declared. Asking for `--format table` surfaces clap's own
/// "unexpected argument" error and exits with clap's default code (`2`),
/// not the `IntoExitCode`-mapped `USER_INPUT (1)`. Pinned because
/// `docs/cli-output-contract.md` calls this out as an explicit deviation
/// from the otherwise-USER_INPUT exit code for malformed flags.
/// `iter compose ls --help` must not advertise `-f`/`--file`. The
/// runtime listing scans the local registry across every project; the
/// per-file flag belongs on `compose config` / `compose ps` / `compose
/// down`. A future regression that re-adds `-f` here would also break
/// `compose_subcommands_agree_on_missing_file_exit_code`'s exclusion.
#[test]
fn compose_ls_help_omits_file_flag() {
    let dir = TempDir::new().expect("tempdir");
    let out = run_iter(dir.path(), &["compose", "ls", "--help"]);
    assert!(
        out.status.success(),
        "compose ls --help failed; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("--file"),
        "compose ls --help must not advertise --file; got:\n{stdout}"
    );
    assert!(
        !stdout.contains(" -f "),
        "compose ls --help must not advertise -f; got:\n{stdout}"
    );
}

/// The renamed `compose config` (formerly `compose ls`) renders the
/// static plan listing — queues, services, triggers — exactly as the old
/// `compose ls` did. Pins the rename so a future drift between
/// `compose_config` and `compose_ls` is caught here, not in user
/// scripts.
#[test]
fn compose_config_renders_static_listing() {
    let dir = TempDir::new().expect("tempdir");
    let queue_path = dir.path().join("q");
    let compose = dir.path().join("compose.iter");
    std::fs::write(
        &compose,
        format!(
            "queue main file {{ path = \"{}\" }}\n\
             service api {{\n\
                 build = \"./Iterfile\"\n\
                 queue = main\n\
             }}\n",
            queue_path.display()
        ),
    )
    .expect("write compose");
    std::fs::write(
        dir.path().join("Iterfile"),
        "workspace local { base = \".\" }\n\
         agent claude {\n  mode = print\n  command = \"claude\"\n}\n\
         runner {\n  agent = claude\n  workspace = local\n  continue_on_error = false\n  behavior = wait\n  prompt = \"hi\"\n}\n",
    )
    .expect("write iterfile");

    let out = run_iter(dir.path(), &["compose", "config", "-q"]);
    assert!(
        out.status.success(),
        "compose config -q failed; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("queue/main"),
        "compose config -q must list `queue/main`; got:\n{stdout}"
    );
    assert!(
        stdout.contains("service/api"),
        "compose config -q must list `service/api`; got:\n{stdout}"
    );
}

/// `iter compose ls` against an empty registry succeeds and emits only
/// the table header (no project rows). Mirrors `ps_table_on_empty_…`
/// for the runtime-listing analogue.
#[test]
fn compose_ls_empty_registry_renders_header_only() {
    let dir = TempDir::new().expect("tempdir");
    let out = Command::new(iter_bin())
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .args(["compose", "ls"])
        .output()
        .expect("spawn iter");
    assert!(
        out.status.success(),
        "compose ls on empty registry must succeed; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    for col in ["NAME", "SERVICES", "RUNNERS", "STATUS"] {
        assert!(
            stdout.contains(col),
            "compose ls table missing column {col}; stdout=\n{stdout}"
        );
    }
}

/// `iter compose ps -p <unknown>` on an empty registry must succeed
/// (no runners → empty listing). Verifies that the slug-derivation
/// short-circuits correctly when the user supplies `-p` without a
/// `compose.iter` file present.
#[test]
fn compose_ps_unknown_project_succeeds_with_no_rows() {
    let dir = TempDir::new().expect("tempdir");
    let out = Command::new(iter_bin())
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .args(["compose", "ps", "-p", "demo", "-q"])
        .output()
        .expect("spawn iter");
    assert!(
        out.status.success(),
        "compose ps -p demo on empty registry must succeed; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stdout.is_empty(),
        "compose ps -p demo -q on empty registry must produce nothing on stdout; got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// `iter compose down -p <unknown>` on an empty registry succeeds — the
/// orchestrator-discovery path must tolerate "no runners labelled with
/// this project" without an error. Mirrors `docker compose down` on a
/// project that was never `up`.
#[test]
fn compose_down_unknown_project_is_noop() {
    let dir = TempDir::new().expect("tempdir");
    let out = Command::new(iter_bin())
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .args(["compose", "down", "-p", "demo"])
        .output()
        .expect("spawn iter");
    assert!(
        out.status.success(),
        "compose down -p demo on empty registry must succeed; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `iter compose ls --help` and `compose ps --help` must mention
/// `-p/--project-name` (the docker-compose-style override). Pins the
/// flag's presence so a refactor of clap arg specs cannot quietly drop
/// it from one of the runtime subcommands.
#[test]
fn compose_runtime_subcommands_advertise_project_name() {
    let dir = TempDir::new().expect("tempdir");
    for sub in [
        &["compose", "up", "--help"][..],
        &["compose", "ps", "--help"][..],
        &["compose", "down", "--help"][..],
    ] {
        let out = run_iter(dir.path(), sub);
        assert!(
            out.status.success(),
            "{sub:?} --help failed; stderr=\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("--project-name"),
            "{sub:?} must advertise --project-name; got:\n{stdout}"
        );
    }
}

// -- Helpers for compose lifecycle tests ------------------------------------

/// Minimal `Iterfile` whose runner registers and then waits forever for a
/// signal that never arrives. `agent.command = "true"` is never invoked
/// because no signals reach the runner — so we don't need a real agent
/// binary on PATH.
const TEST_ITERFILE: &str = r#"
workspace local {
  base = "."
}

agent claude {
  mode = print
  command = "true"
}

runner {
  agent = claude
  workspace = local
  continue_on_error = true
  behavior = wait
  prompt = "noop"
}
"#;

/// Build a minimal `compose.iter` whose lone service name is
/// parameterised. Different test projects under the same HOME need
/// distinct runner names to avoid registry name-lock contention (the
/// label-based discovery layer separates them by project, but the
/// underlying registry name lock is global).
fn test_compose(service: &str) -> String {
    format!(
        r#"
queue main file {{ path = "./.iter/queue" }}

service {service} {{
  build = "./Iterfile"
}}
"#
    )
}

/// Materialise a compose project under `home_dir/<name>/`. The compose
/// file's service is named `<name>_svc` so two projects can coexist
/// under the same HOME without colliding on the registry name lock.
/// Returns `(project_dir, service_name)`.
fn write_compose_project(home_dir: &Path, name: &str) -> (PathBuf, String) {
    let project_dir = home_dir.join(name);
    std::fs::create_dir_all(&project_dir).expect("create project dir");
    let service = format!("{name}_svc").replace('-', "_");
    std::fs::write(project_dir.join("compose.iter"), test_compose(&service))
        .expect("write compose.iter");
    std::fs::write(project_dir.join("Iterfile"), TEST_ITERFILE).expect("write Iterfile");
    (project_dir, service)
}

/// Read every `meta.json` under `<home>/.iter/proc/` and return the
/// parsed records. Used to look at labels without going through the
/// `iter ps` formatter.
fn read_all_proc_metadata(home: &Path) -> Vec<serde_json::Value> {
    let proc_dir = home.join(".iter/proc");
    let Ok(entries) = std::fs::read_dir(&proc_dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let meta_path = entry.path().join("meta.json");
        let Ok(bytes) = std::fs::read(&meta_path) else {
            continue;
        };
        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            out.push(json);
        }
    }
    out
}

/// Records whose `iter.compose.project` label matches `slug`.
fn compose_records_for_project(home: &Path, slug: &str) -> Vec<serde_json::Value> {
    read_all_proc_metadata(home)
        .into_iter()
        .filter(|m| {
            m.get("labels")
                .and_then(|l| l.get("iter.compose.project"))
                .and_then(|v| v.as_str())
                == Some(slug)
        })
        .collect()
}

/// Block until at least `expected` records labelled with `slug` exist
/// AND each one's `status` file has progressed past `initializing` (i.e.
/// the child runner subprocess has actually come up). Returns the
/// records on success, panics with diagnostic context on timeout.
fn wait_for_compose_runners(
    home: &Path,
    slug: &str,
    expected: usize,
    timeout: Duration,
) -> Vec<serde_json::Value> {
    let deadline = Instant::now() + timeout;
    loop {
        let records = compose_records_for_project(home, slug);
        let ready = records.iter().all(|m| {
            let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let path = home.join(format!(".iter/proc/{id}/status"));
            std::fs::read_to_string(&path)
                .map(|s| s.trim() != "initializing" && !s.trim().is_empty())
                .unwrap_or(false)
        });
        if records.len() >= expected && ready {
            return records;
        }
        assert!(
            Instant::now() < deadline,
            "timed out after {:?} waiting for {expected} compose runner(s) for project {slug:?} to be ready; got {} record(s); home contents: {:?}",
            timeout,
            records.len(),
            std::fs::read_dir(home.join(".iter/proc"))
                .map(|it| it.flatten().map(|e| e.file_name()).collect::<Vec<_>>())
                .unwrap_or_default(),
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Best-effort `compose down` (suppresses errors so panics during the
/// test still allow cleanup). Sleeps briefly to let the orchestrator
/// react.
fn compose_down_best_effort(project_dir: &Path, home: &Path) {
    drop(
        Command::new(iter_bin())
            .current_dir(project_dir)
            .env("HOME", home)
            .args(["compose", "down", "-q", "-t", "5"])
            .output(),
    );
    std::thread::sleep(Duration::from_millis(200));
}

/// RAII guard so a panicking test still cleans up its detached
/// orchestrator. Without this, an assertion failure leaves a stranded
/// `iter compose up` haunting the test machine.
struct ComposeGuard {
    project_dir: PathBuf,
    home: PathBuf,
}

impl Drop for ComposeGuard {
    fn drop(&mut self) {
        compose_down_best_effort(&self.project_dir, &self.home);
    }
}

/// Spawn the orchestrator detached and wait for at least one service
/// runner to register. Returns a guard that tears the orchestrator down
/// on drop.
fn compose_up_and_wait(
    home: &Path,
    project_dir: &Path,
    slug: &str,
) -> (ComposeGuard, Vec<serde_json::Value>) {
    compose_up_and_wait_n(home, project_dir, slug, 1)
}

/// Like [`compose_up_and_wait`] but waits for `expected` runners.
fn compose_up_and_wait_n(
    home: &Path,
    project_dir: &Path,
    slug: &str,
    expected: usize,
) -> (ComposeGuard, Vec<serde_json::Value>) {
    let out = Command::new(iter_bin())
        .current_dir(project_dir)
        .env("HOME", home)
        .args(["compose", "up", "-d"])
        .output()
        .expect("spawn iter compose up");
    assert!(
        out.status.success(),
        "compose up -d exit={:?}; stderr=\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let guard = ComposeGuard {
        project_dir: project_dir.to_path_buf(),
        home: home.to_path_buf(),
    };
    let records = wait_for_compose_runners(home, slug, expected, Duration::from_secs(20));
    (guard, records)
}

// -- Compose lifecycle contract tests ---------------------------------------

/// Migration core: the orchestrator MUST NOT appear in the local
/// registry. Mirrors `docker ps` not listing `dockerd`. After
/// `compose up -d` registers a service, no `meta.json` may carry
/// `subcommand = "compose up"` — that would mean we re-introduced the
/// orchestrator-as-runner footgun.
#[test]
fn compose_up_does_not_register_orchestrator() {
    let home = TempDir::new().expect("home tempdir");
    let (project_dir, _service) = write_compose_project(home.path(), "demo-noreg");
    let (_guard, _records) = compose_up_and_wait(home.path(), &project_dir, "demo-noreg");

    let metadata = read_all_proc_metadata(home.path());
    for entry in &metadata {
        let subcommand = entry
            .get("subcommand")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("");
        assert_ne!(
            subcommand, "compose up",
            "orchestrator must not be registered; found name={name:?} subcommand={subcommand:?} in registry"
        );
    }
}

/// Migration core: the orchestrator records its services and ONLY its
/// services. With one `service worker` declared, the registry must hold
/// exactly one compose-tagged record (no triggers, no orchestrator, no
/// stragglers).
#[test]
fn compose_up_registers_only_services() {
    let home = TempDir::new().expect("home tempdir");
    let (project_dir, service_name) = write_compose_project(home.path(), "demo-only-svc");
    let (_guard, records) = compose_up_and_wait(home.path(), &project_dir, "demo-only-svc");

    assert_eq!(
        records.len(),
        1,
        "expected exactly one compose-tagged runner (the service); got {} record(s): {:#?}",
        records.len(),
        records,
    );
    let only = &records[0];
    let service = only
        .get("labels")
        .and_then(|l| l.get("iter.compose.service"))
        .and_then(|v| v.as_str());
    assert_eq!(
        service,
        Some(service_name.as_str()),
        "the sole compose-tagged record must be the {service_name:?} service; got service label={service:?}"
    );

    // Cross-check via `iter ps` to pin the user-visible side of the
    // contract: only one row is returned.
    let ps = Command::new(iter_bin())
        .current_dir(&project_dir)
        .env("HOME", home.path())
        .args(["ps", "-q"])
        .output()
        .expect("iter ps");
    assert!(ps.status.success(), "iter ps failed");
    let lines: Vec<&str> = std::str::from_utf8(&ps.stdout)
        .expect("utf8")
        .lines()
        .filter(|l| !l.is_empty())
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "iter ps must show exactly one runner; got: {lines:?}"
    );
}

/// `iter compose ls` reconstructs the running-project list purely from
/// runner labels (no per-project state file, no orchestrator registry
/// record). After `compose up -d`, the project's slug must appear.
#[test]
fn compose_ls_lists_running_projects() {
    let home = TempDir::new().expect("home tempdir");
    let (project_dir, _service) = write_compose_project(home.path(), "demo-ls");
    let (_guard, _records) = compose_up_and_wait(home.path(), &project_dir, "demo-ls");

    let out = Command::new(iter_bin())
        .current_dir(&project_dir)
        .env("HOME", home.path())
        .args(["compose", "ls", "-q"])
        .output()
        .expect("iter compose ls");
    assert!(
        out.status.success(),
        "compose ls exit={:?}; stderr=\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.lines().any(|l| l.trim() == "demo-ls"),
        "compose ls must list `demo-ls`; got:\n{stdout}"
    );
}

/// `iter compose ps -p <slug>` MUST return only the named project's
/// runners. Sets up two projects under one HOME so a bug that returns
/// "all compose runners" would surface immediately.
#[test]
fn compose_ps_filters_by_project() {
    let home = TempDir::new().expect("home tempdir");
    let (alpha_dir, _alpha_svc) = write_compose_project(home.path(), "alpha");
    let (beta_dir, _beta_svc) = write_compose_project(home.path(), "beta");
    let (_g_alpha, _) = compose_up_and_wait(home.path(), &alpha_dir, "alpha");
    let (_g_beta, _) = compose_up_and_wait(home.path(), &beta_dir, "beta");

    for slug in ["alpha", "beta"] {
        let out = Command::new(iter_bin())
            .current_dir(home.path())
            .env("HOME", home.path())
            .args(["compose", "ps", "-p", slug, "-q"])
            .output()
            .expect("iter compose ps");
        assert!(
            out.status.success(),
            "compose ps -p {slug} exit={:?}; stderr=\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        let lines: Vec<&str> = std::str::from_utf8(&out.stdout)
            .expect("utf8")
            .lines()
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(
            lines.len(),
            1,
            "compose ps -p {slug} must return exactly its own runner; got: {lines:?}"
        );

        // Confirm the returned id matches a runner labelled with this
        // slug. ULIDs are case-insensitive at the spec level — meta.json
        // stores the canonical uppercase form, while `iter ps -q`
        // truncates to a 12-char lowercase prefix for display, so we
        // compare in lowercase.
        let ids: Vec<String> = compose_records_for_project(home.path(), slug)
            .into_iter()
            .filter_map(|m| {
                m.get("id")
                    .and_then(|v| v.as_str())
                    .map(str::to_ascii_lowercase)
            })
            .collect();
        let prefix = lines[0].to_ascii_lowercase();
        assert!(
            ids.iter().any(|id| id.starts_with(&prefix)),
            "compose ps -p {slug} returned id prefix {prefix:?} not present in project records {ids:?}"
        );
    }
}

/// `iter compose down` SIGTERMs the orchestrator (discovered via
/// runner labels — there is no registry record to look up) and every
/// service runner. After it returns, the orchestrator process must be
/// gone and the runner record must be terminal.
#[test]
fn compose_down_stops_services_and_orchestrator() {
    let home = TempDir::new().expect("home tempdir");
    let (project_dir, _service) = write_compose_project(home.path(), "demo-down");
    let (guard, records) = compose_up_and_wait(home.path(), &project_dir, "demo-down");

    let orch_pid: u32 = records[0]
        .get("labels")
        .and_then(|l| l.get("iter.compose.orchestrator_pid"))
        .and_then(|v| v.as_str())
        .expect("orchestrator_pid label present")
        .parse()
        .expect("pid parses as u32");

    // Run `compose down` in the foreground (don't rely on the guard's
    // best-effort drop — we need to assert on its exit code).
    let out = Command::new(iter_bin())
        .current_dir(&project_dir)
        .env("HOME", home.path())
        .args(["compose", "down", "-q", "-t", "5"])
        .output()
        .expect("iter compose down");
    assert!(
        out.status.success(),
        "compose down exit={:?}; stderr=\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );

    // Orchestrator pid must be gone.
    assert!(
        !pid_alive(orch_pid),
        "orchestrator pid {orch_pid} must be reaped after `compose down`"
    );

    // Runner record's status token must be terminal.
    let post = compose_records_for_project(home.path(), "demo-down");
    assert_eq!(post.len(), 1);
    let runner_id = post[0]
        .get("id")
        .and_then(|v| v.as_str())
        .expect("runner id");
    let status =
        std::fs::read_to_string(home.path().join(format!(".iter/proc/{runner_id}/status")))
            .expect("status file");
    let status_trimmed = status.trim();
    assert!(
        matches!(
            status_trimmed,
            "stopped" | "killed" | "failed" | "completed"
        ),
        "runner status after compose down must be terminal; got {status_trimmed:?}"
    );

    drop(guard);
}

/// `kill(pid, 0)` — true if `pid` is a live process the caller can
/// signal, false if it has been reaped (`ESRCH`). Used by
/// `compose_down_stops_services_and_orchestrator` to verify the
/// orchestrator process actually exited.
fn pid_alive(pid: u32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    let raw = i32::try_from(pid).expect("pid fits in i32");
    matches!(kill(Pid::from_raw(raw), None), Ok(()) | Err(Errno::EPERM))
}

/// Regression for Codex Finding 1: `compose up -d` against a malformed
/// compose file used to fork a silent child whose stderr was redirected
/// to `/dev/null`, so the parent exited 0 with no diagnostic. Now the
/// parent pre-validates and surfaces the parse error synchronously.
#[test]
fn compose_up_detached_pre_validates_compose_file() {
    let home = TempDir::new().expect("home tempdir");
    let project_dir = home.path().join("bad-detach");
    std::fs::create_dir_all(&project_dir).expect("create project dir");
    // Garbage compose body — `not a real compose body` is not valid HCL.
    std::fs::write(
        project_dir.join("compose.iter"),
        "not a real compose body\n",
    )
    .expect("write compose.iter");

    let out = Command::new(iter_bin())
        .current_dir(&project_dir)
        .env("HOME", home.path())
        .args(["compose", "up", "-d"])
        .output()
        .expect("spawn iter compose up");

    assert!(
        !out.status.success(),
        "compose up -d on garbage compose file must fail synchronously; \
         exit={:?}; stdout=\n{}\nstderr=\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    // The diagnostic must come from the compose parser, not from a generic
    // wrapper — pre-validation is the whole point. Match on a phrase that
    // only the HCL parse-error renderer (`miette`) emits and on the
    // offending file name, so a regression where the child swallows the
    // error would fail this test even if some other non-empty stderr leaks
    // out.
    assert!(
        stderr.contains("compose.iter"),
        "stderr must reference the offending compose file; stderr=\n{stderr}"
    );
    assert!(
        stderr.contains("expected an identifier")
            || stderr.contains("unknown compose.iter top-level keyword"),
        "stderr must contain a compose-parser diagnostic, not a generic wrapper; \
         stderr=\n{stderr}"
    );

    // No registry record should have been left behind by the failed
    // pre-validation — the parent never forked.
    let proc_dir = home.path().join(".iter/proc");
    let entries = std::fs::read_dir(&proc_dir)
        .map(|it| it.flatten().count())
        .unwrap_or(0);
    assert_eq!(
        entries, 0,
        "compose up -d that fails pre-validation must not leave registry entries"
    );
}

/// Regression for Codex Finding 2: `compose ls` and `compose ps` used to
/// keep returning a project / runner forever after `compose down`,
/// because terminal records were never filtered. Now both commands
/// hide terminal rows by default (matching `docker compose ls`/`ps`
/// semantics); `--all` restores the old behaviour.
/// Run `iter <args>` against `home`/`project_dir` and return stdout as
/// a String. Panics on non-zero exit so the caller can stay terse.
fn run_iter_capture_stdout(home: &Path, project_dir: &Path, args: &[&str]) -> String {
    let out = Command::new(iter_bin())
        .current_dir(project_dir)
        .env("HOME", home)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("spawn iter {args:?}: {e}"));
    assert!(
        out.status.success(),
        "iter {args:?} exit={:?}; stderr=\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8 stdout")
}

#[test]
fn compose_ls_and_ps_hide_terminal_records_after_down() {
    let home = TempDir::new().expect("home tempdir");
    let (project_dir, service) = write_compose_project(home.path(), "demo-hide");
    let (guard, _records) = compose_up_and_wait(home.path(), &project_dir, "demo-hide");

    // Stop the project. After this, every runner is terminal and the
    // orchestrator pid is gone.
    run_iter_capture_stdout(
        home.path(),
        &project_dir,
        &["compose", "down", "-q", "-t", "5"],
    );
    drop(guard);

    // `-q` text and `--format json` must agree on the `--all` filter —
    // otherwise scripts and humans would see different worlds.
    let ls_q = run_iter_capture_stdout(home.path(), &project_dir, &["compose", "ls", "-q"]);
    let ls_lines: Vec<&str> = ls_q.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        !ls_lines.contains(&"demo-hide"),
        "compose ls must hide terminal projects by default; got: {ls_lines:?}"
    );

    let ls_all_q =
        run_iter_capture_stdout(home.path(), &project_dir, &["compose", "ls", "-a", "-q"]);
    let ls_all_lines: Vec<&str> = ls_all_q.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        ls_all_lines.contains(&"demo-hide"),
        "compose ls -a must include terminal projects; got: {ls_all_lines:?}"
    );

    let ps_q = run_iter_capture_stdout(home.path(), &project_dir, &["compose", "ps", "-q"]);
    let ps_lines: Vec<&str> = ps_q.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        ps_lines.is_empty(),
        "compose ps must hide terminal runners by default; got: {ps_lines:?}"
    );

    let ps_all_q =
        run_iter_capture_stdout(home.path(), &project_dir, &["compose", "ps", "-a", "-q"]);
    let ps_all_lines: Vec<&str> = ps_all_q.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        !ps_all_lines.is_empty(),
        "compose ps -a must include terminal runners"
    );

    let ls_json = run_iter_capture_stdout(
        home.path(),
        &project_dir,
        &["compose", "ls", "--format", "json"],
    );
    assert!(
        !ls_json.contains("demo-hide"),
        "compose ls --format json must hide terminal projects by default; got: {ls_json}"
    );

    let ls_json_all = run_iter_capture_stdout(
        home.path(),
        &project_dir,
        &["compose", "ls", "-a", "--format", "json"],
    );
    assert!(
        ls_json_all.contains("demo-hide"),
        "compose ls -a --format json must include terminal projects; got: {ls_json_all}"
    );

    // No runners in the JSON listing means an empty array `[]` (or empty
    // NDJSON stream). Either way, the project's service name must not
    // appear.
    let ps_json = run_iter_capture_stdout(
        home.path(),
        &project_dir,
        &["compose", "ps", "--format", "json"],
    );
    assert!(
        !ps_json.contains(service.as_str()),
        "compose ps --format json must hide terminal runners by default; \
         service={service}; got: {ps_json}"
    );

    let ps_json_all = run_iter_capture_stdout(
        home.path(),
        &project_dir,
        &["compose", "ps", "-a", "--format", "json"],
    );
    assert!(
        ps_json_all.contains(service.as_str()),
        "compose ps -a --format json must include terminal runners; \
         service={service}; got: {ps_json_all}"
    );
}

/// Regression for Codex Finding 3: `compose up -d` used to return the
/// instant the fork succeeded, before any service runner had registered
/// labels. A `compose down` issued in that race window saw an empty
/// registry and silently returned 0, leaving the orchestrator alive
/// forever. Now `up -d` blocks until the first runner is registered, so
/// `down` always finds the orchestrator via labels.
#[test]
fn compose_up_detached_blocks_until_runner_registers() {
    let home = TempDir::new().expect("home tempdir");
    let (project_dir, _service) = write_compose_project(home.path(), "demo-race");

    // `compose up -d` returns — at that moment, at least one runner
    // record must already exist (the parent waited for it).
    let out = Command::new(iter_bin())
        .current_dir(&project_dir)
        .env("HOME", home.path())
        .args(["compose", "up", "-d"])
        .output()
        .expect("compose up -d");
    assert!(
        out.status.success(),
        "compose up -d exit={:?}; stderr=\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let _guard = ComposeGuard {
        project_dir: project_dir.clone(),
        home: home.path().to_path_buf(),
    };

    let records = compose_records_for_project(home.path(), "demo-race");
    assert!(
        !records.is_empty(),
        "by the time `compose up -d` returns, ≥1 service runner must be registered \
         (otherwise `compose down` cannot discover the orchestrator from labels)"
    );
    let orchestrator_pid: u32 = records[0]
        .get("labels")
        .and_then(|l| l.get("iter.compose.orchestrator_pid"))
        .and_then(|v| v.as_str())
        .expect("orchestrator_pid label present")
        .parse()
        .expect("u32 pid");
    assert!(
        pid_alive(orchestrator_pid),
        "orchestrator pid {orchestrator_pid} must be alive immediately after up -d returns"
    );

    // Issue `compose down` immediately. With the readiness wait in place,
    // the orchestrator must be reachable and reaped.
    let down = Command::new(iter_bin())
        .current_dir(&project_dir)
        .env("HOME", home.path())
        .args(["compose", "down", "-q", "-t", "5"])
        .output()
        .expect("iter compose down");
    assert!(
        down.status.success(),
        "compose down exit={:?}; stderr=\n{}",
        down.status.code(),
        String::from_utf8_lossy(&down.stderr)
    );
    assert!(
        !pid_alive(orchestrator_pid),
        "orchestrator pid {orchestrator_pid} must be reaped after immediate down"
    );
}

#[test]
fn inspect_format_table_is_rejected_by_clap_with_exit_two() {
    let dir = TempDir::new().expect("tempdir");
    let out = run_iter(
        dir.path(),
        &["inspect", "--format", "table", "01j0000000000000000000000z"],
    );
    assert!(
        !out.status.success(),
        "iter inspect --format table must fail; stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        out.status.code(),
        Some(2),
        "exit 2 (clap's argument-error default) expected; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unexpected argument"),
        "stderr should carry clap's unknown-flag message; got:\n{stderr}"
    );
}

// -- Targeted compose up/down contract tests ---------------------------------

/// `iter compose down <unknown-service>` must fail with a clear
/// diagnostic when the target does not match any registered service.
#[test]
fn compose_down_unknown_target_fails() {
    let home = TempDir::new().expect("home tempdir");
    let (project_dir, _service) = write_compose_project(home.path(), "demo-unk-tgt");
    let (_guard, _records) = compose_up_and_wait(home.path(), &project_dir, "demo-unk-tgt");

    let out = Command::new(iter_bin())
        .current_dir(&project_dir)
        .env("HOME", home.path())
        .args(["compose", "down", "nonexistent", "-t", "5"])
        .output()
        .expect("spawn iter");
    assert!(
        !out.status.success(),
        "compose down of unknown target must fail; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no runners registered for service target"),
        "stderr should mention unregistered target; got:\n{stderr}"
    );
}

/// `iter compose up <target>` without `--detach` must fail.
#[test]
fn compose_up_targeted_without_detach_fails() {
    let home = TempDir::new().expect("home tempdir");
    let (project_dir, service) = write_compose_project(home.path(), "demo-no-detach");

    let out = Command::new(iter_bin())
        .current_dir(&project_dir)
        .env("HOME", home.path())
        .args(["compose", "up", &service])
        .output()
        .expect("spawn iter");
    assert!(
        !out.status.success(),
        "targeted compose up without --detach must fail; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--detach") || stderr.contains("detach"),
        "stderr should mention --detach requirement; got:\n{stderr}"
    );
}

/// `iter compose up <unknown> --detach` must fail when the target
/// service does not exist in the compose file.
#[test]
fn compose_up_unknown_target_fails() {
    let home = TempDir::new().expect("home tempdir");
    let (project_dir, _service) = write_compose_project(home.path(), "demo-unk-up");

    let out = Command::new(iter_bin())
        .current_dir(&project_dir)
        .env("HOME", home.path())
        .args(["compose", "up", "nonexistent", "--detach"])
        .output()
        .expect("spawn iter");
    assert!(
        !out.status.success(),
        "compose up of unknown target must fail; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown service target"),
        "stderr should mention unknown target; got:\n{stderr}"
    );
}

/// `iter compose down service/NAME` accepts the explicit resource
/// reference syntax. On an empty registry the operation is a no-op.
#[test]
fn compose_down_accepts_service_prefix() {
    let dir = TempDir::new().expect("tempdir");
    let out = Command::new(iter_bin())
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .args(["compose", "down", "service/worker", "-p", "demo"])
        .output()
        .expect("spawn iter");
    // Targeted down on empty registry should fail with "no runners registered"
    // rather than "unsupported resource type" — confirming prefix parsing works.
    assert!(
        !out.status.success(),
        "compose down service/worker on empty registry should fail; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no runners registered for service target"),
        "should report unknown target, not unsupported resource type; got:\n{stderr}"
    );
}

/// `iter compose down trigger/NAME` or `queue/NAME` must fail with
/// an unsupported resource type diagnostic.
#[test]
fn compose_down_rejects_non_service_resource_types() {
    let dir = TempDir::new().expect("tempdir");
    for target in ["trigger/tick", "queue/main"] {
        let out = Command::new(iter_bin())
            .current_dir(dir.path())
            .env("HOME", dir.path())
            .args(["compose", "down", target, "-p", "demo"])
            .output()
            .expect("spawn iter");
        assert!(
            !out.status.success(),
            "compose down {target} must fail; stderr=\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("unsupported resource type"),
            "stderr should mention unsupported resource type for {target}; got:\n{stderr}"
        );
    }
}

/// `service/service/foo` is rejected as an unsupported resource type —
/// nested slashes after the `service/` prefix are not valid service names.
#[test]
fn compose_down_rejects_nested_service_slash() {
    let dir = TempDir::new().expect("tempdir");
    let out = Command::new(iter_bin())
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .args(["compose", "down", "service/service/foo", "-p", "demo"])
        .output()
        .expect("spawn iter");
    assert!(
        !out.status.success(),
        "compose down service/service/foo must fail; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unsupported resource type"),
        "stderr should mention unsupported resource type; got:\n{stderr}"
    );
}

/// `iter compose up --help` advertises the new `--source` flag.
#[test]
fn compose_up_help_advertises_source_flag() {
    let dir = TempDir::new().expect("tempdir");
    let out = run_iter(dir.path(), &["compose", "up", "--help"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("--source"),
        "compose up --help must mention --source; got:\n{stdout}"
    );
}

/// `iter compose down --help` advertises the `--source` flag and
/// positional targets.
#[test]
fn compose_down_help_advertises_target_and_source() {
    let dir = TempDir::new().expect("tempdir");
    let out = run_iter(dir.path(), &["compose", "down", "--help"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("--source"),
        "compose down --help must mention --source; got:\n{stdout}"
    );
    assert!(
        stdout.contains("TARGET"),
        "compose down --help must mention TARGET; got:\n{stdout}"
    );
}

/// `iter compose down` without targets still stops the whole project.
/// This is a regression guard ensuring targeted-down changes don't
/// break the project-wide path.
#[test]
fn compose_down_without_targets_stops_whole_project() {
    let home = TempDir::new().expect("home tempdir");
    let (project_dir, _service) = write_compose_project(home.path(), "demo-full-down");
    let (guard, records) = compose_up_and_wait(home.path(), &project_dir, "demo-full-down");

    let orch_pid: u32 = records[0]
        .get("labels")
        .and_then(|l| l.get("iter.compose.orchestrator_pid"))
        .and_then(|v| v.as_str())
        .expect("orchestrator_pid label")
        .parse()
        .expect("pid parses as u32");

    let out = Command::new(iter_bin())
        .current_dir(&project_dir)
        .env("HOME", home.path())
        .args(["compose", "down", "-q", "-t", "5"])
        .output()
        .expect("iter compose down");
    assert!(
        out.status.success(),
        "compose down exit={:?}; stderr=\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(
        !pid_alive(orch_pid),
        "orchestrator pid {orch_pid} must be reaped after `compose down`"
    );

    let post = compose_records_for_project(home.path(), "demo-full-down");
    assert_eq!(post.len(), 1);
    let status = read_runner_status(home.path(), &post[0]);
    let status_trimmed = status.trim();
    assert!(
        matches!(
            status_trimmed,
            "stopped" | "killed" | "failed" | "completed"
        ),
        "runner status after compose down must be terminal; got {status_trimmed:?}"
    );

    drop(guard);
}

/// Read the status file for a compose runner record.
fn read_runner_status(home: &Path, record: &serde_json::Value) -> String {
    let id = record
        .get("id")
        .and_then(|v| v.as_str())
        .expect("record has id");
    let path = home.join(format!(".iter/proc/{id}/status"));
    std::fs::read_to_string(&path).unwrap_or_else(|_| "unknown".to_owned())
}

/// Extract the `iter.compose.service` label from a metadata record.
fn record_service_name(record: &serde_json::Value) -> &str {
    record
        .get("labels")
        .and_then(|l| l.get("iter.compose.service"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

/// Build a compose.iter with two services sharing distinct Iterfiles.
/// Returns `(project_dir, service_a_name, service_b_name)`.
fn write_two_service_compose_project(home_dir: &Path, name: &str) -> (PathBuf, String, String) {
    let project_dir = home_dir.join(name);
    std::fs::create_dir_all(&project_dir).expect("create project dir");
    let svc_a = format!("{name}_a").replace('-', "_");
    let svc_b = format!("{name}_b").replace('-', "_");
    let compose = format!(
        r#"
queue main file {{ path = "./.iter/queue" }}

service {svc_a} {{
  build = "./IterfileA"
}}

service {svc_b} {{
  build = "./IterfileB"
}}
"#
    );
    std::fs::write(project_dir.join("compose.iter"), compose).expect("write compose.iter");
    std::fs::write(project_dir.join("IterfileA"), TEST_ITERFILE).expect("write IterfileA");
    std::fs::write(project_dir.join("IterfileB"), TEST_ITERFILE).expect("write IterfileB");
    (project_dir, svc_a, svc_b)
}

/// Build a compose.iter with a shell queue (non-addressable) for a
/// single service. Used to verify targeted up fails with a diagnostic.
fn write_non_addressable_compose_project(home_dir: &Path, name: &str) -> (PathBuf, String) {
    let project_dir = home_dir.join(name);
    std::fs::create_dir_all(&project_dir).expect("create project dir");
    let service = format!("{name}_svc").replace('-', "_");
    let compose = format!(
        r#"
queue main shell {{
  enqueue = "echo"
  dequeue = "echo"
}}

service {service} {{
  build = "./Iterfile"
}}
"#
    );
    std::fs::write(project_dir.join("compose.iter"), compose).expect("write compose.iter");
    std::fs::write(project_dir.join("Iterfile"), TEST_ITERFILE).expect("write Iterfile");
    (project_dir, service)
}

// -- Targeted compose lifecycle integration tests -----------------------------

/// `compose down worker-a` stops only worker-a; worker-b stays running.
#[test]
fn compose_down_targeted_stops_only_named_service() {
    let home = TempDir::new().expect("home tempdir");
    let (project_dir, svc_a, svc_b) =
        write_two_service_compose_project(home.path(), "demo-tgt-down");
    let (_guard, records) = compose_up_and_wait_n(home.path(), &project_dir, "demo-tgt-down", 2);

    assert_eq!(
        records.len(),
        2,
        "expected 2 runners; got {}",
        records.len()
    );

    let out = Command::new(iter_bin())
        .current_dir(&project_dir)
        .env("HOME", home.path())
        .args(["compose", "down", &svc_a, "-t", "5"])
        .output()
        .expect("iter compose down targeted");
    assert!(
        out.status.success(),
        "targeted compose down failed; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let post = compose_records_for_project(home.path(), "demo-tgt-down");
    for record in &post {
        let name = record_service_name(record);
        let status = read_runner_status(home.path(), record).trim().to_owned();
        if name == svc_a {
            assert!(
                matches!(status.as_str(), "stopped" | "killed" | "failed"),
                "service {svc_a} should be terminal after targeted down; got {status:?}"
            );
        } else if name == svc_b {
            assert!(
                !matches!(status.as_str(), "stopped" | "killed" | "failed"),
                "service {svc_b} should still be running after targeted down of {svc_a}; got {status:?}"
            );
        }
    }
}

/// `compose up worker-a --detach` starts only worker-a inside an
/// already-running compose project.
#[test]
fn compose_up_targeted_starts_only_named_service() {
    let home = TempDir::new().expect("home tempdir");
    let (project_dir, svc_a, svc_b) = write_two_service_compose_project(home.path(), "demo-tgt-up");
    let (_guard, _records) = compose_up_and_wait_n(home.path(), &project_dir, "demo-tgt-up", 2);

    // Stop svc_a
    let out = Command::new(iter_bin())
        .current_dir(&project_dir)
        .env("HOME", home.path())
        .args(["compose", "down", &svc_a, "-t", "5"])
        .output()
        .expect("iter compose down targeted");
    assert!(out.status.success());

    // Record how many runners exist before targeted up.
    let pre = compose_records_for_project(home.path(), "demo-tgt-up");
    let pre_count = pre.len();

    // Start svc_a again — targeted up with live orchestrator.
    let out = Command::new(iter_bin())
        .current_dir(&project_dir)
        .env("HOME", home.path())
        .args(["compose", "up", &svc_a, "--detach"])
        .output()
        .expect("iter compose up targeted");
    assert!(
        out.status.success(),
        "targeted compose up failed; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Wait for the new runner to appear.
    let post = wait_for_compose_runners(
        home.path(),
        "demo-tgt-up",
        pre_count + 1,
        Duration::from_secs(20),
    );

    // The new svc_a runner should be non-terminal.
    let svc_a_non_terminal = post
        .iter()
        .filter(|r| record_service_name(r) == svc_a)
        .any(|r| {
            let status = read_runner_status(home.path(), r).trim().to_owned();
            !matches!(status.as_str(), "stopped" | "killed" | "failed")
        });
    assert!(
        svc_a_non_terminal,
        "new {svc_a} runner should be in a non-terminal state after targeted up"
    );

    // svc_b should not have gained a new runner.
    let svc_b_records: Vec<_> = post
        .iter()
        .filter(|r| record_service_name(r) == svc_b)
        .collect();
    assert_eq!(
        svc_b_records.len(),
        1,
        "service {svc_b} should still have exactly 1 runner; got {}",
        svc_b_records.len()
    );
}

/// `--source PATH` resolves services by their Iterfile build path.
#[test]
fn compose_down_source_resolves_by_iterfile_path() {
    let home = TempDir::new().expect("home tempdir");
    let (project_dir, svc_a, svc_b) =
        write_two_service_compose_project(home.path(), "demo-src-down");
    let (_guard, records) = compose_up_and_wait_n(home.path(), &project_dir, "demo-src-down", 2);

    assert_eq!(
        records.len(),
        2,
        "expected 2 runners; got {}",
        records.len()
    );

    let out = Command::new(iter_bin())
        .current_dir(&project_dir)
        .env("HOME", home.path())
        .args(["compose", "down", "--source", "./IterfileA", "-t", "5"])
        .output()
        .expect("iter compose down --source");
    assert!(
        out.status.success(),
        "compose down --source failed; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&svc_a),
        "--source should resolve to {svc_a}; stderr=\n{stderr}"
    );

    let post = compose_records_for_project(home.path(), "demo-src-down");
    for record in &post {
        let name = record_service_name(record);
        let status = read_runner_status(home.path(), record).trim().to_owned();
        if name == svc_a {
            assert!(
                matches!(status.as_str(), "stopped" | "killed" | "failed"),
                "service {svc_a} should be terminal after --source down; got {status:?}"
            );
        } else if name == svc_b {
            assert!(
                !matches!(status.as_str(), "stopped" | "killed" | "failed"),
                "service {svc_b} should still be running after --source {svc_a}; got {status:?}"
            );
        }
    }
}

/// Targeted `compose up` with a shell queue (non-addressable) must
/// fail with an actionable diagnostic.
#[test]
fn compose_up_targeted_non_addressable_queue_fails() {
    let home = TempDir::new().expect("home tempdir");
    let (project_dir, service) = write_non_addressable_compose_project(home.path(), "demo-noaddr");

    let out = Command::new(iter_bin())
        .current_dir(&project_dir)
        .env("HOME", home.path())
        .args(["compose", "up", &service, "--detach"])
        .output()
        .expect("spawn iter");
    assert!(
        !out.status.success(),
        "targeted compose up with non-addressable queue must fail; stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("URL-addressable") || stderr.contains("addressable"),
        "stderr should mention addressable queue requirement; got:\n{stderr}"
    );
}
