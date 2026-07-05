//! Unit tests for argv construction and headless output parsing.
//!
//! None of these tests invoke Grok; they exercise pure argv builders and the
//! output parser against synthetic fixtures.

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

use crate::agent::AgentCommand;
use crate::args::ToArgs;
use crate::auth::{LoginCommand, LogoutCommand};
use crate::cli::{Error, Grok};
use crate::leader::{LeaderCommand, LeaderProfileSubcommand, LeaderSubcommand};
use crate::mcp::{McpAdd, McpCommand, McpSubcommand};
use crate::memory::{MemoryCommand, MemorySubcommand};
use crate::ops::{CompletionsCommand, UpdateCommand, VersionCommand, WrapCommand};
use crate::output::{Event, EventType, SingleOutput, StopReason};
use crate::plugin::{MarketplaceSubcommand, PluginCommand, PluginSubcommand};
use crate::run::{RunCommand, RunOptions};
use crate::session::{ExportCommand, SessionsCommand, SessionsSubcommand, TraceCommand};
use crate::single::SingleCommand;
use crate::values::{CompletionShell, McpScope, McpTransport, ResumeTarget, Worktree};
use crate::worktree::{WorktreeCommand, WorktreeSubcommand};

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

/// Write an executable shell script that impersonates a headless `grok` run by
/// emitting fixed stdout/stderr and exit code, so the executor can be tested
/// without invoking the real CLI.
#[cfg(unix)]
fn fake_grok(stdout: &str, stderr: &str, code: i32) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("create fake grok executable");
    write!(
        file,
        "#!/bin/sh\ncat <<'__GROK_STDOUT__'\n{stdout}\n__GROK_STDOUT__\ncat >&2 <<'__GROK_STDERR__'\n{stderr}\n__GROK_STDERR__\nexit {code}\n"
    )
    .expect("write fake grok executable");
    let mut permissions = file
        .as_file()
        .metadata()
        .expect("fake metadata")
        .permissions();
    permissions.set_mode(0o700);
    file.as_file()
        .set_permissions(permissions)
        .expect("make fake grok executable");
    file
}

// ----- argv construction -----------------------------------------------------

#[test]
fn single_json_renders_prompt_then_output_format() {
    let command = SingleCommand::prompt("do the thing").json();
    assert_eq!(
        argv(&command),
        ["-p", "do the thing", "--output-format", "json"]
    );
}

#[test]
fn single_streaming_selects_streaming_json() {
    let command = SingleCommand::prompt("hi").streaming();
    assert_eq!(
        argv(&command),
        ["-p", "hi", "--output-format", "streaming-json"]
    );
}

#[test]
fn single_always_approve_and_resume_render_after_output_format() {
    let command = SingleCommand::prompt("hi")
        .always_approve()
        .resume(ResumeTarget::session("s1"))
        .json();
    assert_eq!(
        argv(&command),
        [
            "-p",
            "hi",
            "--output-format",
            "json",
            "--always-approve",
            "-r",
            "s1"
        ]
    );
}

#[test]
fn single_continue_renders_continue_flag() {
    let command = SingleCommand::prompt("hi").continue_session().json();
    assert_eq!(
        argv(&command),
        ["-p", "hi", "--output-format", "json", "--continue"]
    );
}

#[test]
fn resume_most_recent_renders_bare_r() {
    let command = SingleCommand::prompt("hi")
        .resume(ResumeTarget::MostRecent)
        .json();
    assert_eq!(
        argv(&command),
        ["-p", "hi", "--output-format", "json", "-r"]
    );
}

#[test]
fn prompt_file_source_renders_prompt_file_flag() {
    let command = SingleCommand::prompt_file("task.md").json();
    assert_eq!(
        argv(&command),
        ["--prompt-file", "task.md", "--output-format", "json"]
    );
}

#[test]
fn root_run_forwards_options_then_prompt_positional() {
    let command = RunCommand {
        options: RunOptions {
            model: Some("grok-4".to_owned()),
            always_approve: true,
            ..RunOptions::default()
        },
        prompt: Some("hello".to_owned()),
    };
    assert_eq!(
        argv(&command),
        ["--always-approve", "--model", "grok-4", "hello"]
    );
}

