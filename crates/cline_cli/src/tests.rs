//! Unit tests for argv construction and NDJSON output parsing.
//!
//! None of these tests invoke Cline; they exercise pure argv builders and the
//! run-stream parser against synthetic fixtures. The only processes spawned are
//! throwaway shell scripts that impersonate `cline --json`.

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
use crate::auth::AuthCommand;
use crate::cli::{Cline, Error};
use crate::connect::ConnectCommand;
use crate::history::{HistoryCommand, HistorySubcommand};
use crate::hub::{HubCommand, HubSubcommand};
use crate::ops::{
    ConfigCommand, DashboardCommand, DoctorCommand, DoctorSubcommand, HookCommand, KanbanCommand,
    McpCommand, UpdateCommand, VersionCommand,
};
use crate::output::{EventType, FinishReason, RunOutput};
use crate::plugin::{PluginCommand, PluginSubcommand};
use crate::run::{RunCommand, RunOptions};
use crate::schedule::{ScheduleCommand, ScheduleCreateOptions, ScheduleSubcommand};
use crate::values::{AgentMode, ThinkingLevel};

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

/// Write an executable shell script that impersonates `cline --json` by emitting
/// fixed stdout/stderr and exit code, so the executor can be tested without
/// invoking the real CLI.
#[cfg(unix)]
fn fake_cline(stdout: &str, stderr: &str, code: i32) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("create fake cline executable");
    write!(
        file,
        "#!/bin/sh\ncat <<'__CLINE_STDOUT__'\n{stdout}\n__CLINE_STDOUT__\ncat >&2 <<'__CLINE_STDERR__'\n{stderr}\n__CLINE_STDERR__\nexit {code}\n"
    )
    .expect("write fake cline executable");
    let mut permissions = file
        .as_file()
        .metadata()
        .expect("fake metadata")
        .permissions();
    permissions.set_mode(0o700);
    file.as_file()
        .set_permissions(permissions)
        .expect("make fake cline executable");
    file
}

// ----- argv construction: root run ------------------------------------------

#[test]
fn run_prompt_is_a_bare_positional_without_json() {
    let command = RunCommand::prompt("do the thing");
    assert_eq!(argv(&command), ["do the thing"]);
}

#[test]
fn json_run_places_json_first_then_options_then_prompt() {
    let command = RunCommand {
        options: RunOptions {
            plan: true,
            auto_approve: Some(false),
            thinking: Some(ThinkingLevel::High),
            model: Some("anthropic/claude".to_owned()),
            ..RunOptions::default()
        },
        prompt: Some("go".to_owned()),
    }
    .json();

    assert_eq!(
        argv(&command),
        [
            "--json",
            "--plan",
            "--auto-approve",
            "false",
            "--thinking",
            "high",
            "--model",
            "anthropic/claude",
            "go",
        ]
    );
}

#[test]
fn json_run_without_prompt_omits_the_positional() {
    let command = RunCommand::default().json();
    assert_eq!(argv(&command), ["--json"]);
}

// ----- argv construction: subcommands ---------------------------------------

#[test]
fn auth_renders_flags_then_positional() {
    let command = AuthCommand {
        provider: Some("anthropic".to_owned()),
        apikey: Some("sk-xxx".to_owned()),
        verbose: true,
        ..AuthCommand::default()
    };
    assert_eq!(
        argv(&command),
        [
            "auth",
            "--provider",
            "anthropic",
            "--apikey",
            "sk-xxx",
            "--verbose"
        ]
    );
}

#[test]
fn auth_provider_positional_is_a_bare_argument() {
    let command = AuthCommand {
        provider_positional: Some("openai".to_owned()),
        ..AuthCommand::default()
    };
    assert_eq!(argv(&command), ["auth", "openai"]);
}

#[test]
fn plugin_install_renders_flags_before_source() {
    let command = PluginCommand::new(PluginSubcommand::Install {
        source: "github:foo/bar".to_owned(),
        npm: false,
        git: true,
        force: true,
        json: false,
        cwd: None,
    });
    assert_eq!(
        argv(&command),
        ["plugin", "install", "--git", "--force", "github:foo/bar"]
    );
}

#[test]
fn plugin_uninstall_renders_flag_before_name() {
    let command = PluginCommand::new(PluginSubcommand::Uninstall {
        name: "foo".to_owned(),
        json: true,
        cwd: None,
    });
    assert_eq!(argv(&command), ["plugin", "uninstall", "--json", "foo"]);
}

