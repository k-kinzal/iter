//! Semantic analyzer: lowers the CST into the public typed AST while
//! validating field shapes, kind dispatch, and well-formedness rules.

mod agent;
mod compose;
mod compose_import;
mod compose_resolve;
mod compose_service;
mod compose_trigger;
mod event;
mod field_schema;
mod guard;
mod prompt;
mod queue;
mod runner;
mod source;
mod suggest;
mod template;
mod trigger;
mod value;
mod webhook;
mod workspace;

pub(crate) use compose::lower_compose_and_check;

use guard::lower_guard_pure;
use suggest::{closest, parse_priority};
pub(crate) use template::TemplatePosition;

use crate::ast::{
    ArgDef, Iterfile, NamedDef, NamedPrompt, PromptExpr, PromptValue, Span, Spanned, WorkspaceDef,
    WorkspaceSourceRef,
};
use crate::diagnostic::Diagnostic;
use crate::parser::{CstBlock, CstFile, CstIdent, CstSection};

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

pub(crate) fn lower_and_check(file: CstFile) -> (Option<Iterfile>, Vec<Diagnostic>) {
    let mut analyzer = Analyzer::default();
    let result = analyzer.lower(file);
    let declared: std::collections::BTreeSet<String> =
        result.args.iter().map(|a| a.node.name.clone()).collect();
    analyzer.finish_arg_refs(&declared);
    (Some(result), analyzer.errors)
}

#[derive(Default)]
struct Analyzer {
    errors: Vec<Diagnostic>,
    /// `{{arg.<name>}}` references gathered during template validation,
    /// cross-checked against the file's declared `arg`s once lowering is
    /// complete (args may be declared after the template that uses them).
    arg_refs: Vec<(String, Span)>,
}

impl Analyzer {
    /// Emit an error for every recorded `{{arg.<name>}}` reference whose
    /// name is not among the file's declared `arg`s. Drains `arg_refs`.
    fn finish_arg_refs(&mut self, declared: &std::collections::BTreeSet<String>) {
        let refs = std::mem::take(&mut self.arg_refs);
        for (name, span) in refs {
            if !declared.contains(&name) {
                self.errors.push(
                    Diagnostic::error(
                        span,
                        format!("`{{{{arg.{name}}}}}` references an undeclared arg `{name}`"),
                    )
                    .with_hint(format!(
                        "declare it with `arg {name} {{ default = \"...\" }}`, or `arg {name}` and pass `--arg {name}=...` at run time"
                    )),
                );
            }
        }
    }
}

