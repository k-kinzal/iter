//! Arg resolution: merge Iterfile `arg` defaults with CLI/compose overrides,
//! then render `{{arg.*}}` in every string field of the parsed [`Root`].
//!
//! Substitution is a targeted textual expansion: only `{{arg.<name>}}`
//! references are replaced. Other template references (`{{signal.*}}`,
//! `{{metadata.*}}`, etc.) pass through unchanged — they are resolved
//! later at runtime by the runner.

use std::collections::BTreeMap;

use iter_language::Root;
use thiserror::Error;

/// Errors from arg resolution.
#[derive(Debug, Error)]
pub enum ArgError {
    /// A required arg (no default) was not supplied via override.
    #[error("missing required arg `{name}`")]
    MissingRequired {
        /// Arg name.
        name: String,
    },
    /// An override references an arg not declared in the Iterfile.
    #[error("unknown arg `{name}` (not declared in the Iterfile)")]
    UnknownOverride {
        /// Arg name.
        name: String,
    },
    /// A `{{arg.<name>}}` reference names an arg that was not declared.
    #[error("unknown arg reference `{{{{arg.{name}}}}}` in field value")]
    UnknownReference {
        /// Arg name from the template reference.
        name: String,
    },
}

/// Merge Iterfile-declared args with overrides, validate, and render
/// `{{arg.*}}` in all string fields of `root`.
///
/// `overrides` are CLI `--arg key=value` or compose `args { key = "value" }`
/// values that take precedence over Iterfile defaults.
///
/// # Errors
///
/// Returns [`ArgError::MissingRequired`] when a declared arg has no default
/// and no override, [`ArgError::UnknownOverride`] when an override names an
/// arg not declared in the Iterfile, or [`ArgError::UnknownReference`] when a
/// `{{arg.<name>}}` reference names an undeclared arg.
pub fn resolve_args(root: &mut Root, overrides: &BTreeMap<String, String>) -> Result<(), ArgError> {
    let declared: BTreeMap<&str, Option<&str>> = root
        .args
        .iter()
        .map(|a| (a.node.name.as_str(), a.node.default.as_deref()))
        .collect();

    for key in overrides.keys() {
        if !declared.contains_key(key.as_str()) {
            return Err(ArgError::UnknownOverride { name: key.clone() });
        }
    }

    let mut values: BTreeMap<String, String> = BTreeMap::new();
    for (name, default) in &declared {
        if let Some(override_val) = overrides.get(*name) {
            values.insert((*name).to_owned(), override_val.clone());
        } else if let Some(default_val) = default {
            values.insert((*name).to_owned(), (*default_val).to_owned());
        } else {
            return Err(ArgError::MissingRequired {
                name: (*name).to_owned(),
            });
        }
    }

    render_root(root, &values)
}

/// Replace `{{arg.<name>}}` references in `s` with resolved values.
///
/// Only `{{arg.*}}` references are touched; all other `{{...}}` patterns
/// (e.g. `{{signal.id}}`, `{{metadata.task}}`) pass through unchanged.
fn render_str(s: &mut String, values: &BTreeMap<String, String>) -> Result<(), ArgError> {
    const PREFIX: &str = "{{arg.";

    if !s.contains(PREFIX) {
        return Ok(());
    }

    let mut result = String::with_capacity(s.len());
    let mut rest = s.as_str();

    while let Some(prefix_offset) = rest.find(PREFIX) {
        result.push_str(&rest[..prefix_offset]);
        let after_prefix = &rest[prefix_offset + PREFIX.len()..];

        if let Some(name_len) = arg_name_end(after_prefix) {
            let name = &after_prefix[..name_len];
            let Some(val) = values.get(name) else {
                return Err(ArgError::UnknownReference {
                    name: name.to_owned(),
                });
            };
            result.push_str(val);
            rest = &after_prefix[name_len + 2..]; // skip past name + `}}`
        } else {
            result.push_str(&rest[prefix_offset..=prefix_offset]);
            rest = &rest[prefix_offset + 1..];
        }
    }
    result.push_str(rest);

    *s = result;
    Ok(())
}

