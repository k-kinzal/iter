use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::policy::{NetworkPolicy, Policy};

pub(crate) fn wrap(policy: &Policy, target: &Command) -> Result<Command, crate::Error> {
    validate_profile_paths(policy, target)?;
    let profile = Profile::from_policy(policy, target).render();
    let mut command = Command::new("sandbox-exec");
    command.arg("-p").arg(profile);
    append_target(&mut command, target);
    apply_process_attributes(policy, target, &mut command);
    Ok(command)
}

/// An SBPL profile embeds every policy path as a string literal, so a path
/// that is not valid UTF-8 cannot be expressed — and rendering it lossily
/// would silently produce a rule that never matches the real path. Reject
/// such paths outright.
///
/// The profile embeds the *canonicalized* path (see [`canonical_or_self`]),
/// so validate that same form rather than the raw policy path: a raw UTF-8
/// path whose symlink target is not UTF-8 would otherwise pass here and then
/// be embedded lossily, silently weakening the isolation.
///
/// The working directory validated here mirrors what [`Profile::from_policy`]
/// embeds: the policy's `current_dir`, falling back to the target command's
/// own `current_dir` — so a cwd supplied only on the raw `Command` is covered
/// too.
fn validate_profile_paths(policy: &Policy, target: &Command) -> Result<(), crate::Error> {
    let effective_cwd = policy
        .current_dir_path()
        .or_else(|| target.get_current_dir());
    let paths = policy
        .filesystem()
        .read_only_paths()
        .chain(policy.filesystem().read_write_paths())
        .chain(policy.filesystem().tmpfs_paths())
        .chain(policy.filesystem().denied_paths())
        .chain(policy.process().allowed_executables())
        .chain(effective_cwd);
    for path in paths {
        let embedded = canonical_or_self(path);
        if embedded.to_str().is_none() {
            return Err(crate::Error::InvalidCommand(format!(
                "policy path is not valid UTF-8 and cannot be expressed \
                 in a sandbox profile: {}",
                embedded.display()
            )));
        }
    }
    Ok(())
}

fn append_target(command: &mut Command, target: &Command) {
    command.arg(target.get_program());
    command.args(target.get_args());
}

fn apply_process_attributes(policy: &Policy, source: &Command, target: &mut Command) {
    if policy.environment() == &crate::EnvironmentPolicy::Clear {
        target.env_clear();
    }
    for (key, value) in policy.envs() {
        target.env(key, value);
    }
    apply_command_overrides(source, target);
    if let Some(dir) = policy.current_dir_path() {
        target.current_dir(dir);
    }
}

