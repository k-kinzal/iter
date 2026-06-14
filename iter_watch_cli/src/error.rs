//! Error rendering and exit-code policy.
//!
//! The Rust runtime's default `Result<(), E>` propagation through `main`
//! prints `Debug::fmt(&err)`, producing noisy multi-line dumps for any
//! error type with sources. We render a single-line headline with an
//! indented `caused by:` chain instead.
//!
//! The binary wraps its real entry point with [`run_main`], which calls
//! the closure, renders any error through [`print_error`], maps it to a
//! stable exit code via [`IntoExitCode`], and exits the process
//! explicitly.

use std::error::Error;

use crate::stream::cli_eprintln;

pub(crate) mod exit_codes {
    pub(crate) const SUCCESS: i32 = 0;
    pub(crate) const USER_INPUT: i32 = 1;
    pub(crate) const RUNTIME: i32 = 2;
    pub(crate) const CONFIG: i32 = 64;
    pub(crate) const INTERNAL: i32 = 125;
    pub(crate) const SIGINT: i32 = 130;
}

pub(crate) trait IntoExitCode {
    fn exit_code(&self) -> i32 {
        exit_codes::USER_INPUT
    }
}

pub(crate) fn print_error(err: &(dyn Error + 'static)) {
    cli_eprintln!("Error: {err}");
    let mut source = err.source();
    while let Some(cause) = source {
        cli_eprintln!("  caused by: {cause}");
        source = cause.source();
    }
}

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
