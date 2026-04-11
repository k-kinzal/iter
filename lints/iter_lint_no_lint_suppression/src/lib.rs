//! Forbid `#[allow(...)]` and `#[expect(...)]` that suppress clippy or dylint
//! diagnostics.
//!
//! iter treats lint diagnostics as guardrails, not suggestions. Silencing them
//! with `allow` / `expect` hides problems instead of fixing them. This lint
//! fires whenever an attribute attempts to suppress a `clippy::*` or known
//! dylint lint.
//!
//! This is a pre-expansion lint: it inspects attributes before macro expansion.

#![feature(rustc_private)]
#![warn(unused_extern_crates)]

extern crate rustc_ast;

use rustc_ast::ast::{AttrKind, Attribute};
use rustc_lint::{EarlyContext, EarlyLintPass, LintContext};

dylint_linting::declare_pre_expansion_lint! {
    /// ### What it does
    ///
    /// Denies `#[allow(...)]` and `#[expect(...)]` attributes that target
    /// `clippy::*` lints or known dylint lints.
    ///
    /// ### Why is this bad?
    ///
    /// Suppressing lints hides problems. The project policy is to fix the
    /// code, not silence the messenger.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// #[allow(clippy::needless_return)]
    /// fn foo() -> i32 { return 42; }
    /// ```
    ///
    /// Fix instead:
    ///
    /// ```rust,ignore
    /// fn foo() -> i32 { 42 }
    /// ```
    pub NO_LINT_SUPPRESSION,
    Deny,
    "do not suppress clippy or dylint diagnostics with `#[allow(...)]` or `#[expect(...)]`"
}

const KNOWN_DYLINT_LINTS: &[&str] = &[
    "no_std_print",
    "no_lint_suppression",
];

impl EarlyLintPass for NoLintSuppression {
    fn check_attribute(&mut self, cx: &EarlyContext<'_>, attr: &Attribute) {
        let AttrKind::Normal(ref normal) = attr.kind else {
            return;
        };
        let Some(ident) = normal.item.path.segments.last() else {
            return;
        };
        let attr_name = ident.ident.name.as_str();
        if attr_name != "allow" && attr_name != "expect" {
            return;
        }

        let Some(items) = attr.meta_item_list() else {
            return;
        };

        for item in &items {
            let Some(meta) = item.meta_item() else {
                continue;
            };
            let segments: Vec<&str> = meta
                .path
                .segments
                .iter()
                .map(|s| s.ident.name.as_str())
                .collect();

            if segments.first() == Some(&"clippy") {
                let lint_path = segments.join("::");
                cx.span_lint(NO_LINT_SUPPRESSION, meta.span, |diag| {
                    diag.primary_message(format!(
                        "suppressing `{lint_path}` via `#[{attr_name}(...)]` \
                         is not allowed; fix the underlying diagnostic instead"
                    ));
                });
                continue;
            }

            if segments.len() == 1 && KNOWN_DYLINT_LINTS.contains(&segments[0]) {
                let name = segments[0];
                cx.span_lint(NO_LINT_SUPPRESSION, meta.span, |diag| {
                    diag.primary_message(format!(
                        "suppressing `{name}` via `#[{attr_name}(...)]` \
                         is not allowed; fix the underlying diagnostic instead"
                    ));
                });
            }
        }
    }
}
