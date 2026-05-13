//! Semantic analyzer: lowers the CST into the public typed AST while
//! validating field shapes, kind dispatch, and well-formedness rules.

mod agent;
mod compose;
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

use crate::ast::{ArgDecl, Root, Span, Spanned};
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
        let mut root = Root::default();
        let mut seen = SectionSeen::default();

        for section in file.sections {
            match section {
                RawSection::Block {
                    keyword,
                    keyword_span,
                    kind,
                    kind2,
                    body,
                    span,
                } => {
                    self.lower_block_section(
                        &mut root,
                        &mut seen,
                        BlockSectionParts {
                            keyword,
                            keyword_span,
                            kind,
                            kind2,
                            body,
                            span,
                        },
                    );
                }
                RawSection::Prompt {
                    guard,
                    body,
                    span,
                    body_span,
                    ..
                } => {
                    root.prompts
                        .push(self.lower_prompt(guard, body, span, body_span));
                }
                RawSection::On {
                    event,
                    body,
                    span,
                    keyword_span: _,
                } => {
                    if let Some(decl) = self.lower_event(&event, &body, span) {
                        root.events.push(decl);
                    }
                }
            }
        }
        root
    }

    fn lower_block_section(
        &mut self,
        root: &mut Root,
        seen: &mut SectionSeen,
        parts: BlockSectionParts,
    ) {
        let BlockSectionParts {
            keyword,
            keyword_span,
            kind,
            kind2,
            body,
            span,
        } = parts;
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
                self.lower_arg_section(root, seen, kind, keyword_span, body, span);
            }
            "queue" => {
                if self.reject_duplicate(seen.queue.as_ref(), &span, "queue") {
                    return;
                }
                seen.queue = Some(span.clone());
                if let Some(decl) = self.lower_queue(kind, body, &keyword_span) {
                    root.queue = Some(Spanned::new(decl, span));
                }
            }
            "workspace" => {
                if self.reject_duplicate(seen.workspace.as_ref(), &span, "workspace") {
                    return;
                }
                seen.workspace = Some(span.clone());
                if let Some(decl) = self.lower_workspace(kind, body, &keyword_span) {
                    root.workspace = Some(Spanned::new(decl, span));
                }
            }
            "agent" => {
                if self.reject_duplicate(seen.agent.as_ref(), &span, "agent") {
                    return;
                }
                seen.agent = Some(span.clone());
                if let Some(decl) = self.lower_agent(kind, body, &keyword_span) {
                    root.agent = Some(Spanned::new(decl, span));
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
                    return;
                }
                seen.runner = Some(span.clone());
                if let Some(decl) = self.lower_runner(kind.as_ref(), body, &keyword_span) {
                    root.runner = Some(Spanned::new(decl, span));
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
}

#[derive(Default)]
struct SectionSeen {
    args: std::collections::BTreeMap<String, Span>,
    queue: Option<Span>,
    workspace: Option<Span>,
    agent: Option<Span>,
    runner: Option<Span>,
}

struct BlockSectionParts {
    keyword: String,
    keyword_span: Span,
    kind: Option<RawIdent>,
    kind2: Option<RawIdent>,
    body: Option<RawBlock>,
    span: Span,
}

fn is_valid_arg_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
