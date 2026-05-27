//! `spawner` — orchestrate `iter run --detach` (rev17 §C1 / §C2).
//!
//! The spawner composes [`ProcessRegistry::register_detached`] (which
//! allocates the directory, writes `meta.json` + `status=initializing` +
//! `bootstrap_token`, and acquires the name lock) with a `fork+setsid+exec`
//! using [`std::process::Command`]. The child receives only the ULID via
//! `--process-id <ULID>`; the bootstrap token travels via the filesystem
//! (rev17 §D2 — argv would leak through `ps aux`).
//!
//! ## Process model (rev17 §C2)
//!
//! - `stdin` is `Stdio::null()`.
//! - `stdout` / `stderr` are both bound to `/dev/null`. The runtime opens
//!   `<dir>/log.ndjson` from inside the child and routes everything the
//!   worker emits — agent stdout/stderr, runner tracing, lifecycle events —
//!   into that single NDJSON stream via
//!   [`crate::process::log::ProcessLogSink`]. Anything that bypasses the
//!   in-process sink (e.g. a panic before the runtime is wired) is
//!   intentionally dropped rather than leaked back to the orphaned terminal.
//!   `Stdio::piped()` would leak pipe ends because we `mem::forget` the
//!   `Child` immediately after spawn.
//! - The parent calls `mem::forget(child)`. There is **no reaper thread**
//!   (cf. `iter_core::instance::spawn`): once the parent exits, the child
//!   is reparented to PID 1 (init/launchd) which reaps it for us.
//! - The `LockGuard` is dropped — *not* released — so the lock file path
//!   remains as the on-disk name registry entry; flock auto-releases when
//!   the parent's FD closes.
//!
//! On spawn failure the half-initialized directory is removed and the
//! lock path is unlinked (`LockGuard::release`) so the name is reusable.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::Utc;

use crate::process::id::ProcessId;
use crate::process::registry::{MetadataDraft, ProcessRegistry, RegisterError};

/// Caller-provided fields for [`spawn_detached`]. The [`MetadataDraft`]
/// is reconstructed internally from this struct so the CLI driver only
/// has to think about "what did the user invoke".
#[derive(Clone, Debug)]
pub struct DetachedSpec {
    /// Human-friendly registered name. Validated by the registry.
    pub name: String,
    /// Absolute path of the loaded `Iterfile`.
    pub iterfile: PathBuf,
    /// CLI subcommand verb (`"run"`, `"compose up"`, …).
    pub subcommand: String,
    /// Child argv (excluding `argv[0]` / the program). The spawner appends
    /// `--process-id <ULID>` automatically and callers should NOT include
    /// `--detach`.
    pub args: Vec<String>,
    /// Path to the binary to exec into. Typically `std::env::current_exe()`.
    pub program: PathBuf,
    /// Extra env vars to set on the child (merged on top of the inherited
    /// environment).
    pub env: Vec<(String, String)>,
    /// Whether `--debug` was set on the user-facing subcommand.
    pub debug: bool,
    /// Parent process id, set by orchestrators (e.g. `iter compose up`)
    /// that spawn child processes whose registry records should point
    /// back at the orchestrator. `None` for top-level invocations.
    pub parent_id: Option<ProcessId>,
    /// Free-form labels persisted into `meta.json`. Keys in the
    /// `iter.<feature>.<key>` namespace are reserved for internal use
    /// (e.g. compose stores `iter.compose.project`,
    /// `iter.compose.service`, `iter.compose.orchestrator_pid`,
    /// `iter.compose.orchestrator_start_time`).
    pub labels: BTreeMap<String, String>,
}

/// Outer error type returned by [`spawn_detached`].
#[derive(Debug)]
#[non_exhaustive]
pub enum SpawnError {
    /// Failure inside the registry handshake (name lock contention,
    /// session directory I/O, bootstrap token publication).
    Register(RegisterError),
    /// `Command::spawn` itself failed (fork, exec, or `pre_exec`). The
    /// half-initialized directory has already been removed and the name
    /// lock unpublished.
    Spawn(std::io::Error),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnError::Register(e) => write!(f, "registration: {e}"),
            SpawnError::Spawn(e) => write!(f, "child spawn: {e}"),
        }
    }
}

