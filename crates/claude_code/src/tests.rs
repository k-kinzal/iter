use std::ffi::OsString;
#[cfg(unix)]
use std::io::Write as _;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;

use futures::StreamExt as _;
use pretty_assertions::assert_eq;

use crate::args::ToArgs;
use crate::command::ClaudeCodeCommand;
use crate::*;

fn strings(args: Vec<OsString>) -> Vec<String> {
    args.into_iter()
        .map(|arg| arg.into_string().expect("test args are utf-8"))
        .collect()
}

#[cfg(unix)]
fn fake_claude(stdout: &str, stderr: &str, code: i32) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("create fake claude executable");
    write!(
        file,
        "#!/bin/sh\ncat <<'__CLAUDE_STDOUT__'\n{stdout}\n__CLAUDE_STDOUT__\ncat >&2 <<'__CLAUDE_STDERR__'\n{stderr}\n__CLAUDE_STDERR__\nexit {code}\n"
    )
    .expect("write fake claude executable");
    let mut permissions = file
        .as_file()
        .metadata()
        .expect("fake metadata")
        .permissions();
    permissions.set_mode(0o700);
    file.as_file()
        .set_permissions(permissions)
        .expect("make fake claude executable");
    file
}

#[cfg(unix)]
fn async_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .expect("build tokio runtime")
}

#[test]
fn renders_execute_command_json() {
    let mut command = ExecuteCommand::prompt("fix tests");
    command.permission_mode = Some(PermissionMode::BypassPermissions);
    command.session_id = Some("00000000-0000-4000-8000-000000000000".parse().unwrap());
    let command = command.json();

    assert_eq!(
        strings(command.to_args()),
        vec![
            "--output-format",
            "json",
            "--permission-mode",
            "bypassPermissions",
            "--print",
            "--session-id",
            "00000000-0000-4000-8000-000000000000",
            "--",
            "fix tests",
        ]
    );
}

#[test]
fn json_mode_selects_json_output_args() {
    let command = ExecuteCommand::prompt("structured").json();

    assert_eq!(
        strings(command.to_args()),
        vec!["--output-format", "json", "--print", "--", "structured"]
    );
}

#[test]
fn stream_json_mode_selects_stream_output_args() {
    let command = ExecuteCommand::prompt("stream").stream_json();

    assert_eq!(
        strings(command.to_args()),
        vec![
            "--output-format",
            "stream-json",
            "--print",
            "--verbose",
            "--",
            "stream"
        ]
    );
}

#[test]
fn renders_optional_value_with_equals() {
    let command = ExecuteCommand {
        resume: Some(OptionalValue::Value("session-id".to_owned())),
        debug: Some(OptionalValue::Present),
        prompt: Some("continue".to_owned()),
        ..ExecuteCommand::default()
    };

    assert_eq!(
        strings(command.to_args()),
        vec!["--debug", "--resume=session-id", "--", "continue"]
    );
}

#[test]
fn renders_prompt_only_with_boundary() {
    let command = ExecuteCommand {
        prompt: Some("--summarize".to_owned()),
        ..ExecuteCommand::default()
    };

    assert_eq!(strings(command.to_args()), vec!["--", "--summarize"]);
}

#[test]
fn renders_prompt_boundary_after_variadic_option() {
    let command = ExecuteCommand {
        add_dirs: vec![PathBuf::from("/repo")],
        prompt: Some("do work".to_owned()),
        ..ExecuteCommand::default()
    };

    assert_eq!(
        strings(command.to_args()),
        vec!["--add-dir", "/repo", "--", "do work"]
    );
}

#[test]
fn renders_mcp_add_stdio_with_separator() {
    let mut add = McpAdd::new("repo", "npx");
    add.env.push("TOKEN=abc".to_owned());
    add.args.push("@example/mcp".to_owned());
    add.args.push("--flag".to_owned());

    assert_eq!(
        strings(ClaudeCodeCommand::Mcp(Mcp::Add(add)).to_args()),
        vec![
            "mcp",
            "add",
            "--env",
            "TOKEN=abc",
            "repo",
            "--",
            "npx",
            "@example/mcp",
            "--flag",
        ]
    );
}

#[test]
fn renders_mcp_add_with_explicit_separator_switch() {
    let mut add = McpAdd::new("repo", "npx");
    add.separate_command = Switch::On;

    assert_eq!(
        strings(ClaudeCodeCommand::Mcp(Mcp::Add(add)).to_args()),
        vec!["mcp", "add", "repo", "--", "npx"]
    );
}

#[test]
fn renders_mcp_client_secret_as_prompt_flag() {
    let mut add = McpAdd::new("repo", "https://example.test/sse");
    add.client_secret = Switch::On;
    add.client_id = Some("client-id".to_owned());
    add.transport = Some(McpTransport::Sse);

    assert_eq!(
        strings(ClaudeCodeCommand::Mcp(Mcp::Add(add)).to_args()),
        vec![
            "mcp",
            "add",
            "--client-id",
            "client-id",
            "--client-secret",
            "--transport",
            "sse",
            "repo",
            "https://example.test/sse",
        ]
    );
}

