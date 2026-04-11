//! Process-control helpers shared across triggers and queue backends that
//! manage long-lived subprocesses.

/// Send `SIGTERM` to a child process (Unix) or fall back to
/// [`tokio::process::Child::start_kill`] elsewhere, then await its exit.
///
/// Used by long-lived subprocess hosts ([`ExternalTrigger`](super::super::external_trigger::ExternalTrigger),
/// the shell queue) so cancellation gives the child a chance to flush state
/// before the wait completes.
#[cfg(unix)]
#[allow(dead_code, unsafe_code)]
pub async fn terminate_child(child: &mut tokio::process::Child) {
    if let Some(id) = child.id() {
        // SAFETY: `kill` is a syscall wrapper that is safe to call with any
        // pid/sig combination from any thread.
        unsafe {
            libc::kill(libc::pid_t::try_from(id).unwrap_or(0), libc::SIGTERM);
        }
    }
    drop(child.wait().await);
}

/// Windows fallback for [`terminate_child`].
#[cfg(not(unix))]
#[allow(dead_code)]
pub async fn terminate_child(child: &mut tokio::process::Child) {
    let _ = child.start_kill();
    drop(child.wait().await);
}