impl std::error::Error for SpawnError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SpawnError::Register(e) => Some(e),
            SpawnError::Spawn(e) => Some(e),
        }
    }
}

/// Spawn a fully detached `iter` process and return its [`ProcessId`].
/// # Errors
///
/// Returns an error if the operation fails.
///
/// The CLI driver typically prints the returned ULID to stdout and exits.
#[cfg(unix)]
pub async fn spawn_detached(
    registry: &ProcessRegistry,
    spec: DetachedSpec,
) -> Result<ProcessId, SpawnError> {
    use std::fs::OpenOptions;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let draft = MetadataDraft {
        iterfile: spec.iterfile.clone(),
        subcommand: spec.subcommand.clone(),
        started_at: Utc::now(),
        args: spec.args.clone(),
        env: spec.env.clone(),
        debug: spec.debug,
        parent_id: spec.parent_id,
        labels: spec.labels.clone(),
    };

    let (session, lock, _token) = registry
        .register_detached(&spec.name, draft)
        .await
        .map_err(SpawnError::Register)?;
    let id = session.id();

    let dev_null_out = match OpenOptions::new().write(true).open("/dev/null") {
        Ok(f) => f,
        Err(io) => {
            cleanup_half_init(&session, lock);
            return Err(SpawnError::Spawn(io));
        }
    };
    let dev_null_err = match dev_null_out.try_clone() {
        Ok(f) => f,
        Err(io) => {
            cleanup_half_init(&session, lock);
            return Err(SpawnError::Spawn(io));
        }
    };

    // argv: [...spec.args, "--process-id", "<ULID>"]
    let mut argv: Vec<String> = spec.args.clone();
    argv.push("--process-id".into());
    argv.push(id.to_string());

    let mut cmd = Command::new(&spec.program);
    cmd.args(&argv);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::from(dev_null_out));
    cmd.stderr(Stdio::from(dev_null_err));
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    // SAFETY: setsid(2) is async-signal-safe and only manipulates the
    // forked-child process state. We do not allocate or call into Rust
    // code beyond the nix wrapper, which itself only invokes the syscall.
    unsafe {
        cmd.pre_exec(|| {
            nix::unistd::setsid()
                .map(|_| ())
                .map_err(std::io::Error::from)
        });
    }

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(io) => {
            cleanup_half_init(&session, lock);
            return Err(SpawnError::Spawn(io));
        }
    };
    // Detach: rev17 §C2 — no reaper. init/launchd reaps the child once
    // the parent exits. `mem::forget` discards the `Child` so its `Drop`
    // does NOT call `wait`.
    std::mem::forget(child);

    // Drop session (closes dirfd, status fd, etc.) and lock guard
    // (closes lock fd → flock auto-released by kernel; lock file path
    // remains as the name registry entry).
    drop(session);
    drop(lock);
    Ok(id)
}

#[cfg(not(unix))]
pub async fn spawn_detached(
    _registry: &ProcessRegistry,
    _spec: DetachedSpec,
) -> Result<ProcessId, SpawnError> {
    Err(SpawnError::Spawn(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "spawn_detached is unix-only",
    )))
}

/// Handle returned by [`spawn_unmanaged_detached`]. While the handle is
/// alive the caller can [`try_wait`](Self::try_wait) to detect early
/// child exit; calling [`detach`](Self::detach) `mem::forget`s the
/// underlying [`std::process::Child`] so the kernel reparents the child
/// to PID 1 (init/launchd) which reaps it for us.
///
/// Dropping the handle without calling `detach` is a programmer error —
/// the `Child`'s `Drop` would call `wait` and block — so the type
/// emits a warning via `must_use`.
#[cfg(unix)]
#[must_use = "must call .detach() once readiness is confirmed; otherwise the Child's Drop blocks on wait"]
pub struct UnmanagedChild {
    inner: Option<std::process::Child>,
    pid: u32,
}

