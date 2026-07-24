//! [`Workspace`] / [`ActiveWorkspace`] — the environment in which an agent
//! runs.
//!
//! The two traits split the workspace concept along its lifecycle:
//!
//! - [`Workspace`] is the enduring collaborator the [`Runner`](crate::Runner)
//!   holds for the whole exploration. Each iteration calls
//!   [`setup`](Workspace::setup) on it.
//! - [`ActiveWorkspace`] is one live iteration's working environment: a
//!   working directory plus the authority to spawn processes inside it under
//!   this workspace's world-view (filesystem view, isolation, process
//!   group). It is born by `setup` and consumed by
//!   [`teardown`](ActiveWorkspace::teardown), which reconciles the agent's
//!   work back and returns the persistent path.
//!
//! Both traits are **dyn-compatible**: the runner holds `Box<dyn Workspace>`
//! and receives `Box<dyn ActiveWorkspace>` per iteration. To make the dyn
//! forms legal, the async methods return boxed futures (via
//! [`async_trait`](async_trait::async_trait)) and per-implementation errors
//! are erased into [`WorkspaceError`]. Dispatch cost is irrelevant here:
//! every `setup`/`teardown` does filesystem work and every `spawn` forks a
//! process, dominating an indirect call by orders of magnitude.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::workspace::WorkspaceError;

/// How the child's stdio is wired by [`ActiveWorkspace::spawn`].
///
/// Stdio disposition is owned by the workspace seam: an isolation wrap may
/// rebuild the command and cannot carry caller-set stdio across, so callers
/// express intent through this enum instead of `Command::stdout(..)` etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdioMode {
    /// stdin/stdout/stderr all piped — print-mode capture.
    Piped,
    /// stdin/stdout/stderr all inherited — interactive TUI.
    Inherit,
}

/// The enduring workspace collaborator: where and how the agent runs,
/// held by the [`Runner`](crate::Runner) for the whole exploration.
///
/// Each iteration brackets its work with
/// [`setup`](Workspace::setup) → agent run →
/// [`ActiveWorkspace::teardown`], mirroring the common xUnit fixture idiom.
#[async_trait]
pub trait Workspace: Send + Sync {
    /// Bind the temporary directory owned by the Runner.
    ///
    /// The directory lives for the whole Runner run and is outside every
    /// iteration workspace. Unconfined workspace implementations need no
    /// special handling. Isolation implementations must make this exact
    /// directory readable and writable by spawned agent processes so
    /// Runner-owned command artifacts can be passed without touching the
    /// workspace tree.
    fn set_runner_temporary_directory(&mut self, _path: &Path) {}

    /// Materialise one iteration's working environment.
    ///
    /// # Self-cleaning contract
    ///
    /// On `Err`, `setup` must have released every resource it acquired
    /// (temp dirs, kernel artefacts). A failed `setup` leaves nothing for
    /// the caller to tear down — there is no [`ActiveWorkspace`] to hand
    /// back, and the runner will not attempt any cleanup call.
    ///
    /// `cancel` fires when the runner wants `setup` to abort early.
    async fn setup(
        &mut self,
        cancel: CancellationToken,
    ) -> Result<Box<dyn ActiveWorkspace>, WorkspaceError>;

    /// Stable, human-meaningful label for this workspace kind.
    ///
    /// Surfaced as the `iter.workspace.name` telemetry attribute so a span
    /// names *what kind of* workspace ran (e.g. `"local"`, `"clone"`,
    /// `"sandbox"`) rather than a Rust type path. This is a **label**, not a
    /// discriminant — deliberately a `&'static str` on the `Workspace` trait.
    ///
    /// There is no default body: every implementation must state its own
    /// name so a new workspace kind cannot silently inherit a neutral label
    /// that misreports its telemetry.
    fn name(&self) -> &'static str;
}

/// One live iteration's workspace: a working directory plus the authority
/// to spawn processes inside it.
///
/// The value's existence proves a successful [`Workspace::setup`]; consuming
/// it through [`teardown`](ActiveWorkspace::teardown) is the only way the
/// iteration ends. There is no way to observe a "not yet set up" or
/// "already torn down" `ActiveWorkspace` — those states are unrepresentable.
#[async_trait]
pub trait ActiveWorkspace: Send + Sync {
    /// Working path — the filesystem tree the agent operates in.
    fn path(&self) -> &Path;

