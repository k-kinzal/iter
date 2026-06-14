//! Compose plan construction: parse declarations, build queues and
//! services, and produce a [`ComposePlan`] ready for [`super::run`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iter_core::RunnerBuilder;
use iter_language::{
    Compose, NamedQueue, NamedService, QueueDef, QueueRef, ServiceSource, TelemetryDef,
};

use super::error::ComposeError;
use super::flatten::{FlattenedPlan, flatten_composes};
use super::service_build::build_service;
use super::trigger::{ComposeTrigger, build_trigger};
use crate::queue::queue_from_def;
use iter_core::Queue;

pub(crate) struct ComposeService {
    pub(crate) name: String,
    pub(crate) iterfile_path: PathBuf,
    pub(crate) queue_decl: QueueDef,
    pub(crate) builder: RunnerBuilder,
}

/// Built compose plan ready for execution by [`super::run`].
///
/// Holds the constructed queues and runners in declaration order.
/// Construction is fallible (see [`build`]); execution is async
/// (see [`super::run`]).
pub(crate) struct ComposePlan {
    pub(crate) queues: BTreeMap<String, Arc<dyn Queue>>,
    pub(crate) services: Vec<ComposeService>,
    pub(crate) triggers: Vec<ComposeTrigger>,
    pub(crate) telemetry: Option<TelemetryDef>,
    pub(crate) compose_path: PathBuf,
    pub(crate) sources: BTreeMap<String, PathBuf>,
}

impl std::fmt::Debug for ComposePlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComposePlan")
            .field("queues", &self.queues.keys().collect::<Vec<_>>())
            .field(
                "services",
                &self.services.iter().map(|s| &s.name).collect::<Vec<_>>(),
            )
            .field(
                "triggers",
                &self.triggers.iter().map(|t| &t.name).collect::<Vec<_>>(),
            )
            .field("telemetry", &self.telemetry.is_some())
            .field("compose_path", &self.compose_path)
            .field("sources", &self.sources.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ComposePlan {
    /// Number of named queues built from the compose file.
    #[must_use]
    pub(crate) fn queue_count(&self) -> usize {
        self.queues.len()
    }

    /// Number of services built from the compose file.
    #[must_use]
    pub(crate) fn service_count(&self) -> usize {
        self.services.len()
    }

    /// Iterate built queue names in declaration order.
    pub(crate) fn queue_names(&self) -> impl Iterator<Item = &str> {
        self.queues.keys().map(String::as_str)
    }

    /// Iterate built service names in declaration order.
    pub(crate) fn service_names(&self) -> impl Iterator<Item = &str> {
        self.services.iter().map(|s| s.name.as_str())
    }

    /// Number of triggers in the flattened plan.
    #[must_use]
    pub(crate) fn trigger_count(&self) -> usize {
        self.triggers.len()
    }

    /// Iterate trigger names in the flattened plan.
    pub(crate) fn trigger_names(&self) -> impl Iterator<Item = &str> {
        self.triggers.iter().map(|t| t.name.as_str())
    }

    /// Collect all declared service names as owned strings.
    #[must_use]
    pub(crate) fn all_service_names(&self) -> Vec<String> {
        self.services.iter().map(|s| s.name.clone()).collect()
    }

    /// Return service names whose `iterfile_path` matches `source`.
    #[must_use]
    pub(crate) fn services_for_source(&self, source: &Path) -> Vec<String> {
        let canonical_source =
            std::fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf());
        self.services
            .iter()
            .filter(|s| {
                let canonical_iterfile = std::fs::canonicalize(&s.iterfile_path)
                    .unwrap_or_else(|_| s.iterfile_path.clone());
                canonical_iterfile == canonical_source
            })
            .map(|s| s.name.clone())
            .collect()
    }

    /// Borrow the project-wide telemetry declaration, when present.
    #[must_use]
    pub(crate) fn telemetry(&self) -> Option<&TelemetryDef> {
        self.telemetry.as_ref()
    }

    /// Look up the source compose file for a given element name.
    #[must_use]
    pub(crate) fn source_of(&self, name: &str) -> Option<&Path> {
        self.sources.get(name).map(PathBuf::as_path)
    }
}

