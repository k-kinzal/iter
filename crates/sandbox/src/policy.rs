use std::ffi::{OsStr, OsString};
#[cfg(target_os = "linux")]
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};

/// Network policy applied inside the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkPolicy {
    /// Deny network access.
    Deny,
    /// Allow outbound network access.
    AllowOutbound,
}

/// Process environment policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvironmentPolicy {
    /// Inherit the parent process environment, plus policy and command
    /// overrides.
    Inherit,
    /// Clear the inherited environment and apply only explicit policy and
    /// command overrides.
    Clear,
}

/// Filesystem policy applied inside the sandbox.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilesystemPolicy {
    read_only: Vec<PathBuf>,
    read_write: Vec<PathBuf>,
    tmpfs: Vec<PathBuf>,
    deny: Vec<PathBuf>,
}

impl FilesystemPolicy {
    /// Paths visible read-only inside the sandbox.
    pub fn read_only_paths(&self) -> impl Iterator<Item = &Path> {
        self.read_only.iter().map(PathBuf::as_path)
    }

    /// Paths visible read-write inside the sandbox.
    pub fn read_write_paths(&self) -> impl Iterator<Item = &Path> {
        self.read_write.iter().map(PathBuf::as_path)
    }

    /// Paths backed by an empty temporary filesystem where supported.
    pub fn tmpfs_paths(&self) -> impl Iterator<Item = &Path> {
        self.tmpfs.iter().map(PathBuf::as_path)
    }

    /// Paths explicitly denied by the sandbox.
    pub fn denied_paths(&self) -> impl Iterator<Item = &Path> {
        self.deny.iter().map(PathBuf::as_path)
    }
}

/// Process policy applied inside the sandbox.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessPolicy {
    allow_exec: Vec<PathBuf>,
    #[cfg(target_os = "linux")]
    seccomp_filter_fds: Vec<RawFd>,
}

impl ProcessPolicy {
    /// Executable allow-list requested by the policy.
    pub fn allowed_executables(&self) -> impl Iterator<Item = &Path> {
        self.allow_exec.iter().map(PathBuf::as_path)
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn seccomp_filter_fds(&self) -> &[RawFd] {
        &self.seccomp_filter_fds
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TargetPolicy {
    mach_lookup: Vec<String>,
    file_write_patterns: Vec<String>,
    allow_signal: bool,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TargetPolicy;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TargetPolicy;

/// Complete sandbox policy for the current compilation target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    filesystem: FilesystemPolicy,
    network: NetworkPolicy,
    environment: EnvironmentPolicy,
    process: ProcessPolicy,
    env: Vec<(OsString, OsString)>,
    current_dir: Option<PathBuf>,
    target: TargetPolicy,
}

impl Default for Policy {
    fn default() -> Self {
        Self::new()
    }
}

impl Policy {
    /// Create a default-deny sandbox policy.
    #[must_use]
    pub fn new() -> Self {
        Self {
            filesystem: FilesystemPolicy::default(),
            network: NetworkPolicy::Deny,
            environment: EnvironmentPolicy::Inherit,
            process: ProcessPolicy::default(),
            env: Vec::new(),
            current_dir: None,
            target: TargetPolicy::default(),
        }
    }

    /// Filesystem policy.
    #[must_use]
    pub fn filesystem(&self) -> &FilesystemPolicy {
        &self.filesystem
    }

    /// Network policy.
    #[must_use]
    pub fn network(&self) -> NetworkPolicy {
        self.network
    }

    /// Environment policy.
    #[must_use]
    pub fn environment(&self) -> &EnvironmentPolicy {
        &self.environment
    }

    /// Process policy.
    #[must_use]
    pub fn process(&self) -> &ProcessPolicy {
        &self.process
    }

    /// Current directory requested for the sandboxed process.
    #[must_use]
    pub fn current_dir_path(&self) -> Option<&Path> {
        self.current_dir.as_deref()
    }

    /// Environment variables set by the policy.
    pub fn envs(&self) -> impl Iterator<Item = (&OsStr, &OsStr)> {
        self.env
            .iter()
            .map(|(key, value)| (key.as_os_str(), value.as_os_str()))
    }

    /// Deny network access.
    #[must_use]
    pub fn deny_network(mut self) -> Self {
        self.network = NetworkPolicy::Deny;
        self
    }

    /// Allow outbound network access.
    #[must_use]
    pub fn allow_network(mut self) -> Self {
        self.network = NetworkPolicy::AllowOutbound;
        self
    }

    /// Clear the inherited environment before applying explicit environment
    /// values.
    #[must_use]
    pub fn clear_environment(mut self) -> Self {
        self.environment = EnvironmentPolicy::Clear;
        self
    }

    /// Inherit the parent process environment.
    #[must_use]
    pub fn inherit_environment(mut self) -> Self {
        self.environment = EnvironmentPolicy::Inherit;
        self
    }

    /// Add a read-only filesystem path.
    #[must_use]
    pub fn allow_read(mut self, path: impl Into<PathBuf>) -> Self {
        self.filesystem.read_only.push(path.into());
        self
    }

    /// Add a read-write filesystem path.
    #[must_use]
    pub fn allow_write(mut self, path: impl Into<PathBuf>) -> Self {
        self.filesystem.read_write.push(path.into());
        self
    }

    /// Add an empty temporary filesystem path where the target supports it.
    #[must_use]
    pub fn temporary_filesystem(mut self, path: impl Into<PathBuf>) -> Self {
        self.filesystem.tmpfs.push(path.into());
        self
    }

    /// Explicitly deny a filesystem path.
    #[must_use]
    pub fn deny_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.filesystem.deny.push(path.into());
        self
    }

    /// Restrict process execution to an executable path where the target can
    /// enforce path-level exec policy.
    #[must_use]
    pub fn allow_executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.process.allow_exec.push(path.into());
        self
    }

    /// Set an environment variable inside the sandbox.
    #[must_use]
    pub fn set_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Set the current directory inside the sandbox.
    #[must_use]
    pub fn current_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(path.into());
        self
    }

