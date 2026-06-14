//! Exploration-scoped source provisioning and disposition.
//!
//! A source derives a durable runner base once before the runner loop starts,
//! then disposes that base once after the loop finishes. Per-iteration
//! workspace clone/apply-back remains owned by [`crate::workspace`].

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tempfile::Builder;
use thiserror::Error;
use tokio::fs;
use tokio::process::Command;

use crate::time::{Clock, SystemClock};
use crate::workspace::mirror::filter::{ApplyBackFilter, CloneFilter};
use crate::workspace::mirror::materialize::copy_dir_recursive;
use crate::workspace::mirror::reconcile::{merge_back_impl, sync_back_impl};

/// Runtime source definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceSpec {
    /// Filesystem directory source.
    Directory(DirectorySourceSpec),
    /// Git-backed source.
    Git(GitSourceSpec),
}

/// Directory source definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectorySourceSpec {
    /// Canonical directory path.
    pub path: PathBuf,
    /// Provisioning strategy.
    pub derive: SourceDeriveSpec,
    /// Finish disposition. Absent only for passthrough.
    pub disposition: Option<SourceDispositionSpec>,
}

/// Git source definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitSourceSpec {
    /// Local path or URL locator.
    pub locator: GitLocatorSpec,
    /// Provisioning strategy.
    pub derive: SourceDeriveSpec,
    /// Finish disposition.
    pub disposition: SourceDispositionSpec,
}

/// Git source locator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitLocatorSpec {
    /// Remote URL.
    Url(String),
    /// Local repository path.
    Path(PathBuf),
}

/// Source provisioning strategy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceDeriveSpec {
    /// Use canonical directly.
    Passthrough,
    /// Copy directory once.
    Copy {
        /// Copy-time excludes.
        excludes: Vec<String>,
        /// Preserve source mtimes.
        preserve_mtime: bool,
    },
    /// Create a git worktree.
    Worktree {
        /// Ref to derive from.
        ref_name: Option<String>,
        /// Branch to create/reset for the worktree.
        branch: Option<String>,
    },
    /// Clone a git repository.
    Clone {
        /// Ref to check out.
        ref_name: Option<String>,
        /// Branch to create/reset in the clone.
        branch: Option<String>,
        /// Shallow clone depth.
        depth: Option<u64>,
    },
}

/// Source finish disposition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceDispositionSpec {
    /// Drop base, leave canonical untouched.
    Discard,
    /// Non-destructive merge.
    Merge {
        /// Directory filter excludes.
        excludes: Vec<String>,
        /// Directory filter includes.
        includes: Vec<String>,
        /// Git target branch.
        into: Option<String>,
        /// Git fast-forward policy.
        ff: Option<GitFastForwardSpec>,
    },
    /// Destructive directory sync.
    Sync {
        /// Directory filter excludes.
        excludes: Vec<String>,
        /// Directory filter includes.
        includes: Vec<String>,
    },
    /// Park base and persist an operator decision.
    Defer {
        /// Disposition to run when promoted.
        promote: Box<SourceDispositionSpec>,
    },
}

/// Git fast-forward policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitFastForwardSpec {
    /// Allow fast-forward or merge commit.
    Allow,
    /// Require fast-forward.
    Only,
    /// Force merge commit.
    No,
}

/// A provisioned source base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvisionedSource {
    /// Original source spec.
    pub spec: SourceSpec,
    /// Durable base path passed to per-iteration workspaces.
    pub base_path: PathBuf,
    /// Runtime metadata needed for disposition.
    pub state: ProvisionedState,
}

/// Runtime provisioned state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProvisionedState {
    /// Directory source state.
    Directory {
        /// Canonical directory path.
        canonical: PathBuf,
        /// Whether `base_path` is the canonical path itself.
        passthrough: bool,
    },
    /// Git worktree state.
    GitWorktree {
        /// Local canonical repository.
        repo: PathBuf,
        /// Worktree branch.
        branch: String,
        /// Ref/branch the worktree was derived from.
        derived_from: String,
    },
    /// Git clone state.
    GitClone {
        /// Locator used for clone.
        locator: GitLocatorSpec,
        /// Clone branch.
        branch: String,
        /// Ref/branch the clone was derived from.
        derived_from: String,
    },
}

