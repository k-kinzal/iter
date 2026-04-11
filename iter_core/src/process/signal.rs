//! POSIX signal delivery for [`crate::process::handle::ProcessHandle`].
//!
//! Best-effort: an absent / already-exited target (`ESRCH`) is treated as
//! success because the caller's intent is "make sure this pid is no longer
//! holding state", and a nonexistent process satisfies that trivially. All
//! other errno values surface as [`crate::process::error::ProcessError::Io`].

use crate::process::error::{ProcessError, Result};
use crate::process::pid_file::ProcessIdentity;
use crate::process::proc_info::process_is_alive_with_start_time;

/// Which POSIX signal `ProcessHandle::stop` / `kill` should deliver.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SignalKind {
    /// `SIGTERM` — graceful termination request.
    Term,
    /// `SIGKILL` — forced termination.
    Kill,
}

#[cfg(unix)]
pub(crate) fn send(pid: u32, kind: SignalKind) -> Result<()> {
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid as NixPid;

    let raw = i32::try_from(pid)
        .map_err(|_| ProcessError::Io(std::io::Error::other(format!("pid {pid} out of range"))))?;
    let sig = match kind {
        SignalKind::Term => Signal::SIGTERM,
        SignalKind::Kill => Signal::SIGKILL,
    };
    match kill(NixPid::from_raw(raw), sig) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(err) => Err(ProcessError::Io(std::io::Error::from(err))),
    }
}

#[cfg(not(unix))]
pub(crate) fn send(_pid: u32, _kind: SignalKind) -> Result<()> {
    Err(ProcessError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "signal delivery is unix-only",
    )))
}

/// Send `SIGTERM` to an arbitrary pid that is **not** part of the local
/// registry — typically the compose orchestrator. Absorbs `ESRCH` so a
/// caller racing the target's exit observes success.
///
/// # Errors
///
/// Returns [`ProcessError::Io`] for any errno other than `ESRCH`.
pub fn signal_pid_term(pid: u32) -> Result<()> {
    send(pid, SignalKind::Term)
}

/// Send `SIGKILL` to an arbitrary pid that is **not** part of the local
/// registry — typically the compose orchestrator after a graceful
/// `SIGTERM` window expires. Absorbs `ESRCH` like [`signal_pid_term`].
///
/// # Errors
///
/// Returns [`ProcessError::Io`] for any errno other than `ESRCH`.
pub fn signal_pid_kill(pid: u32) -> Result<()> {
    send(pid, SignalKind::Kill)
}

/// Re-verify the identity's `pid+start_time` fingerprint immediately before
/// signalling. **This narrows but does NOT eliminate the TOCTOU window**
/// between the fingerprint check and the actual `kill(2)`: the kernel
/// can still reuse the pid between the two userspace syscalls, and a
/// process started in that microsecond would receive the signal. Treat
/// this as best-effort hardening over a raw [`signal_pid_term`].
///
/// Returns `Ok(true)` if the signal was delivered to a process whose
/// fingerprint still matched at check time, `Ok(false)` if the
/// fingerprint did not match (i.e. the original process has exited and
/// nothing was signalled). A `false` return is success in the sense
/// "the process we wanted to signal is no longer there".
///
/// On Linux, true atomic-with-the-pid signal delivery requires
/// `pidfd_open(2)` + `pidfd_send_signal(2)` (kernel ≥ 5.3). This helper
/// does not currently use pidfds — see the `signal_pidfd` follow-up tracked
/// in the discovery module's trust-boundary documentation.
///
/// **Trust boundary** — the identity is assumed to come from a trusted
/// source (the local `~/.iter/proc/` registry, written by the same UID).
/// Callers that read identities out of attacker-controllable storage must
/// validate them separately; this helper does not authenticate where the
/// label came from, only that the live process matched it at check time.
///
/// # Errors
///
/// Returns [`ProcessError::Io`] for any errno other than `ESRCH`, or any
/// error surfaced by the fingerprint cross-check.
#[cfg(unix)]
pub fn signal_identity(identity: &ProcessIdentity, kind: SignalDelivery) -> Result<bool> {
    if !process_is_alive_with_start_time(identity)? {
        return Ok(false);
    }
    let raw_kind = match kind {
        SignalDelivery::Term => SignalKind::Term,
        SignalDelivery::Kill => SignalKind::Kill,
    };
    send(identity.pid.as_raw(), raw_kind)?;
    Ok(true)
}

#[cfg(not(unix))]
pub fn signal_identity(_identity: &ProcessIdentity, _kind: SignalDelivery) -> Result<bool> {
    Err(ProcessError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "signal_identity is unix-only",
    )))
}

/// Public wrapper over [`SignalKind`] for callers of [`signal_identity`].
#[derive(Clone, Copy, Debug)]
pub enum SignalDelivery {
    /// `SIGTERM`.
    Term,
    /// `SIGKILL`.
    Kill,
}