#[cfg(unix)]
impl UnmanagedChild {
    /// pid of the spawned process.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Non-blocking poll for child exit. Returns `Ok(Some(status))` if
    /// the child has exited (so the caller can fail fast instead of
    /// waiting for a timeout), `Ok(None)` while it is still running.
    ///
    /// # Errors
    ///
    /// Forwards the underlying `wait4` error.
    pub fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        match self.inner.as_mut() {
            Some(c) => c.try_wait(),
            None => Ok(None),
        }
    }

    /// Forget the underlying [`std::process::Child`] so its `Drop` does
    /// not call `wait`. After this, the child is reparented to PID 1
    /// once the current process exits.
    pub fn detach(mut self) {
        if let Some(c) = self.inner.take() {
            std::mem::forget(c);
        }
    }
}

/// Fork+setsid+exec a child process with stdio redirected to `/dev/null`
/// and **no** registry interaction. The parent receives an
/// [`UnmanagedChild`] handle so it can detect early child exit via
/// [`try_wait`](UnmanagedChild::try_wait); call
/// [`detach`](UnmanagedChild::detach) once readiness is confirmed.
///
/// Used by `iter compose up --detach` to host the orchestrator: compose
/// is stateless (like `docker compose`), so the orchestrator process must
/// not own a `~/.iter/proc/<id>/` record. Discovery instead relies on
/// labels stamped onto each child runner the orchestrator spawns.
///
/// Unlike [`spawn_detached`], this does not allocate a [`ProcessId`],
/// does not write a `meta.json`, does not acquire a name lock, and does
/// not pipe stdio to per-process log files.
///
/// # Errors
///
/// Returns the underlying I/O error if `/dev/null` cannot be opened or
/// `Command::spawn` (fork/exec/`pre_exec`) fails.
#[cfg(unix)]
pub fn spawn_unmanaged_detached(
    program: &std::path::Path,
    args: &[String],
    env: &[(String, String)],
) -> std::io::Result<UnmanagedChild> {
    use std::fs::OpenOptions;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let dev_null_in = OpenOptions::new().read(true).open("/dev/null")?;
    let dev_null_out = OpenOptions::new().write(true).open("/dev/null")?;
    let dev_null_err = dev_null_out.try_clone()?;

    let mut cmd = Command::new(program);
    cmd.args(args);
    cmd.stdin(Stdio::from(dev_null_in));
    cmd.stdout(Stdio::from(dev_null_out));
    cmd.stderr(Stdio::from(dev_null_err));
    for (k, v) in env {
        cmd.env(k, v);
    }
    // SAFETY: setsid(2) is async-signal-safe and only manipulates the
    // forked child's session; no allocation or Rust runtime calls happen
    // between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            nix::unistd::setsid()
                .map(|_| ())
                .map_err(std::io::Error::from)
        });
    }

    let child = cmd.spawn()?;
    let pid = child.id();
    Ok(UnmanagedChild {
        inner: Some(child),
        pid,
    })
}

#[cfg(not(unix))]
pub struct UnmanagedChild;

#[cfg(not(unix))]
impl UnmanagedChild {
    pub fn pid(&self) -> u32 {
        0
    }
    pub fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "UnmanagedChild is unix-only",
        ))
    }
    pub fn detach(self) {}
}

#[cfg(not(unix))]
pub fn spawn_unmanaged_detached(
    _program: &std::path::Path,
    _args: &[String],
    _env: &[(String, String)],
) -> std::io::Result<UnmanagedChild> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "spawn_unmanaged_detached is unix-only",
    ))
}

