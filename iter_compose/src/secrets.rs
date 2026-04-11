//! Secret resolution shared across compose builders.
//!
//! Every backend whose Iterfile schema includes a `SecretExpr`-typed field —
//! webhook secrets, Redis passwords, AWS access keys, Kafka SASL passwords,
//! Azure connection strings, etc. — funnels through [`resolve_secret`] so the
//! literal-vs-`env(VAR)` distinction is handled in one place.

use std::path::PathBuf;

use iter_language::SecretExpr;

/// Errors produced while resolving a [`SecretExpr`].
#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    /// Reading the named environment variable failed (missing or non-Unicode).
    #[error("reading secret from env var {name}: {source}")]
    Env {
        /// Name of the environment variable that could not be read.
        name: String,
        /// Underlying [`std::env::VarError`].
        #[source]
        source: std::env::VarError,
    },
    /// Reading the secret file failed.
    #[error("reading secret from file {}: {source}", path.display())]
    File {
        /// Path of the secret file that could not be read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// Resolve a [`SecretExpr`] to its concrete string value.
///
/// `SecretExpr::Literal` returns the inline string verbatim.
/// `SecretExpr::EnvVar(name)` reads from the process environment and surfaces
/// a clear error if the variable is missing or not valid Unicode.
/// `SecretExpr::File(path)` reads and trims the file contents.
///
/// # Errors
///
/// Returns [`SecretsError`] when the named env var or file cannot be read.
pub fn resolve_secret(secret: &SecretExpr) -> Result<String, SecretsError> {
    match secret {
        SecretExpr::Literal(s) => Ok(s.clone()),
        SecretExpr::EnvVar(name) => std::env::var(name).map_err(|source| SecretsError::Env {
            name: name.clone(),
            source,
        }),
        SecretExpr::File(path) => std::fs::read_to_string(path)
            .map(|s| s.trim().to_owned())
            .map_err(|source| SecretsError::File {
                path: path.clone(),
                source,
            }),
    }
}
