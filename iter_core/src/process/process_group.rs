//! `ProcessGroup` — owns a spawned process tree by its OS process group id
//! so iter can SIGTERM/SIGKILL the entire tree (including grandchildren) on
//! cancel.
//!
//! Why this exists: the agent (`claude --print` etc.) often spawns its own
//! sub-processes (sandboxed shells, tool invocations, recursive `iter run`s).
//! Killing only the direct child leaves those grandchildren running,
//! reparented to launchd/init. `ProcessGroup` records the pgid at spawn
//! time and uses `killpg(2)` to signal the whole group.
//!
//! The intended call shape is:
//!
//! 1. `process_group::configure(&mut command)` before `spawn()` — installs
//!    `process_group(0)` so the child becomes leader of a new group whose
//!    pgid equals its own pid.
//! 2. `let group = ProcessGroup::from_child(&child);` immediately after
//!    `spawn()` succeeds.
//! 3. On cancel, `group.terminate(grace).await` to deliver SIGTERM, wait
//!    `grace`, then SIGKILL.
//!
//! `Drop` is the safety net: a panic or early return between (2) and (3)
//! still SIGKILLs the group so a half-cancelled iter run never leaves the
//! tree alive.
//!
//! Non-unix platforms degrade to a no-op: `configure` does nothing, the
//! struct only holds the pid, and `terminate` falls back to a single
//! `Child::start_kill()` issued by the caller. There is no portable
//! "process group" concept on Windows, so cross-platform users that need
//! tree-killing should reach for a job object instead.

use std::time::Duration;

use tokio::process::{Child, Command};
use tracing::{debug, warn};

/// Apply OS-level process-group settings to a [`Command`] before `spawn`.
///
/// Unix: calls `process_group(0)` so the spawned child becomes the leader
/// of a brand-new group whose pgid equals its own pid. After spawn the
/// caller can read `child.id()` and use it as the pgid for `killpg`.
///
/// Non-unix: no-op. There is no portable "process group" abstraction
/// outside POSIX.
pub fn configure(command: &mut Command) {
    #[cfg(unix)]
    {
        // `tokio::process::Command::process_group` was added in tokio 1.21
        // and forwards to `std::os::unix::process::CommandExt::process_group`.
        // Passing `0` makes the child its own group leader.
        command.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = command;
    }
}

/// Owns a spawned process tree by its pgid.
///
/// Construct with [`ProcessGroup::from_child`] right after `spawn()`. The
/// caller still owns the [`Child`] handle — `ProcessGroup` only stores
/// the pgid so it can outlive the `Child` borrow during a `tokio::select!`
/// arm.
#[derive(Debug)]
pub struct ProcessGroup {
    /// Process-group id on Unix, or the pid we would have killed if this
    /// were Unix. We hold an `Option` so [`Drop`] can no-op after
    /// [`Self::terminate`] reaped the group.
    pgid: Option<u32>,
}

impl ProcessGroup {
    /// Wrap a freshly-spawned child whose [`Command`] was prepared via
    /// [`configure`].
    ///
    /// Returns a group with `pgid = child.id()`. If the child has already
    /// exited and `id()` is `None`, the resulting group is inert
    /// (`terminate` and `Drop` do nothing).
    #[must_use]
    pub fn from_child(child: &Child) -> Self {
        Self { pgid: child.id() }
    }