#[test]
fn connect_channel_is_followed_by_passthrough_args() {
    let command = ConnectCommand {
        channel: Some("slack".to_owned()),
        stop: false,
        channel_args: vec!["--bot-token".to_owned(), "xoxb".to_owned()],
    };
    assert_eq!(argv(&command), ["connect", "slack", "--bot-token", "xoxb"]);
}

#[test]
fn connect_stop_is_a_bare_flag() {
    let command = ConnectCommand {
        stop: true,
        ..ConnectCommand::default()
    };
    assert_eq!(argv(&command), ["connect", "--stop"]);
}

#[test]
fn history_list_renders_pagination_flags() {
    let command = HistoryCommand::new(HistorySubcommand::List {
        json: true,
        limit: Some(10),
        page: None,
        config: None,
    });
    assert_eq!(argv(&command), ["history", "--json", "--limit", "10"]);
}

#[test]
fn history_export_places_output_before_session_id() {
    let command = HistoryCommand::new(HistorySubcommand::Export {
        session_id: "sess-1".to_owned(),
        output: Some(PathBuf::from("/tmp/out.json")),
    });
    assert_eq!(
        argv(&command),
        ["history", "export", "--output", "/tmp/out.json", "sess-1"]
    );
}

#[test]
fn schedule_create_renders_options_then_name() {
    let command = ScheduleCommand::new(ScheduleSubcommand::Create {
        name: "nightly".to_owned(),
        options: Box::new(ScheduleCreateOptions {
            cron: Some("0 0 * * *".to_owned()),
            max_parallel: Some(2),
            mode: Some(AgentMode::Plan),
            autonomous: Some(true),
            json: true,
            ..ScheduleCreateOptions::default()
        }),
    });
    assert_eq!(
        argv(&command),
        [
            "schedule",
            "create",
            "--cron",
            "0 0 * * *",
            "--max-parallel",
            "2",
            "--mode",
            "plan",
            "--autonomous",
            "--json",
            "nightly",
        ]
    );
}

#[test]
fn schedule_create_no_autonomous_renders_negated_flag() {
    let command = ScheduleCommand::new(ScheduleSubcommand::Create {
        name: "nightly".to_owned(),
        options: Box::new(ScheduleCreateOptions {
            autonomous: Some(false),
            ..ScheduleCreateOptions::default()
        }),
    });
    assert_eq!(
        argv(&command),
        ["schedule", "create", "--no-autonomous", "nightly"]
    );
}

#[test]
fn schedule_list_renders_filter_flags() {
    let command = ScheduleCommand::new(ScheduleSubcommand::List {
        disabled: false,
        enabled: true,
        limit: Some(5),
        tags: Some("ci".to_owned()),
        address: None,
        json: true,
    });
    assert_eq!(
        argv(&command),
        [
            "schedule",
            "list",
            "--enabled",
            "--limit",
            "5",
            "--tags",
            "ci",
            "--json"
        ]
    );
}

#[test]
fn schedule_trigger_places_id_before_address_and_json() {
    let command = ScheduleCommand::new(ScheduleSubcommand::Trigger {
        schedule_id: "sch-9".to_owned(),
        address: Some("localhost:1234".to_owned()),
        json: true,
    });
    assert_eq!(
        argv(&command),
        [
            "schedule",
            "trigger",
            "sch-9",
            "--address",
            "localhost:1234",
            "--json"
        ]
    );
}

#[test]
fn schedule_history_places_flags_before_id() {
    let command = ScheduleCommand::new(ScheduleSubcommand::History {
        schedule_id: "sch-9".to_owned(),
        limit: Some(3),
        status: Some("failed".to_owned()),
        address: None,
        json: false,
    });
    assert_eq!(
        argv(&command),
        [
            "schedule", "history", "--limit", "3", "--status", "failed", "sch-9"
        ]
    );
}

#[test]
fn schedule_export_places_to_before_id() {
    let command = ScheduleCommand::new(ScheduleSubcommand::Export {
        schedule_id: "sch-9".to_owned(),
        to: Some(PathBuf::from("/tmp/s.json")),
        address: None,
        json: false,
    });
    assert_eq!(
        argv(&command),
        ["schedule", "export", "--to", "/tmp/s.json", "sch-9"]
    );
}

