//! End-to-end test for the `iteration.*` placeholder root.
//!
//! Drives a small Iterfile through the full pipeline:
//! [`iter_language::parse`] → [`iter_compose::build_prompt_selector`] →
//! [`iter_core::PromptSelector::render`]. For iterations 1..=6 we check
//! that `prompt when iteration.count % 3 == 0` fires *only* on iterations
//! 3 and 6, falling back to the default prompt on every other turn. This
//! is the contract the design doc calls out as the motivating use case
//! ("every N iterations").

use iter_compose::build_prompt_selector;
use iter_core::{IterationContext, Metadata, Signal};
use iter_language::parse;

const ITERFILE_SOURCE: &str = r#"
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

#[test]
fn iteration_count_modulo_fires_only_on_multiples_of_three() {
    let root = parse(ITERFILE_SOURCE).expect("source parses");
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
    let root = parse(source).expect("source parses");
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
    let root = parse(source).expect("source parses");
    let selector = build_prompt_selector(&root).expect("selector builds");
    let signal = Signal::new(Metadata::new());

    let first = selector
        .render(&signal, &IterationContext::for_count(1))
        .expect("render iter 1");
    assert_eq!(first.as_str(), "first run");
}
