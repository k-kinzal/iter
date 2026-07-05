//! [`SandboxWorkspace`] — [`Workspace`] implementation that clones into
//! a tmpdir and confines spawned processes with a kernel-level sandbox.
//!
//! See the [module docs](super) for the conceptual model — the
//! "clone + confine" pipeline and the upper/lower bound contract.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::Workspace;
use crate::time::{Clock, SystemClock};
use crate::workspace::WorkspaceError;
use crate::workspace::workspace::{ActiveWorkspace, StdioMode, finish_spawn};
use async_trait::async_trait;
use tokio::fs;
use tokio_util::sync::CancellationToken;

use crate::workspace::apply_back::ApplyBackMode;
use crate::workspace::clone::CloneSettings;
use crate::workspace::mirror::{CloneFilter, Mirror};

use super::error::SandboxWorkspaceError;
use super::policy::SandboxPolicy;
use super::profile::SandboxProfile;

/// Translate the project's upper-bound [`SandboxPolicy`] and the agent's
/// lower-bound [`SandboxProfile`] into the structured [`sandbox::Policy`]
/// that [`ActiveWorkspace::spawn`] applies to every child command.
fn build_sandbox_policy(
    workspace_path: &Path,
    policy: &SandboxPolicy,
    profile: &SandboxProfile,
) -> sandbox::Policy {
    let mut sandbox_policy = sandbox::Policy::new().current_dir(workspace_path);

    match &policy.network {
        super::policy::NetworkAccess::Off => {
            sandbox_policy = sandbox_policy.deny_network();
        }
        super::policy::NetworkAccess::All => {
            sandbox_policy = sandbox_policy.allow_network();
        }
        super::policy::NetworkAccess::Hosts(_) => {
            if profile.network_hosts.is_empty() {
                sandbox_policy = sandbox_policy.deny_network();
            } else {
                // The built-in platform sandboxes cannot enforce host-level
                // egress filters. This preserves the existing degradation: a
                // host policy opens outbound networking only when the
                // selected agent declared network requirements.
                sandbox_policy = sandbox_policy.allow_network();
            }
        }
    }

    sandbox_policy = sandbox_policy.allow_write(workspace_path);

    for path in policy
        .allow_read_outside
        .iter()
        .chain(profile.file_reads.iter())
    {
        sandbox_policy = sandbox_policy.allow_read(path.as_path());
    }
    for path in policy
        .allow_write_outside
        .iter()
        .chain(profile.file_writes.iter())
    {
        sandbox_policy = sandbox_policy.allow_write(path.as_path());
    }
    for path in &policy.extra_deny_paths {
        sandbox_policy = sandbox_policy.deny_path(path.as_path());
    }
    #[cfg(target_os = "macos")]
    for path in &policy.allow_exec {
        sandbox_policy = sandbox_policy.allow_executable(path.as_path());
    }
    // Environment filtering runs on every sandbox platform. The child starts
    // with a cleared environment and is rebuilt from only the profile's narrow
    // `env_pass` allow-list plus the operator's `declared_env`; without this the
    // agent would inherit the iter process's full host environment (unrelated
    // API tokens, cloud credentials, etc.). The clearing is enforced by the
    // wrapper `Command` (`env_clear()`), not the SBPL/bwrap profile, so it works
    // identically on macOS and Linux.
    {
        sandbox_policy = sandbox_policy.clear_environment();
        for (key, value) in expand_env_pass(&profile.env_pass) {
            sandbox_policy = sandbox_policy.set_env(key, value);
        }
        for (key, value) in &profile.declared_env {
            sandbox_policy = sandbox_policy.set_env(key, value);
        }
    }

    #[cfg(target_os = "macos")]
    {
        if profile.allow_signal {
            sandbox_policy = sandbox_policy.allow_signal();
        }
        for pattern in &profile.file_write_regexes {
            sandbox_policy = sandbox_policy.allow_write_matching(pattern);
        }
    }

    sandbox_policy
}

fn expand_env_pass(patterns: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (name, value) in std::env::vars() {
        if patterns
            .iter()
            .any(|pattern| super::profile::match_env_pattern(pattern, &name))
            && seen.insert(name.clone())
        {
            out.push((name, value));
        }
    }
    out
}