#[test]
fn hub_leaf_is_a_bare_subcommand() {
    assert_eq!(
        argv(&HubCommand::new(HubSubcommand::Ensure)),
        ["hub", "ensure"]
    );
}

#[test]
fn hub_raw_passes_argv_through() {
    let command = HubCommand::new(HubSubcommand::Raw {
        args: vec!["status".to_owned(), "--json".to_owned()],
    });
    assert_eq!(argv(&command), ["hub", "status", "--json"]);
}

#[test]
fn config_renders_json_flag() {
    let command = ConfigCommand {
        json: true,
        config: None,
    };
    assert_eq!(argv(&command), ["config", "--json"]);
}

#[test]
fn doctor_report_renders_report_flags() {
    let command = DoctorCommand::new(DoctorSubcommand::Report {
        cwd: None,
        json: false,
        verbose: true,
    });
    assert_eq!(argv(&command), ["doctor", "--verbose"]);
}

#[test]
fn doctor_fix_appends_fix_subcommand() {
    let command = DoctorCommand::new(DoctorSubcommand::Fix {
        cwd: None,
        json: true,
        verbose: false,
    });
    assert_eq!(argv(&command), ["doctor", "fix", "--json"]);
}

#[test]
fn dashboard_renders_bind_flags() {
    let command = DashboardCommand {
        port: Some(8080),
        no_open: true,
        ..DashboardCommand::default()
    };
    assert_eq!(argv(&command), ["dashboard", "--port", "8080", "--no-open"]);
}

#[test]
fn mcp_passes_argv_through() {
    let command = McpCommand {
        args: vec!["list".to_owned()],
    };
    assert_eq!(argv(&command), ["mcp", "list"]);
}

#[test]
fn hook_passes_argv_through() {
    let command = HookCommand {
        args: vec!["preToolUse".to_owned()],
    };
    assert_eq!(argv(&command), ["hook", "preToolUse"]);
}

#[test]
fn update_renders_verbose_flag() {
    let command = UpdateCommand {
        verbose: true,
        config: None,
    };
    assert_eq!(argv(&command), ["update", "--verbose"]);
}

#[test]
fn version_and_kanban_are_bare() {
    assert_eq!(argv(&VersionCommand), ["version"]);
    assert_eq!(argv(&KanbanCommand::default()), ["kanban"]);
}

// ----- executor context ------------------------------------------------------

#[test]
fn default_executor_uses_cline() {
    assert_eq!(Cline::default().executable(), OsString::from("cline"));
}

#[test]
fn to_process_carries_cwd_and_env() {
    let cline = Cline::default()
        .with_current_dir("/tmp/work")
        .with_env("CLINE_DIR", "/tmp/home");
    let command = cline.to_process(&RunCommand::prompt("hi").json());
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
        [("CLINE_DIR".to_owned(), Some("/tmp/home".to_owned()))]
    );
}

