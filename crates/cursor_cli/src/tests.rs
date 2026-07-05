//! Unit tests for argv construction and `--print` output parsing.
//!
//! None of these tests invoke the real `cursor-agent`; they exercise pure argv
//! builders, the `json`/`stream-json` parser, and the executor against a fake
//! CLI script that only echoes fixed output.

use std::ffi::OsString;
#[cfg(unix)]
use std::io::Write as _;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::process::ExitStatusExt as _;
use std::path::PathBuf;
use std::process::{ExitStatus, Output as ProcessOutput};

use futures::StreamExt as _;
use pretty_assertions::assert_eq;

use crate::args::ToArgs;
use crate::auth::{LoginCommand, LogoutCommand, StatusCommand};
use crate::cli::{Cursor, Error};
use crate::mcp::{McpCommand, McpSubcommand};
use crate::ops::{
    AboutCommand, GenerateRuleCommand, InstallShellIntegrationCommand, ModelsCommand,
    UninstallShellIntegrationCommand, UpdateCommand,
};
use crate::output::{EventType, PrintOutput};
use crate::run::{AgentCommand, PrintCommand, RunCommand, RunOptions};
use crate::session::{CreateChatCommand, LsCommand, ResumeCommand};
use crate::values::{ExecutionMode, ResumeSelector, SandboxMode, Worktree};

/// Render a command's argv as `String`s for readable assertions.
fn argv<C: ToArgs + ?Sized>(command: &C) -> Vec<String> {
    command
        .to_args()
        .into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

fn output(stdout: &str, code: i32) -> ProcessOutput {
    ProcessOutput {
        status: ExitStatus::from_raw(code << 8),
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    }
}

fn async_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .expect("build tokio runtime")
}

/// Write an executable shell script that impersonates `cursor-agent --print` by
/// emitting fixed stdout/stderr and exit code, so the executor can be tested
/// without invoking the real CLI.
#[cfg(unix)]
fn fake_cursor(stdout: &str, stderr: &str, code: i32) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("create fake cursor executable");
    write!(
        file,
        "#!/bin/sh\ncat <<'__CURSOR_STDOUT__'\n{stdout}\n__CURSOR_STDOUT__\ncat >&2 <<'__CURSOR_STDERR__'\n{stderr}\n__CURSOR_STDERR__\nexit {code}\n"
    )
    .expect("write fake cursor executable");
    let mut permissions = file
        .as_file()
        .metadata()
        .expect("fake metadata")
        .permissions();
    permissions.set_mode(0o700);
    file.as_file()
        .set_permissions(permissions)
        .expect("make fake cursor executable");
    file
}

// ----- argv construction -----------------------------------------------------

#[test]
fn print_json_prompt_renders_print_format_then_prompt() {
    let command = PrintCommand::json().prompt("do the thing");
    assert_eq!(
        argv(&command),
        ["--print", "--output-format", "json", "do the thing"]
    );
}

#[test]
fn print_stream_json_with_partial_output_appends_the_flag() {
    let command = PrintCommand {
        stream_partial_output: true,
        ..PrintCommand::stream_json()
    };
    assert_eq!(
        argv(&command),
        [
            "--print",
            "--output-format",
            "stream-json",
            "--stream-partial-output"
        ]
    );
}

#[test]
fn print_text_selects_the_text_format() {
    assert_eq!(
        argv(&PrintCommand::text()),
        ["--print", "--output-format", "text"]
    );
}

