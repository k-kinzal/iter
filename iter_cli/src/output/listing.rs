//! Shared clap fragment for listing-style subcommands.
//!
//! Every `ls` / `ps` flow accepts the same vocabulary:
//!
//! - `-q / --quiet` — emit ID-only output suitable for scripting.
//! - `--format <table|json>` — select the human or machine view.
//! - `--no-trunc` — disable ULID truncation in the human view.

use clap::Args;

use super::format::OutputFormat;

/// Listing flags shared by `iter process ls` (alias `iter ps`),
/// `iter compose ls` (alias `iter compose ps`), and any future
/// listing-style subcommand.
#[derive(Debug, Default, Clone, Copy, Args)]
pub(crate) struct ListingArgs {
    /// Print one record per line in a compact, scripting-friendly form.
    /// Exact shape is subcommand-specific; see the subcommand's `--help`.
    #[arg(short, long)]
    pub(crate) quiet: bool,

    /// Output format.
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    pub(crate) format: OutputFormat,

    /// Disable ULID truncation in the human view.
    #[arg(long = "no-trunc")]
    pub(crate) no_trunc: bool,
}