    /// Spawn `command` under this workspace's world-view.
    ///
    /// Deterministic application order, identical across implementations:
    ///
    /// 0. `current_dir(self.path())` — the caller need not (and must not
    ///    rely on being able to) set the working directory itself.
    /// 1. Isolation wrap, where this workspace has one (e.g. the sandbox
    ///    policy). Wrap errors are reported as `io::Error`.
    /// 2. stdio per [`StdioMode`].
    /// 3. `kill_on_drop(true)`.
    /// 4. Process-group setup, so the child leads its own group and the
    ///    whole tree can be reaped with one `killpg`.
    /// 5. `spawn()`.
    ///
    /// Caller-set stdio / `kill_on_drop` on `command` are **not** preserved:
    /// an isolation wrap rebuilds the command and cannot carry them across,
    /// so this method re-asserts both on every implementation for uniform
    /// semantics. What *is* preserved: program, args, env, and — until step
    /// 0 overwrites it — the working directory.
    ///
    /// # Errors
    ///
    /// Returns an `io::Error` when the child cannot be spawned, or when
    /// this workspace's isolation cannot express the configured policy on
    /// the current platform.
    fn spawn(
        &self,
        command: tokio::process::Command,
        io: StdioMode,
    ) -> std::io::Result<tokio::process::Child>;

    /// Reconcile transient state back and return the **persistent path** —
    /// the durable location of the agent's work after this call.
    ///
    /// Implementations whose working path *is* the persistent location
    /// (local) return the working path. Implementations that used a
    /// throw-away working tree (clone, sandbox) apply the agent's changes
    /// back to the base directory and return that base. Post-teardown event
    /// handlers — for example a project-supplied `shell
    /// "./scripts/persist-run.sh"` handler — operate on the returned path.
    ///
    /// `cancel` fires when the runner wants `teardown` to abort early;
    /// implementations should still make a best effort to persist the
    /// agent's work.
    async fn teardown(
        self: Box<Self>,
        cancel: CancellationToken,
    ) -> Result<PathBuf, WorkspaceError>;
}

/// Steps 2–5 of the [`ActiveWorkspace::spawn`] application order, shared by
/// every implementation so the process-group guarantee cannot be forgotten.
pub(crate) fn finish_spawn(
    mut command: tokio::process::Command,
    io: StdioMode,
) -> std::io::Result<tokio::process::Child> {
    match io {
        StdioMode::Piped => {
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
        }
        StdioMode::Inherit => {
            // Explicit, not default-dependent: an isolation wrap rebuilds
            // the command, so we never rely on inherited defaults surviving.
            command
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
        }
    }
    command.kill_on_drop(true);
    crate::process_group::configure(&mut command);
    command.spawn()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal `ActiveWorkspace` used to exercise the shared spawn tail.
    struct BareActive(PathBuf);

    #[async_trait]
    impl ActiveWorkspace for BareActive {
        fn path(&self) -> &Path {
            &self.0
        }

        fn spawn(
            &self,
            mut command: tokio::process::Command,
            io: StdioMode,
        ) -> std::io::Result<tokio::process::Child> {
            command.current_dir(&self.0);
            finish_spawn(command, io)
        }

        async fn teardown(
            self: Box<Self>,
            _cancel: CancellationToken,
        ) -> Result<PathBuf, WorkspaceError> {
            Ok(self.0)
        }
    }

    #[tokio::test]
    async fn piped_spawn_captures_output_and_sets_cwd() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let canonical = dir.path().canonicalize().expect("canonicalize");
        let active = BareActive(canonical.clone());
        let child = active
            .spawn(tokio::process::Command::new("pwd"), StdioMode::Piped)
            .expect("spawn");
        let output = child.wait_with_output().await.expect("wait");
        assert!(output.status.success());
        let cwd = String::from_utf8_lossy(&output.stdout);
        assert_eq!(cwd.trim(), canonical.to_string_lossy());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn spawned_child_leads_its_own_process_group() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let active = BareActive(dir.path().to_path_buf());
        let mut command = tokio::process::Command::new("sh");
        command.arg("-c").arg("sleep 5");
        let child = active.spawn(command, StdioMode::Piped).expect("spawn");
        let child_pid = libc::pid_t::try_from(child.id().expect("pid")).expect("pid fits");
        // SAFETY: getpgid on a live child pid; failure is reported as -1.
        let group_id = unsafe { libc::getpgid(child_pid) };
        assert_eq!(
            group_id, child_pid,
            "child must be the leader of its own process group",
        );
        drop(child); // kill_on_drop reaps the sleeping child
    }
}
