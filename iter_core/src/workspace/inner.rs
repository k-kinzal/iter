//! [`Workspace`] trait — the environment in which an agent runs.
//!
//! Uses Return-Position-Impl-Trait-In-Trait (RPITIT) so that implementors
//! can write `async fn` bodies without paying for an extra allocation per
//! call. The associated futures are required to be `Send` so they can be
//! polled by the multi-threaded `tokio` runtime.

use std::future::Future;
use std::path::Path;

use tokio_util::sync::CancellationToken;

/// The environment an [`Agent`](crate::agent::Agent) operates in.
///
/// Implementations are responsible for materialising the workspace via
/// [`setup`](Workspace::setup) and cleaning it up via
/// [`teardown`](Workspace::teardown). The runner only ever holds a freshly
/// `provide`d instance for the duration of one signal.
///
/// The `setup`/`teardown` pair deliberately mirrors the common xUnit idiom
/// so users can reason about workspace lifecycle the same way they reason
/// about a test fixture's lifecycle: everything between the two calls is
/// the "interesting" phase during which the agent may mutate the tree.
///
/// # Lifecycle and paths
///
/// A workspace exposes two distinct paths that correspond to two distinct
/// lifecycle phases:
///
/// - [`path`](Workspace::path) — the *working path*, valid during the
///   **active** phase between a successful `setup()` and the start of
///   `teardown()`. This is where the agent operates.
/// - [`final_path`](Workspace::final_path) — the *persistent path*, valid
///   during the **final** phase after `teardown()` has reconciled any
///   transient state (e.g. copy-back from a temp clone). This is where
///   post-teardown event handlers — for example a project-supplied
///   `shell "./scripts/persist-run.sh"` handler — should operate.
///
/// Every [`Workspace`] implementation must provide an explicit
/// [`final_path`](Workspace::final_path). For workspaces whose working
/// and persistent paths are identical (e.g. `LocalWorkspace`), return
/// `self.path()`. Workspaces that materialise the agent's environment
/// in a transient location (e.g. `CloneWorkspace`, `SandboxWorkspace`)
/// must return the durable destination that is still valid after
/// `teardown()` has run.
///
/// The two methods are intentionally split rather than collapsed behind a
/// single ambiguous `path()`: the [`Runner`](crate::Runner) calls `path()`
/// while the active phase holds and switches to `final_path()` exactly
/// once — after `teardown()` returns — so that each call site is
/// type-level tied to the lifecycle phase it observes. Changing one
/// without the other is visible in source review rather than deferred to a
/// runtime surprise.
///
/// # Cancellation
///
/// Both `setup` and `teardown` take a [`CancellationToken`] so the runner
/// can interrupt a hanging long-running operation (for example, a stuck
/// `docker run` during [`SandboxWorkspace`] setup). Implementations should
/// honor the token cooperatively: poll it during long syscalls, pass it down
/// to any child processes, and return an error promptly once it fires.
///
/// [`SandboxWorkspace`]: crate::workspace::SandboxWorkspace
pub trait Workspace: Send + Sync {
    /// Workspace-specific error type.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Materialise the workspace so that an agent can use it.
    ///
    /// `cancel` fires when the runner wants `setup` to abort early.
    fn setup(
        &mut self,
        cancel: CancellationToken,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Tear the workspace down once the agent is done.
    ///
    /// `cancel` fires when the runner wants `teardown` to abort early.
    /// Implementations should still make a best effort to leave the host
    /// filesystem in a consistent state even when `teardown` is interrupted.
    fn teardown(
        &mut self,
        cancel: CancellationToken,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Working path — the filesystem path the agent operates in during
    /// the **active** phase of the workspace lifecycle.
    ///
    /// Defined between the end of a successful
    /// [`setup`](Workspace::setup) call and the start of
    /// [`teardown`](Workspace::teardown). Implementations may return a
    /// best-effort fallback outside the active phase (for diagnostic
    /// convenience), but the [`Runner`](crate::Runner) only reads this
    /// method during the active phase. Post-teardown callers must use
    /// [`final_path`](Workspace::final_path) instead.
    fn path(&self) -> &Path;

    /// Persistent path — the filesystem path where the agent's work
    /// lives after the workspace has been torn down.
    ///
    /// Defined after a successful [`teardown`](Workspace::teardown) call
    /// has reconciled any transient state (e.g. copy-back from a temp
    /// clone). This is the path that post-teardown event handlers —
    /// such as a project-supplied shell handler that persists the run —
    /// should operate on.
    ///
    /// Every implementation must choose its persistent path. For
    /// workspaces whose working path IS the persistent path
    /// (e.g. `LocalWorkspace`), return `self.path()`. For workspaces
    /// that use a throw-away working location (e.g. `CloneWorkspace`,
    /// `SandboxWorkspace`), return the durable destination that is
    /// still valid after `teardown()` has run.
    fn final_path(&self) -> &Path;
}