#[test]
fn renders_fallback_models_as_one_comma_separated_value() {
    let command = ExecuteCommand {
        fallback_models: vec!["model-a".to_owned(), "model-b".to_owned()],
        ..ExecuteCommand::default()
    };

    assert_eq!(
        strings(command.to_args()),
        vec!["--fallback-model", "model-a,model-b"]
    );
}

#[test]
fn renders_plugin_marketplace_add() {
    let command = ClaudeCodeCommand::Plugin(Plugin::Marketplace(PluginMarketplace::Add(
        PluginMarketplaceAdd {
            source: "github:owner/repo".to_owned(),
            scope: Some(PluginScope::Project),
            sparse: vec![PathBuf::from(".claude-plugin"), PathBuf::from("plugins")],
        },
    )));

    assert_eq!(
        strings(command.to_args()),
        vec![
            "plugin",
            "marketplace",
            "add",
            "--scope",
            "project",
            "--sparse",
            ".claude-plugin",
            "--sparse",
            "plugins",
            "--",
            "github:owner/repo",
        ]
    );
}

#[test]
fn renders_direct_subcommand_to_args() {
    let command = Agents {
        json: Switch::On,
        all: Switch::On,
        ..Agents::default()
    };

    assert_eq!(
        strings(command.to_args()),
        vec!["agents", "--all", "--json"]
    );
}

#[test]
fn renders_project_purge() {
    let command = ClaudeCodeCommand::Project(Project::Purge(ProjectPurge {
        target: Some(ProjectPurgeTarget::Path(PathBuf::from("/tmp/project"))),
        dry_run: Switch::On,
        yes: Switch::On,
        ..ProjectPurge::default()
    }));

    assert_eq!(
        strings(command.to_args()),
        vec!["project", "purge", "--dry-run", "--yes", "/tmp/project"]
    );
}

#[test]
fn parses_json_result_output() {
    let parsed: JsonOutput = serde_json::from_str(
        r#"{"type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"sid","num_turns":1,"total_cost_usd":0.01,"usage":{"input_tokens":2},"new_field":{"nested":true}}"#,
    )
    .expect("valid result json");

    assert_eq!(parsed.output_type, JsonOutputType::Result);
    assert_eq!(parsed.subtype, ResultSubtype::Success);
    assert_eq!(parsed.result.as_deref(), Some("ok"));
    assert_eq!(
        parsed.extra.get("new_field"),
        Some(&serde_json::json!({"nested": true}))
    );
}

#[test]
fn preserves_unknown_json_result_subtype_and_fields_on_round_trip() {
    let parsed: JsonOutput = serde_json::from_str(
        r#"{"type":"result","subtype":"future_subtype","is_error":false,"future":42}"#,
    )
    .expect("forward-compatible result json");

    assert_eq!(
        parsed.subtype,
        ResultSubtype::Other("future_subtype".to_owned())
    );

    let serialized = serde_json::to_value(&parsed).expect("serializable result json");
    assert_eq!(serialized["type"], "result");
    assert_eq!(serialized["subtype"], "future_subtype");
    assert_eq!(serialized["future"], 42);
    assert!(serialized.get("result").is_none());
    assert!(serialized.get("session_id").is_none());
    assert!(serialized.get("num_turns").is_none());
    assert!(serialized.get("total_cost_usd").is_none());
    assert!(serialized.get("usage").is_none());
}

#[test]
fn rejects_invalid_json_result_type() {
    let err = serde_json::from_str::<JsonOutput>(
        r#"{"type":"assistant","subtype":"success","is_error":false}"#,
    )
    .expect_err("non-result object must be rejected");

    assert!(err.to_string().contains("expected result event type"));
}

#[test]
fn max_budget_rejects_non_finite_values() {
    assert!(MaxBudgetUsd::new(0.0).is_ok());
    assert!(MaxBudgetUsd::new(f64::NAN).is_err());
    assert!(MaxBudgetUsd::new(f64::INFINITY).is_err());
    assert!(MaxBudgetUsd::new(-0.01).is_err());
}

#[test]
fn parses_stream_json_lines() {
    let parsed = parse_stream_json(
        r#"{"type":"system","session_id":"sid"}
{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}
{"type":"future_event","payload":{"x":1}}
{"type":"result","subtype":"success","is_error":false}"#,
    )
    .expect("valid stream-json");

    assert_eq!(parsed.len(), 4);
    assert_eq!(parsed[0].event_type, StreamEventType::System);
    assert_eq!(parsed[1].event_type, StreamEventType::RateLimitEvent);
    assert_eq!(
        parsed[2].event_type,
        StreamEventType::Other("future_event".to_owned())
    );
    assert_eq!(
        parsed[2].fields.get("payload"),
        Some(&serde_json::json!({"x": 1}))
    );
    assert_eq!(parsed[3].event_type, StreamEventType::Result);
}

