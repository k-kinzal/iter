//! `build_prompt_selector` — pick prompts out of an [`Iterfile`](Iterfile).
//!
//! Translates a runner's guarded `prompt` expression into an
//! [`iter_core::PromptSelector`] the runner can evaluate per-signal. Guards
//! are translated from [`iter_language::PromptGuard`] (a parse-tree type)
//! into [`iter_core::PromptGuard`] (a runtime type) so the runtime layer
//! does not depend on the language crate.
//!
//! # Selection semantics
//!
//! At build time we accept any number of guarded prompts but at most one
//! *unguarded* prompt — the default. Multiple defaults are almost always a
//! user mistake, so we surface a clear error instead of picking one
//! arbitrarily. At run time the selector walks the guarded branches in
//! source order and falls back to the default only when no guard matches;
//! see [`iter_core::PromptSelector::render`] for the exact contract.

use iter_core::{
    CmpOp as CoreCmpOp, IterationField as CoreIterationField, PromptGuard as CorePromptGuard,
    PromptSelector, PromptTemplate, TemplateError,
};
use iter_language::{
    CmpOp as LangCmpOp, IterationField as LangIterationField, Iterfile, PromptDef,
    PromptGuard as LangPromptGuard, Spanned,
};

/// Errors produced while building a [`PromptSelector`] from prompt
/// declarations.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PromptBuildError {
    /// No `prompt` block was declared.
    #[error("iterfile is missing a `prompt` declaration")]
    Missing,
    /// More than one unguarded `prompt` block was declared.
    #[error(
        "iterfile declares more than one unguarded `prompt`; \
         at most one default (unguarded) prompt is allowed — \
         add a `when ...` guard to narrow the extras or merge \
         them into a single template"
    )]
    MultipleDefaults,
    /// A prompt body failed to compile as a [`PromptTemplate`].
    #[error("invalid prompt template: {body:?}")]
    InvalidTemplate {
        /// Raw template body that failed to compile.
        body: String,
        /// Underlying template-compilation error.
        #[source]
        source: TemplateError,
    },
}

/// Build the [`PromptSelector`] the runner should use for `iterfile`.
///
/// Extracts the prompt expression from the first runner and converts
/// named prompt references into resolved `PromptDef` entries for the
/// existing `prompt_selector_from_defs` pipeline.
///
/// # Errors
///
/// * The Iterfile contains no runner or no prompt.
/// * The Iterfile declares more than one unguarded prompt.
pub(crate) fn build_prompt_selector(
    iterfile: &Iterfile,
) -> Result<PromptSelector, PromptBuildError> {
    let runner = iterfile.runners.first().ok_or(PromptBuildError::Missing)?;
    let prompts = crate::start::prompt_defs_from_expr(&runner.node.prompt, &iterfile.prompts);
    prompt_selector_from_defs(&prompts)
}

/// Build the [`PromptSelector`] for a flat slice of prompt declarations.
///
/// Shared by the Iterfile and compose `InlineService` code paths.
///
/// # Errors
///
/// * `prompts` is empty.
/// * More than one entry is unguarded.
pub(crate) fn prompt_selector_from_defs(
    prompts: &[Spanned<PromptDef>],
) -> Result<PromptSelector, PromptBuildError> {
    if prompts.is_empty() {
        return Err(PromptBuildError::Missing);
    }

    let mut branches: Vec<(CorePromptGuard, PromptTemplate)> = Vec::new();
    let mut default: Option<PromptTemplate> = None;

    for spanned in prompts {
        let decl = &spanned.node;
        let template = PromptTemplate::new(decl.body.clone()).map_err(|source| {
            PromptBuildError::InvalidTemplate {
                body: decl.body.clone(),
                source,
            }
        })?;
        if let Some(guard) = &decl.guard {
            branches.push((translate_guard(guard), template));
        } else {
            if default.is_some() {
                return Err(PromptBuildError::MultipleDefaults);
            }
            default = Some(template);
        }
    }

    Ok(PromptSelector::new(branches, default))
}

/// Recursively translate a language-AST guard into the runtime guard type.
/// Pure structural mapping; no semantic validation happens here because the
/// parser already rejected malformed guards.
fn translate_guard(guard: &LangPromptGuard) -> CorePromptGuard {
    match guard {
        LangPromptGuard::MetadataEq { key, value } => CorePromptGuard::MetadataEq {
            key: key.clone(),
            value: value.clone(),
        },
        LangPromptGuard::MetadataNeq { key, value } => CorePromptGuard::MetadataNeq {
            key: key.clone(),
            value: value.clone(),
        },
        LangPromptGuard::IterationCmp {
            field,
            modulus,
            op,
            rhs,
        } => CorePromptGuard::IterationCmp {
            field: translate_iteration_field(*field),
            modulus: *modulus,
            op: translate_cmp_op(*op),
            rhs: *rhs,
        },
        LangPromptGuard::IterationResultEq { value } => CorePromptGuard::IterationResultEq {
            value: value.clone(),
        },
        LangPromptGuard::IterationResultNeq { value } => CorePromptGuard::IterationResultNeq {
            value: value.clone(),
        },
        LangPromptGuard::And(lhs, rhs) => CorePromptGuard::And(
            Box::new(translate_guard(lhs)),
            Box::new(translate_guard(rhs)),
        ),
        LangPromptGuard::Or(lhs, rhs) => CorePromptGuard::Or(
            Box::new(translate_guard(lhs)),
            Box::new(translate_guard(rhs)),
        ),
    }
}

