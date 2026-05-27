//! Semantic analyzer: lowers the CST into the public typed AST while
//! validating field shapes, kind dispatch, and well-formedness rules.

mod agent;
mod compose;
mod compose_import;
mod compose_resolve;
mod compose_service;
mod compose_trigger;
mod event;
mod field_helpers;
mod guard;
mod prompt;
mod queue;
mod runner;
mod suggest;
mod template;
mod trigger;
mod value;
mod webhook;
mod workspace;

pub(crate) use compose::lower_compose_and_check;

use guard::lower_guard_pure;
use suggest::{closest, parse_priority};

use crate::ast::{
    ArgDecl, EventHandlerDecl, NamedDef, NamedPrompt, PromptArm, PromptDecl, PromptExpr,
    PromptValue, Root, RunnerDecl, Span, Spanned,
};
use crate::diagnostic::Diagnostic;
use crate::parser::{RawBlock, RawFile, RawIdent, RawSection};

// Hint strings for diagnostics about removed project-shaped defaults. The
// text is intentionally verbose because each explains *why* iter no longer
// picks the value — the user should not have to read the source to
// understand.
const EXCLUDES_HINT: &str = "iter no longer ships a default exclude list — `[\"target\", \"node_modules\", \".venv\"]` is a project-shaped decision. Use `excludes = []` for \"no exclusions\" or list the directories you want skipped.";
const PRESERVE_MTIME_HINT: &str = "iter no longer picks a default — set `preserve_mtime = true` to copy source mtimes into the clone, or `preserve_mtime = false` to let them bump to \"now\".";
const APPLY_BACK_HINT: &str = "iter no longer picks a default — add an `apply_back { mode = sync | merge | discard; excludes = [...]; includes = [...] }` block. `mode` is required; `excludes` and `includes` default to `[]` (and must stay empty when `mode = discard`).";
const APPLY_BACK_MODE_HINT: &str = "iter no longer picks a default — set `mode = sync` (copy back and delete orphans), `mode = merge` (copy back without deleting), or `mode = discard` (drop the clone on teardown).";
const APPLY_BACK_DISCARD_FILTER_HINT: &str = "remove the field, or change `mode` from `discard` to `sync` or `merge` so the filter has somewhere to apply.";
const COMMAND_HINT: &str = "iter no longer resolves agent binaries from `PATH` by default — set `command = \"...\"` to the binary name (e.g. `\"claude\"`) or an absolute path to pin it.";
const MODE_HINT: &str = "iter no longer picks a default — set `mode = print` for non-interactive batch output or `mode = interactive` for TTY-attached sessions.";
const NETWORK_HINT: &str = "iter no longer defaults sandbox network access — set `network = off`, `network = all`, or `network = [ \"host1\", ... ]`. Agent-required hosts are merged in automatically; list only project-additional hosts.";
const POLICY_HINT: &str = "add `policy { network = off }` (or the network rule your project needs). The agent's declared lower-bound requirements are merged in automatically.";
const CONTINUE_ON_ERROR_HINT: &str = "iter no longer picks a default — set `continue_on_error = true` to keep the loop running after a failed signal, or `continue_on_error = false` to abort.";
const RUNNER_BEHAVIOR_HINT: &str = "iter no longer picks a default — set `behavior = wait` to park on the queue (queue required) or `behavior = loop { delay_secs = N }` to synthesise an empty signal each iteration.";
const TRIGGER_IN_ITERFILE_HINT: &str = "trigger declarations belong in `compose.iter`, not in an Iterfile. Run `iter compose up` against a `compose.iter` that wires this trigger to a service.";

pub(crate) fn lower_and_check(file: RawFile) -> (Option<Root>, Vec<Diagnostic>) {
    let mut analyzer = Analyzer::default();
    let result = analyzer.lower(file);
    (Some(result), analyzer.errors)
}

#[derive(Default)]
struct Analyzer {
    errors: Vec<Diagnostic>,
}