#[test]
fn worktree_named_renders_dash_w_with_name() {
    let command = RunCommand {
        options: RunOptions {
            worktree: Some(Worktree::named("feature")),
            ..RunOptions::default()
        },
        prompt: None,
    };
    assert_eq!(argv(&command), ["-w", "feature"]);
}

#[test]
fn root_run_renders_new_0282_flags_in_order() {
    // The flags introduced in 0.2.82 render in their fixed argv positions.
    let command = RunCommand {
        options: RunOptions {
            chat: true,
            fork_session: true,
            json_schema: Some("{\"type\":\"object\"}".to_owned()),
            session_id: Some("11111111-2222-3333-4444-555555555555".to_owned()),
            worktree_ref: Some("main".to_owned()),
            ..RunOptions::default()
        },
        prompt: None,
    };
    assert_eq!(
        argv(&command),
        [
            "--chat",
            "--fork-session",
            "--json-schema",
            "{\"type\":\"object\"}",
            "--session-id",
            "11111111-2222-3333-4444-555555555555",
            "--worktree-ref",
            "main",
        ]
    );
}

#[test]
fn login_oauth_renders_flag() {
    let command = LoginCommand {
        oauth: true,
        ..LoginCommand::default()
    };
    assert_eq!(argv(&command), ["login", "--oauth"]);
}

#[test]
fn logout_is_bare() {
    assert_eq!(argv(&LogoutCommand::default()), ["logout"]);
}

#[test]
fn agent_stdio_renders_options_then_transport_leaf() {
    let command = AgentCommand {
        model: Some("grok-4".to_owned()),
        ..AgentCommand::stdio()
    };
    assert_eq!(argv(&command), ["agent", "--model", "grok-4", "stdio"]);
}

#[test]
fn mcp_add_stdio_renders_env_name_then_command_and_args_after_dashdash() {
    // `add` takes the server name as a positional, then the command/url, then
    // any server args after `--` so grok does not consume flags like `-y`.
    let command = McpCommand::new(McpSubcommand::Add(Box::new(McpAdd {
        command_or_url: Some("npx".to_owned()),
        args: vec!["-y".to_owned(), "server".to_owned()],
        env: vec![("K".to_owned(), "V".to_owned())],
        ..McpAdd::new("fs")
    })));
    assert_eq!(
        argv(&command),
        [
            "mcp", "add", "--env", "K=V", "fs", "--", "npx", "-y", "server"
        ]
    );
}

#[test]
fn mcp_add_without_args_omits_the_dashdash() {
    // With no server args, the command/url follows the name directly (no `--`).
    let command = McpCommand::new(McpSubcommand::Add(Box::new(McpAdd {
        command_or_url: Some("./server".to_owned()),
        ..McpAdd::new("local")
    })));
    assert_eq!(argv(&command), ["mcp", "add", "local", "./server"]);
}

#[test]
fn mcp_add_http_renders_typed_transport_scope_and_header_then_url() {
    // `--transport` / `--scope` are closed sets; out-of-range strings like
    // `--transport banana` are unrepresentable.
    let command = McpCommand::new(McpSubcommand::Add(Box::new(McpAdd {
        command_or_url: Some("https://example.test/mcp".to_owned()),
        transport: Some(McpTransport::Http),
        scope: Some(McpScope::Project),
        header: vec![("Authorization".to_owned(), "Bearer t".to_owned())],
        ..McpAdd::new("remote")
    })));
    assert_eq!(
        argv(&command),
        [
            "mcp",
            "add",
            "--transport",
            "http",
            "--scope",
            "project",
            "--header",
            "Authorization: Bearer t",
            "remote",
            "https://example.test/mcp",
        ]
    );
}

