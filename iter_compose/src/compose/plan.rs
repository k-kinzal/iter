//! Compose plan construction: parse declarations, build queues and
//! services, and produce a [`ComposePlan`] ready for [`super::run`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iter_core::RunnerBuilder;
use iter_language::{
    AgentDecl, ComposeRoot, ComposeTriggerOverride, EventHandlerDecl, InlineService, NamedQueue,
    NamedService, NamedTrigger, PromptDecl, QueueDecl, QueueRef, Root, RunnerDecl, ServiceSource,
    Spanned, TelemetryDecl, WorkspaceDecl, parse,
};

use super::error::ComposeError;
use super::trigger::{ComposeTrigger, build_trigger};
use crate::agent::AnyAgent;
use crate::assembly;
use crate::compose::load_compose;
use crate::queue::{AnyQueue, build_queue};
use crate::workspace::AnyWorkspace;

pub(crate) struct ComposeService {
    pub(crate) name: String,
    pub(crate) iterfile_path: PathBuf,
    pub(crate) queue_decl: QueueDecl,
    pub(crate) builder: RunnerBuilder<AnyQueue, AnyWorkspace, AnyAgent>,
}

/// Built compose plan ready for execution by [`super::run`].
///
/// Holds the constructed queues and runners in declaration order.
/// Construction is fallible (see [`build`]); execution is async
/// (see [`super::run`]).
pub struct ComposePlan {
    pub(crate) queues: BTreeMap<String, Arc<AnyQueue>>,
    pub(crate) services: Vec<ComposeService>,
    pub(crate) triggers: Vec<ComposeTrigger>,
    pub(crate) telemetry: Option<TelemetryDecl>,
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
    pub fn queue_count(&self) -> usize {
        self.queues.len()
    }

    /// Number of services built from the compose file.
    #[must_use]
    pub fn service_count(&self) -> usize {
        self.services.len()
    }

    /// Iterate built queue names in declaration order.
    pub fn queue_names(&self) -> impl Iterator<Item = &str> {
        self.queues.keys().map(String::as_str)
    }

    /// Iterate built service names in declaration order.
    pub fn service_names(&self) -> impl Iterator<Item = &str> {
        self.services.iter().map(|s| s.name.as_str())
    }

    /// Number of triggers in the flattened plan.
    #[must_use]
    pub fn trigger_count(&self) -> usize {
        self.triggers.len()
    }

    /// Iterate trigger names in the flattened plan.
    pub fn trigger_names(&self) -> impl Iterator<Item = &str> {
        self.triggers.iter().map(|t| t.name.as_str())
    }

    /// Collect all declared service names as owned strings.
    #[must_use]
    pub fn all_service_names(&self) -> Vec<String> {
        self.services.iter().map(|s| s.name.clone()).collect()
    }