fn command_available(command: &str) -> bool {
    let path = Path::new(command);
    if path.is_absolute() || path.components().count() > 1 {
        return path.is_file();
    }

    std::env::var_os("PATH").is_some_and(|path_env| {
        std::env::split_paths(&path_env).any(|entry| entry.join(command).is_file())
    })
}

/// Workspace that clones the base directory into a tmpdir and confines
/// every spawned child command with a kernel-level sandbox.
///
/// # Example
///
/// ```no_run
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// use iter_core::{SandboxProfile, Workspace};
/// use iter_core::workspace::{
///     ApplyBackMode, CloneSettings, NetworkAccess, SandboxPolicy, SandboxWorkspace,
/// };
/// use tokio_util::sync::CancellationToken;
///
/// // In production the profile is assembled from the agent's drivers via
/// // `SandboxProfile::for_drivers`; here we build one by hand with the
/// // public builder API.
/// let mut profile = SandboxProfile::new();
/// profile
///     .allow_network_host("api.anthropic.com:443")
///     .pass_env("CLAUDE_*");
///
/// let mut ws = SandboxWorkspace::new(
///     "/Users/me/my-project",
///     CloneSettings {
///         excludes: vec!["scratch".into()],
///         includes: Vec::new(),
///         preserve_mtime: false,
///         apply_back: ApplyBackMode::Sync,
///         apply_back_excludes: Vec::new(),
///         apply_back_includes: Vec::new(),
///     },
///     SandboxPolicy {
///         network: NetworkAccess::Hosts(vec!["api.anthropic.com".into()]),
///         allow_read_outside: Vec::new(),
///         allow_write_outside: Vec::new(),
///         extra_deny_paths: Vec::new(),
///         allow_exec: Vec::new(),
///     },
///     profile,
/// );
/// let active = ws.setup(CancellationToken::new()).await?;
/// // ... run the agent; every child is spawned via active.spawn(..) ...
/// let persistent = active.teardown(CancellationToken::new()).await?;
/// # let _ = persistent;
/// # Ok(()) }
/// ```
#[derive(Debug)]
pub struct SandboxWorkspace {
    base: PathBuf,
    settings: CloneSettings,
    policy: SandboxPolicy,
    profile: SandboxProfile,
    clock: Arc<dyn Clock>,
}

impl SandboxWorkspace {
    /// Create a new [`SandboxWorkspace`] rooted at `base`.
    ///
    /// Every knob is supplied by the caller. `settings` controls the
    /// clone-layer behaviour (mirrors [`CloneSettings`]). `policy` is the
    /// project's upper-bound rule set from the declaration. `profile` is the
    /// agent's lower-bound OS-access profile, assembled by
    /// [`SandboxProfile::for_drivers`](super::profile::SandboxProfile::for_drivers).
    #[must_use]
    pub fn new(
        base: impl Into<PathBuf>,
        settings: CloneSettings,
        policy: SandboxPolicy,
        profile: SandboxProfile,
    ) -> Self {
        let base = base.into();
        settings.warn_if_merge_gate_defeated("sandbox", &base);
        Self {
            base,
            settings,
            policy,
            profile,
            clock: Arc::new(SystemClock),
        }
    }

    /// Create a new [`SandboxWorkspace`] with an injected clock.
    #[must_use]
    pub fn with_clock(
        base: impl Into<PathBuf>,
        settings: CloneSettings,
        policy: SandboxPolicy,
        profile: SandboxProfile,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let base = base.into();
        settings.warn_if_merge_gate_defeated("sandbox", &base);
        Self {
            base,
            settings,
            policy,
            profile,
            clock,
        }
    }

    /// The current apply-back mode.
    #[must_use]
    pub fn apply_back_mode(&self) -> ApplyBackMode {
        self.settings.apply_back
    }

    /// Returns `true` if a sandbox host command is available for the host
    /// platform (`sandbox-exec` on macOS, `bwrap` on Linux).
    ///
    /// Intended for tests and friendly CLI diagnostics — use it to skip
    /// when the host can't enforce a sandbox.
    #[must_use]
    pub fn detect_backend_available() -> bool {
        #[cfg(target_os = "macos")]
        {
            command_available("sandbox-exec")
        }
        #[cfg(target_os = "linux")]
        {
            command_available("bwrap")
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            false
        }
    }