impl Analyzer {
    fn lower(&mut self, file: RawFile) -> Root {
        // First pass: classify sections and detect old vs new syntax.
        // Old syntax: flat top-level workspace/agent/runner/prompt/on
        //   with no `as` aliases and no references inside runner.
        // New syntax: definitions use `as <name>`, runner carries
        //   `agent = <ref>`, `workspace = <ref>`, etc.
        //
        // Heuristic: if any section has an `as` alias, a named prompt,
        // or a runner body that uses new-syntax features (binding fields
        // like `agent =` / `workspace =`, or nested event handlers),
        // treat as new syntax. Otherwise, desugar as deprecated old syntax.
        let has_new_syntax = file.sections.iter().any(|s| match s {
            RawSection::Block {
                alias,
                keyword,
                body,
                ..
            } => {
                alias.is_some()
                    || (keyword == "runner"
                        && body.as_ref().is_some_and(|b| {
                            !b.event_handlers.is_empty()
                                || b.fields
                                    .iter()
                                    .any(|f| f.name.name == "agent" || f.name.name == "workspace")
                        }))
            }
            RawSection::Prompt { name, .. } => name.is_some(),
            RawSection::On { .. } => false,
        });

        if has_new_syntax {
            self.lower_new_syntax(file)
        } else {
            self.lower_old_syntax(file)
        }
    }