fn translate_iteration_field(field: LangIterationField) -> CoreIterationField {
    match field {
        LangIterationField::Count => CoreIterationField::Count,
        LangIterationField::PreviousExitCode => CoreIterationField::PreviousExitCode,
        LangIterationField::ConsecutiveFailures => CoreIterationField::ConsecutiveFailures,
        LangIterationField::ConsecutiveSuccesses => CoreIterationField::ConsecutiveSuccesses,
    }
}

fn translate_cmp_op(op: LangCmpOp) -> CoreCmpOp {
    match op {
        LangCmpOp::Eq => CoreCmpOp::Eq,
        LangCmpOp::Neq => CoreCmpOp::Neq,
        LangCmpOp::Lt => CoreCmpOp::Lt,
        LangCmpOp::Le => CoreCmpOp::Le,
        LangCmpOp::Gt => CoreCmpOp::Gt,
        LangCmpOp::Ge => CoreCmpOp::Ge,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iter_core::{IterationContext, Metadata, MetadataKey, MetadataValue, Signal};
    use iter_language::{PromptDef, Spanned};

    fn prompt(body: &str, guard: Option<LangPromptGuard>) -> Spanned<PromptDef> {
        Spanned::new(
            PromptDef {
                guard,
                body: body.to_owned(),
            },
            0..0,
        )
    }

    fn signal_with_kind(kind: &str) -> Signal {
        let mut meta = Metadata::new();
        meta.insert(
            MetadataKey::new("kind").unwrap(),
            MetadataValue::String(kind.into()),
        );
        Signal::new(meta)
    }

    fn iter_ctx() -> IterationContext {
        IterationContext::for_test()
    }

    #[test]
    fn missing_prompt_errors() {
        let prompts: Vec<Spanned<PromptDef>> = vec![];
        let err = prompt_selector_from_defs(&prompts).expect_err("must fail");
        assert!(err.to_string().contains("missing a `prompt`"));
    }

    #[test]
    fn single_unguarded_prompt_becomes_default() {
        let prompts = vec![prompt("hello {{signal.id}}", None)];
        let selector = prompt_selector_from_defs(&prompts).expect("build");
        let signal = signal_with_kind("anything");
        let rendered = selector.render(&signal, &iter_ctx()).expect("render");
        assert!(rendered.as_str().starts_with("hello "));
    }

    #[test]
    fn multiple_guarded_prompts_are_supported() {
        let prompts = vec![
            prompt(
                "handle issue {{metadata.kind}}",
                Some(LangPromptGuard::MetadataEq {
                    key: "kind".into(),
                    value: "issue".into(),
                }),
            ),
            prompt(
                "fix ci {{metadata.kind}}",
                Some(LangPromptGuard::MetadataEq {
                    key: "kind".into(),
                    value: "ci_fix".into(),
                }),
            ),
        ];

        let selector = prompt_selector_from_defs(&prompts).expect("build");
        assert_eq!(
            selector
                .render(&signal_with_kind("issue"), &iter_ctx())
                .unwrap()
                .as_str(),
            "handle issue issue"
        );
        assert_eq!(
            selector
                .render(&signal_with_kind("ci_fix"), &iter_ctx())
                .unwrap()
                .as_str(),
            "fix ci ci_fix"
        );
    }

    #[test]
    fn guarded_branches_fall_through_to_default() {
        let prompts = vec![
            prompt(
                "urgent path",
                Some(LangPromptGuard::MetadataEq {
                    key: "kind".into(),
                    value: "urgent".into(),
                }),
            ),
            prompt("default path", None),
        ];

        let selector = prompt_selector_from_defs(&prompts).expect("build");
        assert_eq!(
            selector
                .render(&signal_with_kind("urgent"), &iter_ctx())
                .unwrap()
                .as_str(),
            "urgent path"
        );
        assert_eq!(
            selector
                .render(&signal_with_kind("other"), &iter_ctx())
                .unwrap()
                .as_str(),
            "default path"
        );
    }

    #[test]
    fn multiple_unguarded_prompts_error() {
        let prompts = vec![prompt("a", None), prompt("b", None)];
        let err = prompt_selector_from_defs(&prompts).expect_err("must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("more than one unguarded"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn nested_and_or_guards_translate_structurally() {
        let prompts = vec![prompt(
            "matched",
            Some(LangPromptGuard::And(
                Box::new(LangPromptGuard::MetadataEq {
                    key: "kind".into(),
                    value: "issue".into(),
                }),
                Box::new(LangPromptGuard::Or(
                    Box::new(LangPromptGuard::MetadataNeq {
                        key: "kind".into(),
                        value: "ci_fix".into(),
                    }),
                    Box::new(LangPromptGuard::MetadataEq {
                        key: "kind".into(),
                        value: "urgent".into(),
                    }),
                )),
            )),
        )];

        let selector = prompt_selector_from_defs(&prompts).expect("build");
        // kind=issue: And(Eq(issue), Or(Neq(ci_fix)=true, Eq(urgent)=false)) = true
        assert_eq!(
            selector
                .render(&signal_with_kind("issue"), &iter_ctx())
                .unwrap()
                .as_str(),
            "matched"
        );
        // kind=other: And(Eq(issue)=false, ...) = false → no match, no default
        assert!(
            selector
                .render(&signal_with_kind("other"), &iter_ctx())
                .is_err()
        );
    }

    // End-to-end coverage for the `iteration.*` placeholder root, driving a
    // small Iterfile through `iter_language::parse` → `build_prompt_selector`
    // → `PromptSelector::render`. Absorbed from the former
    // `iter_compose` integration test when the composition layer moved into
    // the CLI.
    #[test]
    fn iteration_count_modulo_fires_only_on_multiples_of_three() {
        let source = r#"
queue memory

workspace clone {
  base = "."
  excludes = []
  preserve_mtime = true
  apply_back {
    mode = sync
  }
}

agent claude {
  mode = print
  command = "claude"
}

runner {
  agent = claude
  workspace = clone
  queue = memory
  continue_on_error = true
  behavior = wait
  prompt {
    iteration.count % 3 == 0 => "TICK-3 n={{iteration.count}}"
    _ => "tick n={{iteration.count}}"
  }
}
"#;
        let root = iter_language::parse(source).expect("source parses");
        let selector = build_prompt_selector(&root).expect("selector builds");
        let signal = Signal::new(Metadata::new());

        let mut log: Vec<(u32, String)> = Vec::new();
        for n in 1..=6u32 {
            let ctx = IterationContext::for_count(n);
            let rendered = selector
                .render(&signal, &ctx)
                .expect("render at iteration n");
            log.push((n, rendered.as_str().to_owned()));
        }

        assert_eq!(
            log,
            vec![
                (1, "tick n=1".to_string()),
                (2, "tick n=2".to_string()),
                (3, "TICK-3 n=3".to_string()),
                (4, "tick n=4".to_string()),
                (5, "tick n=5".to_string()),
                (6, "TICK-3 n=6".to_string()),
            ],
            "every-third guard must select the guarded prompt only on iterations 3 and 6",
        );
    }

    #[test]
    fn iteration_count_comparison_eq_one_fires_only_on_first_iteration() {
        let source = r#"
queue memory

workspace clone {
  base = "."
  excludes = []
  preserve_mtime = true
  apply_back {
    mode = sync
  }
}

agent claude {
  mode = print
  command = "claude"
}

runner {
  agent = claude
  workspace = clone
  queue = memory
  continue_on_error = true
  behavior = wait
  prompt {
    iteration.count == 1 => "first"
    _ => "rest n={{iteration.count}}"
  }
}
"#;
        let root = iter_language::parse(source).expect("source parses");
        let selector = build_prompt_selector(&root).expect("selector builds");
        let signal = Signal::new(Metadata::new());

        let first = selector
            .render(&signal, &IterationContext::for_count(1))
            .expect("render iter 1");
        assert_eq!(first.as_str(), "first");

        let third = selector
            .render(&signal, &IterationContext::for_count(3))
            .expect("render iter 3");
        assert_eq!(third.as_str(), "rest n=3");
    }

    #[test]
    fn iteration_result_eq_selects_branch_when_no_previous_turn() {
        let source = r#"
queue memory

workspace clone {
  base = "."
  excludes = []
  preserve_mtime = true
  apply_back {
    mode = sync
  }
}

agent claude {
  mode = print
  command = "claude"
}

runner {
  agent = claude
  workspace = clone
  queue = memory
  continue_on_error = true
  behavior = wait
  prompt {
    iteration.previous_result == "none" => "first run"
    _ => "regular run n={{iteration.count}}"
  }
}
"#;
        let root = iter_language::parse(source).expect("source parses");
        let selector = build_prompt_selector(&root).expect("selector builds");
        let signal = Signal::new(Metadata::new());

        let first = selector
            .render(&signal, &IterationContext::for_count(1))
            .expect("render iter 1");
        assert_eq!(first.as_str(), "first run");
    }
}
