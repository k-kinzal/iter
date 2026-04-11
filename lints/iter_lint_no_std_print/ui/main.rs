// edition:2024
//
// Fixture for the `no_std_print` lint. Each banned macro call below should
// produce one diagnostic. The control cases at the bottom must NOT trigger.

// Aliases / re-exports of `std` used to demonstrate the documented
// bypass shapes (see the limitation block in `src/lib.rs`). Pre-
// expansion linting cannot resolve any of these to `std` without name
// resolution, which is unavailable at this stage.
extern crate std;
use std as s;

mod child {
    // `use std;` makes `self::std::*` resolve inside this module, and
    // the parent's `extern crate std;` at the crate root makes
    // `super::std::*` resolve here as well.
    use std;

    pub fn demo() {
        // `self::std::println!` — bypass via in-module `use std;`.
        self::std::println!("self alias — known not flagged");
        // `super::std::println!` — bypass via the parent crate root.
        super::std::println!("super alias — known not flagged");
    }
}

// Wrapper macro: a pre-expansion lint visits *macro call sites* (`MacCall`
// nodes), not the token stream inside a `macro_rules!` template, so an
// invocation of `wrapped_println!` is not flagged. The `::std::println!`
// inside the template body is also not flagged — at this stage it's still
// a token tree inside the template arm, not a `MacCall`. After expansion
// it would be one, but pre-expansion lints don't see post-expansion code.
macro_rules! wrapped_println {
    ($($t:tt)*) => { ::std::println!($($t)*) };
}

fn main() {
    // ---- Unqualified banned forms (the original baseline). ----
    println!("hello");
    print!("hello");
    eprintln!("oops");
    eprint!("oops");
    let _ = dbg!(42);

    // ---- Qualified banned forms. The lint must catch these too:
    // `std::println!` is the same forbidden macro with an explicit
    // path prefix, so users cannot bypass the rule by writing
    // `std::println!("...")` instead of `println!("...")`. ----
    //
    // (We don't include a `core::*` case because none of the BANNED
    // macros — `println!`, `print!`, `eprintln!`, `eprint!`, `dbg!` —
    // actually exist in `core`; they live in `std`. The lint still
    // matches `core` defensively in case that ever changes, but a
    // fixture line like `core::dbg!(7)` would fail compilation with
    // E0433 before the lint output could be compared, masking what
    // the test is supposed to assert.)
    std::println!("qualified");
    ::std::println!("absolute");
    std::dbg!(7);

    // ---- Documented bypass shapes. Aliasing or re-exporting `std`,
    // or wrapping a banned macro in a `macro_rules!`, defeats the
    // lint; catching any of these soundly would require name
    // resolution and/or post-expansion analysis, neither of which a
    // pre-expansion lint has. None of the lines below MUST trigger a
    // diagnostic — that is what the `.stderr` fixture pins. ----
    s::println!("aliased — known not flagged");
    crate::std::println!("crate alias — known not flagged");
    child::demo();
    wrapped_println!("wrapped — known not flagged");

    // ---- Control: tracing macros are the approved diagnostic
    // alternative; `cli_println!` / `cli_eprintln!` are the approved
    // user-visible-output alternative. Neither should be flagged.
    // (The fixture doesn't actually depend on `tracing` or `iter_cli`,
    // so we just emit strings with the same shape to document intent.) ----
    let _msg = "tracing::info!(\"ok\");";
    let _msg = "cli_println!(\"ok\");";
}