    /// Lower old-style flat Iterfile with deprecation warnings. Desugars
    /// to the new AST by creating synthetic named definitions and a
    /// synthetic runner that references them.
    #[allow(clippy::too_many_lines)]
    fn lower_old_syntax(&mut self, file: RawFile) -> Root {
        let mut root = Root::default();
        let mut seen = SectionSeen::default();

        // Collect old-style sections.
        let mut old_queue = None;
        let mut old_workspace = None;
        let mut old_agent = None;
        let mut old_runner = None;
        let mut old_prompts: Vec<Spanned<PromptDecl>> = Vec::new();
        let mut old_events: Vec<Spanned<EventHandlerDecl>> = Vec::new();
        let mut queue_kind_name: Option<String> = None;
        let mut workspace_kind_name: Option<String> = None;
        let mut agent_kind_name: Option<String> = None;

        for section in file.sections {
            match section {
                RawSection::Block {
                    keyword,
                    keyword_span,
                    kind,
                    kind2,
                    alias: _,
                    body,
                    span,
                } => {
                    if let Some(extra) = &kind2 {
                        self.errors.push(Diagnostic::error(
                            extra.span.clone(),
                            format!(
                                "unexpected second identifier `{}` after `{}` section",
                                extra.name, keyword,
                            ),
                        ).with_hint("Iterfile sections take a single kind identifier; named sections (`queue main file { ... }`) belong in `compose.iter`."));
                    }
                    match keyword.as_str() {
                        "arg" => {
                            self.lower_arg_section(&mut root, &mut seen, kind, keyword_span, body, span);
                        }
                        "queue" => {
                            if self.reject_duplicate(seen.queue.as_ref(), &span, "queue") {
                                continue;
                            }
                            seen.queue = Some(span.clone());
                            queue_kind_name = kind.as_ref().map(|k| k.name.clone());
                            if let Some(decl) = self.lower_queue(kind, body, &keyword_span) {
                                old_queue = Some(Spanned::new(decl, span));
                            }
                        }
                        "workspace" => {
                            if self.reject_duplicate(seen.workspace.as_ref(), &span, "workspace") {
                                continue;
                            }
                            seen.workspace = Some(span.clone());
                            workspace_kind_name = kind.as_ref().map(|k| k.name.clone());
                            if let Some(decl) = self.lower_workspace(kind, body, &keyword_span) {
                                old_workspace = Some(Spanned::new(decl, span));
                            }
                        }
                        "agent" => {
                            if self.reject_duplicate(seen.agent.as_ref(), &span, "agent") {
                                continue;
                            }
                            seen.agent = Some(span.clone());
                            agent_kind_name = kind.as_ref().map(|k| k.name.clone());
                            if let Some(decl) = self.lower_agent(kind, body, &keyword_span) {
                                old_agent = Some(Spanned::new(decl, span));
                            }
                        }
                        "trigger" => {
                            drop((kind, body));
                            self.errors.push(
                                Diagnostic::error(
                                    span,
                                    "`trigger` is no longer a valid top-level section in an Iterfile",
                                )
                                .with_hint(TRIGGER_IN_ITERFILE_HINT),
                            );
                        }
                        "runner" => {
                            if self.reject_duplicate(seen.runner.as_ref(), &span, "runner") {
                                continue;
                            }
                            seen.runner = Some(span.clone());
                            if let Some(decl) = self.lower_runner_old(kind.as_ref(), body, &keyword_span) {
                                old_runner = Some(Spanned::new(decl, span));
                            }
                        }
                        other => {
                            self.errors.push(Diagnostic::error(
                                keyword_span,
                                format!("unknown top-level keyword `{other}`"),
                            ));
                        }
                    }
                }
                RawSection::Prompt {
                    guard,
                    body,
                    span,
                    body_span,
                    name: _,
                    ..
                } => {
                    old_prompts.push(self.lower_prompt(guard, body, span, body_span));
                }
                RawSection::On {
                    event,
                    body,
                    span,
                    keyword_span: _,
                } => {
                    if let Some(decl) = self.lower_event(&event, &body, span) {
                        old_events.push(decl);
                    }
                }
            }
        }

        // Emit deprecation warning for old syntax.
        self.errors.push(Diagnostic::warning(
            0..0,
            "flat Iterfile syntax is deprecated — use named definitions and runner binding",
        ));

        // Desugar old definitions into named definitions.
        let queue_name = queue_kind_name.unwrap_or_else(|| "default".into());
        let workspace_name = workspace_kind_name.unwrap_or_else(|| "default".into());
        let agent_name = agent_kind_name.unwrap_or_else(|| "default".into());

        if let Some(q) = old_queue {
            let span = q.span.clone();
            root.queues.push(Spanned::new(
                NamedDef { name: queue_name.clone(), decl: q.node },
                span,
            ));
        }
        if let Some(w) = old_workspace {
            let span = w.span.clone();
            root.workspaces.push(Spanned::new(
                NamedDef { name: workspace_name.clone(), decl: w.node },
                span,
            ));
        }
        if let Some(a) = old_agent {
            let span = a.span.clone();
            root.agents.push(Spanned::new(
                NamedDef { name: agent_name.clone(), decl: a.node },
                span,
            ));
        }

        // Build prompt expression from old-style ordered prompts.
        let prompt_expr = Self::build_prompt_expr_from_old_prompts(&old_prompts);

        // Synthesise a runner only when all required definitions succeeded.
        // If agent or workspace lowering failed, the runner would carry
        // dangling references — skip it and let the earlier errors surface.
        if let Some(old_r) = old_runner {
            if !root.agents.is_empty() && !root.workspaces.is_empty() {
                let span = old_r.span.clone();
                root.runners.push(Spanned::new(
                    RunnerDecl {
                        name: None,
                        agent: agent_name,
                        workspace: workspace_name,
                        queue: if root.queues.is_empty() { None } else { Some(queue_name) },
                        continue_on_error: old_r.node.continue_on_error,
                        behavior: old_r.node.behavior,
                        iteration_timeout_secs: old_r.node.iteration_timeout_secs,
                        prompt: prompt_expr,
                        events: old_events,
                    },
                    span,
                ));
            }
        }

        root
    }

