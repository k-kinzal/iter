//! The base-directory helper — where the user's home is.
//!
//! Resolving the user's home directory is **mechanism**, on a par with
//! reading the standard `OTEL_*` env contract: it answers "where on this
//! host does persistent state live", nothing about iter's domain. This is
//! the single place in `iter_core` that reads `$HOME`; every piece that
//! legitimately writes under the home — process records' `proc` paths, the
//! per-workspace stop-hook installs, a driver's `~/.claude` lookup — routes
//! through it.
//!
//! *What* lives under `~/.iter` is operator layout policy and is owned by
//! the CLI, not here. This helper only answers *where the home is*.

use std::path::PathBuf;

/// Resolve the user's home directory from `$HOME`.
///
/// Returns `None` when `$HOME` is unset or empty. Callers that need a
/// subpath compose with [`Path::join`](std::path::Path::join):
///
/// ```ignore
/// let claude = iter_core::home::home_dir().map(|h| h.join(".claude"));
/// ```
#[must_use]
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}