#[test]
fn mcp_transport_and_scope_render_their_closed_set_tokens() {
    assert_eq!(McpTransport::Stdio.as_str(), "stdio");
    assert_eq!(McpTransport::Http.as_str(), "http");
    assert_eq!(McpTransport::Sse.as_str(), "sse");
    assert_eq!(McpScope::User.as_str(), "user");
    assert_eq!(McpScope::Project.as_str(), "project");
}

#[test]
fn mcp_remove_renders_typed_scope_then_name() {
    let command = McpCommand::new(McpSubcommand::Remove {
        name: "fs".to_owned(),
        scope: Some(McpScope::User),
    });
    assert_eq!(argv(&command), ["mcp", "remove", "--scope", "user", "fs"]);
}

#[test]
fn memory_clear_renders_scope_and_yes_flags() {
    let command = MemoryCommand::new(MemorySubcommand::Clear {
        workspace: false,
        global: true,
        all: false,
        yes: true,
    });
    assert_eq!(argv(&command), ["memory", "clear", "--global", "--yes"]);
}

#[test]
fn plugin_install_renders_trust_then_source() {
    let command = PluginCommand::new(PluginSubcommand::Install {
        source: "acme/plugin".to_owned(),
        trust: true,
    });
    assert_eq!(
        argv(&command),
        ["plugin", "install", "--trust", "acme/plugin"]
    );
}

#[test]
fn plugin_marketplace_list_nests_under_marketplace() {
    let command = PluginCommand::new(PluginSubcommand::Marketplace(MarketplaceSubcommand::List {
        args: Vec::new(),
    }));
    assert_eq!(argv(&command), ["plugin", "marketplace", "list"]);
}

#[test]
fn worktree_list_renders_json_and_all() {
    let command = WorktreeCommand::new(WorktreeSubcommand::List {
        repo: None,
        worktree_type: None,
        json: true,
        all: true,
    });
    assert_eq!(argv(&command), ["worktree", "list", "--json", "--all"]);
}

#[test]
fn worktree_rm_renders_flags_then_required_first_id_then_rest() {
    // `rm` always carries at least one id (`first_id`); a zero-id `rm` is
    // unrepresentable because the required positional cannot be empty.
    let command = WorktreeCommand::new(WorktreeSubcommand::Rm {
        first_id: "wt-1".to_owned(),
        rest_ids: vec!["wt-2".to_owned()],
        force: true,
        dry_run: false,
    });
    assert_eq!(
        argv(&command),
        ["worktree", "rm", "--force", "wt-1", "wt-2"]
    );
}

#[test]
fn leader_list_renders_json_flag() {
    let command = LeaderCommand::new(LeaderSubcommand::List { json: true });
    assert_eq!(argv(&command), ["leader", "list", "--json"]);
}

#[test]
fn leader_info_renders_pid_then_json() {
    let command = LeaderCommand::new(LeaderSubcommand::Info {
        pid: Some(4321),
        json: true,
    });
    assert_eq!(
        argv(&command),
        ["leader", "info", "--pid", "4321", "--json"]
    );
}

#[test]
fn leader_kill_is_bare() {
    let command = LeaderCommand::new(LeaderSubcommand::Kill);
    assert_eq!(argv(&command), ["leader", "kill"]);
}

#[test]
fn leader_profile_status_nests_under_profile() {
    let command = LeaderCommand::new(LeaderSubcommand::Profile(LeaderProfileSubcommand::Status {
        args: Vec::new(),
    }));
    assert_eq!(argv(&command), ["leader", "profile", "status"]);
}

#[test]
fn sessions_list_renders_limit() {
    let command = SessionsCommand::new(SessionsSubcommand::List { limit: Some(5) });
    assert_eq!(argv(&command), ["sessions", "list", "--limit", "5"]);
}

#[test]
fn sessions_delete_renders_id() {
    let command = SessionsCommand::new(SessionsSubcommand::Delete {
        id: "sess_1".to_owned(),
    });
    assert_eq!(argv(&command), ["sessions", "delete", "sess_1"]);
}