    /// Materialise the sandbox, returning the concrete
    /// [`SandboxWorkspaceError`]. The [`Workspace`] trait impl erases this into
    /// [`WorkspaceError`].
    ///
    /// Self-cleaning on failure: the only resource acquired is the mirror,
    /// whose backing `TempDir` is dropped (and removed) on every error path.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxWorkspaceError`] when cancelled, on an unsupported
    /// platform, when the base path is missing or not a directory, when a
    /// filter fails to compile, or when materialising the mirror fails.
    pub async fn setup(
        &mut self,
        cancel: CancellationToken,
    ) -> Result<ActiveSandboxWorkspace, SandboxWorkspaceError> {
        if cancel.is_cancelled() {
            return Err(SandboxWorkspaceError::Cancelled);
        }
        // Fail fast where no sandbox host command exists — the workspace
        // must never silently degrade to an unconfined spawn.
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            return Err(SandboxWorkspaceError::UnsupportedPlatform);
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            // ----- Phase 1: clone base into tmpdir -----------------------
            let meta = match fs::metadata(&self.base).await {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(SandboxWorkspaceError::NotFound(self.base.clone()));
                }
                Err(e) => return Err(SandboxWorkspaceError::Io(e)),
            };
            if !meta.is_dir() {
                return Err(SandboxWorkspaceError::NotADirectory(self.base.clone()));
            }

            let clone_filter =
                CloneFilter::compile(&self.settings.excludes, &self.settings.includes)?;
            let apply_back_filter = self.settings.apply_back_filter()?;
            let mirror = Mirror::materialize_with_clock(
                self.base.clone(),
                &clone_filter,
                apply_back_filter,
                self.settings.preserve_mtime,
                Arc::clone(&self.clock),
            )
            .await?;

            // ----- Phase 2: build the structured confinement policy ------
            let policy = build_sandbox_policy(mirror.path(), &self.policy, &self.profile);

            tracing::debug!(
                base = %self.base.display(),
                temp = %mirror.path().display(),
                mode = ?self.settings.apply_back,
                "sandbox workspace set up",
            );
            Ok(ActiveSandboxWorkspace {
                base: self.base.clone(),
                mirror,
                apply_back: self.settings.apply_back,
                policy,
            })
        }
    }
}

#[async_trait]
impl Workspace for SandboxWorkspace {
    async fn setup(
        &mut self,
        cancel: CancellationToken,
    ) -> Result<Box<dyn ActiveWorkspace>, WorkspaceError> {
        SandboxWorkspace::setup(self, cancel)
            .await
            .map(|active| Box::new(active) as Box<dyn ActiveWorkspace>)
            .map_err(WorkspaceError::new)
    }

    fn name(&self) -> &'static str {
        "sandbox"
    }
}

/// The active form of a [`SandboxWorkspace`]: the materialised temp mirror
/// plus the structured confinement policy applied to every spawn.
///
/// The word "sandbox" ends here — the agent side only ever sees an
/// [`ActiveWorkspace`] whose [`spawn`](ActiveWorkspace::spawn) happens to
/// confine.
#[derive(Debug)]
pub struct ActiveSandboxWorkspace {
    base: PathBuf,
    mirror: Mirror,
    apply_back: ApplyBackMode,
    policy: sandbox::Policy,
}

impl ActiveSandboxWorkspace {
    /// Reconcile and tear down, returning the concrete
    /// [`SandboxWorkspaceError`] (the trait impl erases it).
    ///
    /// # Errors
    ///
    /// Returns [`SandboxWorkspaceError`] when reconciling the mirror back
    /// into the base directory (apply-back) fails. The temp tree is removed
    /// on every path.
    pub async fn teardown(
        self,
        cancel: CancellationToken,
    ) -> Result<PathBuf, SandboxWorkspaceError> {
        // Apply-back is not interrupted mid-flight: the agent's in-flight
        // work outranks shutdown book-keeping.
        drop(cancel);
        let apply_back_result = match self.apply_back {
            ApplyBackMode::Discard => Ok(()),
            ApplyBackMode::Sync => self.mirror.sync_back().await,
            ApplyBackMode::Merge => self.mirror.merge_back().await,
        };
        // Close the mirror on every path — an implicit `Drop` on the error
        // path would run the temp tree's removal synchronously on the
        // reactor thread, while `close_best_effort` routes it through
        // `spawn_blocking`.
        self.mirror.close_best_effort().await;
        apply_back_result?;
        tracing::debug!(base = %self.base.display(), "sandbox workspace torn down");
        Ok(self.base)
    }
}

