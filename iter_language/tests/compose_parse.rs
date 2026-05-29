//! Tests for [`iter_language::parse_compose`].
//!
//! Covers the compose.iter top-level grammar (named queue/service/trigger),
//! queue reference resolution (single-queue inference, named lookup,
//! ambiguous and dangling references), the `service { build = ... }` vs
//! inline split, and the rejection paths that distinguish compose from
//! Iterfile syntax (`prompt`/`runner`/top-level `on` are not allowed at
//! compose root).

use iter_language::{
    ComposeRoot, ComposeTriggerOverride, EventName, NamedCompose, NamedQueue, NamedService,
    NamedTrigger, PromptExpr, PromptValue, QueueDecl, QueueRef, ServiceSource, TelemetryProtocol,
    TriggerDecl, WatchEventKind, parse_compose,
};

fn parse(src: &str) -> ComposeRoot {
    match parse_compose(src) {
        Ok(root) => root,
        Err(diagnostics) => panic!(
            "expected compose.iter to parse but got diagnostics:\n{diagnostics:#?}\nsource:\n{src}"
        ),
    }
}

fn parse_err(src: &str) -> Vec<String> {
    let diagnostics = parse_compose(src).expect_err("expected compose.iter to fail");
    diagnostics.into_iter().map(|d| d.message).collect()
}

#[test]
fn empty_compose_is_valid() {
    let root = parse("");
    assert!(root.telemetry.is_none());
    assert!(root.queues.is_empty());
    assert!(root.services.is_empty());
    assert!(root.triggers.is_empty());
}

#[test]
fn telemetry_block_lowers() {
    let root = parse(
        r#"
            telemetry {
                service_name = "iter-dev"
                service_namespace = "experiments"
                endpoint = "http://localhost:4318"
                protocol = "http/protobuf"
                resource_attributes {
                    "deployment.environment" = "dev"
                    "team.name" = "agents"
                }
            }
        "#,
    );
    let telemetry = root.telemetry.expect("telemetry").node;
    assert!(telemetry.enabled);
    assert_eq!(telemetry.service_name.as_deref(), Some("iter-dev"));
    assert_eq!(telemetry.service_namespace.as_deref(), Some("experiments"));
    assert_eq!(telemetry.endpoint.as_deref(), Some("http://localhost:4318"));
    assert_eq!(telemetry.protocol, TelemetryProtocol::HttpProtobuf);
    assert_eq!(
        telemetry.resource_attributes.get("deployment.environment"),
        Some(&"dev".to_string())
    );
    assert_eq!(
        telemetry.resource_attributes.get("team.name"),
        Some(&"agents".to_string())
    );
}

#[test]
fn duplicate_telemetry_block_rejected() {
    let errs = parse_err(
        r#"
            telemetry { endpoint = "http://a:4318" }
            telemetry { endpoint = "http://b:4318" }
        "#,
    );
    assert!(
        errs.iter().any(|m| m.contains("duplicate telemetry block")),
        "got: {errs:#?}"
    );
}

