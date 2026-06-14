//! Forbid return types with hidden or duplicated failure axes.

#![feature(rustc_private)]

extern crate rustc_ast;
extern crate rustc_span;

use rustc_ast::ast::{
    AngleBracketedArg, FnRetTy, GenericArg, GenericArgs, Item, ItemKind, Ty, TyKind,
};
use rustc_lint::{EarlyContext, EarlyLintPass, LintContext};
use rustc_span::Span;

dylint_linting::declare_pre_expansion_lint! {
    /// ### What it does
    ///
    /// Denies return types shaped as `Result<Result<...>, ...>` or
    /// `Option<Result<...>>`.
    ///
    /// ### Why is this bad?
    ///
    /// One operation should expose one failure axis. Failure belongs on the
    /// outside; absence, when benign, belongs inside `Ok`.
    pub RETURN_TYPE_SHAPE,
    Deny,
    "return types must keep failure outermost and non-duplicated"
}

impl EarlyLintPass for ReturnTypeShape {
    fn check_item(&mut self, cx: &EarlyContext<'_>, item: &Item) {
        let ItemKind::Fn(func) = &item.kind else {
            return;
        };
        check_return(cx, &func.sig.decl.output);
    }

    fn check_impl_item(&mut self, cx: &EarlyContext<'_>, item: &rustc_ast::ast::AssocItem) {
        let rustc_ast::ast::AssocItemKind::Fn(func) = &item.kind else {
            return;
        };
        check_return(cx, &func.sig.decl.output);
    }
}

fn check_return(cx: &EarlyContext<'_>, output: &FnRetTy) {
    let FnRetTy::Ty(ty) = output else {
        return;
    };

    if is_option_of_result(ty) {
        emit_option_result(cx, ty.span);
        return;
    }

    if is_nested_result(ty) {
        emit_nested_result(cx, ty.span);
    }
}

fn emit_nested_result(cx: &EarlyContext<'_>, span: Span) {
    cx.span_lint(RETURN_TYPE_SHAPE, span, |diag| {
        diag.primary_message("nested `Result` in return type");
        diag.help(
            "flatten to a single `Result<T, E>` — one operation has one failure axis; \
             merge with `?` / `.and_then(...)` / `.map_err(...)`.",
        );
    });
}

fn emit_option_result(cx: &EarlyContext<'_>, span: Span) {
    cx.span_lint(RETURN_TYPE_SHAPE, span, |diag| {
        diag.primary_message("`Option<Result<…>>` puts absence above failure");
        diag.help(
            "make failure outermost: return `Result<Option<T>, E>`. If `None` is \
             really an error, fold it into `E`; if it is a benign third state, \
             model it inside `Ok`.",
        );
    });
}

fn is_nested_result(ty: &Ty) -> bool {
    is_result(ty) && generic_type_args(ty).iter().any(|arg| is_result(arg))
}

fn is_option_of_result(ty: &Ty) -> bool {
    is_named_type(ty, "Option")
        && generic_type_args(ty)
            .first()
            .is_some_and(|arg| is_result(arg))
}

fn is_result(ty: &Ty) -> bool {
    is_named_type(ty, "Result")
}

fn is_named_type(ty: &Ty, expected: &str) -> bool {
    let TyKind::Path(_, path) = &ty.kind else {
        return false;
    };
    path.segments
        .last()
        .is_some_and(|segment| segment.ident.name.as_str() == expected)
}

fn generic_type_args(ty: &Ty) -> Vec<&Ty> {
    let TyKind::Path(_, path) = &ty.kind else {
        return Vec::new();
    };
    let Some(segment) = path.segments.last() else {
        return Vec::new();
    };
    let Some(args) = &segment.args else {
        return Vec::new();
    };
    let GenericArgs::AngleBracketed(args) = &**args else {
        return Vec::new();
    };
    args.args
        .iter()
        .filter_map(|arg| match arg {
            AngleBracketedArg::Arg(GenericArg::Type(ty)) => Some(&**ty),
            _ => None,
        })
        .collect()
}