/// If `s` starts with a valid arg-name followed by `}}`, returns the byte
/// length of the name. Returns `None` for empty names, unclosed braces,
/// or names containing non-identifier characters.
fn arg_name_end(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'}' && bytes[i + 1] == b'}' {
            return (i > 0).then_some(i);
        }
        let c = bytes[i];
        if !(c.is_ascii_alphanumeric() || c == b'_') {
            return None;
        }
        i += 1;
    }
    None
}

fn render_root(root: &mut Root, values: &BTreeMap<String, String>) -> Result<(), ArgError> {
    for ws in &mut root.workspaces {
        render_workspace(&mut ws.node.decl, values)?;
    }
    for agent in &mut root.agents {
        render_agent(&mut agent.node.decl, values)?;
    }
    for queue in &mut root.queues {
        render_queue(&mut queue.node.decl, values)?;
    }
    for prompt in &mut root.prompts {
        render_str(&mut prompt.node.body, values)?;
    }
    for runner in &mut root.runners {
        for event in &mut runner.node.events {
            render_event(&mut event.node, values)?;
        }
        // Render inline prompt strings.
        render_prompt_expr(&mut runner.node.prompt, values)?;
    }
    Ok(())
}

fn render_prompt_expr(
    expr: &mut iter_language::PromptExpr,
    values: &BTreeMap<String, String>,
) -> Result<(), ArgError> {
    match expr {
        iter_language::PromptExpr::Single(v) => render_prompt_value(v, values),
        iter_language::PromptExpr::Match { arms, default } => {
            for arm in arms {
                render_prompt_value(&mut arm.value, values)?;
            }
            render_prompt_value(default, values)
        }
    }
}

fn render_prompt_value(
    value: &mut iter_language::PromptValue,
    values: &BTreeMap<String, String>,
) -> Result<(), ArgError> {
    match value {
        iter_language::PromptValue::Inline(s) => render_str(s, values),
        iter_language::PromptValue::Ref(_) => Ok(()),
    }
}

fn render_workspace(
    ws: &mut iter_language::WorkspaceDecl,
    values: &BTreeMap<String, String>,
) -> Result<(), ArgError> {
    match ws {
        iter_language::WorkspaceDecl::Local { base } => {
            render_str(base, values)?;
        }
        iter_language::WorkspaceDecl::Clone {
            base,
            remote,
            excludes,
            includes,
            ..
        } => {
            render_str(base, values)?;
            if let Some(r) = remote {
                render_str(r, values)?;
            }
            for s in excludes.iter_mut() {
                render_str(s, values)?;
            }
            for s in includes.iter_mut() {
                render_str(s, values)?;
            }
        }
        iter_language::WorkspaceDecl::Sandbox {
            base,
            excludes,
            includes,
            ..
        } => {
            render_str(base, values)?;
            for s in excludes.iter_mut() {
                render_str(s, values)?;
            }
            for s in includes.iter_mut() {
                render_str(s, values)?;
            }
        }
    }
    Ok(())
}

fn render_agent(
    agent: &mut iter_language::AgentDecl,
    values: &BTreeMap<String, String>,
) -> Result<(), ArgError> {
    match agent {
        iter_language::AgentDecl::Claude {
            command, args, env, ..
        }
        | iter_language::AgentDecl::Codex {
            command, args, env, ..
        }
        | iter_language::AgentDecl::Gemini {
            command, args, env, ..
        }
        | iter_language::AgentDecl::Hermes {
            command, args, env, ..
        }
        | iter_language::AgentDecl::Antigravity {
            command, args, env, ..
        }
        | iter_language::AgentDecl::Copilot {
            command, args, env, ..
        }
        | iter_language::AgentDecl::Cursor {
            command, args, env, ..
        }
        | iter_language::AgentDecl::Cline {
            command, args, env, ..
        }
        | iter_language::AgentDecl::OpenCode {
            command, args, env, ..
        } => {
            render_str(command, values)?;
            for a in args.iter_mut() {
                render_str(a, values)?;
            }
            for v in env.values_mut() {
                render_str(v, values)?;
            }
        }
        iter_language::AgentDecl::Generic { command, env } => {
            for c in command.iter_mut() {
                render_str(c, values)?;
            }
            for v in env.values_mut() {
                render_str(v, values)?;
            }
        }
        iter_language::AgentDecl::Router { agents, .. } => {
            for (_name, sub_decl) in agents.iter_mut() {
                render_agent(sub_decl, values)?;
            }
        }
    }
    Ok(())
}