    /// Lower new-style Iterfile with named definitions and runner binding.
    #[allow(clippy::too_many_lines)]
    fn lower_new_syntax(&mut self, file: RawFile) -> Root {
        let mut root = Root::default();
        let mut seen = SectionSeen::default();

        // Pending top-level `on` sections — will be attached to runner if present.
        let mut pending_events: Vec<Spanned<EventHandlerDecl>> = Vec::new();

        for section in file.sections {
            match section {
                RawSection::Block {
                    keyword,
                    keyword_span,
                    kind,
                    kind2,
                    alias,
                    body,
                    span,
                } => {
                    if let Some(extra) = &kind2 {
                        self.errors.push(Diagnostic::error(
                            extra.span.clone(),
                            format!(
                                "unexpected second identifier `{}` after `{}` section",
                                extra.name, keyword,
                            ),
                        ).with_hint("Iterfile sections take a single kind identifier; named sections (`queue main file { ... }`) belong in `compose.iter`."));
                    }
                    match keyword.as_str() {
                        "arg" => {
                            self.lower_arg_section(&mut root, &mut seen, kind, keyword_span, body, span);
                        }
                        "queue" => {
                            let def_name = alias.as_ref().map(|a| a.name.clone())
                                .or_else(|| kind.as_ref().map(|k| k.name.clone()));
                            let Some(name) = def_name else {
                                self.errors.push(Diagnostic::error(
                                    keyword_span.clone(),
                                    "`queue` requires a kind (and optionally `as <name>`)",
                                ));
                                continue;
                            };
                            if self.reject_duplicate_name(&seen.queue_names, &name, &span, "queue") {
                                continue;
                            }
                            seen.queue_names.insert(name.clone(), span.clone());
                            if let Some(decl) = self.lower_queue(kind, body, &keyword_span) {
                                root.queues.push(Spanned::new(
                                    NamedDef { name, decl },
                                    span,
                                ));
                            }
                        }
                        "workspace" => {
                            let def_name = alias.as_ref().map(|a| a.name.clone())
                                .or_else(|| kind.as_ref().map(|k| k.name.clone()));
                            let Some(name) = def_name else {
                                self.errors.push(Diagnostic::error(
                                    keyword_span.clone(),
                                    "`workspace` requires a kind (and optionally `as <name>`)",
                                ));
                                continue;
                            };
                            if self.reject_duplicate_name(&seen.workspace_names, &name, &span, "workspace") {
                                continue;
                            }
                            seen.workspace_names.insert(name.clone(), span.clone());
                            if let Some(decl) = self.lower_workspace(kind, body, &keyword_span) {
                                root.workspaces.push(Spanned::new(
                                    NamedDef { name, decl },
                                    span,
                                ));
                            }
                        }
                        "agent" => {
                            let def_name = alias.as_ref().map(|a| a.name.clone())
                                .or_else(|| kind.as_ref().map(|k| k.name.clone()));
                            let Some(name) = def_name else {
                                self.errors.push(Diagnostic::error(
                                    keyword_span.clone(),
                                    "`agent` requires a kind (and optionally `as <name>`)",
                                ));
                                continue;
                            };
                            if self.reject_duplicate_name(&seen.agent_names, &name, &span, "agent") {
                                continue;
                            }
                            seen.agent_names.insert(name.clone(), span.clone());
                            if let Some(decl) = self.lower_agent(kind, body, &keyword_span) {
                                root.agents.push(Spanned::new(
                                    NamedDef { name, decl },
                                    span,
                                ));
                            }
                        }
                        "trigger" => {
                            drop((kind, body));
                            self.errors.push(
                                Diagnostic::error(
                                    span,
                                    "`trigger` is no longer a valid top-level section in an Iterfile",
                                )
                                .with_hint(TRIGGER_IN_ITERFILE_HINT),
                            );
                        }
                        "runner" => {
                            if let Some(decl) = self.lower_runner_new(kind.as_ref(), alias, body, &keyword_span) {
                                root.runners.push(Spanned::new(decl, span));
                            }
                        }
                        other => {
                            self.errors.push(Diagnostic::error(
                                keyword_span,
                                format!("unknown top-level keyword `{other}`"),
                            ));
                        }
                    }
                }
                RawSection::Prompt {
                    name,
                    guard,
                    body,
                    span,
                    body_span,
                    ..
                } => {
                    if let Some(name_ident) = name {
                        if guard.is_some() {
                            self.errors.push(Diagnostic::error(
                                span.clone(),
                                "named prompt definitions (`prompt as <name>`) cannot have `when` guards",
                            ));
                        }
                        self.validate_template(&body, &body_span);
                        if self.reject_duplicate_name(&seen.prompt_names, &name_ident.name, &span, "prompt") {
                            continue;
                        }
                        seen.prompt_names.insert(name_ident.name.clone(), span.clone());
                        root.prompts.push(Spanned::new(
                            NamedPrompt { name: name_ident.name, body },
                            span,
                        ));
                    } else {
                        self.errors.push(Diagnostic::error(
                            span,
                            "top-level `prompt` without `as <name>` is deprecated in new-syntax files; use `prompt as <name> \"...\"` or put the prompt inside a runner block",
                        ));
                    }
                }
                RawSection::On {
                    event,
                    body,
                    span,
                    keyword_span: _,
                } => {
                    self.errors.push(Diagnostic::warning(
                        span.clone(),
                        "top-level `on` is deprecated — move event handlers inside the runner block",
                    ));
                    if let Some(decl) = self.lower_event(&event, &body, span) {
                        pending_events.push(decl);
                    }
                }
            }
        }

        // Validate runner references.
        for runner in &root.runners {
            if !root.agents.iter().any(|a| a.node.name == runner.node.agent) {
                self.errors.push(Diagnostic::error(
                    runner.span.clone(),
                    format!("runner references agent `{}` which is not defined", runner.node.agent),
                ));
            }
            if !root.workspaces.iter().any(|w| w.node.name == runner.node.workspace) {
                self.errors.push(Diagnostic::error(
                    runner.span.clone(),
                    format!("runner references workspace `{}` which is not defined", runner.node.workspace),
                ));
            }
            if let Some(ref q) = runner.node.queue {
                if !root.queues.iter().any(|qd| qd.node.name == *q) {
                    self.errors.push(Diagnostic::error(
                        runner.span.clone(),
                        format!("runner references queue `{q}` which is not defined"),
                    ));
                }
            }
            // Validate prompt references.
            Self::validate_prompt_refs(&runner.node.prompt, &root.prompts, &runner.span, &mut self.errors);
        }

        // Attach pending top-level events to runners (backward compat shim).
        if !pending_events.is_empty() {
            for runner in &mut root.runners {
                runner.node.events.extend(pending_events.clone());
            }
        }

        root
    }