/// Build a [`ComposePlan`] from a parsed `compose.iter`.
///
/// `compose_path` is the absolute path of the compose file. Its parent
/// directory resolves relative `service { build = ... }` paths; the
/// path itself is recorded as the `iterfile` field of the registry
/// entry for inline services so `iter inspect` always points at a real
/// source file.
///
/// # Errors
///
/// * A queue declaration is invalid (e.g. unreachable Redis URL).
/// * A `service { build = "./Iterfile" }` target cannot be loaded, is
///   missing a required section, or declares its own `queue` block
///   (compose-managed services must inherit the queue from
///   `compose.iter`).
/// * A queue reference points at a name not declared in the file.
pub(crate) fn build(root: &Compose, compose_path: &Path) -> Result<ComposePlan, ComposeError> {
    let compose_dir = compose_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut visited = BTreeSet::new();
    visited.insert(compose_path.to_path_buf());
    let FlattenedPlan {
        root: flat,
        sources,
    } = flatten_composes(root, compose_path, compose_dir, &mut visited)?;

    if flat.services.is_empty() {
        return Err(ComposeError::NoServices {
            path: compose_path.to_path_buf(),
        });
    }

    let mut queues: BTreeMap<String, Arc<dyn Queue>> = BTreeMap::new();
    for spanned in &flat.queues {
        let NamedQueue { name, decl } = &spanned.node;
        let queue = queue_from_def(decl).map_err(|source| ComposeError::QueueBuild {
            name: name.clone(),
            source,
        })?;
        queues.insert(name.clone(), queue);
    }

    let mut services = Vec::with_capacity(flat.services.len());
    for spanned in &flat.services {
        let NamedService { name, source } = &spanned.node;
        let queue_ref = match source {
            ServiceSource::Build { queue, .. } => queue
                .as_ref()
                .ok_or_else(|| ComposeError::UnresolvedServiceQueue(name.clone()))?,
            ServiceSource::Inline(inline) => inline
                .queue
                .as_ref()
                .ok_or_else(|| ComposeError::UnresolvedServiceQueue(name.clone()))?,
        };
        let queue_decl = lookup_queue_decl(queue_ref, &flat)?;
        let service = build_service(
            name,
            source,
            &queues,
            compose_dir,
            compose_path,
            false,
            queue_decl,
        )?;
        services.push(service);
    }

    let mut triggers = Vec::with_capacity(flat.triggers.len());
    for spanned in &flat.triggers {
        let trigger = build_trigger(&spanned.node, &queues)?;
        triggers.push(trigger);
    }

    Ok(ComposePlan {
        queues,
        services,
        triggers,
        telemetry: flat.telemetry.map(|t| t.node),
        compose_path: compose_path.to_path_buf(),
        sources,
    })
}

/// Output of [`build_single_service`].
pub(crate) struct SingleServiceBuild {
    /// Path recorded into the per-service process registry entry.
    pub(crate) iterfile_path: PathBuf,
    /// Runner builder ready for `.build()`.
    pub(crate) builder: RunnerBuilder,
}

/// Build only the named service from a parsed compose file.
///
/// Used by `iter run --service NAME -f compose.iter`: the compose
/// orchestrator spawns this command for each service whose queue is
/// URL-addressable, and the child re-parses the same compose file and
/// runs only its own service in-process.
///
/// # Errors
///
/// * The named service is not present in the compose file.
/// * The named service's referenced queue cannot be built.
/// * Building the service itself fails.
pub(crate) fn build_single_service(
    root: &Compose,
    compose_path: &Path,
    service_name: &str,
    once: bool,
) -> Result<SingleServiceBuild, ComposeError> {
    let compose_dir = compose_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut visited = BTreeSet::new();
    visited.insert(compose_path.to_path_buf());
    let FlattenedPlan { root: flat, .. } =
        flatten_composes(root, compose_path, compose_dir, &mut visited)?;

    let spanned = flat
        .services
        .iter()
        .find(|s| s.node.name == service_name)
        .ok_or_else(|| ComposeError::UnknownService(service_name.to_owned()))?;

    let NamedService { name, source } = &spanned.node;

    let queue_ref = match source {
        ServiceSource::Build { queue, .. } => queue
            .as_ref()
            .ok_or_else(|| ComposeError::UnresolvedServiceQueue(name.clone()))?,
        ServiceSource::Inline(inline) => inline
            .queue
            .as_ref()
            .ok_or_else(|| ComposeError::UnresolvedServiceQueue(name.clone()))?,
    };
    let queue_name = match queue_ref {
        QueueRef::Named(n) => n.clone(),
        QueueRef::Anonymous => {
            return Err(ComposeError::UnresolvedAnonymousQueueRef);
        }
    };
    let queue_decl = flat
        .queues
        .iter()
        .find(|q| q.node.name == queue_name)
        .map(|q| q.node.decl.clone())
        .ok_or_else(|| ComposeError::UnknownQueue(queue_name.clone()))?;
    let queue_arc = queue_from_def(&queue_decl).map_err(|source| ComposeError::QueueBuild {
        name: queue_name,
        source,
    })?;

    let mut queues: BTreeMap<String, Arc<dyn Queue>> = BTreeMap::new();
    if let QueueRef::Named(n) = queue_ref {
        queues.insert(n.clone(), queue_arc);
    }

    let service = build_service(
        name,
        source,
        &queues,
        compose_dir,
        compose_path,
        once,
        queue_decl,
    )?;
    let ComposeService {
        name: _,
        iterfile_path,
        queue_decl: _,
        builder,
    } = service;
    Ok(SingleServiceBuild {
        iterfile_path,
        builder,
    })
}