#[test]
#[cfg(unix)]
fn execute_json_returns_parsed_result_even_when_exit_is_unsuccessful() {
    let script = fake_claude(
        r#"{"type":"result","subtype":"error_during_execution","is_error":true}"#,
        "failed",
        2,
    );
    let cli = ClaudeCode::new(script.path().as_os_str());

    let result = async_runtime()
        .block_on(cli.execute(&ExecuteCommand::default().json()))
        .expect("parseable JSON result wins over non-zero exit");

    assert_eq!(result.subtype, ResultSubtype::ErrorDuringExecution);
    assert!(result.is_error);
}

#[test]
#[cfg(unix)]
fn execute_json_returns_cli_error_for_unparseable_unsuccessful_output() {
    let script = fake_claude("not json", "bad exit", 2);
    let cli = ClaudeCode::new(script.path().as_os_str());

    let err = async_runtime()
        .block_on(cli.execute(&ExecuteCommand::default().json()))
        .expect_err("unparseable unsuccessful output is a CLI error");

    assert!(matches!(err, Error::Cli { .. }));
    if let Error::Cli {
        exit_code,
        stdout,
        stderr,
    } = err
    {
        assert_eq!(exit_code, Some(2));
        assert!(stdout.contains("not json"));
        assert!(stderr.contains("bad exit"));
    }
}

#[test]
#[cfg(unix)]
fn execute_json_returns_json_error_for_unparseable_successful_output() {
    let script = fake_claude("not json", "", 0);
    let cli = ClaudeCode::new(script.path().as_os_str());

    let err = async_runtime()
        .block_on(cli.execute(&ExecuteCommand::default().json()))
        .expect_err("unparseable successful output is a JSON error");

    assert!(matches!(err, Error::Json(_)));
}

#[test]
#[cfg(unix)]
fn execute_stream_json_returns_events_even_when_exit_is_unsuccessful() {
    let script = fake_claude(
        r#"{"type":"result","subtype":"error_during_execution","is_error":true}"#,
        "failed",
        2,
    );
    let cli = ClaudeCode::new(script.path().as_os_str());

    let mut events = async_runtime()
        .block_on(cli.execute(&ExecuteCommand::default().stream_json()))
        .expect("parseable stream events win over non-zero exit");

    async_runtime().block_on(async {
        let event = events
            .next()
            .await
            .expect("stream event")
            .expect("parse event");
        assert_eq!(event.event_type, StreamEventType::Result);
        assert!(events.next().await.is_none());
    });
}

#[test]
#[cfg(unix)]
fn execute_stream_json_returns_cli_error_for_empty_unsuccessful_output() {
    let script = fake_claude("", "bad exit", 2);
    let cli = ClaudeCode::new(script.path().as_os_str());

    let err = async_runtime()
        .block_on(cli.execute(&ExecuteCommand::default().stream_json()))
        .expect_err("empty unsuccessful stream output is a CLI error");

    assert!(matches!(err, Error::Cli { .. }));
    if let Error::Cli { stderr, .. } = err {
        assert!(stderr.contains("bad exit"));
    }
}

#[test]
#[cfg(unix)]
fn execute_subcommand_returns_stdout_without_exposing_process_api() {
    let script = fake_claude("agents output", "", 0);
    let claude = ClaudeCode::new(script.path().as_os_str());

    let output = async_runtime()
        .block_on(claude.execute(&Agents {
            all: Switch::On,
            ..Agents::default()
        }))
        .expect("subcommands execute through ClaudeCode");

    assert!(output.contains("agents output"));
}

#[test]
#[cfg(unix)]
fn stream_json_reader_yields_events_and_waits_successfully() {
    let script = fake_claude(
        r#"
{"type":"system","session_id":"sid"}

{"type":"result","subtype":"success","is_error":false}"#,
        "",
        0,
    );
    let cli = ClaudeCode::new(script.path().as_os_str());

    async_runtime().block_on(async {
        let mut stream = cli
            .stream(&ExecuteCommand::default().stream_json())
            .expect("start stream-json reader");

        let first = stream
            .next()
            .await
            .expect("first event")
            .expect("read first event");
        let second = stream
            .next()
            .await
            .expect("second event")
            .expect("read second event");

        assert_eq!(first.event_type, StreamEventType::System);
        assert_eq!(second.event_type, StreamEventType::Result);
        assert!(stream.next().await.is_none());
    });
}

#[test]
#[cfg(unix)]
fn stream_json_reader_reports_cli_error_on_wait_failure() {
    let script = fake_claude(
        r#"{"type":"result","subtype":"error_during_execution","is_error":true}"#,
        "stream failed",
        9,
    );
    let cli = ClaudeCode::new(script.path().as_os_str());

    async_runtime().block_on(async {
        let mut stream = cli
            .stream(&ExecuteCommand::default().stream_json())
            .expect("start stream-json reader");
        let event = stream
            .next()
            .await
            .expect("event before failure")
            .expect("read event");

        assert_eq!(event.event_type, StreamEventType::Result);

        let err = stream
            .next()
            .await
            .expect("final stream status")
            .expect_err("non-zero exit is a CLI error");
        assert!(matches!(err, Error::Cli { .. }));
        if let Error::Cli {
            exit_code, stderr, ..
        } = err
        {
            assert_eq!(exit_code, Some(9));
            assert!(stderr.contains("stream failed"));
        }
    });
}
