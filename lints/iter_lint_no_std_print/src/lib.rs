//! Forbid `println!`, `print!`, `eprintln!`, `eprint!`, and `dbg!` in non-test code.
//!
//! iter standardises on the `tracing` crate for diagnostics so that operator
//! output is structured, level-filterable, and routable to backends. Direct
//! use of the `std` print macros bypasses that channel; use `tracing::info!`
//! / `tracing::debug!` / `tracing::error!` instead.
//!
//! This is a pre-expansion lint: it inspects macro *call sites* before the
//! macros are expanded, so it sees `println!(...)` in source rather than its
//! expansion to `std::io::_print(...)`.

#![feature(rustc_private)]
#![warn(unused_extern_crates)]

// `dylint_linting::declare_pre_expansion_lint!` already injects
// `extern crate rustc_lint;` and `extern crate rustc_session;`, so we
// only declare what we additionally use here.
extern crate rustc_ast;

use rustc_ast::ast::MacCall;
use rustc_lint::{EarlyContext, EarlyLintPass, LintContext};

dylint_linting::declare_pre_expansion_lint! {
    /// ### What it does
    ///
    /// Warns on calls to `println!`, `print!`, `eprintln!`, `eprint!`, and
    /// `dbg!` in non-test code.
    ///
    /// ### Why is this bad?
    ///
    /// iter routes operator-facing output through the `tracing` crate. Using
    /// the `std` print macros bypasses level filtering, structured fields,
    /// and span context — the things that make logs useful in production.
    ///
    /// ### Example
    ///
    /// ```rust,ignore
    /// println!("starting runner {}", id);
    /// ```
    ///
    /// Use instead:
    ///
    /// ```rust,ignore
    /// tracing::info!(runner = %id, "starting runner");
    /// ```
    pub NO_STD_PRINT,
    Warn,
    "use the `tracing` crate's macros instead of std `println!`/`eprintln!`/`print!`/`eprint!`/`dbg!`"
}

const BANNED: &[&str] = &["println", "print", "eprintln", "eprint", "dbg"];

impl EarlyLintPass for NoStdPrint {
    fn check_mac(&mut self, cx: &EarlyContext<'_>, mac: &MacCall) {
        // Skip when compiling a test build. Covers `#[cfg(test)]` blocks in
        // library/binary crates as well as integration tests under `tests/`,
        // which are always compiled with `--test`. Test diagnostics may
        // legitimately use the std print macros.
        if cx.sess().opts.test {
            return;
        }
        // Strip a leading path-root segment (the encoding of `::` in
        // `::std::println!`). In `rustc_ast`, `::foo` becomes a path
        // whose first segment has the special name `{{root}}`. Without
        // this strip, `::std::println!` is a 3-segment path and the
        // `len() == 2` check below would miss it — that was the actual
        // bypass we hit with `::std::println!("absolute")` in the
        // fixture.
        let segs: &[_] = match mac.path.segments.first() {
            Some(first) if first.ident.name.as_str() == "{{root}}" => {
                &mac.path.segments[1..]
            }
            _ => &mac.path.segments[..],
        };
        let Some(last) = segs.last() else {
            return;
        };
        let name = last.ident.name;
        let name_str = name.as_str();
        if !BANNED.contains(&name_str) {
            return;
        }
        // Fire on unqualified calls (`println!`) AND on explicit
        // `std::println!` / `std::dbg!` / similar — those are the
        // exact same forbidden macros, just with an explicit path
        // prefix. Without this, `std::println!("...")` would silently
        // bypass the lint. (`core` is matched defensively; none of
        // the banned macros currently live in `core`, but matching it
        // costs nothing and future-proofs against a re-home.)
        // Foreign-crate macros that happen to share a name (e.g.
        // `tracing::debug!` is fine because `debug` is not in BANNED,
        // but a hypothetical `myapp::println!` would be left alone)
        // remain unflagged.
        //
        // Known limitations: any path whose textual root is something
        // other than `std` / `core` / `::std` / `::core` is left
        // alone, even if it ultimately *resolves* to `std`. Pre-
        // expansion linting can only inspect the token sequence — it
        // has no name resolution and no post-expansion view — so all
        // of the following bypass this rule:
        //
        //   use std as s; s::println!("…");                  // alias
        //   extern crate std; crate::std::println!("…");     // crate-root re-export
        //   mod m { use std; fn f() { self::std::println!(...) } } // self
        //   mod m { fn f() { super::std::println!(...) } }   // super (parent re-exports std)
        //   macro_rules! mp { () => { println!("…") } } mp!() // wrapper macro
        //
        // The first four require name resolution to flag; the
        // wrapper-macro case requires post-expansion analysis (the
        // `println!` inside a `macro_rules!` template arm is still a
        // token tree, not a `MacCall`, until expansion). The UI
        // fixture pins each shape so the gap is visible in
        // `cargo test`. The alternative — a heuristic that fires
        // whenever the last two segments are `std|core::<banned>` —
        // would false-positive on local modules named `std`, and
        // would not catch the wrapper-macro case at all. We accept
        // the gap and rely on code review for these unusual shapes.
        let is_unqualified = segs.len() == 1;
        let is_std_or_core = segs.len() == 2
            && matches!(segs[0].ident.name.as_str(), "std" | "core");
        if !is_unqualified && !is_std_or_core {
            return;
        }
        cx.span_lint(NO_STD_PRINT, mac.span(), |diag| {
            diag.primary_message(format!(
                "avoid `{name_str}!`; use the `tracing` crate \
                 (`tracing::info!`, `tracing::debug!`, `tracing::error!`) \
                 for diagnostics, or `cli_println!` / `cli_eprintln!` \
                 (from `iter_cli::output`) for user-visible CLI output"
            ));
        });
    }
}