pub(super) fn lookup_queue(
    queue_ref: &QueueRef,
    queues: &BTreeMap<String, Arc<dyn Queue>>,
) -> Result<Arc<dyn Queue>, ComposeError> {
    match queue_ref {
        QueueRef::Named(name) => queues
            .get(name)
            .cloned()
            .ok_or_else(|| ComposeError::UnknownQueue(name.clone())),
        QueueRef::Anonymous => Err(ComposeError::UnresolvedAnonymousQueueRef),
    }
}

fn lookup_queue_decl(queue_ref: &QueueRef, root: &Compose) -> Result<QueueDef, ComposeError> {
    match queue_ref {
        QueueRef::Named(name) => root
            .queues
            .iter()
            .find(|spanned| spanned.node.name == *name)
            .map(|spanned| spanned.node.decl.clone())
            .ok_or_else(|| ComposeError::UnknownQueue(name.clone())),
        QueueRef::Anonymous => Err(ComposeError::UnresolvedAnonymousQueueRef),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::load_compose;

    fn write_compose(dir: &Path, content: &str) -> PathBuf {
        let path = dir.join("compose.iter");
        std::fs::write(&path, content).expect("write compose file");
        path
    }

    #[test]
    fn all_service_names_returns_declared_names() {
        let dir = tempfile::tempdir().expect("tmp");
        let iterfile = dir.path().join("Iterfile");
        std::fs::write(
            &iterfile,
            r#"
workspace local { base = "." }
agent claude { mode = print command = "claude" }
runner {
  agent = claude
  workspace = local
  continue_on_error = false
  behavior = wait
  prompt = "noop"
}
"#,
        )
        .expect("write iterfile");
        let path = write_compose(
            dir.path(),
            r#"
queue main file { path = "./.iter/queue" }
service alpha { build = "./Iterfile" }
"#,
        );
        let root = load_compose(&path).expect("load");
        let canonical = std::fs::canonicalize(&path).unwrap();
        let plan = build(&root, &canonical).expect("build");
        let names = plan.all_service_names();
        assert_eq!(names, vec!["alpha"]);
    }

    #[test]
    fn services_for_source_matches_iterfile_path() {
        let dir = tempfile::tempdir().expect("tmp");
        let iterfile = dir.path().join("Iterfile");
        std::fs::write(
            &iterfile,
            r#"
workspace local { base = "." }
agent claude { mode = print command = "claude" }
runner {
  agent = claude
  workspace = local
  continue_on_error = false
  behavior = wait
  prompt = "noop"
}
"#,
        )
        .expect("write iterfile");
        let path = write_compose(
            dir.path(),
            r#"
queue main file { path = "./.iter/queue" }
service alpha { build = "./Iterfile" }
"#,
        );
        let root = load_compose(&path).expect("load");
        let canonical = std::fs::canonicalize(&path).unwrap();
        let plan = build(&root, &canonical).expect("build");
        let matched = plan.services_for_source(&iterfile);
        assert_eq!(matched, vec!["alpha"]);
        let unmatched = plan.services_for_source(Path::new("/nonexistent/Iterfile"));
        assert!(unmatched.is_empty());
    }
}
