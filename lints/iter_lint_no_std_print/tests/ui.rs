// UI test entry point. Runs the lint against fixtures under `ui/` and
// compares emitted diagnostics against the corresponding `.stderr` files.
//
// First run (or after intentional diagnostic changes):
//     env BLESS=1 cargo test -p iter_lint_no_std_print --test ui
// then commit the regenerated `.stderr` files.

#[test]
fn ui() {
    dylint_testing::ui_test(env!("CARGO_PKG_NAME"), "ui");
}