    fn validate_prompt_refs(
        prompt: &PromptExpr,
        prompts: &[Spanned<NamedPrompt>],
        span: &Span,
        errors: &mut Vec<Diagnostic>,
    ) {
        let mut check_value = |v: &PromptValue| {
            if let PromptValue::Ref(name) = v {
                if !prompts.iter().any(|p| p.node.name == *name) {
                    errors.push(Diagnostic::error(
                        span.clone(),
                        format!("runner references prompt `{name}` which is not defined"),
                    ));
                }
            }
        };
        match prompt {
            PromptExpr::Single(v) => check_value(v),
            PromptExpr::Match { arms, default } => {
                for arm in arms {
                    check_value(&arm.value);
                }
                check_value(default);
            }
        }
    }

    /// Convert old-style ordered prompt list into a `PromptExpr`.
    fn build_prompt_expr_from_old_prompts(prompts: &[Spanned<PromptDecl>]) -> PromptExpr {
        if prompts.is_empty() {
            return PromptExpr::Single(PromptValue::Inline(String::new()));
        }
        if prompts.len() == 1 && prompts[0].node.guard.is_none() {
            return PromptExpr::Single(PromptValue::Inline(prompts[0].node.body.clone()));
        }
        // Multiple prompts or guarded prompts → match expression.
        let mut arms = Vec::new();
        let mut default = None;
        for p in prompts {
            if let Some(guard) = &p.node.guard {
                arms.push(PromptArm {
                    guard: guard.clone(),
                    value: PromptValue::Inline(p.node.body.clone()),
                });
            } else {
                default = Some(PromptValue::Inline(p.node.body.clone()));
            }
        }
        let default = default.unwrap_or_else(|| PromptValue::Inline(String::new()));
        if arms.is_empty() {
            PromptExpr::Single(default)
        } else {
            PromptExpr::Match { arms, default }
        }
    }

