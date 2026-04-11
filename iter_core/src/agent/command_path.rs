//! [`CommandPath`] — the on-disk location of an agent binary.
//!
//! Agents accept a user-supplied `command` string (e.g. `"claude"`,
//! `"/opt/bin/claude"`, `"./wrapper"`). The sandbox layer needs to grant
//! read access to the resolved binary *and* to its canonical target when
//! the user hands us a symlink / shim (volta, nvm, asdf, homebrew cask).
//! This module owns that resolution — agents call [`CommandPath::resolve`]
//! and expose the result via their own path accessors.

use std::path::{Path, PathBuf};

/// A command name or path that has been resolved against the filesystem.
///
/// Holds the resolved path produced by PATH lookup (or verbatim for an
/// absolute / explicitly-relative input) together with its canonicalized
/// form when the two differ — the sandbox backend needs both so a symlink
/// target can be mapped into the child process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandPath {
    /// The path `execvp` would actually open — a literal filename on
    /// `PATH` or the user-supplied absolute/relative path.
    resolved: PathBuf,
    /// Canonical target when [`resolved`](Self::resolved) is a symlink.
    /// `None` when the resolved path is already canonical.
    canonical: Option<PathBuf>,
}

impl CommandPath {
    /// Resolve `command` into an absolute filesystem path.
    ///
    /// * Absolute or explicitly-relative (`./foo`, `bin/foo`) commands are
    ///   returned as-is when they point at an existing regular file.
    /// * Name-only commands (`"claude"`) are looked up through `PATH`.
    ///
    /// Returns `None` when nothing on disk matches — the sandbox profile
    /// then simply omits a binary allowance and the eventual `execvp`
    /// surfaces the "No such file or directory" error at its natural site.
    #[must_use]
    pub fn resolve(command: &str) -> Option<Self> {
        let p = Path::new(command);
        let resolved = if p.is_absolute() || command.contains(std::path::MAIN_SEPARATOR) {
            if p.is_file() {
                p.to_path_buf()
            } else {
                return None;
            }
        } else {
            let path_env = std::env::var_os("PATH")?;
            std::env::split_paths(&path_env)
                .map(|entry| entry.join(command))
                .find(|candidate| candidate.is_file())?
        };

        let canonical = std::fs::canonicalize(&resolved)
            .ok()
            .filter(|c| *c != resolved);

        Some(Self {
            resolved,
            canonical,
        })
    }

    /// Paths the sandbox must grant read access to so the binary can be
    /// `execve`'d. Both the resolved path (may be a symlink shim) and the
    /// canonical target appear when they differ; otherwise just the
    /// resolved path.
    #[must_use]
    pub fn reads(&self) -> Vec<PathBuf> {
        let mut out = Vec::with_capacity(2);
        out.push(self.resolved.clone());
        if let Some(c) = self.canonical.as_ref() {
            out.push(c.clone());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn resolve_absolute_existing_file() {
        let tmp = TempDir::new().expect("tmp");
        let bin = tmp.path().join("fake-cmd");
        std::fs::write(&bin, b"#!/bin/sh\nexit 0\n").expect("write");
        let cp = CommandPath::resolve(bin.to_str().unwrap()).expect("resolve");
        assert!(cp.reads().iter().any(|p| p == &bin));
    }

    #[test]
    fn resolve_name_via_path() {
        let tmp = TempDir::new().expect("tmp");
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("mkdir");
        let bin = bin_dir.join("command-path-probe");
        std::fs::write(&bin, b"#!/bin/sh\nexit 0\n").expect("write");

        let saved = std::env::var_os("PATH");
        // SAFETY: this test mutates process env; tests in this module
        // run serially under cargo's default scheduler for a single
        // binary. PATH is restored before return.
        unsafe {
            std::env::set_var("PATH", bin_dir.as_os_str());
        }
        let cp = CommandPath::resolve("command-path-probe").expect("resolve");
        unsafe {
            match saved {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
        }
        assert!(cp.reads().iter().any(|p| p == &bin));
    }

    #[test]
    fn resolve_missing_returns_none() {
        assert!(CommandPath::resolve("/definitely/not/here/absent-binary-xyz").is_none());
    }

    #[test]
    fn reads_include_canonical_target_behind_symlink() {
        let tmp = TempDir::new().expect("tmp");
        let target = tmp.path().join("real-cmd");
        std::fs::write(&target, b"#!/bin/sh\nexit 0\n").expect("write target");
        let symlink = tmp.path().join("cmd-link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &symlink).expect("symlink");
        #[cfg(not(unix))]
        std::fs::copy(&target, &symlink).expect("copy fallback");

        let cp = CommandPath::resolve(symlink.to_str().unwrap()).expect("resolve");
        let reads = cp.reads();
        assert!(reads.iter().any(|p| p == &symlink));
        #[cfg(unix)]
        {
            let canonical = std::fs::canonicalize(&target).expect("canonicalize");
            assert!(reads.iter().any(|p| p == &canonical));
        }
    }
}
