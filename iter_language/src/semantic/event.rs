//! `on <event> { ... }` top-level handler lowering plus shell-action collection.

use super::{Analyzer, closest};
use crate::ast::{Action, EventHandlerDecl, EventName, Span, Spanned};
use crate::diagnostic::Diagnostic;
use crate::parser::{RawAction, RawBlock, RawIdent};

impl Analyzer {
    pub(super) fn lower_event(
        &mut self,
        event: &RawIdent,
        body: &RawBlock,
        span: Span,
    ) -> Option<Spanned<EventHandlerDecl>> {
        let event_name = if let Some((e, deprecated_alias)) =
            EventName::parse_with_deprecation(&event.name)
        {
            if let Some(alias) = deprecated_alias {
                let canonical = e.as_str();
                self.errors.push(
                    Diagnostic::warning(
                        event.span.clone(),
                        format!("event name `{alias}` is deprecated; use `{canonical}` instead",),
                    )
                    .with_hint(format!("rename `on {alias}` to `on {canonical}`")),
                );
            }
            e
        } else {
            let suggestion = closest(&event.name, EventName::ALL);
            let mut diag = Diagnostic::error(
                event.span.clone(),
                format!("unknown event name `{}`", event.name),
            );
            if let Some(s) = suggestion {
                diag = diag.with_hint(format!("did you mean `{s}`?"));
            }
            self.errors.push(diag);
            return None;
        };
        let actions = self.lower_actions(body);
        if !body.fields.is_empty() {
            for f in &body.fields {
                self.errors.push(Diagnostic::error(
                    f.span.clone(),
                    format!(
                        "field `{}` is not allowed inside an event handler block",
                        f.name.name
                    ),
                ));
            }
        }
        if !body.routes.is_empty() {
            for r in &body.routes {
                self.errors.push(Diagnostic::error(
                    r.span.clone(),
                    "nested `on \"...\"` routes are only valid inside `trigger webhook`",
                ));
            }
        }
        Some(Spanned::new(
            EventHandlerDecl {
                event: event_name,
                actions,
            },
            span,
        ))
    }

    pub(super) fn lower_actions(&mut self, block: &RawBlock) -> Vec<Action> {
        let mut out = Vec::new();
        for raw in &block.actions {
            let RawAction { command, .. } = raw;
            self.validate_template(command, &raw.span);
            out.push(Action::Shell(command.clone()));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::lower_and_check;
    use crate::diagnostic::Severity;
    use crate::parse_to_cst;

    /// A minimal Iterfile prologue: every required section before any
    /// `on` block, so the analyzer accepts the file as a whole and the
    /// only diagnostic that survives is the one we want to inspect.
    const PROLOGUE: &str = r#"
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
  continue_on_error = false
  behavior = wait
}
prompt "Iterate."
"#;

    fn analyze(src: &str) -> Vec<crate::Diagnostic> {
        let (cst, mut diags) = parse_to_cst(src);
        let cst = cst.expect("parser produced a CST");
        let (_root, sem) = lower_and_check(cst);
        diags.extend(sem);
        diags
    }

    #[test]
    fn deprecated_alias_emits_one_warning_with_canonical_hint() {
        let src = format!("{PROLOGUE}\non workspace_torndown {{ shell \"echo done\" }}\n");
        let diags = analyze(&src);

        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && !d.message.contains("flat Iterfile syntax"))
            .collect();
        assert_eq!(
            warnings.len(),
            1,
            "exactly one warning for one alias use; got {diags:?}"
        );
        let w = warnings[0];
        assert!(
            w.message.contains("workspace_torndown"),
            "warning names the alias: {}",
            w.message
        );
        assert!(
            w.message.contains("workspace_teardown_finished"),
            "warning recommends the canonical: {}",
            w.message
        );
        let hint = w.hint.as_deref().unwrap_or("");
        assert!(
            hint.contains("workspace_teardown_finished"),
            "hint steers to canonical: {hint}"
        );

        // Span check: the warning should point at the alias token, not
        // the whole `on` block. We assert the slice equals the alias.
        let span = w.span.clone();
        assert_eq!(&src[span], "workspace_torndown");
    }

    #[test]
    fn canonical_event_name_emits_no_warning() {
        let src = format!("{PROLOGUE}\non workspace_teardown_finished {{ shell \"echo done\" }}\n");
        let diags = analyze(&src);
        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && !d.message.contains("flat Iterfile syntax"))
            .collect();
        assert!(
            warnings.is_empty(),
            "canonical name must not warn; got {warnings:?}"
        );
    }

    #[test]
    fn each_deprecated_alias_warns_separately() {
        // Multiple aliases in one file: each should produce its own
        // warning with the corresponding canonical recommendation.
        let cases = [
            ("workspace_setting_up", "workspace_setup_starting"),
            ("workspace_set_up", "workspace_setup_finished"),
            ("workspace_tearing_down", "workspace_teardown_starting"),
            ("workspace_torndown", "workspace_teardown_finished"),
        ];
        let mut body = String::new();
        for (alias, _) in cases {
            use std::fmt::Write as _;
            writeln!(body, "on {alias} {{ shell \"echo {alias}\" }}").expect("write to String");
        }
        let src = format!("{PROLOGUE}\n{body}");
        let diags = analyze(&src);
        let warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Warning && !d.message.contains("flat Iterfile syntax"))
            .collect();
        assert_eq!(warnings.len(), cases.len(), "one warning per alias");
        for (alias, canonical) in cases {
            assert!(
                warnings
                    .iter()
                    .any(|w| w.message.contains(alias) && w.message.contains(canonical)),
                "warning for `{alias}` -> `{canonical}` missing in {warnings:?}"
            );
        }
    }

    #[test]
    fn unknown_event_name_is_an_error_not_a_warning() {
        let src = format!("{PROLOGUE}\non not_a_real_event {{ shell \"echo x\" }}\n");
        let diags = analyze(&src);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect();
        assert!(
            !errors.is_empty(),
            "unknown event name must error; diagnostics: {diags:?}"
        );
        // And critically: the unknown-name spell-check must not steer
        // the user toward a deprecated alias.
        for e in &errors {
            let hint = e.hint.as_deref().unwrap_or("");
            for alias in [
                "workspace_setting_up",
                "workspace_set_up",
                "workspace_tearing_down",
                "workspace_torndown",
            ] {
                assert!(
                    !hint.contains(alias),
                    "spell-check hint must not point at deprecated alias `{alias}`: {hint}"
                );
            }
        }
    }
}
