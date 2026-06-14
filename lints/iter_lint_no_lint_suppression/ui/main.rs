// edition:2024
//
// Fixture for the `no_lint_suppression` lint.

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

// ---- Should be flagged: warn downgrades denied lints ----

#[warn(clippy::needless_return)]
fn clippy_warn() -> i32 {
    return 42;
}

#[warn(unsafe_code)]
fn rust_builtin_warn() {}

// ---- Should be flagged: rust built-in lint suppression ----

#[allow(unsafe_code)]
fn rust_builtin_allow() {}

// ---- Should be flagged: dylint lint suppression ----

#[allow(no_lint_suppression)]
fn dylint_allow() {}

#[expect(no_lint_suppression)]
fn dylint_expect() {}

#[warn(no_lint_suppression)]
fn dylint_warn() {}

#[allow(meaningful_type_names)]
fn meaningful_type_names_allow() {}

// ---- Multiple lints in one attribute ----

#[allow(dead_code, clippy::needless_return, unused_variables)]
fn mixed_allow() -> i32 {
    return 42;
}

// ---- Should NOT be flagged: non-lint metadata in expect ----

#[expect(dead_code, reason = "exercise non-lint expect metadata")]
fn expect_with_reason() {}

// ---- Should be flagged: conditional lint-level attributes ----

#[cfg_attr(all(), allow(dead_code))]
fn cfg_attr_allow() {}

fn main() {
    clippy_allow();
    clippy_group_all();
    clippy_group_pedantic();
    clippy_expect();
    clippy_warn();
    rust_builtin_warn();
    rust_builtin_allow();
    dylint_allow();
    dylint_expect();
    dylint_warn();
    meaningful_type_names_allow();
    mixed_allow();
    cfg_attr_allow();
}