    /// Return service names whose `iterfile_path` matches `source`.
    ///
    /// Both paths are canonicalized best-effort before comparison so
    /// symlinks and relative segments do not cause mismatches.
    #[must_use]
    pub fn services_for_source(&self, source: &Path) -> Vec<String> {
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
    pub fn telemetry(&self) -> Option<&TelemetryDecl> {
        self.telemetry.as_ref()
    }

    /// Look up the source compose file for a given element name.
    /// Returns `None` for elements declared directly in the root compose file.
    #[must_use]
    pub fn source_of(&self, name: &str) -> Option<&Path> {
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
pub fn build(root: &ComposeRoot, compose_path: &Path) -> Result<ComposePlan, ComposeError> {
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

    let mut queues: BTreeMap<String, Arc<AnyQueue>> = BTreeMap::new();
    for spanned in &flat.queues {
        let NamedQueue { name, decl } = &spanned.node;
        let queue = build_queue(decl).map_err(|source| ComposeError::QueueBuild {
            name: name.clone(),
            source,
        })?;
        queues.insert(name.clone(), Arc::new(queue));
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
pub struct SingleServiceBuild {
    /// Service name as declared in compose.
    pub name: String,
    /// Path recorded into the per-service process registry entry.
    pub iterfile_path: PathBuf,
    /// Runner builder ready for `.build()`. The caller is responsible
    /// for attaching the `LifecycleObserver` from the per-process
    /// runtime before building.
    pub builder: RunnerBuilder<AnyQueue, AnyWorkspace, AnyAgent>,
}

/// Build only the named service from a parsed compose file.
///
/// Used by `iter run --service NAME -f compose.iter`: the compose
/// orchestrator spawns this command for each service whose queue is
/// URL-addressable, and the child re-parses the same compose file and
/// runs only its own service in-process. Sibling services and triggers
/// are not constructed.
///
/// `compose_path` is the absolute path of the compose file. Its parent
/// directory resolves relative `service { build = ... }` paths; the
/// path itself is recorded as the `iterfile` field of the registry
/// entry for inline services.
///
/// Returns a builder ready for `.build()` along with the service-side
/// metadata (`iterfile_path` to record into the process registry).
///
/// # Errors
///
/// * The named service is not present in the compose file.
/// * The named service's referenced queue cannot be built.
/// * Building the service itself fails (missing section, agent build,
///   prompt build, etc.).
pub fn build_single_service(
    root: &ComposeRoot,
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
    let queue_arc =
        Arc::new(
            build_queue(&queue_decl).map_err(|source| ComposeError::QueueBuild {
                name: queue_name,
                source,
            })?,
        );

    let mut queues: BTreeMap<String, Arc<AnyQueue>> = BTreeMap::new();
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
        name: built_name,
        iterfile_path,
        queue_decl: _,
        builder,
    } = service;
    Ok(SingleServiceBuild {
        name: built_name,
        iterfile_path,
        builder,
    })
}

fn build_service(
    name: &str,
    source: &ServiceSource,
    queues: &BTreeMap<String, Arc<AnyQueue>>,
    compose_dir: &Path,
    compose_path: &Path,
    once: bool,
    queue_decl: QueueDecl,
) -> Result<ComposeService, ComposeError> {
    match source {
        ServiceSource::Build { path, queue, args } => {
            let absolute = if path.is_absolute() {
                path.clone()
            } else {
                compose_dir.join(path)
            };
            let source_text =
                std::fs::read_to_string(&absolute).map_err(|e| ComposeError::io(&absolute, e))?;
            let mut root = parse(&source_text)
                .map_err(|diags| ComposeError::parse(&absolute, &source_text, &diags))?;
            if root.queue.is_some() {
                return Err(ComposeError::BuildTargetHasQueue {
                    service: name.to_owned(),
                    path: absolute,
                });
            }
            crate::arg::resolve_args(&mut root, args).map_err(|e| ComposeError::ArgResolve {
                service: name.to_owned(),
                source: e,
            })?;
            let queue_ref = queue
                .as_ref()
                .ok_or_else(|| ComposeError::UnresolvedServiceQueue(name.to_owned()))?;
            let queue_arc = lookup_queue(queue_ref, queues)?;
            build_service_from_root(name, &root, queue_arc, absolute, once, queue_decl)
        }
        ServiceSource::Inline(inline) => {
            let queue_ref = inline
                .queue
                .as_ref()
                .ok_or_else(|| ComposeError::UnresolvedServiceQueue(name.to_owned()))?;
            let queue_arc = lookup_queue(queue_ref, queues)?;
            build_service_from_inline(
                name,
                inline,
                queue_arc,
                compose_path.to_path_buf(),
                once,
                queue_decl,
            )
        }
    }
}

fn build_service_from_root(
    name: &str,
    root: &Root,
    queue: Arc<AnyQueue>,
    iterfile_path: PathBuf,
    once: bool,
    queue_decl: QueueDecl,
) -> Result<ComposeService, ComposeError> {
    let workspace_decl = root.workspace.as_ref().map(|s| &s.node).ok_or_else(|| {
        ComposeError::ServiceMissingSection {
            service: name.to_owned(),
            section: "workspace",
        }
    })?;
    let agent_decl = root.agent.as_ref().map(|s| &s.node).ok_or_else(|| {
        ComposeError::ServiceMissingSection {
            service: name.to_owned(),
            section: "agent",
        }
    })?;
    let runner_decl = root.runner.as_ref().map(|s| &s.node).ok_or_else(|| {
        ComposeError::ServiceMissingSection {
            service: name.to_owned(),
            section: "runner",
        }
    })?;

    finalize_service(
        name,
        queue,
        ServiceDecls {
            workspace: workspace_decl,
            agent: agent_decl,
            runner: runner_decl,
            prompts: &root.prompts,
            events: &root.events,
        },
        iterfile_path,
        once,
        queue_decl,
    )
}

fn build_service_from_inline(
    name: &str,
    inline: &InlineService,
    queue: Arc<AnyQueue>,
    iterfile_path: PathBuf,
    once: bool,
    queue_decl: QueueDecl,
) -> Result<ComposeService, ComposeError> {
    let workspace_decl = inline.workspace.as_ref().map(|s| &s.node).ok_or_else(|| {
        ComposeError::ServiceMissingSection {
            service: name.to_owned(),
            section: "workspace",
        }
    })?;
    let agent_decl = inline.agent.as_ref().map(|s| &s.node).ok_or_else(|| {
        ComposeError::ServiceMissingSection {
            service: name.to_owned(),
            section: "agent",
        }
    })?;
    let runner_decl = inline.runner.as_ref().map(|s| &s.node).ok_or_else(|| {
        ComposeError::ServiceMissingSection {
            service: name.to_owned(),
            section: "runner",
        }
    })?;

    finalize_service(
        name,
        queue,
        ServiceDecls {
            workspace: workspace_decl,
            agent: agent_decl,
            runner: runner_decl,
            prompts: &inline.prompts,
            events: &inline.events,
        },
        iterfile_path,
        once,
        queue_decl,
    )
}

#[derive(Clone, Copy)]
struct ServiceDecls<'a> {
    workspace: &'a WorkspaceDecl,
    agent: &'a AgentDecl,
    runner: &'a RunnerDecl,
    prompts: &'a [Spanned<PromptDecl>],
    events: &'a [Spanned<EventHandlerDecl>],
}

fn finalize_service(
    name: &str,
    queue: Arc<AnyQueue>,
    decls: ServiceDecls<'_>,
    iterfile_path: PathBuf,
    once: bool,
    queue_decl: QueueDecl,
) -> Result<ComposeService, ComposeError> {
    let builder = assembly::assemble_runner_builder(
        Some(queue),
        decls.workspace,
        decls.agent,
        decls.runner,
        decls.prompts,
        decls.events,
        once,
    )
    .map_err(|source| ComposeError::Assembly {
        service: name.to_owned(),
        source,
    })?;

    Ok(ComposeService {
        name: name.to_string(),
        iterfile_path,
        queue_decl,
        builder,
    })
}

#[derive(Debug)]
struct FlattenedPlan {
    root: ComposeRoot,
    sources: BTreeMap<String, PathBuf>,
}

fn flatten_composes(
    root: &ComposeRoot,
    compose_path: &Path,
    compose_dir: &Path,
    visited: &mut BTreeSet<PathBuf>,
) -> Result<FlattenedPlan, ComposeError> {
    if root.composes.is_empty() {
        return Ok(FlattenedPlan {
            root: root.clone(),
            sources: BTreeMap::new(),
        });
    }

    let mut flat = ComposeRoot {
        telemetry: root.telemetry.clone(),
        queues: root.queues.clone(),
        services: root.services.clone(),
        triggers: root.triggers.clone(),
        composes: Vec::new(),
    };
    let mut sources: BTreeMap<String, PathBuf> = BTreeMap::new();

    for spanned_compose in &root.composes {
        let known_queue_names: BTreeSet<String> =
            flat.queues.iter().map(|q| q.node.name.clone()).collect();
        let known_service_names: BTreeSet<String> =
            flat.services.iter().map(|s| s.node.name.clone()).collect();
        let known_trigger_names: BTreeSet<String> =
            flat.triggers.iter().map(|t| t.node.name.clone()).collect();

        let compose_ref = &spanned_compose.node;
        let child_path = resolve_child_path(compose_ref, compose_dir)?;

        if !visited.insert(child_path.clone()) {
            return Err(ComposeError::CircularComposeImport {
                path: child_path,
                chain: visited.iter().cloned().collect(),
            });
        }

        let child_root = load_compose(&child_path)?;
        let child_dir = child_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));

        let FlattenedPlan {
            root: child_flat,
            sources: child_sources,
        } = flatten_composes(&child_root, &child_path, child_dir, visited)?;

        visited.remove(&child_path);

        validate_overrides(compose_ref, &child_root, &child_path)?;

        merge_child_into_flat(
            compose_ref,
            &child_flat,
            child_dir,
            &child_path,
            compose_path,
            &known_queue_names,
            &known_service_names,
            &known_trigger_names,
            &mut flat,
            &mut sources,
        )?;

        sources.extend(child_sources);
    }

    Ok(FlattenedPlan {
        root: flat,
        sources,
    })
}

fn resolve_child_path(
    compose_ref: &iter_language::NamedCompose,
    compose_dir: &Path,
) -> Result<PathBuf, ComposeError> {
    let raw = if compose_ref.path.is_absolute() {
        compose_ref.path.clone()
    } else {
        compose_dir.join(&compose_ref.path)
    };
    std::fs::canonicalize(&raw).map_err(|e| ComposeError::io(&raw, e))
}

#[allow(clippy::too_many_arguments)]
fn merge_child_into_flat(
    compose_ref: &iter_language::NamedCompose,
    child_flat: &ComposeRoot,
    child_dir: &Path,
    child_path: &Path,
    compose_path: &Path,
    parent_queue_names: &BTreeSet<String>,
    parent_service_names: &BTreeSet<String>,
    parent_trigger_names: &BTreeSet<String>,
    flat: &mut ComposeRoot,
    sources: &mut BTreeMap<String, PathBuf>,
) -> Result<(), ComposeError> {
    let mut imported_services: Vec<Spanned<NamedService>> = Vec::new();
    let mut imported_triggers: Vec<Spanned<NamedTrigger>> = Vec::new();

    for child_service in &child_flat.services {
        let mut service = child_service.clone();
        if let ServiceSource::Build { ref mut path, .. } = service.node.source {
            if path.is_relative() {
                *path = child_dir.join(&*path);
            }
        }
        if let Some(svc_override) = compose_ref.services.get(&service.node.name) {
            if let Some(ref queue_ref) = svc_override.queue {
                match &mut service.node.source {
                    ServiceSource::Build { queue, .. } => {
                        *queue = Some(queue_ref.clone());
                    }
                    ServiceSource::Inline(inline) => {
                        inline.queue = Some(queue_ref.clone());
                    }
                }
            }
        }
        imported_services.push(service);
    }

    for child_trigger in &child_flat.triggers {
        let name = &child_trigger.node.name;
        match compose_ref.triggers.get(name) {
            Some(ComposeTriggerOverride::Disabled) => {}
            Some(ComposeTriggerOverride::Override { target }) => {
                let mut trigger = child_trigger.clone();
                if let Some(queue_ref) = target {
                    trigger.node.target = queue_ref.clone();
                }
                imported_triggers.push(trigger);
            }
            None => {
                imported_triggers.push(child_trigger.clone());
            }
        }
    }

    for child_queue in &child_flat.queues {
        let name = &child_queue.node.name;
        if let Some(parent_ref) = compose_ref.queues.get(name) {
            rebind_queue_refs_in_services(&mut imported_services, name, parent_ref);
            rebind_queue_refs_in_triggers(&mut imported_triggers, name, parent_ref);
        } else {
            check_name_collision(name, "queue", parent_queue_names, child_path, compose_path)?;
            let mut queue = child_queue.clone();
            if let QueueDecl::File { ref mut path } = queue.node.decl {
                let p = PathBuf::from(&*path);
                if p.is_relative() {
                    *path = child_dir.join(&p).to_string_lossy().into_owned();
                }
            }
            sources.insert(name.clone(), child_path.to_path_buf());
            flat.queues.push(queue);
        }
    }

    for service in &imported_services {
        let name = &service.node.name;
        check_name_collision(
            name,
            "service",
            parent_service_names,
            child_path,
            compose_path,
        )?;
        sources.insert(name.clone(), child_path.to_path_buf());
    }
    flat.services.extend(imported_services);

    for trigger in &imported_triggers {
        let name = &trigger.node.name;
        check_name_collision(
            name,
            "trigger",
            parent_trigger_names,
            child_path,
            compose_path,
        )?;
        sources.insert(name.clone(), child_path.to_path_buf());
    }
    flat.triggers.extend(imported_triggers);

    Ok(())
}

fn validate_overrides(
    compose_ref: &iter_language::NamedCompose,
    child_root: &ComposeRoot,
    child_path: &Path,
) -> Result<(), ComposeError> {
    for queue_name in compose_ref.queues.keys() {
        if !child_root.queues.iter().any(|q| &q.node.name == queue_name) {
            return Err(ComposeError::UnknownChildQueue {
                compose_path: child_path.to_path_buf(),
                queue_name: queue_name.clone(),
            });
        }
    }
    for service_name in compose_ref.services.keys() {
        if !child_root
            .services
            .iter()
            .any(|s| &s.node.name == service_name)
        {
            return Err(ComposeError::UnknownChildService {
                compose_path: child_path.to_path_buf(),
                service_name: service_name.clone(),
            });
        }
    }
    for trigger_name in compose_ref.triggers.keys() {
        if !child_root
            .triggers
            .iter()
            .any(|t| &t.node.name == trigger_name)
        {
            return Err(ComposeError::UnknownChildTrigger {
                compose_path: child_path.to_path_buf(),
                trigger_name: trigger_name.clone(),
            });
        }
    }
    Ok(())
}

fn check_name_collision(
    name: &str,
    kind: &'static str,
    parent_names: &BTreeSet<String>,
    child_path: &Path,
    parent_path: &Path,
) -> Result<(), ComposeError> {
    if parent_names.contains(name) {
        return Err(ComposeError::ComposeNameCollision {
            kind,
            name: name.to_owned(),
            child_path: child_path.to_path_buf(),
            parent_path: parent_path.to_path_buf(),
        });
    }
    Ok(())
}

fn rebind_queue_refs_in_services(
    services: &mut [Spanned<NamedService>],
    old_name: &str,
    new_ref: &QueueRef,
) {
    for service in services.iter_mut() {
        let queue_slot: &mut Option<QueueRef> = match &mut service.node.source {
            ServiceSource::Build { queue, .. } => queue,
            ServiceSource::Inline(inline) => &mut inline.queue,
        };
        if let &mut Some(QueueRef::Named(ref n)) = queue_slot {
            if n == old_name {
                *queue_slot = Some(new_ref.clone());
            }
        }
    }
}

fn rebind_queue_refs_in_triggers(
    triggers: &mut [Spanned<NamedTrigger>],
    old_name: &str,
    new_ref: &QueueRef,
) {
    for trigger in triggers.iter_mut() {
        if let QueueRef::Named(ref n) = trigger.node.target {
            if n == old_name {
                trigger.node.target = new_ref.clone();
            }
        }
    }
}

pub(crate) fn lookup_queue(
    queue_ref: &QueueRef,
    queues: &BTreeMap<String, Arc<AnyQueue>>,
) -> Result<Arc<AnyQueue>, ComposeError> {
    match queue_ref {
        QueueRef::Named(name) => queues
            .get(name)
            .cloned()
            .ok_or_else(|| ComposeError::UnknownQueue(name.clone())),
        QueueRef::Anonymous => Err(ComposeError::UnresolvedAnonymousQueueRef),
    }
}

fn lookup_queue_decl(queue_ref: &QueueRef, root: &ComposeRoot) -> Result<QueueDecl, ComposeError> {
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
    fn flatten_imports_child_queues_services_triggers() {
        let parent_dir = tempfile::tempdir().expect("tmp");
        let child_dir = parent_dir.path().join("child");
        std::fs::create_dir_all(&child_dir).expect("mkdir");

        write_compose(
            &child_dir,
            r#"
                queue child_q file { path = "./.iter/child_queue" }
                trigger child_cron cron { schedule = "0 * * * *" target = child_q }
            "#,
        );

        let parent_path = write_compose(
            parent_dir.path(),
            r#"
                queue parent_q file { path = "./.iter/parent_queue" }
                compose child { build = "./child/compose.iter" }
            "#,
        );

        let root = load_compose(&parent_path).expect("load parent");
        let mut visited = BTreeSet::new();
        visited.insert(std::fs::canonicalize(&parent_path).unwrap());
        let result = flatten_composes(
            &root,
            &std::fs::canonicalize(&parent_path).unwrap(),
            parent_dir.path(),
            &mut visited,
        )
        .expect("flatten");

        let queue_names: Vec<_> = result
            .root
            .queues
            .iter()
            .map(|q| q.node.name.as_str())
            .collect();
        assert!(queue_names.contains(&"parent_q"), "got: {queue_names:?}");
        assert!(queue_names.contains(&"child_q"), "got: {queue_names:?}");

        let trigger_names: Vec<_> = result
            .root
            .triggers
            .iter()
            .map(|t| t.node.name.as_str())
            .collect();
        assert!(
            trigger_names.contains(&"child_cron"),
            "got: {trigger_names:?}"
        );

        assert!(result.sources.contains_key("child_q"));
        assert!(result.sources.contains_key("child_cron"));
    }

    #[test]
    fn flatten_queue_override_rebinds_child_triggers() {
        let parent_dir = tempfile::tempdir().expect("tmp");
        let child_dir = parent_dir.path().join("child");
        std::fs::create_dir_all(&child_dir).expect("mkdir");

        write_compose(
            &child_dir,
            r#"
                queue child_q file { path = "./.iter/child_queue" }
                trigger child_cron cron { schedule = "0 * * * *" target = child_q }
            "#,
        );

        let parent_path = write_compose(
            parent_dir.path(),
            r#"
                queue parent_q file { path = "./.iter/parent_queue" }
                compose child {
                    build = "./child/compose.iter"
                    queues = {
                        child_q = parent_q
                    }
                }
            "#,
        );

        let root = load_compose(&parent_path).expect("load parent");
        let mut visited = BTreeSet::new();
        visited.insert(std::fs::canonicalize(&parent_path).unwrap());
        let result = flatten_composes(
            &root,
            &std::fs::canonicalize(&parent_path).unwrap(),
            parent_dir.path(),
            &mut visited,
        )
        .expect("flatten");

        let queue_names: Vec<_> = result
            .root
            .queues
            .iter()
            .map(|q| q.node.name.as_str())
            .collect();
        assert_eq!(
            queue_names,
            vec!["parent_q"],
            "child_q should be elided by override"
        );

        let child_trigger = result
            .root
            .triggers
            .iter()
            .find(|t| t.node.name == "child_cron")
            .expect("child_cron trigger");
        assert!(
            matches!(&child_trigger.node.target, QueueRef::Named(n) if n == "parent_q"),
            "child_cron target should be rebound to parent_q, got: {:?}",
            child_trigger.node.target
        );
    }

    #[test]
    fn flatten_circular_import_rejected() {
        let dir = tempfile::tempdir().expect("tmp");
        let a_dir = dir.path().join("a");
        let b_dir = dir.path().join("b");
        std::fs::create_dir_all(&a_dir).expect("mkdir");
        std::fs::create_dir_all(&b_dir).expect("mkdir");

        write_compose(&a_dir, r#"compose b { build = "../b/compose.iter" }"#);
        write_compose(&b_dir, r#"compose a { build = "../a/compose.iter" }"#);

        let a_path = a_dir.join("compose.iter");
        let root = load_compose(&a_path).expect("load a");
        let canonical = std::fs::canonicalize(&a_path).unwrap();
        let mut visited = BTreeSet::new();
        visited.insert(canonical.clone());
        let err = flatten_composes(&root, &canonical, &a_dir, &mut visited)
            .expect_err("should detect cycle");
        assert!(
            matches!(err, ComposeError::CircularComposeImport { .. }),
            "expected CircularComposeImport, got: {err:?}"
        );
    }

    #[test]
    fn flatten_name_collision_rejected() {
        let parent_dir = tempfile::tempdir().expect("tmp");
        let child_dir = parent_dir.path().join("child");
        std::fs::create_dir_all(&child_dir).expect("mkdir");

        write_compose(
            &child_dir,
            r#"queue shared file { path = "./.iter/child_queue" }"#,
        );

        let parent_path = write_compose(
            parent_dir.path(),
            r#"
                queue shared file { path = "./.iter/parent_queue" }
                compose child { build = "./child/compose.iter" }
            "#,
        );

        let root = load_compose(&parent_path).expect("load parent");
        let canonical = std::fs::canonicalize(&parent_path).unwrap();
        let mut visited = BTreeSet::new();
        visited.insert(canonical.clone());
        let err = flatten_composes(&root, &canonical, parent_dir.path(), &mut visited)
            .expect_err("should detect collision");
        assert!(
            matches!(err, ComposeError::ComposeNameCollision { kind: "queue", ref name, .. } if name == "shared"),
            "expected ComposeNameCollision for 'shared', got: {err:?}"
        );
    }

    #[test]
    fn flatten_trigger_disabled_by_override() {
        let parent_dir = tempfile::tempdir().expect("tmp");
        let child_dir = parent_dir.path().join("child");
        std::fs::create_dir_all(&child_dir).expect("mkdir");

        write_compose(
            &child_dir,
            r#"
                queue child_q file { path = "./.iter/child_queue" }
                trigger noisy cron { schedule = "* * * * *" target = child_q }
                trigger keeper cron { schedule = "0 0 * * *" target = child_q }
            "#,
        );

        let parent_path = write_compose(
            parent_dir.path(),
            r#"
                compose child {
                    build = "./child/compose.iter"
                    triggers = {
                        noisy = disabled
                    }
                }
            "#,
        );

        let root = load_compose(&parent_path).expect("load parent");
        let canonical = std::fs::canonicalize(&parent_path).unwrap();
        let mut visited = BTreeSet::new();
        visited.insert(canonical.clone());
        let result =
            flatten_composes(&root, &canonical, parent_dir.path(), &mut visited).expect("flatten");

        let trigger_names: Vec<_> = result
            .root
            .triggers
            .iter()
            .map(|t| t.node.name.as_str())
            .collect();
        assert!(
            !trigger_names.contains(&"noisy"),
            "noisy should be disabled, got: {trigger_names:?}"
        );
        assert!(
            trigger_names.contains(&"keeper"),
            "keeper should remain, got: {trigger_names:?}"
        );
    }

    #[test]
    fn flatten_child_queue_path_resolved_relative_to_child_dir() {
        let parent_dir = tempfile::tempdir().expect("tmp");
        let child_dir = parent_dir.path().join("sub");
        std::fs::create_dir_all(&child_dir).expect("mkdir");

        write_compose(
            &child_dir,
            r#"queue child_q file { path = "./data/queue" }"#,
        );

        let parent_path = write_compose(
            parent_dir.path(),
            r#"compose child { build = "./sub/compose.iter" }"#,
        );

        let root = load_compose(&parent_path).expect("load parent");
        let canonical = std::fs::canonicalize(&parent_path).unwrap();
        let mut visited = BTreeSet::new();
        visited.insert(canonical.clone());
        let result =
            flatten_composes(&root, &canonical, parent_dir.path(), &mut visited).expect("flatten");

        let child_queue = result
            .root
            .queues
            .iter()
            .find(|q| q.node.name == "child_q")
            .expect("child_q");
        if let QueueDecl::File { path } = &child_queue.node.decl {
            let canonical_child_dir = std::fs::canonicalize(&child_dir).unwrap();
            assert!(
                path.contains(canonical_child_dir.to_str().unwrap()),
                "child queue path should be resolved relative to child dir, got: {path}"
            );
        } else {
            panic!("expected file queue decl");
        }
    }

    #[test]
    fn flatten_sibling_name_collision_rejected() {
        let parent_dir = tempfile::tempdir().expect("tmp");
        let child_a = parent_dir.path().join("a");
        let child_b = parent_dir.path().join("b");
        std::fs::create_dir_all(&child_a).expect("mkdir");
        std::fs::create_dir_all(&child_b).expect("mkdir");

        write_compose(
            &child_a,
            r#"queue shared file { path = "./.iter/a_queue" }"#,
        );
        write_compose(
            &child_b,
            r#"queue shared file { path = "./.iter/b_queue" }"#,
        );

        let parent_path = write_compose(
            parent_dir.path(),
            r#"
                compose a { build = "./a/compose.iter" }
                compose b { build = "./b/compose.iter" }
            "#,
        );

        let root = load_compose(&parent_path).expect("load parent");
        let canonical = std::fs::canonicalize(&parent_path).unwrap();
        let mut visited = BTreeSet::new();
        visited.insert(canonical.clone());
        let err = flatten_composes(&root, &canonical, parent_dir.path(), &mut visited)
            .expect_err("should detect sibling collision");
        assert!(
            matches!(err, ComposeError::ComposeNameCollision { kind: "queue", ref name, .. } if name == "shared"),
            "expected ComposeNameCollision for 'shared', got: {err:?}"
        );
    }

    #[test]
    fn validate_overrides_rejects_grandchild_name() {
        let parent_dir = tempfile::tempdir().expect("tmp");
        let child_dir = parent_dir.path().join("child");
        let grandchild_dir = parent_dir.path().join("grandchild");
        std::fs::create_dir_all(&child_dir).expect("mkdir");
        std::fs::create_dir_all(&grandchild_dir).expect("mkdir");

        write_compose(
            &grandchild_dir,
            r#"queue grandchild_q file { path = "./.iter/gc_queue" }"#,
        );
        write_compose(
            &child_dir,
            &format!(
                r#"compose gc {{ build = "{}" }}"#,
                grandchild_dir.join("compose.iter").display()
            ),
        );

        let parent_path = write_compose(
            parent_dir.path(),
            &format!(
                r#"
                queue parent_q file {{ path = "./.iter/parent_queue" }}
                compose child {{
                    build = "{}"
                    queues = {{
                        grandchild_q = parent_q
                    }}
                }}
            "#,
                child_dir.join("compose.iter").display()
            ),
        );

        let root = load_compose(&parent_path).expect("load parent");
        let canonical = std::fs::canonicalize(&parent_path).unwrap();
        let mut visited = BTreeSet::new();
        visited.insert(canonical.clone());
        let err = flatten_composes(&root, &canonical, parent_dir.path(), &mut visited)
            .expect_err("should reject grandchild override");
        assert!(
            matches!(err, ComposeError::UnknownChildQueue { ref queue_name, .. } if queue_name == "grandchild_q"),
            "expected UnknownChildQueue for 'grandchild_q', got: {err:?}"
        );
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
runner { continue_on_error = false behavior = wait }
prompt "noop"
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
runner { continue_on_error = false behavior = wait }
prompt "noop"
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
