//! [`Workspace`] trait — the environment in which an agent runs.
//!
//! The trait is **dyn-compatible**: the per-iteration workspace supply yields
//! `Box<dyn Workspace>`. To make `dyn Workspace` legal, the methods return
//! boxed futures (via [`async_trait`](async_trait::async_trait)) and the
//! per-implementation error is erased into [`WorkspaceError`] — `dyn
//! Workspace` names no associated type. Dispatch cost is irrelevant here:
//! every `setup`/`teardown` does filesystem work (or spawns a sandbox) that
//! dominates an indirect call by orders of magnitude.

use std::ffi::OsString;
use std::path::Path;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::workspace::WorkspaceError;

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
#[async_trait]
pub trait Workspace: Send + Sync {
    /// Materialise the workspace so that an agent can use it.
    ///
    /// `cancel` fires when the runner wants `setup` to abort early.
    async fn setup(&mut self, cancel: CancellationToken) -> Result<(), WorkspaceError>;

    /// Tear the workspace down once the agent is done.
    ///
    /// `cancel` fires when the runner wants `teardown` to abort early.
    /// Implementations should still make a best effort to leave the host
    /// filesystem in a consistent state even when `teardown` is interrupted.
    async fn teardown(&mut self, cancel: CancellationToken) -> Result<(), WorkspaceError>;

    /// Stable, human-meaningful label for this workspace kind.
    ///
    /// Surfaced as the `iter.workspace.name` telemetry attribute so a span
    /// names *what kind of* workspace ran (e.g. `"local"`, `"clone"`,
    /// `"sandbox"`) rather than a Rust type path. This is a **label**, not a
    /// discriminant — deliberately a `&'static str` on the `Workspace` trait,
    /// distinct in role from any sandbox-kind enum.
    ///
    /// The default returns a neutral placeholder and exists **only** as a
    /// migration shim so existing impls compile during the workspace-axis
    /// change; it is removed once every impl states its own name. Concrete
    /// drivers override it.
    fn name(&self) -> &'static str {
        "workspace"
    }

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

    /// Argv prefix the agent's child commands must be launched under for
    /// this workspace's isolation to take effect.
    ///
    /// This is **command-construction data**, not an environment variable
    /// and not operator configuration: it is the typed handoff from
    /// workspace setup to agent invocation within a single runner
    /// iteration. The [`Runner`](crate::Runner) reads it from the active
    /// workspace after a successful [`setup`](Workspace::setup) and threads
    /// it into the agent's per-iteration invocation; process-launch helpers
    /// splice it in front of the agent's own program/args.
    ///
    /// The default implementation returns an empty slice unconditionally —
    /// the correct answer for every workspace that runs the agent verbatim
    /// (`local`, `clone`). Only [`SandboxWorkspace`] overrides it, and its
    /// override returns a non-empty prefix only during the **active** phase
    /// (between a successful `setup()` and `teardown()`); before setup or
    /// after teardown it, too, reports empty.
    ///
    /// [`SandboxWorkspace`]: crate::workspace::SandboxWorkspace
    fn sandbox_command_prefix(&self) -> &[OsString] {
        &[]
    }
}