#[test]
fn print_renders_format_then_options_then_prompt_in_order() {
    let options = RunOptions {
        api_key: Some("k".to_owned()),
        headers: vec![
            ("A".to_owned(), "1".to_owned()),
            ("B".to_owned(), "2".to_owned()),
        ],
        cloud: true,
        mode: Some(ExecutionMode::Plan),
        model: Some("gpt-5".to_owned()),
        force: true,
        sandbox: Some(SandboxMode::Disabled),
        trust: true,
        workspace: Some(PathBuf::from("/w")),
        ..RunOptions::default()
    };
    let command = PrintCommand {
        options,
        prompt: vec!["go".to_owned()],
        ..PrintCommand::json()
    };
    assert_eq!(
        argv(&command),
        [
            "--print",
            "--output-format",
            "json",
            "--api-key",
            "k",
            "--header",
            "A: 1",
            "--header",
            "B: 2",
            "--cloud",
            "--mode",
            "plan",
            "--model",
            "gpt-5",
            "--force",
            "--sandbox",
            "disabled",
            "--trust",
            "--workspace",
            "/w",
            "go",
        ]
    );
}

#[test]
fn resume_without_value_is_a_bare_flag() {
    let command = RunCommand {
        options: RunOptions {
            resume: Some(ResumeSelector::Prompt),
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(argv(&command), ["--resume"]);
}

#[test]
fn resume_with_chat_id_appends_the_value() {
    let command = RunCommand {
        options: RunOptions {
            resume: Some(ResumeSelector::Chat("c1".to_owned())),
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(argv(&command), ["--resume", "c1"]);
}

#[test]
fn worktree_optional_value_renders_both_shapes() {
    let auto = RunCommand {
        options: RunOptions {
            worktree: Some(Worktree::Auto),
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(argv(&auto), ["--worktree"]);

    let named = RunCommand {
        options: RunOptions {
            worktree: Some(Worktree::Named("wt".to_owned())),
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(argv(&named), ["--worktree", "wt"]);
}

#[test]
fn header_pairs_render_as_name_colon_value() {
    // The name/value split is typed, so a header can only be built from a
    // `(name, value)` pair — a bare colon-less string is unrepresentable.
    // Each pair renders as the CLI's `Name: Value` form, and the value side
    // may itself contain colons without ambiguity.
    let command = RunCommand {
        options: RunOptions {
            headers: vec![
                ("Authorization".to_owned(), "Bearer xyz".to_owned()),
                ("X-Custom".to_owned(), "a: b".to_owned()),
            ],
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(
        argv(&command),
        [
            "--header",
            "Authorization: Bearer xyz",
            "--header",
            "X-Custom: a: b",
        ]
    );
}

#[test]
fn root_run_forwards_prompt_without_print() {
    assert_eq!(argv(&RunCommand::prompt("hello")), ["hello"]);
}

#[test]
fn agent_subcommand_prefixes_agent() {
    assert_eq!(argv(&AgentCommand::prompt("hi")), ["agent", "hi"]);
}

#[test]
fn auth_commands_are_bare() {
    assert_eq!(argv(&LoginCommand), ["login"]);
    assert_eq!(argv(&LogoutCommand), ["logout"]);
    assert_eq!(argv(&StatusCommand), ["status"]);
}

#[test]
fn mcp_list_tools_forwards_identifier() {
    let command = McpCommand::new(McpSubcommand::ListTools {
        identifier: "srv".to_owned(),
    });
    assert_eq!(argv(&command), ["mcp", "list-tools", "srv"]);
}

#[test]
fn mcp_list_is_bare() {
    assert_eq!(argv(&McpCommand::new(McpSubcommand::List)), ["mcp", "list"]);
}

#[test]
fn session_commands_are_bare() {
    assert_eq!(argv(&CreateChatCommand), ["create-chat"]);
    assert_eq!(argv(&LsCommand), ["ls"]);
    assert_eq!(argv(&ResumeCommand), ["resume"]);
}

#[test]
fn ops_commands_are_bare() {
    assert_eq!(argv(&ModelsCommand), ["models"]);
    assert_eq!(argv(&AboutCommand), ["about"]);
    assert_eq!(argv(&UpdateCommand), ["update"]);
    assert_eq!(argv(&GenerateRuleCommand), ["generate-rule"]);
    assert_eq!(
        argv(&InstallShellIntegrationCommand),
        ["install-shell-integration"]
    );
    assert_eq!(
        argv(&UninstallShellIntegrationCommand),
        ["uninstall-shell-integration"]
    );
}

// ----- executor context ------------------------------------------------------

#[test]
fn default_executor_uses_cursor_agent() {
    assert_eq!(
        Cursor::default().executable(),
        OsString::from("cursor-agent")
    );
}

#[test]
fn to_process_carries_cwd_and_env() {
    let cursor = Cursor::default()
        .with_current_dir("/tmp/work")
        .with_env("CURSOR_API_KEY", "secret");
    let command = cursor.to_process(&PrintCommand::json().prompt("hi"));
    let process = command.as_std();
    assert_eq!(
        process.get_current_dir(),
        Some(std::path::Path::new("/tmp/work"))
    );
    let env: Vec<_> = process
        .get_envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect();
    assert_eq!(
        env,
        [("CURSOR_API_KEY".to_owned(), Some("secret".to_owned()))]
    );
}

#[test]
fn with_env_replaces_existing_key() {
    let cursor = Cursor::default().with_env("K", "one").with_env("K", "two");
    let envs: Vec<_> = cursor
        .envs()
        .map(|(key, value)| {
            (
                key.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect();
    assert_eq!(envs, [("K".to_owned(), "two".to_owned())]);
}

#[test]
fn remove_env_drops_the_key() {
    let mut cursor = Cursor::default().with_env("K", "one");
    cursor.remove_env("K");
    assert_eq!(cursor.envs().count(), 0);
}

// ----- output parsing --------------------------------------------------------

const RESULT_JSON: &str = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":4200,"result":"all done","session_id":"sess_1","request_id":"req_9","usage":{"input_tokens":12,"output_tokens":7,"num_turns":1}}"#;

const STREAM_JSON: &str = r#"{"type":"system","subtype":"init","session_id":"sess_1"}
{"type":"assistant","message":"working"}
{"type":"result","subtype":"success","is_error":false,"result":"all done","session_id":"sess_1","request_id":"req_9","usage":{"input_tokens":12,"output_tokens":7}}"#;

#[test]
fn single_json_object_is_parsed_as_one_result_event() {
    let parsed = PrintOutput::parse(RESULT_JSON);
    assert_eq!(parsed.events().len(), 1);
    assert!(parsed.succeeded());
    assert_eq!(
        parsed.result_record().expect("result record").event_type(),
        EventType::Result
    );
}

#[test]
fn result_record_exposes_terminal_verdict() {
    let parsed = PrintOutput::parse(RESULT_JSON);
    assert_eq!(parsed.session_id().as_deref(), Some("sess_1"));
    assert_eq!(parsed.request_id().as_deref(), Some("req_9"));
    assert_eq!(parsed.final_message().as_deref(), Some("all done"));
    assert_eq!(parsed.subtype().as_deref(), Some("success"));
    assert_eq!(parsed.duration_ms(), Some(4200));
    assert_eq!(parsed.is_error_flag(), Some(false));
    let usage = parsed.usage();
    assert_eq!(usage.input_tokens, Some(12));
    assert_eq!(usage.output_tokens, Some(7));
    assert_eq!(usage.num_turns, Some(1));
}

#[test]
fn stream_json_collects_events_and_finds_terminal_result() {
    let parsed = PrintOutput::parse(STREAM_JSON);
    assert_eq!(parsed.events().len(), 3);
    assert_eq!(parsed.events()[1].event_type(), EventType::Assistant);
    assert!(parsed.succeeded());
    assert_eq!(parsed.final_message().as_deref(), Some("all done"));
}

#[test]
fn parse_skips_non_json_and_non_object_lines() {
    let stream = "Loading...\n42\n[1,2,3]\n{\"type\":\"result\",\"result\":\"ok\"}\n";
    let parsed = PrintOutput::parse(stream);
    assert_eq!(parsed.events().len(), 1);
    assert_eq!(parsed.final_message().as_deref(), Some("ok"));
}

#[test]
fn no_result_record_is_not_a_success() {
    let parsed = PrintOutput::parse(r#"{"type":"assistant","message":"partial"}"#);
    assert!(!parsed.succeeded());
    assert!(parsed.result_record().is_none());
}

#[test]
fn error_record_surfaces_its_message() {
    let parsed = PrintOutput::parse(r#"{"type":"error","message":"rate limited"}"#);
    assert!(!parsed.succeeded());
    assert_eq!(parsed.error_message().as_deref(), Some("rate limited"));
    assert_eq!(
        parsed.error_record().expect("error record").event_type(),
        EventType::Error
    );
}

#[test]
fn unknown_type_falls_through_to_other() {
    let parsed = PrintOutput::parse(r#"{"type":"telemetry","value":1}"#);
    assert_eq!(
        parsed.events()[0].event_type(),
        EventType::Other(Some("telemetry".to_owned()))
    );
}

#[test]
fn try_from_succeeds_when_events_present_even_on_failure_exit() {
    let parsed = PrintOutput::try_from(output(RESULT_JSON, 1)).expect("events present => Ok");
    assert!(parsed.succeeded());
}

#[test]
fn try_from_errors_only_when_empty_and_failed() {
    let result = PrintOutput::try_from(output("not json at all\n", 2));
    assert!(
        matches!(
            result,
            Err(Error::Cli {
                exit_code: Some(2),
                ..
            })
        ),
        "expected Error::Cli with exit code 2, got {result:?}"
    );
}

#[test]
fn try_from_succeeds_on_empty_but_successful() {
    let parsed = PrintOutput::try_from(output("", 0)).expect("success => Ok even when empty");
    assert!(parsed.events().is_empty());
}

// ----- executor end-to-end (fake CLI) ---------------------------------------

#[cfg(unix)]
#[test]
fn execute_parses_json_from_a_fake_cursor() {
    let script = fake_cursor(RESULT_JSON, "", 0);
    let cursor = Cursor::new(script.path());
    let parsed = async_runtime()
        .block_on(cursor.execute(&PrintCommand::json().prompt("hi")))
        .expect("execute succeeds");
    assert!(parsed.succeeded());
    assert_eq!(parsed.session_id().as_deref(), Some("sess_1"));
}

#[cfg(unix)]
#[test]
fn execute_reports_cli_error_on_empty_failure() {
    let script = fake_cursor("not json", "boom", 2);
    let cursor = Cursor::new(script.path());
    let result = async_runtime().block_on(cursor.execute(&PrintCommand::json().prompt("hi")));
    assert!(
        matches!(
            result,
            Err(Error::Cli {
                exit_code: Some(2),
                ..
            })
        ),
        "expected Error::Cli, got {result:?}"
    );
}

#[cfg(unix)]
#[test]
fn stream_yields_events_then_verifies_exit() {
    let script = fake_cursor(STREAM_JSON, "", 0);
    let cursor = Cursor::new(script.path());
    async_runtime().block_on(async {
        let events: Vec<_> = cursor
            .stream(&PrintCommand::stream_json().prompt("hi"))
            .expect("spawn stream")
            .collect()
            .await;
        let types: Vec<_> = events
            .into_iter()
            .map(|event| event.expect("event ok").event_type())
            .collect();
        assert_eq!(
            types,
            [EventType::System, EventType::Assistant, EventType::Result]
        );
    });
}

#[cfg(unix)]
#[test]
fn stream_surfaces_failure_exit_after_events() {
    let script = fake_cursor(r#"{"type":"system"}"#, "bad exit", 3);
    let cursor = Cursor::new(script.path());
    async_runtime().block_on(async {
        let mut stream = cursor
            .stream(&PrintCommand::stream_json().prompt("hi"))
            .expect("spawn stream");
        let first = stream.next().await.expect("one event").expect("event ok");
        assert_eq!(first.event_type(), EventType::System);
        let tail = stream.next().await.expect("exit verdict");
        assert!(
            matches!(
                tail,
                Err(Error::Cli {
                    exit_code: Some(3),
                    ..
                })
            ),
            "expected Error::Cli on non-zero exit, got {tail:?}"
        );
    });
}
