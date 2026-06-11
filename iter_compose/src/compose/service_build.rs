use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iter_language::{InlineService, Iterfile, QueueDef, ServiceSource, parse};

use super::error::ComposeError;
use super::plan::{ComposeService, lookup_queue};
use crate::assembly;
use iter_core::Queue;

pub(super) fn build_service(
    name: &str,
    source: &ServiceSource,
    queues: &BTreeMap<String, Arc<dyn Queue>>,
    compose_dir: &Path,
    compose_path: &Path,
    once: bool,
    queue_decl: QueueDef,
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
            if !root.queues.is_empty() {
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
    root: &Iterfile,
    queue: Arc<dyn Queue>,
    iterfile_path: PathBuf,
    once: bool,
    queue_decl: QueueDef,
) -> Result<ComposeService, ComposeError> {
    let runner = root
        .runners
        .first()
        .ok_or_else(|| ComposeError::ServiceMissingSection {
            service: name.to_owned(),
            section: "runner",
        })?;
    if root.workspaces.is_empty() {
        return Err(ComposeError::ServiceMissingSection {
            service: name.to_owned(),
            section: "workspace",
        });
    }
    if root.agents.is_empty() {
        return Err(ComposeError::ServiceMissingSection {
            service: name.to_owned(),
            section: "agent",
        });
    }

    let builder =
        assembly::assemble_from_root(root, &runner.node, Some(queue), once).map_err(|source| {
            ComposeError::Assembly {
                service: name.to_owned(),
                source,
            }
        })?;

    Ok(ComposeService {
        name: name.to_string(),
        iterfile_path,
        queue_decl,
        builder,
    })
}

fn build_service_from_inline(
    name: &str,
    inline: &InlineService,
    queue: Arc<dyn Queue>,
    iterfile_path: PathBuf,
    once: bool,
    queue_decl: QueueDef,
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

    // Prompt and event data flow through the runner declaration. Inline
    // service prompts are always inline literals — the semantic analyzer
    // rejects named prompt references in inline runners — so the named-prompt
    // resolution set is empty here.
    let prompts = assembly::build_prompt_decls_from_expr_pub(&runner_decl.prompt, &[]);

    let builder = assembly::assemble_runner_builder(
        Some(queue),
        workspace_decl,
        agent_decl,
        runner_decl,
        &prompts,
        &runner_decl.events,
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
