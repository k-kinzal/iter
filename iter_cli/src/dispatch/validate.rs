//! `iter validate` — parse and semantic-check an Iterfile, then exit.
//!
//! Compose files are `iter compose validate`'s deliverable; this verb
//! validates Iterfiles only, whatever the file is named. One compatibility
//! carve-out remains: the canonical basename `compose.iter` delegates to
//! the compose validator (with a stderr note) so the historical
//! `iter validate compose.iter` spelling keeps working.

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::DEFAULT_COMPOSE_FILE;
use crate::cli::ComposeValidateArgs;
use crate::dispatch::compose::{ComposePlanError, run_compose_validate};
use crate::dispatch::load::{DEFAULT_ITERFILE, LoadError, load_iterfile};
use crate::output::{
    IntoExitCode, ValidateCounts, ValidateFormat, ValidateOk, cli_eprintln, cli_println,
    exit_codes, print_json_compact,
};

/// Errors produced by [`run_validate`].
#[derive(Debug, Error)]
pub(crate) enum ValidateCmdError {
    /// Validating an Iterfile failed.
    #[error(transparent)]
    Load(#[from] LoadError),
    /// Delegated `compose.iter` validation failed.
    #[error("validating compose file at {}: {source}", path.display())]
    Compose {
        /// Resolved compose file path.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: Box<ComposePlanError>,
    },
    /// Serialising the validate-JSON envelope failed.
    #[error("serializing validate output: {0}")]
    JsonSerialize(#[source] serde_json::Error),
}

impl IntoExitCode for ValidateCmdError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::Load(e) => e.exit_code(),
            Self::Compose { source, .. } => source.exit_code(),
            Self::JsonSerialize(_) => exit_codes::INTERNAL,
        }
    }
}

/// Validate the Iterfile at `path` (or `./Iterfile` when `None`).
///
/// The file is always parsed as an Iterfile, whatever it is named, with
/// one exception: the exact basename `compose.iter` delegates to the
/// `iter compose validate` code path and prints a stderr note saying so.
/// The delegated exit code and `--format` output are the compose
/// validator's.
///
/// # Errors
///
/// Returns the rendered diagnostics on failure. When the leading
/// diagnostic reports a compose-only construct, a hint pointing at
/// `iter compose validate -f` is appended.
pub(crate) fn run_validate(
    path: Option<&Path>,
    format: ValidateFormat,
) -> Result<(), ValidateCmdError> {
    let resolved = match path {
        Some(p) => p.to_path_buf(),
        None => PathBuf::from(DEFAULT_ITERFILE),
    };
    if resolved.file_name().and_then(|n| n.to_str()) == Some(DEFAULT_COMPOSE_FILE) {
        cli_eprintln!("note: compose files are validated by 'iter compose validate'; delegating");
        return run_compose_validate(&ComposeValidateArgs {
            file: Some(resolved.clone()),
            format,
        })
        .map_err(|source| ValidateCmdError::Compose {
            path: resolved,
            source: Box::new(source),
        });
    }

    let loaded = load_iterfile(Some(&resolved)).map_err(|err| compose_hinted(err, &resolved))?;
    match format {
        ValidateFormat::Text => cli_println!("OK"),
        ValidateFormat::Json => {
            let envelope = ValidateOk {
                ok: true,
                summary: ValidateCounts {
                    queues: loaded.iterfile.queues.len(),
                    services: 1,
                    triggers: 0,
                },
            };
            print_json_compact(&envelope).map_err(ValidateCmdError::JsonSerialize)?;
        }
    }
    Ok(())
}

/// Append the compose pointer when Iterfile parsing tripped over a
/// compose-only construct, so a compose-format file under a non-canonical
/// name fails with guidance instead of a bare grammar error.
fn compose_hinted(err: LoadError, path: &Path) -> ValidateCmdError {
    match err {
        LoadError::Parse {
            rendered,
            compose_section: true,
        } => ValidateCmdError::Load(LoadError::Parse {
            rendered: format!(
                "{rendered}\nhint: this looks like a compose file; \
                 use 'iter compose validate -f {}'",
                path.display()
            ),
            compose_section: true,
        }),
        other => ValidateCmdError::Load(other),
    }
}