fn render_queue(
    queue: &mut iter_language::QueueDecl,
    values: &BTreeMap<String, String>,
) -> Result<(), ArgError> {
    match queue {
        iter_language::QueueDecl::Memory => {}
        iter_language::QueueDecl::File { path } => {
            render_str(path, values)?;
        }
        iter_language::QueueDecl::Redis { url, key } => {
            render_str(url, values)?;
            render_str(key, values)?;
        }
        iter_language::QueueDecl::Shell {
            enqueue,
            dequeue,
            close,
            interpreter,
            ..
        } => {
            render_str(enqueue, values)?;
            render_str(dequeue, values)?;
            if let Some(c) = close {
                render_str(c, values)?;
            }
            if let Some(i) = interpreter {
                render_str(i, values)?;
            }
        }
        iter_language::QueueDecl::Sqs(cfg) => render_sqs(cfg, values)?,
        iter_language::QueueDecl::PubSub(cfg) => render_pubsub(cfg, values)?,
        iter_language::QueueDecl::Kafka(cfg) => render_kafka(cfg, values)?,
        iter_language::QueueDecl::Kinesis(cfg) => render_kinesis(cfg, values)?,
        iter_language::QueueDecl::ServiceBus(cfg) => render_servicebus(cfg, values)?,
    }
    Ok(())
}

fn render_sqs(
    cfg: &mut iter_language::SqsConfig,
    values: &BTreeMap<String, String>,
) -> Result<(), ArgError> {
    match &mut cfg.identity {
        iter_language::SqsIdentity::Url(url) => render_str(url, values)?,
        iter_language::SqsIdentity::NameWithAccount { name, account_id } => {
            render_str(name, values)?;
            render_str(account_id, values)?;
        }
        iter_language::SqsIdentity::Unset => {}
    }
    render_opt(&mut cfg.region, values)?;
    render_opt(&mut cfg.endpoint_url, values)?;
    Ok(())
}

fn render_pubsub(
    cfg: &mut iter_language::PubSubConfig,
    values: &BTreeMap<String, String>,
) -> Result<(), ArgError> {
    render_str(&mut cfg.project, values)?;
    render_str(&mut cfg.topic, values)?;
    render_str(&mut cfg.subscription, values)?;
    render_opt(&mut cfg.endpoint, values)?;
    Ok(())
}

fn render_kafka(
    cfg: &mut iter_language::KafkaConfig,
    values: &BTreeMap<String, String>,
) -> Result<(), ArgError> {
    render_str(&mut cfg.bootstrap_servers, values)?;
    render_opt(&mut cfg.client_id, values)?;
    Ok(())
}

fn render_kinesis(
    cfg: &mut iter_language::KinesisConfig,
    values: &BTreeMap<String, String>,
) -> Result<(), ArgError> {
    match &mut cfg.identity {
        iter_language::KinesisIdentity::Arn(s) | iter_language::KinesisIdentity::Name(s) => {
            render_str(s, values)?;
        }
        iter_language::KinesisIdentity::Unset => {}
    }
    render_opt(&mut cfg.region, values)?;
    render_opt(&mut cfg.endpoint_url, values)?;
    Ok(())
}

fn render_servicebus(
    cfg: &mut iter_language::ServiceBusConfig,
    values: &BTreeMap<String, String>,
) -> Result<(), ArgError> {
    render_opt(&mut cfg.fully_qualified_namespace, values)?;
    render_opt(&mut cfg.queue_name, values)?;
    render_opt(&mut cfg.topic_name, values)?;
    render_opt(&mut cfg.subscription_name, values)?;
    render_opt(&mut cfg.custom_endpoint_address, values)?;
    Ok(())
}

fn render_opt(opt: &mut Option<String>, values: &BTreeMap<String, String>) -> Result<(), ArgError> {
    if let Some(s) = opt {
        render_str(s, values)?;
    }
    Ok(())
}

