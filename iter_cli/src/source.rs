//! Source declaration adapter and process-record pending decision helpers.

use std::path::{Path, PathBuf};

use iter_core::process::ProcessRecord;
use iter_core::source::{
    DirectorySourceSpec, GitFastForwardSpec, GitLocatorSpec, GitSourceSpec, PendingSourceDecision,
    SourceDeriveSpec, SourceDispositionSpec, SourceError, SourceSpec,
};
use iter_language::{
    GitFastForward, GitLocator, SourceDef, SourceDerive, SourceDisposition, WorkspaceDef,
    WorkspaceSourceRef,
};
use thiserror::Error;

/// File storing a deferred source decision under a proc record.
pub const PENDING_SOURCE_FILE: &str = "pending-source.json";

/// Source adapter errors.
#[derive(Debug, Error)]
pub enum SourceBuildError {
    /// Workspace references a named source that does not exist.
    #[error("workspace references source `{name}` which is not defined")]
    UnknownSource {
        /// Source name.
        name: String,
    },
    /// Source runtime failed.
    #[error(transparent)]
    Runtime(#[from] SourceError),
    /// Pending decision JSON failed to serialise.
    #[error("serialising pending source decision: {0}")]
    JsonWrite(#[source] serde_json::Error),
    /// Pending decision JSON failed to parse.
    #[error("reading pending source decision: {0}")]
    JsonRead(#[source] serde_json::Error),
    /// Pending decision I/O failed.
    #[error("pending source decision I/O: {0}")]
    Io(#[from] std::io::Error),
    /// No pending decision exists on the process record.
    #[error("process has no pending source decision")]
    NoPendingDecision,
    /// Deferred source disposition requires a process record.
    #[error("deferred source disposition requires a process record")]
    NoProcessRecord,
}

/// A provisioned source attached to a runner.
pub struct ActiveSource {
    source: Box<dyn iter_core::source::Source>,
    provisioned: iter_core::source::ProvisionedSource,
}

impl ActiveSource {
    /// Dispose after runner finish.
    pub async fn dispose(self) -> Result<Option<PendingSourceDecision>, SourceBuildError> {
        self.source
            .dispose(self.provisioned)
            .await
            .map_err(SourceBuildError::Runtime)
    }
}

/// Return the workspace source reference, if any.
#[must_use]
pub fn workspace_source_ref(workspace: &WorkspaceDef) -> Option<&WorkspaceSourceRef> {
    match workspace {
        WorkspaceDef::Local { source, .. }
        | WorkspaceDef::Clone { source, .. }
        | WorkspaceDef::Sandbox { source, .. } => source.as_ref(),
    }
}

/// Return a clone of `workspace` with a concrete base path and no source reference.
#[must_use]
pub fn workspace_with_base(workspace: &WorkspaceDef, base: &Path) -> WorkspaceDef {
    let base = base.display().to_string();
    match workspace {
        WorkspaceDef::Local { .. } => WorkspaceDef::Local { base, source: None },
        WorkspaceDef::Clone {
            remote,
            excludes,
            includes,
            preserve_mtime,
            apply_back,
            ..
        } => WorkspaceDef::Clone {
            base,
            source: None,
            remote: remote.clone(),
            excludes: excludes.clone(),
            includes: includes.clone(),
            preserve_mtime: *preserve_mtime,
            apply_back: apply_back.clone(),
        },
        WorkspaceDef::Sandbox {
            excludes,
            includes,
            preserve_mtime,
            apply_back,
            policy,
            ..
        } => WorkspaceDef::Sandbox {
            base,
            source: None,
            excludes: excludes.clone(),
            includes: includes.clone(),
            preserve_mtime: *preserve_mtime,
            apply_back: apply_back.clone(),
            policy: policy.clone(),
        },
    }
}

/// Provision the source referenced by a workspace, returning a concrete workspace def.
pub async fn provision_for_workspace(
    workspace: &WorkspaceDef,
    sources: &[iter_language::Spanned<iter_language::NamedDef<SourceDef>>],
) -> Result<(WorkspaceDef, Option<ActiveSource>), SourceBuildError> {
    let Some(source_ref) = workspace_source_ref(workspace) else {
        return Ok((workspace.clone(), None));
    };
    match source_ref {
        WorkspaceSourceRef::Path(path) => {
            Ok((workspace_with_base(workspace, Path::new(path)), None))
        }
        WorkspaceSourceRef::Named(name) => {
            let source_decl = sources
                .iter()
                .find(|source| source.node.name == *name)
                .map(|source| &source.node.decl)
                .ok_or_else(|| SourceBuildError::UnknownSource { name: name.clone() })?;
            let spec = source_spec_from_def(source_decl);
            let source = iter_core::source::source_from_spec(spec);
            let provisioned = source.provision().await?;
            let workspace = workspace_with_base(workspace, &provisioned.base_path);
            Ok((
                workspace,
                Some(ActiveSource {
                    source,
                    provisioned,
                }),
            ))
        }
    }
}

/// Convert a language source declaration to a core runtime source spec.
#[must_use]
pub fn source_spec_from_def(def: &SourceDef) -> SourceSpec {
    match def {
        SourceDef::Directory {
            path,
            derive,
            disposition,
        } => SourceSpec::Directory(DirectorySourceSpec {
            path: PathBuf::from(path),
            derive: derive_spec_from_def(derive),
            disposition: disposition.as_ref().map(disposition_spec_from_def),
        }),
        SourceDef::Git {
            locator,
            derive,
            disposition,
        } => SourceSpec::Git(GitSourceSpec {
            locator: match locator {
                GitLocator::Url(url) => GitLocatorSpec::Url(url.clone()),
                GitLocator::Path(path) => GitLocatorSpec::Path(PathBuf::from(path)),
            },
            derive: derive_spec_from_def(derive),
            disposition: disposition_spec_from_def(disposition),
        }),
    }
}

fn derive_spec_from_def(def: &SourceDerive) -> SourceDeriveSpec {
    match def {
        SourceDerive::Passthrough => SourceDeriveSpec::Passthrough,
        SourceDerive::Copy {
            excludes,
            preserve_mtime,
        } => SourceDeriveSpec::Copy {
            excludes: excludes.clone(),
            preserve_mtime: *preserve_mtime,
        },
        SourceDerive::Worktree { ref_name, branch } => SourceDeriveSpec::Worktree {
            ref_name: ref_name.clone(),
            branch: branch.clone(),
        },
        SourceDerive::Clone {
            ref_name,
            branch,
            depth,
        } => SourceDeriveSpec::Clone {
            ref_name: ref_name.clone(),
            branch: branch.clone(),
            depth: *depth,
        },
    }
}

fn disposition_spec_from_def(def: &SourceDisposition) -> SourceDispositionSpec {
    match def {
        SourceDisposition::Discard => SourceDispositionSpec::Discard,
        SourceDisposition::Merge {
            excludes,
            includes,
            into,
            ff,
        } => SourceDispositionSpec::Merge {
            excludes: excludes.clone(),
            includes: includes.clone(),
            into: into.clone(),
            ff: ff.map(ff_spec_from_def),
        },
        SourceDisposition::Sync { excludes, includes } => SourceDispositionSpec::Sync {
            excludes: excludes.clone(),
            includes: includes.clone(),
        },
        SourceDisposition::Defer { promote } => SourceDispositionSpec::Defer {
            promote: Box::new(disposition_spec_from_def(promote)),
        },
    }
}

fn ff_spec_from_def(def: GitFastForward) -> GitFastForwardSpec {
    match def {
        GitFastForward::Allow => GitFastForwardSpec::Allow,
        GitFastForward::Only => GitFastForwardSpec::Only,
        GitFastForward::No => GitFastForwardSpec::No,
    }
}

/// Persist a pending source decision under a process directory.
pub fn write_pending_source(
    record_dir: &Path,
    pending: &PendingSourceDecision,
) -> Result<(), SourceBuildError> {
    let bytes = serde_json::to_vec_pretty(pending).map_err(SourceBuildError::JsonWrite)?;
    std::fs::write(record_dir.join(PENDING_SOURCE_FILE), bytes)?;
    Ok(())
}

/// Read a pending source decision from a process record.
pub fn read_pending_source(
    record: &ProcessRecord,
) -> Result<PendingSourceDecision, SourceBuildError> {
    let path = record.dir().join(PENDING_SOURCE_FILE);
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(SourceBuildError::NoPendingDecision);
        }
        Err(err) => return Err(SourceBuildError::Io(err)),
    };
    serde_json::from_slice(&bytes).map_err(SourceBuildError::JsonRead)
}

/// Remove a pending source decision from a process record.
pub fn clear_pending_source(record: &ProcessRecord) -> Result<(), SourceBuildError> {
    let path = record.dir().join(PENDING_SOURCE_FILE);
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(SourceBuildError::Io(err)),
    }
}
