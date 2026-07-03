use std::path::Path;
use std::process::Command;

use crate::Error;
use crate::policy::{EnvironmentPolicy, NetworkPolicy, Policy};

pub(crate) fn wrap(policy: &Policy, target: &Command) -> Result<Command, Error> {
    if policy.process().allowed_executables().next().is_some() {
        return Err(Error::UnsupportedPolicy {
            command: "bwrap",
            reason:
                "path-level executable allow-lists are not representable by the selected sandbox command"
                    .to_owned(),
        });
    }

    let mut command = Command::new("bwrap");
    command
        .arg("--die-with-parent")
        .arg("--new-session")
        .arg("--unshare-all")
        .arg("--disable-userns");

    if policy.network() == NetworkPolicy::AllowOutbound {
        command.arg("--share-net");
    }
    if policy.environment() == &EnvironmentPolicy::Clear {
        command.arg("--clearenv");
    }

    for path in default_system_paths() {
        if path.exists() {
            push_pair(&mut command, "--ro-bind", path, path);
        }
    }
    command.arg("--proc").arg("/proc");
    command.arg("--dev").arg("/dev");

    for path in policy.filesystem().read_only_paths() {
        push_pair(&mut command, "--ro-bind-try", path, path);
    }
    for path in policy.filesystem().read_write_paths() {
        push_pair(&mut command, "--bind-try", path, path);
    }
    for path in policy.filesystem().tmpfs_paths() {
        command.arg("--tmpfs").arg(path);
    }
    for path in policy.filesystem().denied_paths() {
        command.arg("--tmpfs").arg(path);
    }
    for (key, value) in policy.envs() {
        command.arg("--setenv").arg(key).arg(value);
    }
    for (key, value) in target.get_envs() {
        match value {
            Some(value) => {
                command.arg("--setenv").arg(key).arg(value);
            }
            None => {
                command.arg("--unsetenv").arg(key);
            }
        }
    }
    for fd in policy.process().seccomp_filter_fds() {
        command.arg("--add-seccomp-fd").arg(fd.to_string());
    }
    if let Some(dir) = policy
        .current_dir_path()
        .or_else(|| target.get_current_dir())
    {
        command.arg("--chdir").arg(dir);
    }

    command.arg("--");
    append_target(&mut command, target);
    apply_process_attributes(policy, target, &mut command);
    Ok(command)
}

fn push_pair(command: &mut Command, flag: &str, first: &Path, second: &Path) {
    command.arg(flag).arg(first).arg(second);
}

fn append_target(command: &mut Command, target: &Command) {
    command.arg(target.get_program());
    command.args(target.get_args());
}

fn apply_process_attributes(policy: &Policy, source: &Command, target: &mut Command) {
    if policy.environment() == &EnvironmentPolicy::Clear {
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

fn default_system_paths() -> impl Iterator<Item = &'static Path> {
    ["/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/etc"]
        .into_iter()
        .map(Path::new)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn wraps_with_bwrap_arguments() {
        let policy = Policy::new()
            .allow_network()
            .allow_read("/input")
            .allow_write("/work")
            .temporary_filesystem("/tmp")
            .clear_environment()
            .set_env("PATH", "/usr/bin");
        let mut target = Command::new("/bin/sh");
        target.arg("-c");

        let command = wrap(&policy, &target).expect("wrap");
        let args = command
            .get_args()
            .map(OsStr::to_string_lossy)
            .collect::<Vec<_>>();

        assert_eq!(command.get_program(), "bwrap");
        assert!(args.iter().any(|arg| arg == "--share-net"));
        assert!(
            args.windows(3)
                .any(|w| w == ["--ro-bind-try", "/input", "/input"])
        );
        assert!(
            args.windows(3)
                .any(|w| w == ["--bind-try", "/work", "/work"])
        );
        assert!(args.windows(2).any(|w| w == ["--tmpfs", "/tmp"]));
        assert!(
            args.windows(3)
                .any(|w| w == ["--setenv", "PATH", "/usr/bin"])
        );
        assert!(args.ends_with(&["--".into(), "/bin/sh".into(), "-c".into()]));
    }
}
