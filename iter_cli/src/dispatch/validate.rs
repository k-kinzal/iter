//! `iter validate` — parse and semantic-check an Iterfile or `compose.iter`,
//! then exit. The file kind is detected from its basename so the same
//! command works for both formats.

use std::path::Path;

use crate::dispatch::compose::{ValidateAutodetectError, validate_path_autodetect};
use crate::output::ValidateFormat;

/// Errors produced by [`run_validate`].
pub type ValidateCmdError = ValidateAutodetectError;

/// Validate the file at `path` (or `./Iterfile` when `None`).
///
/// Auto-detects whether the target is an Iterfile or a `compose.iter`
/// based on its basename, then runs the corresponding validator. Prints
/// "OK" via the underlying validator on success.
///
/// # Errors
///
/// Returns the rendered diagnostics on failure.
pub fn run_validate(path: Option<&Path>, format: ValidateFormat) -> Result<(), ValidateCmdError> {
    validate_path_autodetect(path, format)
}