#[test]
fn export_renders_session_id_then_output_positional() {
    let command = ExportCommand {
        output: Some(PathBuf::from("out.md")),
        ..ExportCommand::new("s1")
    };
    assert_eq!(argv(&command), ["export", "s1", "out.md"]);
}

#[test]
fn trace_renders_local_json_then_session_id() {
    let command = TraceCommand {
        local: true,
        json: true,
        ..TraceCommand::new("s1")
    };
    assert_eq!(argv(&command), ["trace", "--local", "--json", "s1"]);
}

#[test]
fn completions_shell_is_a_bare_positional() {
    assert_eq!(
        argv(&CompletionsCommand::new(CompletionShell::Zsh)),
        ["completions", "zsh"]
    );
}

#[test]
fn version_renders_json() {
    let command = VersionCommand {
        json: true,
        ..VersionCommand::default()
    };
    assert_eq!(argv(&command), ["version", "--json"]);
}

#[test]
fn update_renders_check_and_version() {
    let command = UpdateCommand {
        check: true,
        version: Some("0.2.82".to_owned()),
        ..UpdateCommand::default()
    };
    assert_eq!(argv(&command), ["update", "--check", "--version", "0.2.82"]);
}

#[test]
fn wrap_renders_command_then_args() {
    let command = WrapCommand {
        args: vec!["notes.md".to_owned()],
        ..WrapCommand::new("nvim")
    };
    assert_eq!(argv(&command), ["wrap", "nvim", "notes.md"]);
}

// ----- executor context ------------------------------------------------------

#[test]
fn default_executor_uses_grok() {
    assert_eq!(Grok::default().executable(), OsString::from("grok"));
}

#[test]
fn to_process_carries_cwd_and_env() {
    let grok = Grok::default()
        .with_current_dir("/tmp/work")
        .with_env("XAI_API_KEY", "secret");
    let command = grok.to_process(&SingleCommand::prompt("hi").json());
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
    assert_eq!(env, [("XAI_API_KEY".to_owned(), Some("secret".to_owned()))]);
}

