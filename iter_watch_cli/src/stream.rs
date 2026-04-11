//! `cli_println!` / `cli_eprintln!` macros and the shared I/O error policy.
//!
//! These macros write to a locked stdout/stderr handle and silently drop
//! `BrokenPipe`. Every other I/O error panics — the same behaviour as
//! `println!` — which matters for ID-emitting paths where silently
//! dropping a write while still exiting `0` would corrupt downstream
//! automation.
//!
//! Diagnostics belong on `tracing::*`. Use these macros only for output
//! that is part of the CLI's user-visible contract.

#[doc(hidden)]
#[inline]
pub(crate) fn handle_io_result(result: ::std::io::Result<()>) {
    match result {
        Ok(()) => {}
        Err(err) if err.kind() == ::std::io::ErrorKind::BrokenPipe => {}
        Err(err) => panic!("CLI write failed: {err}"),
    }
}

#[allow(unused_macros)]
macro_rules! cli_println {
    () => {{
        use ::std::io::Write as _;
        let stdout = ::std::io::stdout();
        let mut handle = stdout.lock();
        $crate::stream::handle_io_result(::std::writeln!(handle));
    }};
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let stdout = ::std::io::stdout();
        let mut handle = stdout.lock();
        $crate::stream::handle_io_result(::std::writeln!(handle, $($arg)*));
    }};
}

macro_rules! cli_eprintln {
    () => {{
        use ::std::io::Write as _;
        let stderr = ::std::io::stderr();
        let mut handle = stderr.lock();
        $crate::stream::handle_io_result(::std::writeln!(handle));
    }};
    ($($arg:tt)*) => {{
        use ::std::io::Write as _;
        let stderr = ::std::io::stderr();
        let mut handle = stderr.lock();
        $crate::stream::handle_io_result(::std::writeln!(handle, $($arg)*));
    }};
}

#[allow(unused_imports)]
pub(crate) use cli_eprintln;
#[allow(unused_imports)]
pub(crate) use cli_println;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    #[test]
    fn ok_passes_through() {
        handle_io_result(Ok(()));
    }

    #[test]
    fn broken_pipe_is_swallowed() {
        handle_io_result(Err(Error::new(ErrorKind::BrokenPipe, "downstream gone")));
    }

    #[test]
    #[should_panic(expected = "CLI write failed")]
    fn other_errors_panic() {
        handle_io_result(Err(Error::new(ErrorKind::PermissionDenied, "nope")));
    }
}