    /// SIGTERM the group, wait up to `grace`, then SIGKILL anything that
    /// still has not exited.
    ///
    /// `ESRCH` (group already gone) is treated as success at every step:
    /// the caller's intent is "make sure this tree is no longer holding
    /// state", and an absent group satisfies that trivially. Any other
    /// errno is logged at `debug` and otherwise swallowed — this method
    /// is a best-effort cleanup, not a place to surface new errors that
    /// would shadow the original cancel reason.
    ///
    /// After this returns, [`Drop`] is a no-op.
    pub async fn terminate(&mut self, grace: Duration) {
        let Some(pgid) = self.pgid.take() else {
            return;
        };

        #[cfg(unix)]
        {
            send_killpg(pgid, libc::SIGTERM);
            tokio::time::sleep(grace).await;
            send_killpg(pgid, libc::SIGKILL);
        }
        #[cfg(not(unix))]
        {
            // Non-unix has no killpg; the caller's `Child::start_kill`
            // path is the only available recourse. This branch only runs
            // when we somehow recorded a pid on a non-unix host, which
            // should not happen in practice.
            let _ = (pgid, grace);
        }
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        let Some(pgid) = self.pgid.take() else {
            return;
        };
        #[cfg(unix)]
        {
            // Best-effort SIGKILL. We cannot await a grace period from
            // `Drop`, and a leaked tree is a worse outcome than a
            // forced kill, so jump straight to SIGKILL.
            send_killpg(pgid, libc::SIGKILL);
            debug!(
                pgid,
                "ProcessGroup dropped without explicit terminate; sent SIGKILL"
            );
        }
        #[cfg(not(unix))]
        {
            let _ = pgid;
        }
    }
}