#[async_trait]
impl ActiveWorkspace for ActiveSandboxWorkspace {
    fn path(&self) -> &Path {
        self.mirror.path()
    }

    fn spawn(
        &self,
        mut command: tokio::process::Command,
        io: StdioMode,
    ) -> std::io::Result<tokio::process::Child> {
        use sandbox::tokio::CommandExt as _;
        // ⓪ cwd — the wrap preserves program/args/env/cwd, nothing else.
        command.current_dir(self.mirror.path());
        // ① confinement wrap; policy incompatibilities (e.g. Linux
        //    allow_exec) surface as io::Error at the spawn seam.
        let command = command
            .sandboxed(&self.policy)
            .map_err(std::io::Error::other)?
            .into_process();
        // ②③④⑤ stdio, kill_on_drop, process group, spawn — shared tail.
        finish_spawn(command, io)
    }

    async fn teardown(
        self: Box<Self>,
        cancel: CancellationToken,
    ) -> Result<PathBuf, WorkspaceError> {
        (*self).teardown(cancel).await.map_err(WorkspaceError::new)
    }
}

#[cfg(test)]
mod tests {
    use super::super::policy::NetworkAccess;
    use super::*;
    use tempfile::TempDir;

    fn clone_settings() -> CloneSettings {
        CloneSettings {
            excludes: Vec::new(),
            includes: Vec::new(),
            preserve_mtime: true,
            apply_back: ApplyBackMode::Sync,
            apply_back_excludes: Vec::new(),
            apply_back_includes: Vec::new(),
        }
    }

    fn default_deny_policy() -> SandboxPolicy {
        SandboxPolicy {
            network: NetworkAccess::Off,
            allow_read_outside: Vec::new(),
            allow_write_outside: Vec::new(),
            extra_deny_paths: Vec::new(),
            allow_exec: Vec::new(),
        }
    }

    #[tokio::test]
    async fn setup_missing_base_errors() {
        let mut ws = SandboxWorkspace::new(
            "/definitely/not/a/path/sandbox",
            clone_settings(),
            default_deny_policy(),
            SandboxProfile::default(),
        );
        let err = ws
            .setup(CancellationToken::new())
            .await
            .expect_err("should err");
        assert!(matches!(err, SandboxWorkspaceError::NotFound(_)));
    }

    #[tokio::test]
    async fn setup_when_cancelled_errors() {
        let base = TempDir::new().expect("tempdir");
        let mut ws = SandboxWorkspace::new(
            base.path(),
            clone_settings(),
            default_deny_policy(),
            SandboxProfile::default(),
        );
        let token = CancellationToken::new();
        token.cancel();
        let err = ws.setup(token).await.expect_err("should err");
        assert!(matches!(err, SandboxWorkspaceError::Cancelled));
    }

    #[tokio::test]
    async fn temp_dir_cleaned_up_after_teardown() {
        if !SandboxWorkspace::detect_backend_available() {
            return;
        }
        let base = TempDir::new().expect("tempdir");
        fs::write(base.path().join("a.txt"), b"hi")
            .await
            .expect("write");
        let mut ws = SandboxWorkspace::new(
            base.path(),
            clone_settings(),
            default_deny_policy(),
            SandboxProfile::default(),
        );
        let active = ws.setup(CancellationToken::new()).await.expect("setup");
        let temp = active.path().to_path_buf();
        assert!(temp.exists());
        let persistent = active
            .teardown(CancellationToken::new())
            .await
            .expect("teardown");
        assert_eq!(persistent, base.path());
        assert!(!temp.exists(), "temp dir must be removed after teardown");
    }