#[test]
fn with_env_replaces_existing_key() {
    let cline = Cline::default().with_env("K", "one").with_env("K", "two");
    let envs: Vec<_> = cline
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

// ----- output parsing --------------------------------------------------------

const RUN_STREAM: &str = r#"{"type":"session_started","sessionId":"sess-1"}
{"type":"assistant_message","message":"working"}
{"type":"run_result","finishReason":"completed","sessionId":"sess-1","message":"all done"}"#;

#[test]
fn parse_collects_object_events_only() {
    let parsed = RunOutput::parse(RUN_STREAM);
    assert_eq!(parsed.events().len(), 3);
    assert_eq!(parsed.events()[2].event_type(), EventType::RunResult);
}

#[test]
fn parse_skips_non_json_and_non_object_lines() {
    let stream =
        "Loading model...\n42\n[1,2,3]\n{\"type\":\"run_result\",\"finishReason\":\"completed\"}\n";
    let parsed = RunOutput::parse(stream);
    assert_eq!(parsed.events().len(), 1);
    assert_eq!(parsed.events()[0].event_type(), EventType::RunResult);
}

#[test]
fn run_result_reports_session_message_and_finish() {
    let parsed = RunOutput::parse(RUN_STREAM);
    assert_eq!(parsed.session_id().as_deref(), Some("sess-1"));
    assert_eq!(parsed.final_message().as_deref(), Some("all done"));
    let finish = parsed.finish_reason().expect("a terminal record");
    assert_eq!(finish, FinishReason::Completed);
    assert!(finish.is_completed());
}

#[test]
fn non_completed_finish_reason_is_reported_verbatim() {
    let parsed = RunOutput::parse(r#"{"type":"run_result","finishReason":"aborted"}"#);
    let finish = parsed.finish_reason().expect("a terminal record");
    assert!(!finish.is_completed());
    assert_eq!(finish.as_str(), "aborted");
}

#[test]
fn last_run_result_wins_over_earlier_error() {
    let stream = concat!(
        r#"{"type":"error","message":"transient"}"#,
        "\n",
        r#"{"type":"run_result","finishReason":"completed","message":"done"}"#,
    );
    let parsed = RunOutput::parse(stream);
    assert!(
        parsed
            .finish_reason()
            .expect("terminal record")
            .is_completed()
    );
    // The error event is still surfaced for callers that want it.
    assert_eq!(parsed.failure_message().as_deref(), Some("transient"));
}

#[test]
fn failure_message_prefers_run_aborted_reason() {
    let parsed = RunOutput::parse(r#"{"type":"run_aborted","reason":"user cancelled"}"#);
    assert_eq!(parsed.failure_message().as_deref(), Some("user cancelled"));
    assert!(parsed.run_result().is_none());
}

#[test]
fn failure_message_falls_back_to_error_event() {
    let parsed = RunOutput::parse(r#"{"type":"error","message":"rate limited"}"#);
    assert_eq!(parsed.failure_message().as_deref(), Some("rate limited"));
}

#[test]
fn no_terminal_record_yields_no_run_result() {
    let parsed = RunOutput::parse(r#"{"type":"assistant_message","message":"hi"}"#);
    assert!(parsed.run_result().is_none());
    assert!(parsed.failure_message().is_none());
}

#[test]
fn try_from_succeeds_when_events_present_even_on_failure_exit() {
    let parsed = RunOutput::try_from(output(RUN_STREAM, 1)).expect("events present => Ok");
    assert!(
        parsed
            .finish_reason()
            .expect("terminal record")
            .is_completed()
    );
}

#[test]
fn try_from_errors_only_when_empty_and_failed() {
    let result = RunOutput::try_from(output("not json at all\n", 2));
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
    let parsed = RunOutput::try_from(output("", 0)).expect("success => Ok even when empty");
    assert!(parsed.events().is_empty());
}

// ----- executor end-to-end (fake CLI) ---------------------------------------

#[cfg(unix)]
#[test]
fn execute_parses_json_stream_from_a_fake_cline() {
    let script = fake_cline(RUN_STREAM, "", 0);
    let cline = Cline::new(script.path());
    let parsed = async_runtime()
        .block_on(cline.execute(&RunCommand::prompt("hi").json()))
        .expect("execute succeeds");
    assert_eq!(parsed.session_id().as_deref(), Some("sess-1"));
    assert!(
        parsed
            .finish_reason()
            .expect("terminal record")
            .is_completed()
    );
}

#[cfg(unix)]
#[test]
fn execute_reports_cli_error_on_empty_failure() {
    let script = fake_cline("not json", "boom", 2);
    let cline = Cline::new(script.path());
    let result = async_runtime().block_on(cline.execute(&RunCommand::prompt("hi").json()));
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
    let script = fake_cline(RUN_STREAM, "", 0);
    let cline = Cline::new(script.path());
    async_runtime().block_on(async {
        let events: Vec<_> = cline
            .stream(&RunCommand::prompt("hi").json())
            .expect("spawn stream")
            .collect()
            .await;
        let types: Vec<_> = events
            .into_iter()
            .map(|event| event.expect("event ok").event_type())
            .collect();
        assert_eq!(
            types,
            [
                EventType::Other(Some("session_started".to_owned())),
                EventType::Other(Some("assistant_message".to_owned())),
                EventType::RunResult,
            ]
        );
    });
}

#[cfg(unix)]
#[test]
fn stream_surfaces_failure_exit_after_events() {
    let script = fake_cline(r#"{"type":"run_aborted","reason":"x"}"#, "bad exit", 3);
    let cline = Cline::new(script.path());
    async_runtime().block_on(async {
        let mut stream = cline
            .stream(&RunCommand::prompt("hi").json())
            .expect("spawn stream");
        let first = stream.next().await.expect("one event").expect("event ok");
        assert_eq!(first.event_type(), EventType::RunAborted);
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
