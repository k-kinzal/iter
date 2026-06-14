//! Forbid inline lint-level attributes that suppress diagnostics.
//!
//! iter treats lint diagnostics as guardrails, not suggestions. Silencing them
//! with `allow`, `warn`, or `expect` hides problems instead of fixing them.
//! This lint fires whenever an attribute attempts to suppress a Rust, Clippy,
//! or Dylint lint inline.
//!
//! This is a pre-expansion lint: it inspects attributes before macro expansion.

#![feature(rustc_private)]

extern crate rustc_ast;

use rustc_ast::ast::{AttrKind, Attribute, MetaItem, MetaItemInner, MetaItemKind};
use rustc_lint::{EarlyContext, EarlyLintPass, LintContext};

dylint_linting::declare_pre_expansion_lint! {
    /// ### What it does
    ///
    /// Denies inline allow, warn, and expect attributes that target Rust,
    /// Clippy, or Dylint lints.
    ///
    /// ### Why is this bad?
    ///
    /// Suppressing lints hides problems. The project policy is to fix the
    /// code, not silence the messenger.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// #[/* allow */(clippy::needless_return)]
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
    "do not suppress diagnostics with inline lint-level attributes"
}

const LINT_LEVEL_ATTRIBUTES: &[&str] = &["allow", "expect", "warn"];

impl EarlyLintPass for NoLintSuppression {
    fn check_attribute(&mut self, cx: &EarlyContext<'_>, attr: &Attribute) {
        let AttrKind::Normal(ref normal) = attr.kind else {
            return;
        };
        let Some(ident) = normal.item.path.segments.last() else {
            return;
        };
        let attr_name = ident.ident.name.as_str();

        if LINT_LEVEL_ATTRIBUTES.contains(&attr_name) {
            lint_items(cx, attr.meta_item_list().as_deref());
        } else if attr_name == "cfg_attr" {
            lint_cfg_attr(cx, attr.meta_item_list().as_deref());
        }
    }
}

fn lint_cfg_attr(cx: &EarlyContext<'_>, items: Option<&[MetaItemInner]>) {
    let Some(items) = items else {
        return;
    };

    for item in items.iter().skip(1) {
        let Some(meta) = item.meta_item() else {
            continue;
        };
        let Some(ident) = meta.path.segments.last() else {
            continue;
        };
        let attr_name = ident.ident.name.as_str();
        if !LINT_LEVEL_ATTRIBUTES.contains(&attr_name) {
            continue;
        }
        let MetaItemKind::List(ref nested_items) = meta.kind else {
            continue;
        };

        lint_items(cx, Some(nested_items));
    }
}

fn lint_items(cx: &EarlyContext<'_>, items: Option<&[MetaItemInner]>) {
    let Some(items) = items else {
        return;
    };

    for item in items {
        let Some(meta) = item.meta_item() else {
            continue;
        };

        if !is_lint_argument(meta) {
            continue;
        }

        let lint_path = meta
            .path
            .segments
            .iter()
            .map(|s| s.ident.name.as_str())
            .collect::<Vec<_>>()
            .join("::");

        let mut diag = cx.sess().dcx().struct_span_err(
            meta.span,
            format!("suppressing `{lint_path}` inline is not allowed"),
        );
        diag.help(
            "fix the underlying diagnostic; if the lint is wrong for this project, set it to \
             `allow` once in `[workspace.lints]` (or the `iter_core` table) with a reason — \
             never inline.",
        );
        diag.emit();
    }
}

fn is_lint_argument(meta: &MetaItem) -> bool {
    matches!(meta.kind, MetaItemKind::Word)
}