#[cfg(unix)]
fn send_killpg(pgid: u32, signum: libc::c_int) {
    let Ok(raw) = i32::try_from(pgid) else {
        debug!(pgid, "pgid out of i32 range; skipping killpg");
        return;
    };
    // SAFETY: `killpg` is a stable POSIX system call. Passing a pgid
    // that no longer exists yields `ESRCH`, which we treat as success.
    let rc = unsafe { libc::killpg(raw, signum) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::ESRCH) => {
                // Group already gone; the caller's intent is satisfied.
            }
            Some(libc::EPERM) => {
                // We lack permission to signal this group — typically because
                // the child changed its real UID (e.g. inside a container or
                // via sudo). The tree will keep running; surface this loudly
                // so operators can see it.
                warn!(
                    pgid,
                    signum,
                    error = %err,
                    "killpg returned EPERM; process tree may survive (uid changed?)",
                );
            }
            _ => {
                debug!(pgid, signum, error = %err, "killpg returned non-ESRCH error");
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command as TokioCommand;

    /// Helper: read `getpgid(pid)` via libc. Returns `-1` (and sets errno)
    /// when the process has exited.
    fn getpgid(pid: u32) -> i32 {
        // SAFETY: `getpgid` is a stable POSIX system call. We pass a
        // best-effort i32 cast of the pid; truncation just yields a
        // pgid that won't match the true one and the assertion will
        // catch the regression.
        unsafe { libc::getpgid(i32::try_from(pid).unwrap_or(0)) }
    }

    /// Helper: `kill(pid, 0)` returns 0 if the process exists, -1 with
    /// `ESRCH` if it does not.
    fn pid_alive(pid: i32) -> bool {
        // SAFETY: `kill(pid, 0)` is the canonical "is this pid alive"
        // probe; signal 0 performs error checks without delivering.
        let rc = unsafe { libc::kill(pid, 0) };
        if rc == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }

    #[tokio::test]
    async fn spawns_in_new_process_group() {
        let mut cmd = TokioCommand::new("sleep");
        cmd.arg("0.5");
        configure(&mut cmd);
        let mut child = cmd.spawn().expect("spawn sleep");
        let child_pid = child.id().expect("child pid");
        let group_id = getpgid(child_pid);
        assert_eq!(
            group_id,
            i32::try_from(child_pid).expect("child pid fits in i32"),
            "configured child should be its own process-group leader"
        );
        drop(child.wait().await);
    }

    #[tokio::test]
    async fn terminate_group_kills_grandchild() {
        // Spawn a bash that backgrounds a long sleep and waits on it.
        // The grandchild (`sleep 60`) inherits the new process group,
        // so `killpg` should reach it too.
        let mut cmd = TokioCommand::new("bash");
        cmd.arg("-c").arg("sleep 60 & echo $! ; wait");
        cmd.stdout(std::process::Stdio::piped());
        configure(&mut cmd);
        let mut child = cmd.spawn().expect("spawn bash");
        let mut group = ProcessGroup::from_child(&child);

        // Read the grandchild pid bash printed.
        let stdout = child.stdout.take().expect("stdout");
        let mut lines = BufReader::new(stdout).lines();
        let line = lines.next_line().await.expect("read line").expect("line");
        let grandchild: i32 = line.trim().parse().expect("grandchild pid");

        // Sanity: grandchild is alive before terminate.
        assert!(pid_alive(grandchild), "grandchild should be alive");

        group.terminate(Duration::from_millis(100)).await;
        drop(child.wait().await);

        // Give the kernel a beat to finish reaping after SIGKILL.
        for _ in 0..50 {
            if !pid_alive(grandchild) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            !pid_alive(grandchild),
            "grandchild should have been reaped via killpg"
        );
    }

    #[tokio::test]
    async fn terminate_group_escalates_to_sigkill() {
        // Child ignores SIGTERM and busy-loops without spawning any child
        // process — this guarantees bash itself must absorb the kill,
        // rather than exiting because a fork()ed `sleep` was reaped.
        // Use `--noprofile --norc` so user dotfiles cannot reset the trap,
        // and write the pid out so we can verify it survives the SIGTERM.
        let mut cmd = TokioCommand::new("bash");
        cmd.arg("--noprofile")
            .arg("--norc")
            .arg("-c")
            .arg("trap '' TERM; echo ready; while :; do :; done");
        cmd.stdout(std::process::Stdio::piped());
        configure(&mut cmd);
        let mut child = cmd.spawn().expect("spawn bash");
        let pid = i32::try_from(child.id().expect("child pid")).expect("pid fits i32");
        let mut group = ProcessGroup::from_child(&child);

        let stdout = child.stdout.take().expect("stdout");
        let mut lines = BufReader::new(stdout).lines();
        drop(lines.next_line().await.expect("read ready"));

        // Long grace so we can observe SIGTERM being ignored, then SIGKILL.
        let grace = Duration::from_millis(300);
        let terminate = tokio::spawn(async move {
            group.terminate(grace).await;
        });

        // Mid-grace: SIGTERM should already have fired and been ignored.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            pid_alive(pid),
            "process should still be alive after SIGTERM (trap ignores it)"
        );

        terminate.await.expect("terminate task");
        let status = child.wait().await.expect("wait");

        assert_eq!(
            status.signal(),
            Some(libc::SIGKILL),
            "process that ignored SIGTERM should die from SIGKILL escalation"
        );
    }

    #[tokio::test]
    async fn drop_terminates_group() {
        let mut cmd = TokioCommand::new("bash");
        cmd.arg("-c").arg("sleep 60 & echo $! ; wait");
        cmd.stdout(std::process::Stdio::piped());
        configure(&mut cmd);
        let mut child = cmd.spawn().expect("spawn bash");

        let stdout = child.stdout.take().expect("stdout");
        let mut lines = BufReader::new(stdout).lines();
        let line = lines.next_line().await.expect("read line").expect("line");
        let grandchild: i32 = line.trim().parse().expect("grandchild pid");
        assert!(pid_alive(grandchild));

        // Sanity: the bash parent must still be alive so we know `Drop`
        // (rather than the absence of a pgid) is what reaps the grandchild.
        // Otherwise this test could pass even with a no-op `Drop`.
        assert!(
            child.id().is_some(),
            "bash parent must still be alive so the pgid is real",
        );

        // Drop the group without calling terminate. The Drop impl should
        // SIGKILL the whole tree.
        drop(ProcessGroup::from_child(&child));
        drop(child.wait().await);

        for _ in 0..50 {
            if !pid_alive(grandchild) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            !pid_alive(grandchild),
            "grandchild should die when ProcessGroup is dropped"
        );
    }
}