fn render_event(
    event: &mut iter_language::EventHandlerDecl,
    values: &BTreeMap<String, String>,
) -> Result<(), ArgError> {
    for action in &mut event.actions {
        match action {
            iter_language::Action::Shell(cmd) => {
                render_str(cmd, values)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use iter_language::parse;

    #[test]
    fn resolve_args_renders_workspace_base() {
        let source = r#"
arg worktree_name = "default-name"
workspace local { base = "/path/to/{{arg.worktree_name}}" }
agent claude { mode = print command = "claude" }
runner { continue_on_error = false behavior = loop }
prompt "noop"
"#;
        let mut root = parse(source).expect("parse");
        resolve_args(&mut root, &BTreeMap::new()).expect("resolve");
        match &root.workspaces.first().unwrap().node.decl {
            iter_language::WorkspaceDecl::Local { base } => {
                assert_eq!(base, "/path/to/default-name");
            }
            other => panic!("unexpected workspace: {other:?}"),
        }
    }

    #[test]
    fn resolve_args_override_takes_precedence() {
        let source = r#"
arg worktree_name = "default-name"
workspace local { base = "/path/to/{{arg.worktree_name}}" }
agent claude { mode = print command = "claude" }
runner { continue_on_error = false behavior = loop }
prompt "noop"
"#;
        let mut root = parse(source).expect("parse");
        let mut overrides = BTreeMap::new();
        overrides.insert("worktree_name".to_owned(), "override-name".to_owned());
        resolve_args(&mut root, &overrides).expect("resolve");
        match &root.workspaces.first().unwrap().node.decl {
            iter_language::WorkspaceDecl::Local { base } => {
                assert_eq!(base, "/path/to/override-name");
            }
            other => panic!("unexpected workspace: {other:?}"),
        }
    }

    #[test]
    fn resolve_args_required_missing_errors() {
        let source = r#"
arg worktree_name
workspace local { base = "/path/to/{{arg.worktree_name}}" }
agent claude { mode = print command = "claude" }
runner { continue_on_error = false behavior = loop }
prompt "noop"
"#;
        let mut root = parse(source).expect("parse");
        let err = resolve_args(&mut root, &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, ArgError::MissingRequired { ref name } if name == "worktree_name"));
    }

    #[test]
    fn resolve_args_unknown_override_errors() {
        let source = r#"
workspace local { base = "." }
agent claude { mode = print command = "claude" }
runner { continue_on_error = false behavior = loop }
prompt "noop"
"#;
        let mut root = parse(source).expect("parse");
        let mut overrides = BTreeMap::new();
        overrides.insert("nope".to_owned(), "val".to_owned());
        let err = resolve_args(&mut root, &overrides).unwrap_err();
        assert!(matches!(err, ArgError::UnknownOverride { ref name } if name == "nope"));
    }

    #[test]
    fn resolve_args_renders_prompt_body() {
        let source = r#"
arg task = "review"
workspace local { base = "." }
agent claude { mode = print command = "claude" }
runner { continue_on_error = false behavior = loop }
prompt "Do the {{arg.task}} task."
"#;
        let mut root = parse(source).expect("parse");
        resolve_args(&mut root, &BTreeMap::new()).expect("resolve");
        match &root.runners.first().unwrap().node.prompt {
            iter_language::PromptExpr::Single(iter_language::PromptValue::Inline(s)) => {
                assert_eq!(s, "Do the review task.");
            }
            other => panic!("unexpected prompt expr: {other:?}"),
        }
    }

    #[test]
    fn resolve_args_renders_event_shell() {
        let source = r#"
arg dir = "/tmp/work"
workspace local { base = "." }
agent claude { mode = print command = "claude" }
runner { continue_on_error = false behavior = loop }
prompt "noop"
on runner_starting { shell "mkdir -p {{arg.dir}}" }
"#;
        let mut root = parse(source).expect("parse");
        resolve_args(&mut root, &BTreeMap::new()).expect("resolve");
        match &root.runners.first().unwrap().node.events[0].node.actions[0] {
            iter_language::Action::Shell(cmd) => {
                assert_eq!(cmd, "mkdir -p /tmp/work");
            }
        }
    }

    #[test]
    fn resolve_args_renders_agent_env_values() {
        let source = r#"
arg worktree = "exp-1"
workspace local { base = "." }
agent claude {
  mode = print
  command = "claude"
  env {
    WORKTREE_NAME = "{{arg.worktree}}"
  }
}
runner { continue_on_error = false behavior = loop }
prompt "noop"
"#;
        let mut root = parse(source).expect("parse");
        resolve_args(&mut root, &BTreeMap::new()).expect("resolve");
        let env = match &root.agents.first().unwrap().node.decl {
            iter_language::AgentDecl::Claude { env, .. } => env,
            other => panic!("unexpected agent: {other:?}"),
        };
        assert_eq!(env.get("WORKTREE_NAME"), Some(&"exp-1".to_string()));
    }

    #[test]
    fn resolve_args_no_args_is_noop() {
        let source = r#"
workspace local { base = "." }
agent claude { mode = print command = "claude" }
runner { continue_on_error = false behavior = loop }
prompt "noop"
"#;
        let mut root = parse(source).expect("parse");
        resolve_args(&mut root, &BTreeMap::new()).expect("resolve");
    }

    #[test]
    fn resolve_args_required_supplied_via_override() {
        let source = r#"
arg worktree_name
workspace local { base = "/path/to/{{arg.worktree_name}}" }
agent claude { mode = print command = "claude" }
runner { continue_on_error = false behavior = loop }
prompt "noop"
"#;
        let mut root = parse(source).expect("parse");
        let mut overrides = BTreeMap::new();
        overrides.insert("worktree_name".to_owned(), "supplied-name".to_owned());
        resolve_args(&mut root, &overrides).expect("resolve");
        match &root.workspaces.first().unwrap().node.decl {
            iter_language::WorkspaceDecl::Local { base } => {
                assert_eq!(base, "/path/to/supplied-name");
            }
            other => panic!("unexpected workspace: {other:?}"),
        }
    }

    #[test]
    fn runtime_templates_survive_arg_resolution() {
        let source = r#"
arg env = "staging"
workspace local { base = "." }
agent claude { mode = print command = "claude" }
runner { continue_on_error = false behavior = loop }
prompt "Deploy {{arg.env}} for {{signal.id}} via {{metadata.source}}."
"#;
        let mut root = parse(source).expect("parse");
        resolve_args(&mut root, &BTreeMap::new()).expect("resolve");
        match &root.runners.first().unwrap().node.prompt {
            iter_language::PromptExpr::Single(iter_language::PromptValue::Inline(s)) => {
                assert_eq!(
                    s,
                    "Deploy staging for {{signal.id}} via {{metadata.source}}."
                );
            }
            other => panic!("unexpected prompt expr: {other:?}"),
        }
    }

    #[test]
    fn unknown_arg_reference_errors() {
        let source = r#"
arg known = "val"
workspace local { base = "." }
agent claude { mode = print command = "claude" }
runner { continue_on_error = false behavior = loop }
prompt "{{arg.typo}}"
"#;
        let mut root = parse(source).expect("parse");
        let err = resolve_args(&mut root, &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, ArgError::UnknownReference { ref name } if name == "typo"));
    }

    #[test]
    fn non_ascii_content_preserved() {
        let source = "
arg greeting = \"hello\"
workspace local { base = \".\" }
agent claude { mode = print command = \"claude\" }
runner { continue_on_error = false behavior = loop }
prompt \"{{arg.greeting}} \u{4e16}\u{754c}\"
";
        let mut root = parse(source).expect("parse");
        resolve_args(&mut root, &BTreeMap::new()).expect("resolve");
        match &root.runners.first().unwrap().node.prompt {
            iter_language::PromptExpr::Single(iter_language::PromptValue::Inline(s)) => {
                assert_eq!(s, "hello \u{4e16}\u{754c}");
            }
            other => panic!("unexpected prompt expr: {other:?}"),
        }
    }

    #[test]
    fn undeclared_arg_reference_in_no_arg_file_errors() {
        let source = r#"
workspace local { base = "." }
agent claude { mode = print command = "claude" }
runner { continue_on_error = false behavior = loop }
prompt "{{arg.oops}}"
"#;
        let mut root = parse(source).expect("parse");
        let err = resolve_args(&mut root, &BTreeMap::new()).unwrap_err();
        assert!(matches!(err, ArgError::UnknownReference { ref name } if name == "oops"));
    }
}
