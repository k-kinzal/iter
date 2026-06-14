//! Regression coverage for the removal of flat Iterfile syntax.
//!
//! Flat top-level definitions used to desugar into a synthetic runner behind a
//! deprecation warning. That path is gone: a top-level prompt must still fail,
//! while a runner may implicitly bind the single available agent/workspace.

use iter_language::parse;

const FLAT: &str = r#"
queue memory
workspace clone {
  base = "."
  excludes = []
  preserve_mtime = true
  apply_back { mode = sync }
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

const BOUND: &str = r#"
queue memory
workspace clone {
  base = "."
  excludes = []
  preserve_mtime = true
  apply_back { mode = sync }
}
agent claude {
  mode = print
  command = "claude"
}
runner {
  agent = claude
  workspace = clone
  queue = memory
  continue_on_error = false
  behavior = wait
  prompt = "Iterate."
}
"#;

#[test]
fn single_agent_workspace_omission_is_not_flat_syntax_error() {
    let diags = parse(FLAT).expect_err("flat Iterfile must no longer validate");
    assert!(
        diags.iter().all(|d| !d
            .message
            .contains("flat Iterfile syntax is no longer supported")),
        "single-definition runner binding should not emit the old flat-syntax error; got: {:?}",
        diags.iter().map(|d| d.message.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn top_level_prompt_is_rejected() {
    let diags = parse(FLAT).expect_err("flat Iterfile must no longer validate");
    assert!(
        diags.iter().any(|d| d
            .message
            .contains("top-level `prompt \"...\"` is no longer supported")),
        "expected the top-level-prompt error; got: {:?}",
        diags.iter().map(|d| d.message.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn named_definition_runner_binding_still_validates() {
    parse(BOUND).expect("the named-definition + runner-binding form must still validate");
}