fn apply_command_overrides(source: &Command, target: &mut Command) {
    if let Some(dir) = source.get_current_dir() {
        target.current_dir(dir);
    }
    for (key, value) in source.get_envs() {
        match value {
            Some(value) => {
                target.env(key, value);
            }
            None => {
                target.env_remove(key);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Profile {
    rules: Vec<Rule>,
}

impl Profile {
    fn from_policy(policy: &Policy, target: &Command) -> Self {
        let mut profile = Self::default_deny()
            .allow([Operation::ProcessFork], [])
            .allow([Operation::SysctlRead], [])
            .allow([Operation::FileReadMetadata], [])
            .allow(
                [Operation::FileWriteAll],
                [
                    Filter::literal("/dev/null"),
                    Filter::literal("/dev/stdout"),
                    Filter::literal("/dev/stderr"),
                ],
            );
        profile = if policy.target().allows_signal() {
            profile.allow([Operation::Signal], [])
        } else {
            profile.allow([Operation::Signal], [Filter::TargetSelf])
        };

        let allow_exec = policy.process().allowed_executables().collect::<Vec<_>>();
        if allow_exec.is_empty() {
            profile = profile.allow([Operation::ProcessExec], []);
        } else {
            let filters = allow_exec
                .iter()
                .map(|path| Filter::literal(canonical_or_self(path).display().to_string()))
                .collect::<Vec<_>>();
            profile = profile
                .allow([Operation::ProcessExec], filters.clone())
                .allow([Operation::ProcessExecInterpreter], filters);
        }

        let mut read_filters = Vec::new();
        read_filters.push(Filter::literal("/"));
        for path in default_read_paths() {
            read_filters.push(Filter::subpath(path));
        }
        for path in policy
            .filesystem()
            .read_only_paths()
            .chain(policy.filesystem().read_write_paths())
        {
            read_filters.push(Filter::subpath(canonical_or_self(path)));
        }
        for path in allow_exec {
            read_filters.push(Filter::literal(
                canonical_or_self(path).display().to_string(),
            ));
        }
        if let Some(dir) = policy
            .current_dir_path()
            .or_else(|| target.get_current_dir())
        {
            read_filters.push(Filter::subpath(canonical_or_self(dir)));
        }
        profile = profile.allow(
            [Operation::FileReadData, Operation::FileReadXattr],
            read_filters,
        );

        let write_filters = policy
            .filesystem()
            .read_write_paths()
            .map(|path| Filter::subpath(canonical_or_self(path)))
            .collect::<Vec<_>>();
        if !write_filters.is_empty() {
            profile = profile.allow([Operation::FileWriteAll], write_filters);
        }
        for pattern in policy.target().file_write_patterns() {
            profile = profile.allow([Operation::FileWriteAll], [Filter::regex(pattern)]);
        }

        match policy.network() {
            NetworkPolicy::Deny => {
                profile = profile.deny([Operation::NetworkAll], []);
            }
            NetworkPolicy::AllowOutbound => {
                profile = profile
                    .allow([Operation::NetworkOutbound], [])
                    .allow([Operation::NetworkBind], [Filter::LocalIp]);
            }
        }

        let mut mach_lookup = vec![
            Filter::global_name("com.apple.SecurityServer"),
            Filter::global_name("com.apple.trustd"),
            Filter::global_name("com.apple.mDNSResponder"),
        ];
        mach_lookup.extend(
            policy
                .target()
                .mach_lookup()
                .iter()
                .map(Filter::global_name),
        );
        profile = profile.allow([Operation::MachLookup], mach_lookup);

        let denied = policy
            .filesystem()
            .denied_paths()
            .chain(policy.filesystem().tmpfs_paths())
            .map(|path| Filter::subpath(canonical_or_self(path)))
            .collect::<Vec<_>>();
        if !denied.is_empty() {
            profile = profile.deny([Operation::FileReadAll, Operation::FileWriteAll], denied);
        }

        profile
    }

    fn default_deny() -> Self {
        Self {
            rules: vec![Rule::deny([Operation::Default], [])],
        }
    }

    fn allow(
        mut self,
        operations: impl IntoIterator<Item = Operation>,
        filters: impl IntoIterator<Item = Filter>,
    ) -> Self {
        self.rules.push(Rule::allow(operations, filters));
        self
    }

    fn deny(
        mut self,
        operations: impl IntoIterator<Item = Operation>,
        filters: impl IntoIterator<Item = Filter>,
    ) -> Self {
        self.rules.push(Rule::deny(operations, filters));
        self
    }

    fn render(&self) -> String {
        let mut buf = String::new();
        buf.push_str("(version 1)\n");
        for rule in &self.rules {
            rule.render(&mut buf);
        }
        buf
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    Allow,
    Deny,
}

impl Decision {
    fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    decision: Decision,
    operations: Vec<Operation>,
    filters: Vec<Filter>,
}

impl Rule {
    fn allow(
        operations: impl IntoIterator<Item = Operation>,
        filters: impl IntoIterator<Item = Filter>,
    ) -> Self {
        Self {
            decision: Decision::Allow,
            operations: operations.into_iter().collect(),
            filters: filters.into_iter().collect(),
        }
    }

    fn deny(
        operations: impl IntoIterator<Item = Operation>,
        filters: impl IntoIterator<Item = Filter>,
    ) -> Self {
        Self {
            decision: Decision::Deny,
            operations: operations.into_iter().collect(),
            filters: filters.into_iter().collect(),
        }
    }

    fn render(&self, buf: &mut String) {
        write!(buf, "({}", self.decision.as_str()).ok();
        for operation in &self.operations {
            write!(buf, " {}", operation.render()).ok();
        }
        if self.filters.is_empty() {
            buf.push_str(")\n");
            return;
        }
        for filter in &self.filters {
            write!(buf, "\n    {}", filter.render()).ok();
        }
        buf.push_str(")\n");
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Operation {
    Default,
    ProcessExec,
    ProcessExecInterpreter,
    ProcessFork,
    Signal,
    SysctlRead,
    FileReadAll,
    FileReadData,
    FileReadMetadata,
    FileReadXattr,
    FileWriteAll,
    NetworkAll,
    NetworkOutbound,
    NetworkBind,
    MachLookup,
}

impl Operation {
    fn render(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::ProcessExec => "process-exec",
            Self::ProcessExecInterpreter => "process-exec-interpreter",
            Self::ProcessFork => "process-fork",
            Self::Signal => "signal",
            Self::SysctlRead => "sysctl-read",
            Self::FileReadAll => "file-read*",
            Self::FileReadData => "file-read-data",
            Self::FileReadMetadata => "file-read-metadata",
            Self::FileReadXattr => "file-read-xattr",
            Self::FileWriteAll => "file-write*",
            Self::NetworkAll => "network*",
            Self::NetworkOutbound => "network-outbound",
            Self::NetworkBind => "network-bind",
            Self::MachLookup => "mach-lookup",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Filter {
    Literal(String),
    Subpath(String),
    Regex(String),
    GlobalName(String),
    TargetSelf,
    LocalIp,
}

impl Filter {
    fn literal(value: impl Into<String>) -> Self {
        Self::Literal(value.into())
    }

    fn subpath(path: impl Into<PathBuf>) -> Self {
        Self::Subpath(path.into().display().to_string())
    }

    fn regex(value: impl Into<String>) -> Self {
        Self::Regex(value.into())
    }

    fn global_name(value: impl Into<String>) -> Self {
        Self::GlobalName(value.into())
    }

    fn render(&self) -> String {
        match self {
            Self::Literal(value) => format!("(literal {})", sb_string(value)),
            Self::Subpath(value) => format!("(subpath {})", sb_string(value)),
            Self::Regex(value) => format!("(regex #{})", sb_string(value)),
            Self::GlobalName(value) => format!("(global-name {})", sb_string(value)),
            Self::TargetSelf => "(target self)".to_owned(),
            Self::LocalIp => "(local ip)".to_owned(),
        }
    }
}

fn default_read_paths() -> impl Iterator<Item = &'static str> {
    [
        "/usr",
        "/System",
        "/Library/Preferences",
        "/private/var/db",
        "/private/etc",
        "/dev",
        "/bin",
        "/sbin",
        "/opt/homebrew",
    ]
    .into_iter()
}

fn canonical_or_self(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn sb_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_with_sandbox_exec_profile() {
        let policy = Policy::new()
            .allow_network()
            .allow_read("/input")
            .allow_write("/work")
            .allow_executable("/bin/sh")
            .allow_mach_lookup("com.example.service")
            .allow_write_matching("^/tmp/example-.*$");
        let target = Command::new("/bin/sh");
        let command = wrap(&policy, &target).expect("wrap");
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(command.get_program(), "sandbox-exec");
        assert_eq!(args[0], "-p");
        assert!(args[1].contains("(allow process-exec"));
        assert!(args[1].contains("(allow signal\n    (target self))"));
        assert!(args[1].contains("/input"));
        assert!(args[1].contains("network-outbound"));
        assert!(args[1].contains("com.example.service"));
        assert!(args[1].contains("^/tmp/example-.*$"));
    }

    #[test]
    fn allow_signal_renders_unfiltered_signal_operation() {
        let policy = Policy::new().allow_signal();
        let target = Command::new("/bin/sh");
        let command = wrap(&policy, &target).expect("wrap");
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(args[1].contains("(allow signal)"));
        assert!(!args[1].contains("(target self)"));
    }

    #[test]
    fn quotes_sbpl_strings() {
        assert_eq!(sb_string(r#"a\b"c"#), r#""a\\b\"c""#);
    }

    #[test]
    fn rejects_non_utf8_cwd_set_only_on_target_command() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        // A cwd supplied only on the raw `Command` (no `Policy::current_dir`)
        // still reaches the SBPL profile through `from_policy`'s
        // `.or_else(|| target.get_current_dir())` fallback. If it is not valid
        // UTF-8 it cannot be expressed as a profile string literal, so `wrap`
        // must reject it rather than silently render a rule that never matches
        // the real path. This locks in the parity between `validate_profile_paths`
        // and `from_policy` for the target-command fallback branch specifically.
        let non_utf8 = OsStr::from_bytes(&[b'/', b'f', b'o', 0xff]);
        let mut target = Command::new("/bin/sh");
        target.current_dir(Path::new(non_utf8));

        // Policy sets no current_dir — so the fallback to the target command's
        // cwd is the only thing under test.
        let policy = Policy::new();
        let result = wrap(&policy, &target);

        assert!(
            matches!(result, Err(crate::Error::InvalidCommand(_))),
            "non-UTF-8 cwd set only on the target Command must be rejected, got {result:?}"
        );
    }
}
