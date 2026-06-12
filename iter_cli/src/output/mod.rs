//! User-facing output helpers for the `iter` binary.
//!
//! Every byte the binary writes to stdout/stderr flows through one of
//! the modules here:
//!
//! - [`mod@stream`]   — `cli_println!` / `cli_eprintln!` macros and the
//!   shared `BrokenPipe` policy.
//! - [`mod@error`]    — `IntoExitCode`, `print_error`, `run_main`.
//! - [`mod@format`]   — `OutputFormat`, `ValidateFormat`, JSON/NDJSON helpers.
//! - [`mod@table`]    — `Table` (tabwriter-backed elastic table renderer).
//! - [`mod@time`]     — `relative_time` for the human ps view.
//! - [`mod@id`]       — `trunc_id` for ULID truncation.
//! - [`mod@listing`]  — `ListingArgs` clap fragment (`-q --format --no-trunc`).

pub(crate) mod error;
pub(crate) mod format;
pub(crate) mod id;
pub(crate) mod listing;
pub(crate) mod stream;
pub(crate) mod table;
pub(crate) mod time;

pub(crate) use error::{IntoExitCode, exit_codes, run_main};
pub(crate) use format::{
    OutputFormat, ValidateFormat, ValidateOk, ValidateSummary, print_json_array,
    print_json_compact, print_json_pretty, print_ndjson_record,
};
pub(crate) use id::trunc_id;
pub(crate) use listing::ListingArgs;
pub(crate) use stream::{cli_eprintln, cli_println};
pub(crate) use table::Table;
pub(crate) use time::relative_time;