    #[tokio::test]
    async fn repeated_setup_teardown_cycles_on_one_workspace() {
        // The runner holds one Workspace for the whole exploration; two full
        // cycles on the same instance are the regression net for that model.
        if !SandboxWorkspace::detect_backend_available() {
            return;
        }
        let base = TempDir::new().expect("tempdir");
        fs::write(base.path().join("a.txt"), b"v0")
            .await
            .expect("write");
        let mut ws = SandboxWorkspace::new(
            base.path(),
            clone_settings(),
            default_deny_policy(),
            SandboxProfile::default(),
        );
        for round in 1..=2 {
            let active = ws.setup(CancellationToken::new()).await.expect("setup");
            fs::write(active.path().join("a.txt"), format!("v{round}"))
                .await
                .expect("write");
            let persistent = active
                .teardown(CancellationToken::new())
                .await
                .expect("teardown");
            assert_eq!(persistent, base.path());
            let back = fs::read_to_string(base.path().join("a.txt"))
                .await
                .expect("read");
            assert_eq!(back, format!("v{round}"));
        }
    }

    // ----- policy mapping unit tests (no process spawned) -----------------

    #[test]
    fn network_off_maps_to_deny() {
        let policy = default_deny_policy();
        let profile = SandboxProfile::new();
        let sp = build_sandbox_policy(Path::new("/tmp/ws"), &policy, &profile);
        assert_eq!(sp.network(), sandbox::NetworkPolicy::Deny);
    }

    #[test]
    fn network_all_maps_to_allow() {
        let mut policy = default_deny_policy();
        policy.network = NetworkAccess::All;
        let sp = build_sandbox_policy(Path::new("/tmp/ws"), &policy, &SandboxProfile::new());
        assert_eq!(sp.network(), sandbox::NetworkPolicy::AllowOutbound);
    }

    #[test]
    fn network_hosts_with_empty_profile_stays_deny() {
        let mut policy = default_deny_policy();
        policy.network = NetworkAccess::Hosts(vec!["api.example.com".into()]);
        let sp = build_sandbox_policy(Path::new("/tmp/ws"), &policy, &SandboxProfile::new());
        assert_eq!(sp.network(), sandbox::NetworkPolicy::Deny);
    }

    #[test]
    fn network_hosts_with_profile_hosts_degrades_to_allow() {
        let mut policy = default_deny_policy();
        policy.network = NetworkAccess::Hosts(vec!["api.example.com".into()]);
        let mut profile = SandboxProfile::new();
        profile.allow_network_host("api.example.com:443");
        let sp = build_sandbox_policy(Path::new("/tmp/ws"), &policy, &profile);
        assert_eq!(sp.network(), sandbox::NetworkPolicy::AllowOutbound);
    }

    #[test]
    fn read_write_unions_policy_and_profile_paths() {
        let mut policy = default_deny_policy();
        policy.allow_read_outside = vec![PathBuf::from("/etc/from-policy")];
        policy.allow_write_outside = vec![PathBuf::from("/var/from-policy")];
        policy.extra_deny_paths = vec![PathBuf::from("/secret")];
        let mut profile = SandboxProfile::new();
        profile.allow_read("/etc/from-profile");
        profile.allow_write("/var/from-profile");

        let ws = Path::new("/tmp/ws");
        let sp = build_sandbox_policy(ws, &policy, &profile);

        let reads: Vec<_> = sp.filesystem().read_only_paths().collect();
        assert!(reads.contains(&Path::new("/etc/from-policy")));
        assert!(reads.contains(&Path::new("/etc/from-profile")));
        let writes: Vec<_> = sp.filesystem().read_write_paths().collect();
        assert!(writes.contains(&ws), "workspace itself must be writable");
        assert!(writes.contains(&Path::new("/var/from-policy")));
        assert!(writes.contains(&Path::new("/var/from-profile")));
        let denied: Vec<_> = sp.filesystem().denied_paths().collect();
        assert!(denied.contains(&Path::new("/secret")));
    }

