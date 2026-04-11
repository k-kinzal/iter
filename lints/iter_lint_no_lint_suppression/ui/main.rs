// edition:2024
//
// Fixture for the `no_lint_suppression` lint.
//
// `unknown_lints` is a standard Rust lint, not clippy/dylint, so allowing it
// is fine and avoids noise from lint names that rustc does not know about
// (e.g. `no_std_print` which is only registered when its dylint library is
// loaded).
#![allow(unknown_lints)]
#![allow(unused)]

// ---- Should be flagged: clippy lint suppression via allow ----

#[allow(clippy::needless_return)]
fn clippy_allow() -> i32 {
    return 42;
}

#[allow(clippy::all)]
fn clippy_group_all() {}

#[allow(clippy::pedantic)]
fn clippy_group_pedantic() {}

// ---- Should be flagged: clippy lint suppression via expect ----

#[expect(clippy::needless_return)]
fn clippy_expect() -> i32 {
    return 42;
}

// ---- Should be flagged: dylint lint suppression ----

#[allow(no_std_print)]
fn dylint_allow() {}

#[expect(no_std_print)]
fn dylint_expect() {}

#[allow(no_lint_suppression)]
fn meta_allow() {}

// ---- Multiple lints in one attribute ----

#[allow(dead_code, clippy::needless_return, unused_variables)]
fn mixed_allow() -> i32 {
    return 42;
}

// ---- Should NOT be flagged: standard Rust lints ----

#[allow(dead_code)]
fn rust_allow() {}

#[allow(unused_variables)]
fn rust_allow2() {
    let _x = 42;
}

#[expect(dead_code)]
fn rust_expect() {}

// ---- Should NOT be flagged: inner crate-level allow of Rust lints ----
// (the `#![allow(unknown_lints)]` and `#![allow(unused)]` above are also
// control cases.)

fn main() {
    clippy_allow();
    clippy_group_all();
    clippy_group_pedantic();
    clippy_expect();
    dylint_allow();
    dylint_expect();
    meta_allow();
    mixed_allow();
}