#[cfg(unix)]
fn cleanup_half_init(
    session: &std::sync::Arc<crate::process::session::ProcessSession>,
    lock: crate::process::name_lock::LockGuard,
) {
    let dir = session.paths().dir().to_path_buf();
    drop(std::fs::remove_dir_all(&dir));
    drop(lock.release());
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::process::registry::ProcessRegistry;
    use crate::process::status::ProcessStatus;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn sample_spec(name: &str, program: PathBuf) -> DetachedSpec {
        DetachedSpec {
            name: name.into(),
            iterfile: PathBuf::from("/tmp/Iterfile"),
            subcommand: "run".into(),
            args: vec!["run".into()],
            program,
            env: vec![],
            debug: false,
            parent_id: None,
            labels: BTreeMap::new(),
        }
    }

    /// Smoke test: spawn `/usr/bin/true` so we exercise the registry
    /// handshake + fork/exec mechanics without depending on the real
    /// `iter` binary. `true` ignores the appended `--process-id <ULID>`
    /// and exits 0 immediately; we verify the directory was created
    /// with `status=initializing` (the child never got far enough to
    /// adopt and flip to `running`).
    #[tokio::test]
    async fn spawn_detached_creates_record_and_runs_child() {
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).expect("open registry");
        let spec = sample_spec("alpha", PathBuf::from("/usr/bin/true"));

        let id = spawn_detached(&registry, spec)
            .await
            .expect("spawn detached");

        // Proc dir was published.
        let dir = tmp.path().join(id.to_string());
        assert!(dir.exists(), "proc dir must exist");
        // Initializing was written before fork (child never reaches
        // adoption with /usr/bin/true).
        let rec = registry.get(id).expect("get record");
        assert_eq!(
            rec.read_status_token().expect("status"),
            ProcessStatus::Initializing
        );
        // Lock body is published.
        let lock_path = tmp.path().join(".locks").join("alpha");
        assert!(lock_path.exists(), "lock body must remain");
        // bootstrap_token survives until adoption deletes it; `true`
        // never adopts, so it is still on disk.
        assert!(dir.join("bootstrap_token").exists());
    }

    /// When the `program` path does not exist, `Command::spawn` fails;
    /// the half-initialized directory and lock body must both be gone.
    #[tokio::test]
    async fn spawn_detached_cleans_up_on_program_not_found() {
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).expect("open registry");
        let spec = sample_spec(
            "missing",
            PathBuf::from("/nonexistent/path/that/cannot/exist"),
        );

        let err = spawn_detached(&registry, spec)
            .await
            .expect_err("must fail");
        match err {
            SpawnError::Spawn(_) => {}
            other => panic!("unexpected: {other:?}"),
        }

        // Half-init cleanup removed everything.
        let lock_path = tmp.path().join(".locks").join("missing");
        assert!(!lock_path.exists(), "lock must be released");
        // No proc dir under tmp besides .locks.
        let mut leftover = vec![];
        for entry in std::fs::read_dir(tmp.path()).unwrap() {
            let e = entry.unwrap();
            if e.file_name() != ".locks" {
                leftover.push(e.file_name().to_string_lossy().into_owned());
            }
        }
        assert!(leftover.is_empty(), "no leftover dirs: {leftover:?}");
    }

    /// Argv must end with `--process-id <ULID>`. We verify by spawning
    /// a tiny script that writes its argv into a marker file.
    #[tokio::test]
    async fn spawn_detached_appends_process_id_argv() {
        let tmp = TempDir::new().unwrap();
        let registry = ProcessRegistry::open(tmp.path()).expect("open registry");
        let marker = tmp.path().join("marker.txt");

        // Tiny shell script: dump argv into `marker.txt`, one per line.
        let script_dir = TempDir::new().unwrap();
        let script_path = script_dir.path().join("dump_argv.sh");
        let body = format!(
            "#!/bin/sh\nfor a in \"$@\"; do echo \"$a\"; done > {}\n",
            marker.display()
        );
        std::fs::write(&script_path, body).unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut spec = sample_spec("argv", script_path);
        spec.args = vec!["one".into(), "two".into()];

        let id = spawn_detached(&registry, spec)
            .await
            .expect("spawn detached");

        // Wait briefly for the child to write the marker.
        for _ in 0..200 {
            if marker.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let argv = std::fs::read_to_string(&marker).expect("marker was not written");
        let lines: Vec<&str> = argv.lines().collect();
        assert_eq!(
            lines,
            vec!["one", "two", "--process-id", &id.to_string()],
            "argv must be passed through and end with --process-id <ULID>",
        );
    }
}
