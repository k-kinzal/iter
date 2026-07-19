use iter_language::{CompletionConditionDef, CompletionConditionErrorPolicy, parse};

fn iterfile_with_completion(body: &str) -> String {
    format!(
        r#"
workspace local {{ source = "." }}
agent claude {{ mode = print command = "claude" }}
runner {{
  agent = claude
  workspace = local
  continue_on_error = false
  behavior = loop
  prompt = "noop"
  completion {{
    {body}
  }}
}}
"#
    )
}

#[test]
fn parses_ordered_named_completion_conditions() {
    let root = parse(&iterfile_with_completion(
        r#"
condition iterations as iteration_budget { max = 50 }
condition shell as goal_reached {
  run = "./scripts/exploration-complete.sh"
  timeout = 30s
  on_error = abort
}
condition elapsed as time_budget { duration = 90m }
condition deadline as reporting_day { at = "2026-08-01T00:00:00+09:00" }
"#,
    ))
    .expect("completion syntax should parse");

    let conditions = &root.runners[0]
        .node
        .completion
        .as_ref()
        .expect("completion")
        .conditions;
    assert_eq!(conditions.len(), 4);
    assert!(matches!(
        conditions[0].node,
        CompletionConditionDef::Iterations {
            ref name,
            max: 50
        } if name == "iteration_budget"
    ));
    assert!(matches!(
        conditions[1].node,
        CompletionConditionDef::Shell {
            ref name,
            timeout_secs: 30,
            on_error: CompletionConditionErrorPolicy::Abort,
            ..
        } if name == "goal_reached"
    ));
    assert!(matches!(
        conditions[2].node,
        CompletionConditionDef::Elapsed {
            ref name,
            duration_secs: 5400
        } if name == "time_budget"
    ));
    assert!(matches!(
        conditions[3].node,
        CompletionConditionDef::Deadline { ref name, .. }
            if name == "reporting_day"
    ));
}

#[test]
fn rejects_empty_completion_and_invalid_condition_fields() {
    let cases = [
        (
            iterfile_with_completion(""),
            "requires at least one condition",
        ),
        (
            iterfile_with_completion("condition iterations as budget { max = 0 }"),
            "`max` must be positive",
        ),
        (
            iterfile_with_completion(r#"condition shell as goal { run = "true" timeout = 1s }"#),
            "requires `on_error`",
        ),
        (
            iterfile_with_completion(r#"condition deadline as cutoff { at = "tomorrow" }"#),
            "RFC 3339",
        ),
    ];

    for (source, expected) in cases {
        let errors = parse(&source).expect_err("case should be rejected");
        assert!(
            errors.iter().any(|error| error.message.contains(expected)),
            "expected {expected:?}, got {errors:#?}"
        );
    }
}

#[test]
fn rejects_duplicate_condition_names() {
    let errors = parse(&iterfile_with_completion(
        r"
condition iterations as budget { max = 1 }
condition elapsed as budget { duration = 1m }
",
    ))
    .expect_err("duplicate names should fail");
    assert!(errors.iter().any(|error| {
        error
            .message
            .contains("duplicate completion condition name")
    }));
}
