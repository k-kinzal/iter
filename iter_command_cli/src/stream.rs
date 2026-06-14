//! `cli_println!` / `cli_eprintln!` macros and the shared I/O error policy.
//!
//! These macros write to a locked stdout/stderr handle and silently drop
//! `BrokenPipe`. Every other I/O error is reported and exits non-zero,
//! which matters for ID-emitting paths where silently dropping a write
//! while still exiting `0` would corrupt downstream automation.
//!
//! Diagnostics belong on `tracing::*`. Use these macros only for output
//! that is part of the CLI's user-visible contract.

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

macro_rules! cli_println {
    () => {{
        use ::std::io::Write as _;
        let stdout = ::std::io::stdout();
        let mut handle = stdout.lock();
        if let Err(err) = $crate::stream::handle_io_result(::std::writeln!(handle)) {
            $crate::stream::fail_cli_write(err);
        }
    }};
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let stdout = ::std::io::stdout();
        let mut handle = stdout.lock();
        if let Err(err) = $crate::stream::handle_io_result(::std::writeln!(handle, $($arg)*)) {
            $crate::stream::fail_cli_write(err);
        }
    }};
}

macro_rules! cli_eprintln {
    () => {{
        use ::std::io::Write as _;
        let stderr = ::std::io::stderr();
        let mut handle = stderr.lock();
        if let Err(err) = $crate::stream::handle_io_result(::std::writeln!(handle)) {
            $crate::stream::fail_cli_write(err);
        }
    }};
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let stderr = ::std::io::stderr();
        let mut handle = stderr.lock();
        if let Err(err) = $crate::stream::handle_io_result(::std::writeln!(handle, $($arg)*)) {
            $crate::stream::fail_cli_write(err);
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
