use iter_language::{AgentDef, SourceDef, SourceDerive, parse};

fn parse_errs(source: &str) -> Vec<String> {
    parse(source)
        .expect_err("source should be invalid")
        .into_iter()
        .map(|d| d.message)
        .collect()
}

fn iterfile_with_agent(agent: &str) -> String {
    format!(
        r#"
workspace local {{ base = "/tmp/iter-language-test" }}
{agent}
runner {{
  agent = claude
  workspace = local
  continue_on_error = false
  behavior = loop
  prompt = "noop"
}}
"#
    )
}

#[test]
fn parses_directory_and_git_sources() {
    let root = parse(
        r#"
source directory as snap {
  path = "/repo"
  derive = copy { excludes = ["target"] preserve_mtime = false }
  disposition = merge { excludes = ["*.tmp"] includes = ["src/**"] }
}
source git as wt {
  path = "/repo"
  derive = worktree { ref = "HEAD" branch = "iter/test" }
  disposition = merge { into = "main" ff = only }
}
source git as cloned {
  url = "https://example.invalid/repo.git"
  derive = clone { ref = "main" depth = 1 }
  disposition = discard
}
workspace clone as dev {
  source = snap
  excludes = []
  preserve_mtime = true
  apply_back { mode = merge }
}
agent claude { mode = print command = "claude" }
runner {
  agent = claude
  workspace = dev
  continue_on_error = false
  behavior = loop
  prompt = "noop"
}
"#,
    )
    .expect("valid source syntax");

    assert_eq!(root.sources.len(), 3);
    assert!(matches!(
        root.sources[0].node.decl,
        SourceDef::Directory {
            derive: SourceDerive::Copy { .. },
            ..
        }
    ));
}

#[test]
fn source_path_sugar_on_workspace_is_valid() {
    let root = parse(
        r#"
workspace local { source = "/repo" }
agent claude { mode = print command = "claude" }
runner {
  agent = claude
  workspace = local
  continue_on_error = false
  behavior = loop
  prompt = "noop"
}
"#,
    )
    .expect("valid source path sugar");
    assert!(root.sources.is_empty());
}

#[test]
fn output_schema_accepts_json_document_and_direct_json() {
    let json_document = parse(&iterfile_with_agent(
        r#"
agent claude {
  mode = print
  output_schema = """{"type":"object","properties":{"decision":{"type":"string","enum":["fix","continue"]}},"required":["decision"],"additionalProperties":false}"""
}
"#,
    ))
    .expect("JSON document schema");
    let direct = parse(&iterfile_with_agent(
        r#"
agent claude {
  mode = print
  output_schema {
    type = "object"
    properties {
      decision {
        type = "string"
        enum = ["fix", "continue"]
      }
    }
    required = ["decision"]
    additionalProperties = false
  }
}
"#,
    ))
    .expect("direct JSON schema");

    let schema = |root: &iter_language::Iterfile| match &root.agents[0].node.decl {
        AgentDef::Claude {
            output_schema: Some(schema),
            ..
        } => schema.value.clone(),
        other => panic!("unexpected agent definition: {other:?}"),
    };
    assert_eq!(schema(&json_document), schema(&direct));
}

#[test]
fn output_schema_is_rejected_for_interactive_and_unsupported_agents() {
    let interactive = parse_errs(&iterfile_with_agent(
        r#"
agent claude {
  mode = interactive
  output_schema = """{"type":"object"}"""
}
"#,
    ));
    assert!(
        interactive
            .iter()
            .any(|message| message.contains("only valid in print mode")),
        "{interactive:?}",
    );

    let unsupported = parse_errs(&iterfile_with_agent(
        r#"
agent gemini {
  mode = print
  output_schema = """{"type":"object"}"""
}
"#,
    ));
    assert!(
        unsupported
            .iter()
            .any(|message| message.contains("unknown field `output_schema`")),
        "{unsupported:?}",
    );
}

#[test]
fn output_schema_rejects_paths_and_invalid_schemas() {
    let path = parse_errs(&iterfile_with_agent(
        r#"
agent claude {
  mode = print
  output_schema = "./schemas/review.json"
}
"#,
    ));
    assert!(
        path.iter()
            .any(|message| message.contains("invalid JSON in `output_schema`")),
        "{path:?}",
    );

    let invalid = parse_errs(&iterfile_with_agent(
        r#"
agent claude {
  mode = print
  output_schema = """{"type":42}"""
}
"#,
    ));
    assert!(
        invalid
            .iter()
            .any(|message| message.contains("invalid JSON Schema")),
        "{invalid:?}",
    );
}