    #[test]
    fn env_is_cleared_and_rebuilt_from_allowlist_on_all_platforms() {
        // Regression guard for macOS/Linux env-filtering parity: the built
        // policy must clear the child's environment and rebuild it only from
        // the profile's `env_pass` allow-list plus `declared_env`, on EVERY
        // platform. A `#[cfg]` that skipped this on one platform would leak the
        // iter process's full host environment (unrelated API tokens, cloud
        // credentials) into the sandboxed agent.
        let policy = default_deny_policy();
        let mut profile = SandboxProfile::new();
        profile
            .declared_env
            .push(("ITER_TEST_DECLARED".to_owned(), "declared-value".to_owned()));
        let sp = build_sandbox_policy(Path::new("/tmp/ws"), &policy, &profile);

        assert_eq!(
            sp.environment(),
            &sandbox::EnvironmentPolicy::Clear,
            "sandboxed child env must be cleared, not inherited from the host",
        );
        assert!(
            sp.envs()
                .any(|(k, v)| k == std::ffi::OsStr::new("ITER_TEST_DECLARED")
                    && v == std::ffi::OsStr::new("declared-value")),
            "declared_env must be rebuilt onto the cleared environment",
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn policy_preserves_profile_signal_requirement() {
        let policy = default_deny_policy();
        let mut profile = SandboxProfile::new();
        profile.allow_signal();
        let sandbox_policy =
            build_sandbox_policy(Path::new("/tmp/iter-sandbox-policy"), &policy, &profile);

        let mut command = std::process::Command::new("/bin/sh");
        command.arg("-c").arg("true");
        let wrapped = sandbox::std::Command::from_process(command, &sandbox_policy)
            .expect("wrap")
            .into_process();
        let args = wrapped
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(args[1].contains("(allow signal)"));
        assert!(!args[1].contains("(target self)"));
    }

    // ----- spawn seam tests ------------------------------------------------

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn confined_spawn_allows_workspace_write_and_denies_outside() {
        if !SandboxWorkspace::detect_backend_available() {
            return;
        }
        let base = TempDir::new().expect("tempdir");
        let outside = TempDir::new().expect("outside dir");
        let mut ws = SandboxWorkspace::new(
            base.path(),
            clone_settings(),
            default_deny_policy(),
            SandboxProfile::default(),
        );
        let active = ws.setup(CancellationToken::new()).await.expect("setup");

        // Inside the workspace: allowed.
        let mut inside = tokio::process::Command::new("/bin/sh");
        inside.arg("-c").arg("echo confined > inside.txt");
        let child = active
            .spawn(inside, StdioMode::Piped)
            .expect("spawn inside");
        let out = child.wait_with_output().await.expect("wait");
        assert!(
            out.status.success(),
            "workspace write must succeed: {}",
            String::from_utf8_lossy(&out.stderr),
        );
        assert!(active.path().join("inside.txt").exists());

        // Outside the workspace: denied by the default-deny world-view.
        let mut escape = tokio::process::Command::new("/bin/sh");
        escape.arg("-c").arg(format!(
            "echo escape > {}/out.txt",
            outside.path().display()
        ));
        let child = active
            .spawn(escape, StdioMode::Piped)
            .expect("spawn escape attempt");
        let out = child.wait_with_output().await.expect("wait");
        assert!(
            !out.status.success(),
            "write outside the world-view must be denied",
        );
        assert!(!outside.path().join("out.txt").exists());

        active
            .teardown(CancellationToken::new())
            .await
            .expect("teardown");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn allow_exec_policy_fails_at_spawn_on_linux() {
        // bwrap cannot express path-level exec allow-lists; the spawn seam
        // must surface sandbox::Error::UnsupportedPolicy as io::Error.
        let base = TempDir::new().expect("tempdir");
        let mut policy = default_deny_policy();
        policy.allow_exec = vec![PathBuf::from("/usr/bin/true")];
        let mut ws = SandboxWorkspace::new(
            base.path(),
            clone_settings(),
            policy,
            SandboxProfile::default(),
        );
        let active = ws.setup(CancellationToken::new()).await.expect("setup");
        let err = active
            .spawn(tokio::process::Command::new("true"), StdioMode::Piped)
            .expect_err("allow_exec must be rejected by the bwrap wrap");
        assert_eq!(err.kind(), std::io::ErrorKind::Other);
        Box::new(active)
            .teardown(CancellationToken::new())
            .await
            .expect("teardown");
    }
}
