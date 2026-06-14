//! Error rendering and exit-code policy for the `iter` binary.
//!
//! The Rust runtime's default `Result<(), E>` propagation through `main`
//! prints `Debug::fmt(&err)`, which produces noisy multi-line dumps for
//! any error type with sources. The CLI requires a single-line headline
//! with an indented `caused by:` chain instead.
//!
//! `main` therefore wraps its real entry point with [`run_main`], which
//! calls the closure, renders any error through [`print_error`], maps it
//! to a stable exit code via [`IntoExitCode`], and exits the process
//! explicitly — bypassing the runtime's `Debug::fmt` path.

use std::error::Error;

use super::stream::cli_eprintln;

/// Stable exit codes used by the binary.
pub(crate) mod exit_codes {
    pub(crate) const SUCCESS: i32 = 0;
    pub(crate) const USER_INPUT: i32 = 1;
    pub(crate) const RUNTIME: i32 = 2;
    pub(crate) const CONFIG: i32 = 64;
    pub(crate) const INTERNAL: i32 = 125;
    pub(crate) const SIGINT: i32 = 130;
}

/// An error type that can map itself to a process exit code.
///
/// Default exit code is [`exit_codes::USER_INPUT`] (1). Override per
/// variant in the implementing type to give operators a predictable
/// signal for what kind of failure happened.
pub(crate) trait IntoExitCode {
    fn exit_code(&self) -> i32 {
        exit_codes::USER_INPUT
    }
}

/// Render an error and its full source chain to stderr.
///
/// Format:
///
/// ```text
/// Error: <Display of err>
///   caused by: <Display of err.source()>
///   caused by: <Display of err.source().source()>
/// ```
pub(crate) fn print_error(err: &(dyn Error + 'static)) {
    cli_eprintln!("Error: {err}");
    let mut source = err.source();
    while let Some(cause) = source {
        cli_eprintln!("  caused by: {cause}");
        source = cause.source();
    }
}

/// Run the binary's real entry point and translate its `Result` into a
/// rendered error + explicit exit code.
pub(crate) fn run_main<E, F>(f: F) -> !
where
    E: IntoExitCode + Error + 'static,
    F: FnOnce() -> Result<(), E>,
{
    match f() {
        Ok(()) => std::process::exit(exit_codes::SUCCESS),
        Err(err) => {
            let code = err.exit_code();
            print_error(&err);
            std::process::exit(code);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use thiserror::Error;

    #[derive(Debug, Error)]
    enum TestErr {
        #[error("outer failure")]
        WithSource(#[source] Inner),
        #[error("standalone failure")]
        Standalone,
    }

    #[derive(Debug, Error)]
    #[error("inner failure: {0}")]
    struct Inner(String);

    impl IntoExitCode for TestErr {
        fn exit_code(&self) -> i32 {
            match self {
                Self::WithSource(_) => exit_codes::RUNTIME,
                Self::Standalone => exit_codes::USER_INPUT,
            }
        }
    }

    #[test]
    fn default_exit_code_is_user_input() {
        #[derive(Debug, Error)]
        #[error("default")]
        struct DefaultErr;
        impl IntoExitCode for DefaultErr {}
        assert_eq!(DefaultErr.exit_code(), exit_codes::USER_INPUT);
    }

    #[test]
    fn exit_code_overrides_apply() {
        assert_eq!(TestErr::Standalone.exit_code(), exit_codes::USER_INPUT);
        let with_src = TestErr::WithSource(Inner("disk".into()));
        assert_eq!(with_src.exit_code(), exit_codes::RUNTIME);
    }

    #[test]
    fn print_error_does_not_panic_on_chain() {
        let err = TestErr::WithSource(Inner("disk".into()));
        print_error(&err);
    }
}