impl Analyzer {
    /// Lower an Iterfile into the typed [`Iterfile`] AST.
    ///
    /// The Iterfile grammar is named definitions (`agent`/`workspace`/`queue`
    /// and `prompt as <name>`) bound by a `runner { agent = ... workspace = ... }`
    /// block. The legacy flat form — top-level definitions desugared into a
    /// synthetic runner — is no longer supported; its constructs are reported
    /// as semantic errors that name the replacement.
    #[allow(clippy::too_many_lines)]
    fn lower(&mut self, file: CstFile) -> Iterfile {
        let mut root = Iterfile::default();
        let mut seen = SectionSeen::default();

        for section in file.sections {
            match section {
                CstSection::Block {
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
                            self.lower_arg_section(
                                &mut root,
                                &mut seen,
                                kind,
                                keyword_span,
                                body,
                                span,
                            );
                        }
                        "queue" => {
                            let def_name = alias
                                .as_ref()
                                .map(|a| a.name.clone())
                                .or_else(|| kind.as_ref().map(|k| k.name.clone()));
                            let Some(name) = def_name else {
                                self.errors.push(Diagnostic::error(
                                    keyword_span.clone(),
                                    "`queue` requires a kind (and optionally `as <name>`)",
                                ));
                                continue;
                            };
                            if self.reject_duplicate_name(&seen.queue_names, &name, &span, "queue")
                            {
                                continue;
                            }
                            seen.queue_names.insert(name.clone(), span.clone());
                            if let Some(decl) = self.lower_queue(kind, body, &keyword_span) {
                                root.queues
                                    .push(Spanned::new(NamedDef { name, decl }, span));
                            }
                        }
                        "workspace" => {
                            let def_name = alias
                                .as_ref()
                                .map(|a| a.name.clone())
                                .or_else(|| kind.as_ref().map(|k| k.name.clone()));
                            let Some(name) = def_name else {
                                self.errors.push(Diagnostic::error(
                                    keyword_span.clone(),
                                    "`workspace` requires a kind (and optionally `as <name>`)",
                                ));
                                continue;
                            };
                            if self.reject_duplicate_name(
                                &seen.workspace_names,
                                &name,
                                &span,
                                "workspace",
                            ) {
                                continue;
                            }
                            seen.workspace_names.insert(name.clone(), span.clone());
                            if let Some(decl) = self.lower_workspace(kind, body, &keyword_span) {
                                root.workspaces
                                    .push(Spanned::new(NamedDef { name, decl }, span));
                            }
                        }
                        "source" => {
                            let def_name = alias
                                .as_ref()
                                .map(|a| a.name.clone())
                                .or_else(|| kind.as_ref().map(|k| k.name.clone()));
                            let Some(name) = def_name else {
                                self.errors.push(Diagnostic::error(
                                    keyword_span.clone(),
                                    "`source` requires a kind (and optionally `as <name>`)",
                                ));
                                continue;
                            };
                            if self.reject_duplicate_name(
                                &seen.source_names,
                                &name,
                                &span,
                                "source",
                            ) {
                                continue;
                            }
                            seen.source_names.insert(name.clone(), span.clone());
                            if let Some(decl) = self.lower_source(kind, body, &keyword_span) {
                                root.sources
                                    .push(Spanned::new(NamedDef { name, decl }, span));
                            }
                        }
                        "agent" => {
                            let def_name = alias
                                .as_ref()
                                .map(|a| a.name.clone())
                                .or_else(|| kind.as_ref().map(|k| k.name.clone()));
                            let Some(name) = def_name else {
                                self.errors.push(Diagnostic::error(
                                    keyword_span.clone(),
                                    "`agent` requires a kind (and optionally `as <name>`)",
                                ));
                                continue;
                            };
                            if self.reject_duplicate_name(&seen.agent_names, &name, &span, "agent")
                            {
                                continue;
                            }
                            seen.agent_names.insert(name.clone(), span.clone());
                            if let Some(decl) = self.lower_agent(kind, body, &keyword_span) {
                                root.agents
                                    .push(Spanned::new(NamedDef { name, decl }, span));
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
                            if let Some(decl) =
                                self.lower_runner_new(kind.as_ref(), alias, body, &keyword_span)
                            {
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
                CstSection::Prompt {
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
                        self.validate_template(&body, &body_span, TemplatePosition::Prompt);
                        if self.reject_duplicate_name(
                            &seen.prompt_names,
                            &name_ident.name,
                            &span,
                            "prompt",
                        ) {
                            continue;
                        }
                        seen.prompt_names
                            .insert(name_ident.name.clone(), span.clone());
                        root.prompts.push(Spanned::new(
                            NamedPrompt {
                                name: name_ident.name,
                                body,
                            },
                            span,
                        ));
                    } else {
                        self.errors.push(Diagnostic::error(
                            span,
                            "top-level `prompt \"...\"` is no longer supported; define a named prompt with `prompt as <name> \"...\"` and reference it, or write the prompt inside the runner block (`prompt = \"...\"` or a `prompt { <guard> => ..., _ => ... }` match)",
                        ));
                    }
                }
                CstSection::On { span, .. } => {
                    self.errors.push(Diagnostic::error(
                        span,
                        "top-level `on <event>` is no longer supported; move event handlers inside the runner block as `on <event> { ... }`",
                    ));
                }
            }
        }

        // Validate runner references.
        for workspace in &root.workspaces {
            if let Some(WorkspaceSourceRef::Named(name)) =
                workspace_source_ref(&workspace.node.decl)
                && !root.sources.iter().any(|s| s.node.name == *name)
            {
                self.errors.push(Diagnostic::error(
                    workspace.span.clone(),
                    format!("workspace references source `{name}` which is not defined"),
                ));
            }
        }

        // Validate runner references.
        for runner in &root.runners {
            if !root.agents.iter().any(|a| a.node.name == runner.node.agent) {
                self.errors.push(Diagnostic::error(
                    runner.span.clone(),
                    format!(
                        "runner references agent `{}` which is not defined",
                        runner.node.agent
                    ),
                ));
            }
            if !root
                .workspaces
                .iter()
                .any(|w| w.node.name == runner.node.workspace)
            {
                self.errors.push(Diagnostic::error(
                    runner.span.clone(),
                    format!(
                        "runner references workspace `{}` which is not defined",
                        runner.node.workspace
                    ),
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
            Self::validate_prompt_refs(
                &runner.node.prompt,
                &root.prompts,
                &runner.span,
                &mut self.errors,
            );
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

    fn lower_arg_section(
        &mut self,
        root: &mut Iterfile,
        seen: &mut SectionSeen,
        kind: Option<CstIdent>,
        keyword_span: Span,
        body: Option<CstBlock>,
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
            ArgDef {
                name: name_ident.name,
                default,
            },
            span,
        ));
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
                Diagnostic::error(span.clone(), format!("duplicate {label} name `{name}`"))
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

fn workspace_source_ref(workspace: &WorkspaceDef) -> Option<&WorkspaceSourceRef> {
    match workspace {
        WorkspaceDef::Local { source, .. }
        | WorkspaceDef::Clone { source, .. }
        | WorkspaceDef::Sandbox { source, .. } => source.as_ref(),
    }
}

#[derive(Default)]
struct SectionSeen {
    args: std::collections::BTreeMap<String, Span>,
    // Name-based duplicate guards for named definitions.
    queue_names: std::collections::BTreeMap<String, Span>,
    workspace_names: std::collections::BTreeMap<String, Span>,
    source_names: std::collections::BTreeMap<String, Span>,
    agent_names: std::collections::BTreeMap<String, Span>,
    prompt_names: std::collections::BTreeMap<String, Span>,
}

pub(super) fn is_valid_arg_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