#[test]
fn with_env_replaces_existing_key() {
    let grok = Grok::default().with_env("K", "one").with_env("K", "two");
    let envs: Vec<_> = grok
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

const SINGLE_JSON: &str = r#"{"text":"all done","stopReason":"EndTurn","sessionId":"sess_1","requestId":"req_1","thought":"thinking"}"#;

const STREAM_JSON: &str = r#"{"type":"text","data":"all "}
{"type":"text","data":"done"}
{"type":"end","stopReason":"EndTurn","sessionId":"sess_1","requestId":"req_1","text":"all done"}"#;

#[test]
fn single_object_reports_session_message_reason_and_metadata() {
    let parsed = SingleOutput::parse(SINGLE_JSON);
    assert!(parsed.terminal().is_some());
    assert_eq!(parsed.session_id().as_deref(), Some("sess_1"));
    assert_eq!(parsed.request_id().as_deref(), Some("req_1"));
    assert_eq!(parsed.final_message().as_deref(), Some("all done"));
    assert_eq!(parsed.thought().as_deref(), Some("thinking"));
    assert_eq!(parsed.stop_reason(), StopReason::Stop);
    assert!(parsed.stop_reason().is_stop());
    assert!(parsed.usage().is_empty());
    assert_eq!(parsed.reported_error(), None);
}

#[test]
fn streaming_end_event_is_the_terminal_object() {
    let parsed = SingleOutput::parse(STREAM_JSON);
    assert!(parsed.terminal().is_some());
    assert_eq!(parsed.session_id().as_deref(), Some("sess_1"));
    assert_eq!(parsed.final_message().as_deref(), Some("all done"));
    assert_eq!(parsed.stop_reason(), StopReason::Stop);
}

#[test]
fn error_object_is_reported_not_read_as_success() {
    let parsed = SingleOutput::parse(r#"{"type":"error","message":"model overloaded"}"#);
    assert_eq!(parsed.reported_error().as_deref(), Some("model overloaded"));
    assert_eq!(parsed.session_id(), None);
    assert_eq!(parsed.stop_reason(), StopReason::Unknown);
}

#[test]
fn streaming_error_event_is_not_swallowed_by_a_success_end() {
    let stream = r#"{"type":"text","data":"partial"}
{"type":"error","message":"boom"}
{"type":"end","stopReason":"EndTurn","sessionId":"sess_1"}"#;
    let parsed = SingleOutput::parse(stream);
    assert!(parsed.terminal().is_some());
    assert_eq!(parsed.reported_error().as_deref(), Some("boom"));
}

#[test]
fn usage_is_parsed_defensively_when_present() {
    let stream = r#"{"text":"hi","sessionId":"s","usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15}}"#;
    let usage = SingleOutput::parse(stream).usage();
    assert!(!usage.is_empty());
    assert_eq!(usage.input_tokens, Some(10));
    assert_eq!(usage.output_tokens, Some(5));
    assert_eq!(usage.total_tokens, Some(15));
}

#[test]
fn stop_reason_parses_end_turn() {
    assert_eq!(
        SingleOutput::parse(r#"{"stopReason":"EndTurn"}"#).stop_reason(),
        StopReason::Stop
    );
    assert_eq!(
        SingleOutput::parse(r#"{"stopReason":"MaxTokens"}"#).stop_reason(),
        StopReason::Other("MaxTokens".to_owned())
    );
}

#[test]
fn event_reads_type_and_text() {
    let event = Event::from_value(serde_json::json!({"type":"text","data":"hi"}));
    assert_eq!(event.event_type(), EventType::Text);
    assert_eq!(event.text().as_deref(), Some("hi"));
}

#[test]
fn try_from_succeeds_when_terminal_present_even_on_failure_exit() {
    let parsed = SingleOutput::try_from(output(SINGLE_JSON, 1)).expect("terminal present => Ok");
    assert_eq!(parsed.session_id().as_deref(), Some("sess_1"));
}

#[test]
fn try_from_errors_only_when_no_terminal_and_failed() {
    let result = SingleOutput::try_from(output("not json at all\n", 2));
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
    let parsed = SingleOutput::try_from(output("", 0)).expect("success => Ok even when empty");
    assert!(parsed.terminal().is_none());
}

// ----- executor end-to-end (fake CLI) ---------------------------------------

#[cfg(unix)]
#[test]
fn execute_parses_json_from_a_fake_grok() {
    let script = fake_grok(SINGLE_JSON, "", 0);
    let grok = Grok::new(script.path());
    let parsed = async_runtime()
        .block_on(grok.execute(&SingleCommand::prompt("hi").json()))
        .expect("execute succeeds");
    assert_eq!(parsed.session_id().as_deref(), Some("sess_1"));
    assert_eq!(parsed.final_message().as_deref(), Some("all done"));
}

#[cfg(unix)]
#[test]
fn execute_reports_cli_error_on_empty_failure() {
    let script = fake_grok("not json", "boom", 2);
    let grok = Grok::new(script.path());
    let result = async_runtime().block_on(grok.execute(&SingleCommand::prompt("hi").json()));
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
    let script = fake_grok(STREAM_JSON, "", 0);
    let grok = Grok::new(script.path());
    async_runtime().block_on(async {
        let events: Vec<_> = grok
            .stream(&SingleCommand::prompt("hi").streaming())
            .expect("spawn stream")
            .collect()
            .await;
        let types: Vec<_> = events
            .into_iter()
            .map(|event| event.expect("event ok").event_type())
            .collect();
        assert_eq!(types, [EventType::Text, EventType::Text, EventType::End]);
    });
}

#[cfg(unix)]
#[test]
fn stream_surfaces_failure_exit_after_events() {
    let script = fake_grok(r#"{"type":"text","data":"partial"}"#, "bad exit", 3);
    let grok = Grok::new(script.path());
    async_runtime().block_on(async {
        let mut stream = grok
            .stream(&SingleCommand::prompt("hi").streaming())
            .expect("spawn stream");
        let first = stream.next().await.expect("one event").expect("event ok");
        assert_eq!(first.event_type(), EventType::Text);
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
