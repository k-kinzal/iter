//! `cli_println!` / `cli_eprintln!` macros and the shared I/O error policy.
//!
//! These macros write to a locked stdout/stderr handle and silently drop
//! `BrokenPipe` so that `iter ps | head` never panics when the consumer
//! closes its end of the pipe. Every other I/O error (`ENOSPC`, closed
//! descriptor, permission denied, …) is reported and exits non-zero. That
//! asymmetry matters for ID-emitting paths like
//! `iter run --detach` and `iter enqueue`, where a script captures the
//! emitted ULID via `ID=$(iter run --detach Iterfile)`. Silently dropping a
//! write while still exiting `0` would corrupt downstream automation.
//!
//! Diagnostics belong on `tracing::*`. Use these macros only for output
//! that is part of the CLI's user-visible contract.

/// Internal helper used by the macros. Discards `ErrorKind::BrokenPipe`
/// and returns every other error.
#[doc(hidden)]
#[inline]
pub(crate) fn handle_io_result(result: ::std::io::Result<()>) -> ::std::io::Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ::std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(err) => Err(err),
    }
}

#[doc(hidden)]
pub(crate) fn fail_cli_write(err: ::std::io::Error) -> ! {
    use ::std::io::Write as _;

    let stderr = ::std::io::stderr();
    let mut handle = stderr.lock();
    drop(::std::writeln!(handle, "CLI write failed: {err}"));
    ::std::process::exit(1);
}

/// Like `println!`, but writes to a locked stdout handle and silently
/// ignores `BrokenPipe`. Other I/O errors are reported and exit non-zero.
///
/// Reserved for stdout — the data channel of every iter binary. Do not
/// emit chatter (progress, status, banners) through this macro; that
/// belongs on `cli_eprintln!`.
macro_rules! cli_println {
    () => {{
        use ::std::io::Write as _;
        let stdout = ::std::io::stdout();
        let mut handle = stdout.lock();
        if let Err(err) = $crate::output::stream::handle_io_result(::std::writeln!(handle)) {
            $crate::output::stream::fail_cli_write(err);
        }
    }};
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let stdout = ::std::io::stdout();
        let mut handle = stdout.lock();
        if let Err(err) = $crate::output::stream::handle_io_result(::std::writeln!(handle, $($arg)*)) {
            $crate::output::stream::fail_cli_write(err);
        }
    }};
}

/// Like `eprintln!`, but writes to a locked stderr handle and silently
/// ignores `BrokenPipe`. Other I/O errors are reported and exit non-zero.
///
/// Reserved for stderr — the chatter channel of every iter binary
/// (progress, warnings, banners, status confirmations).
macro_rules! cli_eprintln {
    () => {{
        use ::std::io::Write as _;
        let stderr = ::std::io::stderr();
        let mut handle = stderr.lock();
        if let Err(err) = $crate::output::stream::handle_io_result(::std::writeln!(handle)) {
            $crate::output::stream::fail_cli_write(err);
        }
    }};
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let stderr = ::std::io::stderr();
        let mut handle = stderr.lock();
        if let Err(err) = $crate::output::stream::handle_io_result(::std::writeln!(handle, $($arg)*)) {
            $crate::output::stream::fail_cli_write(err);
        }
    }};
}

pub(crate) use cli_eprintln;
pub(crate) use cli_println;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    #[test]
    fn ok_passes_through() {
        handle_io_result(Ok(())).unwrap();
    }

    #[test]
    fn broken_pipe_is_swallowed() {
        handle_io_result(Err(Error::new(ErrorKind::BrokenPipe, "downstream gone"))).unwrap();
    }

    #[test]
    fn other_errors_are_returned() {
        let err = handle_io_result(Err(Error::new(ErrorKind::PermissionDenied, "nope")))
            .expect_err("permission denied should be returned");
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
    }
}
