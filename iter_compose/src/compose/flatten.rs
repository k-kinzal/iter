use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use iter_language::{
    ComposeRoot, ComposeTriggerOverride, NamedService, NamedTrigger, QueueDecl, QueueRef,
    ServiceSource, Spanned,
};

use super::error::ComposeError;
use crate::compose::load_compose;

#[derive(Debug)]
pub(super) struct FlattenedPlan {
    pub(super) root: ComposeRoot,
    pub(super) sources: BTreeMap<String, PathBuf>,
}

pub(super) fn flatten_composes(
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