#[test]
fn agent_output_expression_is_scoped_to_agent_finished() {
    let valid = iterfile_with_agent(
        r"
agent claude { mode = print }
",
    )
    .replace(
        "  prompt = \"noop\"",
        r#"  prompt = "noop"
  on agent_finished when agent.output.decision == "fix" {
    shell "echo fix"
  }"#,
    );
    let root = parse(&valid).expect("agent output expression on agent_finished");
    assert!(root.runners[0].node.events[0].node.condition.is_some());

    let invalid = iterfile_with_agent(
        r"
agent claude { mode = print }
",
    )
    .replace(
        "  prompt = \"noop\"",
        r#"  prompt = "noop"
  on agent_starting when agent.output.decision == "fix" {
    shell "echo fix"
  }"#,
    );
    let messages = parse_errs(&invalid);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("expression root `agent` is not available here")),
        "{messages:?}",
    );
}

#[test]
fn expression_iteration_fields_match_the_runtime_context() {
    let valid = iterfile_with_agent("agent claude { mode = print }").replace(
        "  prompt = \"noop\"",
        r#"  prompt {
    iteration.started_at >= "2026-01-01T00:00:00Z" => "started"
    _ => "waiting"
  }"#,
    );
    parse(&valid).expect("runtime iteration fields must validate");

    for field in ["current_iteration_started_at", "previous_finished_at"] {
        let invalid = iterfile_with_agent("agent claude { mode = print }").replace(
            "  prompt = \"noop\"",
            &format!(
                r#"  prompt {{
    iteration.{field} == "x" => "never"
    _ => "waiting"
  }}"#
            ),
        );
        let messages = parse_errs(&invalid);
        assert!(
            messages
                .iter()
                .any(|message| message.contains(&format!("unknown iteration field `{field}`"))),
            "{messages:?}",
        );
    }
}

#[test]
fn metadata_expressions_preserve_the_string_contract() {
    for expression in ["metadata.attempts == 3", "metadata.enabled == true"] {
        let invalid = iterfile_with_agent("agent claude { mode = print }").replace(
            "  prompt = \"noop\"",
            &format!(
                r#"  prompt {{
    {expression} => "selected"
    _ => "default"
  }}"#
            ),
        );
        let messages = parse_errs(&invalid);
        assert!(
            messages
                .iter()
                .any(|message| message.contains("operands have incompatible types")),
            "{messages:?}",
        );
    }
}

#[test]
fn previous_result_rejects_unknown_values() {
    let invalid = iterfile_with_agent("agent claude { mode = print }").replace(
        "  prompt = \"noop\"",
        r#"  prompt {
    iteration.previous_result == "succes" => "selected"
    _ => "default"
  }"#,
    );
    let messages = parse_errs(&invalid);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("unknown iteration.previous_result value `succes`")),
        "{messages:?}",
    );
}

#[test]
fn statically_non_boolean_conditions_are_rejected() {
    for expression in ["iteration.count", "metadata.kind", "signal"] {
        let invalid = iterfile_with_agent("agent claude { mode = print }").replace(
            "  prompt = \"noop\"",
            &format!(
                r#"  prompt {{
    {expression} => "selected"
    _ => "default"
  }}"#
            ),
        );
        let messages = parse_errs(&invalid);
        assert!(
            messages
                .iter()
                .any(|message| message.contains("condition expression must evaluate to a boolean")),
            "{messages:?}",
        );
    }
}

#[test]
fn prompt_match_accepts_literal_first_common_expressions() {
    let valid = iterfile_with_agent("agent claude { mode = print }").replace(
        "  prompt = \"noop\"",
        r#"  prompt {
    5 % iteration.count == 0 => "numeric"
    "security" == metadata.kind => "metadata"
    _ => "default"
  }"#,
    );
    parse(&valid).expect("literal-first common expressions must parse");
}

#[test]
fn rejects_required_source_errors() {
    let cases = [
        (
            r#"source directory { path = "/repo" derive = passthrough disposition = discard }"#,
            "`disposition` is forbidden when `derive = passthrough`",
        ),
        (
            r#"source directory { path = "/repo" derive = copy }"#,
            "`disposition` is required when `derive` creates a separate base",
        ),
        (
            r#"source directory { path = "/repo" derive = worktree disposition = discard }"#,
            "`worktree` and `clone` derive require `source git`",
        ),
        (
            r#"source git { url = "u" path = "/repo" derive = clone disposition = discard }"#,
            "source git requires exactly one of `url` or `path`, found both",
        ),
        (
            r"source git { derive = clone disposition = discard }",
            "source git requires exactly one of `url` or `path`",
        ),
        (
            r#"source directory { path = "/repo" derive = copy disposition = defer { promote = defer { promote = discard } } }"#,
            "`defer.promote` cannot itself be `defer`",
        ),
        (
            r"workspace local { source = missing }",
            "workspace references source `missing` which is not defined",
        ),
        (
            r#"workspace local { base = "/repo" source = other }"#,
            "workspace local cannot set both `base` and `source`",
        ),
    ];

    for (source, expected) in cases {
        let messages = parse_errs(source);
        assert!(
            messages.iter().any(|m| m.contains(expected)),
            "expected {expected:?} in {messages:?}",
        );
    }
}