#[test]
fn named_queue_with_kind_lowers() {
    let root = parse(r#"queue main file { path = "./.iter/queue" }"#);
    assert_eq!(root.queues.len(), 1);
    let NamedQueue { name, decl } = &root.queues[0].node;
    assert_eq!(name, "main");
    assert!(matches!(decl, QueueDecl::File { path } if path == "./.iter/queue"));
}

#[test]
fn duplicate_queue_names_rejected() {
    let errs = parse_err(
        r#"
            queue main file { path = "./a" }
            queue main file { path = "./b" }
        "#,
    );
    assert!(
        errs.iter()
            .any(|m| m.contains("duplicate queue name `main`")),
        "got: {errs:#?}"
    );
}

#[test]
fn queue_without_kind_diagnoses() {
    let errs = parse_err(r#"queue main { path = "./a" }"#);
    assert!(
        errs.iter().any(|m| m.contains("requires a backend kind")),
        "got: {errs:#?}"
    );
}

#[test]
fn service_build_inherits_single_queue() {
    let root = parse(
        r#"
            queue main file { path = "./.iter/queue" }
            service runner { build = "./Iterfile" }
        "#,
    );
    assert_eq!(root.services.len(), 1);
    let NamedService { name, source } = &root.services[0].node;
    assert_eq!(name, "runner");
    let ServiceSource::Build { queue, .. } = source else {
        panic!("expected build source");
    };
    assert!(matches!(queue, Some(QueueRef::Named(name)) if name == "main"));
}

#[test]
fn service_with_explicit_queue() {
    let root = parse(
        r#"
            queue main file { path = "./a" }
            queue logs file { path = "./b" }
            service runner { build = "./Iterfile" queue = main }
        "#,
    );
    let ServiceSource::Build { queue, .. } = &root.services[0].node.source else {
        panic!("expected build")
    };
    assert!(matches!(queue, Some(QueueRef::Named(name)) if name == "main"));
}

#[test]
fn ambiguous_queue_reference_rejected() {
    let errs = parse_err(
        r#"
            queue main file { path = "./a" }
            queue logs file { path = "./b" }
            service runner { build = "./Iterfile" }
        "#,
    );
    assert!(
        errs.iter().any(|m| m.contains("more than one queue")),
        "got: {errs:#?}"
    );
}

#[test]
fn dangling_queue_reference_rejected() {
    let errs = parse_err(
        r#"
            queue main file { path = "./a" }
            service runner { build = "./Iterfile" queue = ghost }
        "#,
    );
    assert!(
        errs.iter().any(|m| m.contains("`ghost` is not declared")),
        "got: {errs:#?}"
    );
}

#[test]
fn inline_service_runner_carries_prompt_and_events() {
    let root = parse(
        r#"
            queue main file { path = "./.iter/queue" }
            service worker {
                queue = main
                workspace_local { base = "." }
                agent_claude { mode = print command = "claude" }
                runner {
                    continue_on_error = false
                    behavior = loop
                    prompt = "explore the workspace"
                    on agent_finished { shell "echo done" }
                }
            }
        "#,
    );
    let ServiceSource::Inline(inline) = &root.services[0].node.source else {
        panic!("expected inline source");
    };
    let runner = inline.runner.as_ref().expect("inline runner present");
    assert!(
        matches!(&runner.node.prompt, PromptExpr::Single(PromptValue::Inline(s)) if s == "explore the workspace"),
        "prompt must flow through the runner: {:?}",
        runner.node.prompt,
    );
    assert_eq!(
        runner.node.events.len(),
        1,
        "event handler must flow through the runner",
    );
    assert_eq!(runner.node.events[0].node.event, EventName::AgentFinished);
}

#[test]
fn inline_service_runner_supports_prompt_match() {
    let root = parse(
        r#"
            queue main file { path = "./.iter/queue" }
            service worker {
                queue = main
                workspace_local { base = "." }
                agent_claude { mode = print command = "claude" }
                runner {
                    continue_on_error = false
                    behavior = loop
                    prompt {
                        iteration.count % 25 == 0 => "review changes"
                        _                         => "refactor the module"
                    }
                }
            }
        "#,
    );
    let ServiceSource::Inline(inline) = &root.services[0].node.source else {
        panic!("expected inline source");
    };
    let runner = inline.runner.as_ref().expect("inline runner present");
    let PromptExpr::Match { arms, default } = &runner.node.prompt else {
        panic!("expected prompt match expression, got {:?}", runner.node.prompt);
    };
    assert_eq!(arms.len(), 1, "one guarded arm expected");
    assert!(matches!(default, PromptValue::Inline(s) if s == "refactor the module"));
}

#[test]
fn inline_service_runner_rejects_prompt_ref() {
    let errs = parse_err(
        r#"
            queue main file { path = "./.iter/queue" }
            service worker {
                queue = main
                workspace_local { base = "." }
                agent_claude { mode = print command = "claude" }
                runner {
                    continue_on_error = false
                    behavior = loop
                    prompt = recovery
                }
            }
        "#,
    );
    assert!(
        errs.iter().any(|m| m
            .contains("named prompt reference `recovery` is not valid in an inline service runner")),
        "got: {errs:#?}"
    );
}

#[test]
fn inline_service_runner_requires_prompt() {
    let errs = parse_err(
        r#"
            queue main file { path = "./.iter/queue" }
            service worker {
                queue = main
                workspace_local { base = "." }
                agent_claude { mode = print command = "claude" }
                runner {
                    continue_on_error = false
                    behavior = loop
                }
            }
        "#,
    );
    assert!(
        errs.iter().any(|m| m.contains("runner requires `prompt`")),
        "got: {errs:#?}"
    );
}

#[test]
fn trigger_cron_with_target() {
    let root = parse(
        r#"
            queue main file { path = "./a" }
            trigger nightly cron { schedule = "0 0 * * *" target = main }
        "#,
    );
    assert_eq!(root.triggers.len(), 1);
    let NamedTrigger {
        name,
        decl,
        target,
        terminate_on_completion,
    } = &root.triggers[0].node;
    assert_eq!(name, "nightly");
    assert!(matches!(decl, TriggerDecl::Cron { schedule, .. } if schedule == "0 0 * * *"));
    assert!(matches!(target, QueueRef::Named(n) if n == "main"));
    assert!(!terminate_on_completion);
}

#[test]
fn trigger_terminate_on_completion() {
    let root = parse(
        r#"
            queue main file { path = "./a" }
            trigger batch files {
                from = ["path:./inbox.txt"]
                target = main
                terminate_on_completion = true
            }
        "#,
    );
    assert_eq!(root.triggers.len(), 1);
    let NamedTrigger {
        name,
        target,
        terminate_on_completion,
        ..
    } = &root.triggers[0].node;
    assert_eq!(name, "batch");
    assert!(matches!(target, QueueRef::Named(n) if n == "main"));
    assert!(terminate_on_completion);
}

#[test]
fn trigger_terminate_on_completion_defaults_to_false() {
    let root = parse(
        r#"
            queue main file { path = "./a" }
            trigger nightly cron { schedule = "0 0 * * *" target = main }
        "#,
    );
    assert!(!root.triggers[0].node.terminate_on_completion);
}

#[test]
fn trigger_target_wrong_type_diagnoses() {
    let errs = parse_err(
        r#"
            queue main file { path = "./a" }
            queue logs file { path = "./b" }
            trigger nightly cron { schedule = "0 0 * * *" target = 42 }
        "#,
    );
    assert!(
        errs.iter()
            .any(|m| m.contains("`target` must be a queue name")),
        "got: {errs:#?}"
    );
}

#[test]
fn trigger_loop_kind_rejected() {
    let errs = parse_err(
        r#"
            queue main file { path = "./a" }
            trigger ralph loop { delay = 30s target = main }
        "#,
    );
    assert!(
        errs.iter()
            .any(|m| m.contains("`loop` is no longer a trigger kind")),
        "got: {errs:#?}"
    );
}

#[test]
fn prompt_at_compose_root_rejected() {
    let errs = parse_err(r#"prompt "hello""#);
    assert!(
        errs.iter()
            .any(|m| m.contains("`prompt` is not a valid top-level compose.iter section")),
        "got: {errs:#?}"
    );
}

#[test]
fn unknown_top_level_keyword_rejected() {
    let errs = parse_err("network main { driver = bridge }");
    assert!(
        errs.iter()
            .any(|m| m.contains("unknown compose.iter top-level keyword `network`")),
        "got: {errs:#?}"
    );
}

// --- Nested compose block tests ---

#[test]
fn compose_block_parses() {
    let root = parse(
        r#"
            queue main file { path = "./.iter/queue" }
            compose child { build = "./child/compose.iter" }
        "#,
    );
    assert_eq!(root.composes.len(), 1);
    let NamedCompose {
        name,
        path,
        queues,
        services,
        triggers,
    } = &root.composes[0].node;
    assert_eq!(name, "child");
    assert_eq!(path.to_str().unwrap(), "./child/compose.iter");
    assert!(queues.is_empty());
    assert!(services.is_empty());
    assert!(triggers.is_empty());
}

#[test]
fn compose_block_with_queue_override() {
    let root = parse(
        r#"
            queue parent_q file { path = "./q" }
            compose child {
                build = "./child/compose.iter"
                queues = {
                    child_q = parent_q
                }
            }
        "#,
    );
    assert_eq!(root.composes.len(), 1);
    let overrides = &root.composes[0].node.queues;
    assert_eq!(overrides.len(), 1);
    assert!(matches!(overrides.get("child_q"), Some(QueueRef::Named(n)) if n == "parent_q"));
}

#[test]
fn compose_block_with_service_override() {
    let root = parse(
        r#"
            queue main file { path = "./q" }
            compose child {
                build = "./child/compose.iter"
                services = {
                    worker = {
                        queue = main
                    }
                }
            }
        "#,
    );
    let svc_overrides = &root.composes[0].node.services;
    assert_eq!(svc_overrides.len(), 1);
    let worker = svc_overrides.get("worker").unwrap();
    assert!(matches!(&worker.queue, Some(QueueRef::Named(n)) if n == "main"));
}

#[test]
fn compose_block_with_trigger_disable() {
    let root = parse(
        r#"
            queue main file { path = "./q" }
            compose child {
                build = "./child/compose.iter"
                triggers = {
                    noisy = disabled
                }
            }
        "#,
    );
    let trig_overrides = &root.composes[0].node.triggers;
    assert_eq!(trig_overrides.len(), 1);
    assert!(matches!(
        trig_overrides.get("noisy"),
        Some(ComposeTriggerOverride::Disabled)
    ));
}

#[test]
fn compose_block_with_trigger_target_override() {
    let root = parse(
        r#"
            queue main file { path = "./q" }
            compose child {
                build = "./child/compose.iter"
                triggers = {
                    scanner = {
                        target = main
                    }
                }
            }
        "#,
    );
    let trig_overrides = &root.composes[0].node.triggers;
    assert!(matches!(
        trig_overrides.get("scanner"),
        Some(ComposeTriggerOverride::Override {
            target: Some(QueueRef::Named(n))
        }) if n == "main"
    ));
}

#[test]
fn compose_block_missing_build_rejected() {
    let errs = parse_err(
        r#"
            queue main file { path = "./q" }
            compose child { queues = { a = main } }
        "#,
    );
    assert!(
        errs.iter().any(|m| m.contains("requires a `build` field")),
        "got: {errs:#?}"
    );
}

#[test]
fn compose_block_missing_body_rejected() {
    let errs = parse_err("compose child");
    assert!(
        errs.iter().any(|m| m.contains("requires a body")),
        "got: {errs:#?}"
    );
}

#[test]
fn duplicate_compose_name_rejected() {
    let errs = parse_err(
        r#"
            compose a { build = "./a/compose.iter" }
            compose a { build = "./b/compose.iter" }
        "#,
    );
    assert!(
        errs.iter()
            .any(|m| m.contains("duplicate compose name `a`")),
        "got: {errs:#?}"
    );
}

#[test]
fn compose_queue_override_dangling_rejected() {
    let errs = parse_err(
        r#"
            compose child {
                build = "./child/compose.iter"
                queues = {
                    child_q = ghost
                }
            }
        "#,
    );
    assert!(
        errs.iter().any(|m| m.contains("`ghost` is not declared")),
        "got: {errs:#?}"
    );
}

#[test]
fn watch_kinds_duplicate_deduplicates_in_ast() {
    let root = parse(
        r#"
            queue main file { path = "./.iter/queue" }
            trigger t watch {
                dir = "./src"
                kinds = ["created", "created", "modified"]
                target = main
            }
        "#,
    );
    let trigger = &root.triggers[0].node;
    let TriggerDecl::Watch { ref kinds, .. } = trigger.decl else {
        panic!("expected Watch trigger");
    };
    assert_eq!(
        kinds,
        &[WatchEventKind::Created, WatchEventKind::Modified],
        "duplicates should be removed from the AST"
    );
}

#[test]
fn watch_kinds_invalid_value_is_rejected() {
    let errs = parse_err(
        r#"
            queue main file { path = "./.iter/queue" }
            trigger t watch {
                dir = "./src"
                kinds = ["created", "deleted"]
                target = main
            }
        "#,
    );
    assert!(
        errs.iter()
            .any(|m| m.contains("unknown watch event kind `deleted`")),
        "got: {errs:#?}"
    );
}