/// Pending deferred source decision stored in a process record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingSourceDecision {
    /// Provisioned source state.
    pub provisioned: ProvisionedSource,
    /// Inner disposition to execute on promote.
    pub promote: SourceDispositionSpec,
}

/// Source runtime behavior.
#[async_trait]
pub trait Source: Send + Sync {
    /// Provision a durable base.
    async fn provision(&self) -> Result<ProvisionedSource, SourceError>;

    /// Dispose a provisioned base. Returns a pending decision for deferred disposition.
    async fn dispose(
        &self,
        provisioned: ProvisionedSource,
    ) -> Result<Option<PendingSourceDecision>, SourceError>;
}

/// Source runtime error.
#[derive(Debug, Error)]
pub enum SourceError {
    /// Filesystem operation failed.
    #[error("source filesystem error: {0}")]
    Io(#[from] io::Error),
    /// Glob filter failed to compile.
    #[error("source filter error: {0}")]
    Filter(#[from] globset::Error),
    /// Git command failed.
    #[error("git command failed: {program} {args}: {stderr}")]
    Git {
        /// Program name.
        program: String,
        /// Rendered arguments.
        args: String,
        /// Stderr text.
        stderr: String,
    },
    /// Invalid source configuration reached runtime.
    #[error("invalid source configuration: {0}")]
    InvalidConfig(String),
}

/// Build a boxed source runtime from a spec.
#[must_use]
pub fn source_from_spec(spec: SourceSpec) -> Box<dyn Source> {
    source_from_spec_with_clock(spec, Arc::new(SystemClock))
}

/// Build a boxed source runtime from a spec with an injected clock.
#[must_use]
pub fn source_from_spec_with_clock(spec: SourceSpec, clock: Arc<dyn Clock>) -> Box<dyn Source> {
    match spec {
        SourceSpec::Directory(spec) => Box::new(DirectorySource { spec, clock }),
        SourceSpec::Git(spec) => Box::new(GitSource { spec, clock }),
    }
}

/// Dispose a pending source decision with a concrete operator choice.
///
/// # Errors
///
/// Returns an error if the recorded disposition cannot be applied to the
/// parked base or canonical source.
pub async fn promote_pending(decision: PendingSourceDecision) -> Result<(), SourceError> {
    dispose_with(&decision.provisioned, &decision.promote).await
}

/// Discard a pending source decision.
///
/// # Errors
///
/// Returns an error if the parked base cannot be removed.
pub async fn discard_pending(decision: PendingSourceDecision) -> Result<(), SourceError> {
    discard_base(&decision.provisioned).await
}

struct DirectorySource {
    spec: DirectorySourceSpec,
    clock: Arc<dyn Clock>,
}

#[async_trait]
impl Source for DirectorySource {
    async fn provision(&self) -> Result<ProvisionedSource, SourceError> {
        match &self.spec.derive {
            SourceDeriveSpec::Passthrough => Ok(ProvisionedSource {
                spec: SourceSpec::Directory(self.spec.clone()),
                base_path: self.spec.path.clone(),
                state: ProvisionedState::Directory {
                    canonical: self.spec.path.clone(),
                    passthrough: true,
                },
            }),
            SourceDeriveSpec::Copy {
                excludes,
                preserve_mtime,
            } => {
                let base_path = durable_temp_path("iter-source-dir").await?;
                let filter = CloneFilter::compile(excludes, &[])?;
                copy_dir_recursive(
                    &self.spec.path,
                    &base_path,
                    &filter,
                    *preserve_mtime,
                    self.clock.as_ref(),
                )
                .await?;
                Ok(ProvisionedSource {
                    spec: SourceSpec::Directory(self.spec.clone()),
                    base_path,
                    state: ProvisionedState::Directory {
                        canonical: self.spec.path.clone(),
                        passthrough: false,
                    },
                })
            }
            SourceDeriveSpec::Worktree { .. } | SourceDeriveSpec::Clone { .. } => Err(
                SourceError::InvalidConfig("directory source cannot use git derive".into()),
            ),
        }
    }

    async fn dispose(
        &self,
        provisioned: ProvisionedSource,
    ) -> Result<Option<PendingSourceDecision>, SourceError> {
        dispose_provisioned(provisioned, self.spec.disposition.as_ref()).await
    }
}

struct GitSource {
    spec: GitSourceSpec,
    clock: Arc<dyn Clock>,
}

#[async_trait]
impl Source for GitSource {
    async fn provision(&self) -> Result<ProvisionedSource, SourceError> {
        match &self.spec.derive {
            SourceDeriveSpec::Worktree { ref_name, branch } => {
                let GitLocatorSpec::Path(repo) = &self.spec.locator else {
                    return Err(SourceError::InvalidConfig(
                        "git worktree derive requires a local `path` locator".into(),
                    ));
                };
                let base_path = durable_temp_path("iter-source-git-worktree").await?;
                fs::remove_dir_all(&base_path).await?;
                let derived_from = ref_name.clone().unwrap_or_else(|| "HEAD".to_string());
                let branch = branch
                    .clone()
                    .unwrap_or_else(|| auto_branch_name(self.clock.as_ref()));
                run_git(
                    repo,
                    [
                        OsStr::new("worktree"),
                        OsStr::new("add"),
                        OsStr::new("-B"),
                        OsStr::new(&branch),
                        base_path.as_os_str(),
                        OsStr::new(&derived_from),
                    ],
                )
                .await?;
                Ok(ProvisionedSource {
                    spec: SourceSpec::Git(self.spec.clone()),
                    base_path,
                    state: ProvisionedState::GitWorktree {
                        repo: repo.clone(),
                        branch,
                        derived_from,
                    },
                })
            }
            SourceDeriveSpec::Clone {
                ref_name,
                branch,
                depth,
            } => {
                let base_path = durable_temp_path("iter-source-git-clone").await?;
                fs::remove_dir_all(&base_path).await?;
                let locator_arg = match &self.spec.locator {
                    GitLocatorSpec::Url(url) => url.clone(),
                    GitLocatorSpec::Path(path) => path.display().to_string(),
                };
                let mut args = vec!["clone".to_string()];
                if let Some(depth) = depth {
                    args.push("--depth".into());
                    args.push(depth.to_string());
                }
                args.push(locator_arg);
                args.push(base_path.display().to_string());
                run_cmd("git", &args, None).await?;

                let derived_from = ref_name.clone().unwrap_or_else(|| "HEAD".to_string());
                let branch = branch
                    .clone()
                    .unwrap_or_else(|| auto_branch_name(self.clock.as_ref()));
                run_git(
                    &base_path,
                    [
                        OsStr::new("checkout"),
                        OsStr::new("-B"),
                        OsStr::new(&branch),
                        OsStr::new(&derived_from),
                    ],
                )
                .await?;
                Ok(ProvisionedSource {
                    spec: SourceSpec::Git(self.spec.clone()),
                    base_path,
                    state: ProvisionedState::GitClone {
                        locator: self.spec.locator.clone(),
                        branch,
                        derived_from,
                    },
                })
            }
            SourceDeriveSpec::Passthrough | SourceDeriveSpec::Copy { .. } => Err(
                SourceError::InvalidConfig("git source cannot use directory derive".into()),
            ),
        }
    }

    async fn dispose(
        &self,
        provisioned: ProvisionedSource,
    ) -> Result<Option<PendingSourceDecision>, SourceError> {
        dispose_provisioned(provisioned, Some(&self.spec.disposition)).await
    }
}

async fn dispose_provisioned(
    provisioned: ProvisionedSource,
    disposition: Option<&SourceDispositionSpec>,
) -> Result<Option<PendingSourceDecision>, SourceError> {
    match disposition {
        None => Ok(None),
        Some(SourceDispositionSpec::Defer { promote }) => Ok(Some(PendingSourceDecision {
            provisioned,
            promote: promote.as_ref().clone(),
        })),
        Some(disposition) => {
            dispose_with(&provisioned, disposition).await?;
            Ok(None)
        }
    }
}

async fn dispose_with(
    provisioned: &ProvisionedSource,
    disposition: &SourceDispositionSpec,
) -> Result<(), SourceError> {
    match disposition {
        SourceDispositionSpec::Discard => discard_base(provisioned).await,
        SourceDispositionSpec::Merge {
            excludes,
            includes,
            into,
            ff,
        } => match &provisioned.state {
            ProvisionedState::Directory {
                canonical,
                passthrough,
            } => {
                if *passthrough {
                    return Ok(());
                }
                let filter =
                    ApplyBackFilter::compile_with_workspace_excludes(excludes, includes, &[])?;
                merge_back_impl(canonical, &provisioned.base_path, &filter).await?;
                Ok(())
            }
            ProvisionedState::GitWorktree {
                repo,
                branch,
                derived_from,
            } => {
                git_merge(repo, branch, into.as_deref().unwrap_or(derived_from), *ff).await?;
                discard_base(provisioned).await
            }
            ProvisionedState::GitClone {
                locator,
                branch,
                derived_from,
            } => {
                let target = into.as_deref().unwrap_or(derived_from);
                match locator {
                    GitLocatorSpec::Url(url) => {
                        run_git(
                            &provisioned.base_path,
                            [
                                OsStr::new("push"),
                                OsStr::new(url),
                                OsStr::new(&format!("{branch}:{target}")),
                            ],
                        )
                        .await?;
                    }
                    GitLocatorSpec::Path(path) => {
                        run_git(
                            &provisioned.base_path,
                            [
                                OsStr::new("push"),
                                path.as_os_str(),
                                OsStr::new(&format!("{branch}:{target}")),
                            ],
                        )
                        .await?;
                    }
                }
                discard_base(provisioned).await
            }
        },
        SourceDispositionSpec::Sync { excludes, includes } => {
            let ProvisionedState::Directory {
                canonical,
                passthrough,
            } = &provisioned.state
            else {
                return Err(SourceError::InvalidConfig(
                    "sync disposition is only supported for directory sources".into(),
                ));
            };
            if *passthrough {
                return Ok(());
            }
            let filter = ApplyBackFilter::compile_with_workspace_excludes(excludes, includes, &[])?;
            sync_back_impl(canonical, &provisioned.base_path, &filter).await?;
            discard_base(provisioned).await
        }
        SourceDispositionSpec::Defer { .. } => Err(SourceError::InvalidConfig(
            "defer cannot be executed directly".into(),
        )),
    }
}

async fn discard_base(provisioned: &ProvisionedSource) -> Result<(), SourceError> {
    match &provisioned.state {
        ProvisionedState::Directory {
            passthrough: true, ..
        } => Ok(()),
        ProvisionedState::Directory {
            passthrough: false, ..
        }
        | ProvisionedState::GitClone { .. } => {
            remove_dir_if_exists(&provisioned.base_path).await?;
            Ok(())
        }
        ProvisionedState::GitWorktree { repo, .. } => {
            run_git(
                repo,
                [
                    OsStr::new("worktree"),
                    OsStr::new("remove"),
                    OsStr::new("--force"),
                    provisioned.base_path.as_os_str(),
                ],
            )
            .await?;
            Ok(())
        }
    }
}

async fn git_merge(
    repo: &Path,
    branch: &str,
    target: &str,
    ff: Option<GitFastForwardSpec>,
) -> Result<(), SourceError> {
    run_git(repo, [OsStr::new("checkout"), OsStr::new(target)]).await?;
    let ff_arg = match ff.unwrap_or(GitFastForwardSpec::Allow) {
        GitFastForwardSpec::Allow => "--ff",
        GitFastForwardSpec::Only => "--ff-only",
        GitFastForwardSpec::No => "--no-ff",
    };
    run_git(
        repo,
        [OsStr::new("merge"), OsStr::new(ff_arg), OsStr::new(branch)],
    )
    .await
}

async fn durable_temp_path(prefix: &str) -> io::Result<PathBuf> {
    let prefix = prefix.to_owned();
    tokio::task::spawn_blocking(move || {
        let dir = Builder::new().prefix(&prefix).tempdir()?;
        Ok::<PathBuf, io::Error>(dir.keep())
    })
    .await
    .map_err(io::Error::other)?
}

async fn remove_dir_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

async fn run_git<I, S>(cwd: &Path, args: I) -> Result<(), SourceError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<String> = args
        .into_iter()
        .map(|arg| arg.as_ref().to_string_lossy().into_owned())
        .collect();
    run_cmd("git", &args, Some(cwd)).await
}

async fn run_cmd(program: &str, args: &[String], cwd: Option<&Path>) -> Result<(), SourceError> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command.output().await?;
    if output.status.success() {
        return Ok(());
    }
    Err(SourceError::Git {
        program: program.to_string(),
        args: args.join(" "),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

fn auto_branch_name(clock: &dyn Clock) -> String {
    let millis = clock
        .system_time()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("iter/source/{millis}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::fs;

    static GIT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.expect("mkdir");
        }
        fs::write(path, contents).await.expect("write");
    }

    async fn git(args: &[&str], cwd: Option<&Path>) {
        run_cmd(
            "git",
            &args
                .iter()
                .map(|arg| (*arg).to_string())
                .collect::<Vec<_>>(),
            cwd,
        )
        .await
        .expect("git command");
    }

    async fn init_repo(path: &Path) {
        git(&["init", "-b", "main"], Some(path)).await;
        git(
            &["config", "user.email", "iter@example.invalid"],
            Some(path),
        )
        .await;
        git(&["config", "user.name", "iter test"], Some(path)).await;
        write(&path.join("file.txt"), "base").await;
        git(&["add", "file.txt"], Some(path)).await;
        git(&["commit", "-m", "base"], Some(path)).await;
    }

    #[tokio::test]
    async fn directory_copy_sync_updates_and_deletes() {
        let canonical = tempfile::tempdir().expect("canonical");
        write(&canonical.path().join("keep.txt"), "old").await;
        write(&canonical.path().join("drop.txt"), "drop").await;
        let source = DirectorySource {
            spec: DirectorySourceSpec {
                path: canonical.path().to_path_buf(),
                derive: SourceDeriveSpec::Copy {
                    excludes: Vec::new(),
                    preserve_mtime: true,
                },
                disposition: Some(SourceDispositionSpec::Sync {
                    excludes: Vec::new(),
                    includes: Vec::new(),
                }),
            },
            clock: Arc::new(SystemClock),
        };

        let provisioned = source.provision().await.expect("provision");
        write(&provisioned.base_path.join("keep.txt"), "new").await;
        fs::remove_file(provisioned.base_path.join("drop.txt"))
            .await
            .expect("remove");
        source.dispose(provisioned).await.expect("dispose");

        assert_eq!(
            fs::read_to_string(canonical.path().join("keep.txt"))
                .await
                .expect("read"),
            "new"
        );
        assert!(!canonical.path().join("drop.txt").exists());
    }

    #[tokio::test]
    async fn directory_copy_discard_leaves_canonical() {
        let canonical = tempfile::tempdir().expect("canonical");
        write(&canonical.path().join("keep.txt"), "old").await;
        let source = DirectorySource {
            spec: DirectorySourceSpec {
                path: canonical.path().to_path_buf(),
                derive: SourceDeriveSpec::Copy {
                    excludes: Vec::new(),
                    preserve_mtime: true,
                },
                disposition: Some(SourceDispositionSpec::Discard),
            },
            clock: Arc::new(SystemClock),
        };

        let provisioned = source.provision().await.expect("provision");
        write(&provisioned.base_path.join("keep.txt"), "new").await;
        source.dispose(provisioned).await.expect("dispose");

        assert_eq!(
            fs::read_to_string(canonical.path().join("keep.txt"))
                .await
                .expect("read"),
            "old"
        );
    }

    #[tokio::test]
    async fn git_worktree_discard_leaves_canonical_branch() {
        let _guard = GIT_TEST_LOCK.lock().await;
        let repo = tempfile::tempdir().expect("repo");
        init_repo(repo.path()).await;
        let source = GitSource {
            spec: GitSourceSpec {
                locator: GitLocatorSpec::Path(repo.path().to_path_buf()),
                derive: SourceDeriveSpec::Worktree {
                    ref_name: Some("main".into()),
                    branch: Some("iter/test-discard".into()),
                },
                disposition: SourceDispositionSpec::Discard,
            },
            clock: Arc::new(SystemClock),
        };

        let provisioned = source.provision().await.expect("provision");
        assert!(provisioned.base_path.join("file.txt").exists());
        write(&provisioned.base_path.join("file.txt"), "changed").await;
        git(&["add", "file.txt"], Some(&provisioned.base_path)).await;
        git(&["commit", "-m", "change"], Some(&provisioned.base_path)).await;
        source.dispose(provisioned).await.expect("dispose");

        git(&["checkout", "main"], Some(repo.path())).await;
        assert_eq!(
            fs::read_to_string(repo.path().join("file.txt"))
                .await
                .expect("read"),
            "base"
        );
    }

    #[tokio::test]
    async fn git_worktree_merge_folds_branch_back() {
        let _guard = GIT_TEST_LOCK.lock().await;
        let repo = tempfile::tempdir().expect("repo");
        init_repo(repo.path()).await;
        let source = GitSource {
            spec: GitSourceSpec {
                locator: GitLocatorSpec::Path(repo.path().to_path_buf()),
                derive: SourceDeriveSpec::Worktree {
                    ref_name: Some("main".into()),
                    branch: Some("iter/test-merge".into()),
                },
                disposition: SourceDispositionSpec::Merge {
                    excludes: Vec::new(),
                    includes: Vec::new(),
                    into: Some("main".into()),
                    ff: Some(GitFastForwardSpec::Only),
                },
            },
            clock: Arc::new(SystemClock),
        };

        let provisioned = source.provision().await.expect("provision");
        write(&provisioned.base_path.join("file.txt"), "changed").await;
        git(&["add", "file.txt"], Some(&provisioned.base_path)).await;
        git(&["commit", "-m", "change"], Some(&provisioned.base_path)).await;
        source.dispose(provisioned).await.expect("dispose");

        git(&["checkout", "main"], Some(repo.path())).await;
        assert_eq!(
            fs::read_to_string(repo.path().join("file.txt"))
                .await
                .expect("read"),
            "changed"
        );
    }

    #[tokio::test]
    async fn defer_parks_then_promote_executes_inner_disposition() {
        let canonical = tempfile::tempdir().expect("canonical");
        write(&canonical.path().join("keep.txt"), "old").await;
        let source = DirectorySource {
            spec: DirectorySourceSpec {
                path: canonical.path().to_path_buf(),
                derive: SourceDeriveSpec::Copy {
                    excludes: Vec::new(),
                    preserve_mtime: true,
                },
                disposition: Some(SourceDispositionSpec::Defer {
                    promote: Box::new(SourceDispositionSpec::Sync {
                        excludes: Vec::new(),
                        includes: Vec::new(),
                    }),
                }),
            },
            clock: Arc::new(SystemClock),
        };

        let provisioned = source.provision().await.expect("provision");
        write(&provisioned.base_path.join("keep.txt"), "new").await;
        let pending = source
            .dispose(provisioned)
            .await
            .expect("dispose")
            .expect("pending decision");
        assert_eq!(
            fs::read_to_string(canonical.path().join("keep.txt"))
                .await
                .expect("read"),
            "old"
        );

        promote_pending(pending).await.expect("promote");
        assert_eq!(
            fs::read_to_string(canonical.path().join("keep.txt"))
                .await
                .expect("read"),
            "new"
        );
    }
}