    fn lower_arg_section(
        &mut self,
        root: &mut Root,
        seen: &mut SectionSeen,
        kind: Option<RawIdent>,
        keyword_span: Span,
        body: Option<RawBlock>,
        span: Span,
    ) {
        let Some(name_ident) = kind else {
            self.errors
                .push(Diagnostic::error(keyword_span, "`arg` requires a name"));
            return;
        };
        if let Some(prev_span) = seen.args.get(&name_ident.name) {
            self.errors.push(
                Diagnostic::error(
                    span.clone(),
                    format!("duplicate `arg` declaration for `{}`", name_ident.name),
                )
                .with_hint(format!(
                    "previous declaration at bytes {}..{}",
                    prev_span.start, prev_span.end
                )),
            );
            return;
        }
        if !is_valid_arg_name(&name_ident.name) {
            self.errors.push(
                Diagnostic::error(
                    name_ident.span.clone(),
                    format!("invalid arg name `{}`", name_ident.name),
                )
                .with_hint(
                    "arg names must start with a letter or underscore and contain only ASCII alphanumerics and underscores",
                ),
            );
            return;
        }
        seen.args.insert(name_ident.name.clone(), span.clone());
        let default = body.and_then(|block| {
            let mut fields = self.collect_fields(Some(block));
            let val = self.take_optional_string(&mut fields, "default");
            self.reject_unknown_fields(&mut fields, &["default"], "arg");
            val
        });
        root.args.push(Spanned::new(
            ArgDecl {
                name: name_ident.name,
                default,
            },
            span,
        ));
    }

    fn reject_duplicate(&mut self, prev: Option<&Span>, span: &Span, label: &str) -> bool {
        if let Some(p) = prev {
            self.errors.push(
                Diagnostic::error(span.clone(), format!("duplicate `{label}` declaration"))
                    .with_hint(format!(
                        "previous declaration at bytes {}..{}",
                        p.start, p.end
                    )),
            );
            true
        } else {
            false
        }
    }

    fn reject_duplicate_name(
        &mut self,
        names: &std::collections::BTreeMap<String, Span>,
        name: &str,
        span: &Span,
        label: &str,
    ) -> bool {
        if let Some(prev) = names.get(name) {
            self.errors.push(
                Diagnostic::error(
                    span.clone(),
                    format!("duplicate {label} name `{name}`"),
                )
                .with_hint(format!(
                    "previous declaration at bytes {}..{}",
                    prev.start, prev.end,
                )),
            );
            true
        } else {
            false
        }
    }
}

#[derive(Default)]
struct SectionSeen {
    args: std::collections::BTreeMap<String, Span>,
    // Old-syntax duplicate guards (singular).
    queue: Option<Span>,
    workspace: Option<Span>,
    agent: Option<Span>,
    runner: Option<Span>,
    // New-syntax name-based duplicate guards.
    queue_names: std::collections::BTreeMap<String, Span>,
    workspace_names: std::collections::BTreeMap<String, Span>,
    agent_names: std::collections::BTreeMap<String, Span>,
    prompt_names: std::collections::BTreeMap<String, Span>,
}


fn is_valid_arg_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