    /// Add a seccomp BPF program fd to the sandbox.
    ///
    /// This method exists only for targets that support seccomp. The fd must
    /// remain valid until the command is spawned.
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn seccomp_filter_fd(mut self, fd: RawFd) -> Self {
        self.process.seccomp_filter_fds.push(fd);
        self
    }

    /// Allow lookup of a Mach service by global name.
    ///
    /// This method exists only for targets with Mach service lookup controls.
    #[cfg(target_os = "macos")]
    #[must_use]
    pub fn allow_mach_lookup(mut self, global_name: impl Into<String>) -> Self {
        self.target.mach_lookup.push(global_name.into());
        self
    }

    /// Allow writes to paths matching a sandbox path pattern.
    ///
    /// This method exists only for targets whose sandbox language supports
    /// path-pattern write rules.
    #[cfg(target_os = "macos")]
    #[must_use]
    pub fn allow_write_matching(mut self, pattern: impl Into<String>) -> Self {
        self.target.file_write_patterns.push(pattern.into());
        self
    }

    /// Allow the sandbox profile's unfiltered `signal` operation.
    ///
    /// The default macOS profile allows `signal` only with `(target self)`.
    /// This widens that rule to `(allow signal)`.
    #[cfg(target_os = "macos")]
    #[must_use]
    pub fn allow_signal(mut self) -> Self {
        self.target.allow_signal = true;
        self
    }

    /// Target-specific policy extensions. Only macOS has any today —
    /// the accessor is gated with the consuming platform code so other
    /// targets do not carry a dead accessor.
    #[cfg(target_os = "macos")]
    pub(crate) fn target(&self) -> &TargetPolicy {
        &self.target
    }
}

#[cfg(target_os = "macos")]
impl TargetPolicy {
    pub(crate) fn mach_lookup(&self) -> &[String] {
        &self.mach_lookup
    }

    pub(crate) fn file_write_patterns(&self) -> &[String] {
        &self.file_write_patterns
    }

    pub(crate) fn allows_signal(&self) -> bool {
        self.allow_signal
    }
}
