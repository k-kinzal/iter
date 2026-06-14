//! Output format selection for list / inspect / validate subcommands.
//!
//! Listing-style commands accept `--format <table|json>`; inspect-style
//! commands default to JSON; validate accepts `--format <text|json>`.
//! All three flag types live here so the clap surface stays uniform
//! across resources.

use clap::ValueEnum;
use serde::Serialize;

use super::stream::cli_println;

/// Format selector for listing-style subcommands (`ls`, `ps`).
///
/// `Table` is curated for human consumption; `Json` emits one record
/// per line (NDJSON) so the output is composable with `jq`, `xargs`,
/// and shell pipelines.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum OutputFormat {
    /// Human-readable elastic-tab table (default).
    #[default]
    Table,
    /// Newline-delimited JSON: one record per line.
    Json,
}

/// Format selector for `iter validate` / `iter compose validate`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ValidateFormat {
    /// Human-readable single-line summary (default).
    #[default]
    Text,
    /// JSON object with `ok` flag and a `summary` block.
    Json,
}

/// Validate-summary envelope shared by `iter validate --format json` and
/// `iter compose validate --format json`.
#[derive(Debug, Serialize)]
pub(crate) struct ValidateOk {
    pub(crate) ok: bool,
    pub(crate) summary: ValidateCounts,
}

/// The `summary` block of [`ValidateOk`].
#[derive(Debug, Serialize)]
pub(crate) struct ValidateCounts {
    pub(crate) queues: usize,
    pub(crate) services: usize,
    pub(crate) triggers: usize,
}

/// Pretty-print a single value as JSON to stdout.
pub(crate) fn print_json_pretty<T: Serialize>(value: &T) -> Result<(), serde_json::Error> {
    let body = serde_json::to_string_pretty(value)?;
    cli_println!("{body}");
    Ok(())
}

/// Print one record as a single line of NDJSON.
pub(crate) fn print_ndjson_record<T: Serialize>(value: &T) -> Result<(), serde_json::Error> {
    let body = serde_json::to_string(value)?;
    cli_println!("{body}");
    Ok(())
}

/// Print a single JSON document compactly (no indentation, single line).
pub(crate) fn print_json_compact<T: Serialize>(value: &T) -> Result<(), serde_json::Error> {
    let body = serde_json::to_string(value)?;
    cli_println!("{body}");
    Ok(())
}

/// Pretty-print a JSON array (single document, multi-line).
pub(crate) fn print_json_array<T: Serialize>(values: &[T]) -> Result<(), serde_json::Error> {
    let body = serde_json::to_string_pretty(values)?;
    cli_println!("{body}");
    Ok(())
}
