//! Unit tests for argv construction and JSONL output parsing.
//!
//! None of these tests invoke Codex; they exercise pure argv builders and the
//! stream parser against synthetic fixtures.

use std::ffi::OsString;
#[cfg(unix)]
use std::io::Write as _;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{ExitStatus, Output as ProcessOutput};

use futures::StreamExt as _;
use pretty_assertions::assert_eq;

use crate::args::ToArgs;
use crate::auth::{LoginCommand, LogoutCommand};
use crate::cli::{Codex, Error};
use crate::exec::{
    ExecCommand, ExecOptions, ExecResumeCommand, ExecReviewCommand, ExecSubcommandOptions,
};
use crate::features::{FeaturesCommand, FeaturesSubcommand};
use crate::mcp::{McpCommand, McpSubcommand, McpTransport};
use crate::ops::{
    CompletionCommand, DoctorCommand, RemoteControlCommand, RemoteControlSubcommand, SandboxCommand,
};
use crate::options::{CommonConfig, GlobalConfig};
use crate::output::{EventType, ExecOutput, TurnStatus};
use crate::plugin::{PluginCommand, PluginMarketplaceSubcommand, PluginSubcommand};
use crate::review::ReviewCommand;
use crate::run::{RunCommand, RunOptions};
use crate::session::{ArchiveCommand, ForkCommand, ResumeCommand, UnarchiveCommand};
use crate::values::{
    ApprovalPolicy, Color, CompletionShell, ConfigOverride, LocalProvider, SandboxMode,
};

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

/// Write an executable shell script that impersonates `codex exec --json` by
/// emitting fixed stdout/stderr and exit code, so the executor can be tested
/// without invoking the real CLI.
#[cfg(unix)]
fn fake_codex(stdout: &str, stderr: &str, code: i32) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("create fake codex executable");
    write!(
        file,
        "#!/bin/sh\ncat <<'__CODEX_STDOUT__'\n{stdout}\n__CODEX_STDOUT__\ncat >&2 <<'__CODEX_STDERR__'\n{stderr}\n__CODEX_STDERR__\nexit {code}\n"
    )
    .expect("write fake codex executable");
    let mut permissions = file
        .as_file()
        .metadata()
        .expect("fake metadata")
        .permissions();
    permissions.set_mode(0o700);
    file.as_file()
        .set_permissions(permissions)
        .expect("make fake codex executable");
    file
}

// ----- argv construction -----------------------------------------------------

#[test]
fn exec_json_prompt_renders_exec_json_then_prompt() {
    let command = ExecCommand::prompt("do the thing").json();
    assert_eq!(argv(&command), ["exec", "--json", "do the thing"]);
}

#[test]
fn exec_without_json_omits_the_flag() {
    let command = ExecCommand::prompt("hi");
    assert_eq!(argv(&command), ["exec", "hi"]);
}

#[test]
fn exec_renders_common_then_options_then_prompt_in_order() {
    let command = ExecCommand {
        common: CommonConfig {
            model: Some("gpt-5-codex".to_owned()),
            sandbox: Some(SandboxMode::WorkspaceWrite),
            ..CommonConfig::default()
        },
        options: ExecOptions {
            skip_git_repo_check: true,
            color: Some(Color::Never),
            ..ExecOptions::default()
        },
        prompt: Some("go".to_owned()),
    }
    .json();

    assert_eq!(
        argv(&command),
        [
            "exec",
            "--json",
            "--model",
            "gpt-5-codex",
            "--sandbox",
            "workspace-write",
            "--skip-git-repo-check",
            "--color",
            "never",
            "go",
        ]
    );
}

#[test]
fn config_overrides_render_as_key_equals_value() {
    let command = ExecCommand {
        common: CommonConfig {
            config: vec![ConfigOverride::new("features.codex_hooks", "true")],
            ..CommonConfig::default()
        },
        ..ExecCommand::default()
    };
    assert_eq!(
        argv(&command),
        ["exec", "--config", "features.codex_hooks=true"]
    );
}

#[test]
fn exec_resume_places_json_and_last_before_positionals() {
    let command = ExecResumeCommand {
        json: true,
        last: true,
        session_id: Some("abc123".to_owned()),
        prompt: Some("continue".to_owned()),
        ..ExecResumeCommand::default()
    };
    assert_eq!(
        argv(&command),
        ["exec", "resume", "--json", "--last", "abc123", "continue"]
    );
}

#[test]
fn root_run_forwards_prompt_and_options() {
    let command = RunCommand {
        options: RunOptions {
            ask_for_approval: Some(ApprovalPolicy::Never),
            search: true,
            ..RunOptions::default()
        },
        prompt: Some("hello".to_owned()),
        ..RunCommand::default()
    };
    assert_eq!(
        argv(&command),
        ["--ask-for-approval", "never", "--search", "hello"]
    );
}

#[test]
fn root_run_forwards_raw_args_before_prompt() {
    let command = RunCommand::prompt("hello").with_args(["--flag", "value"]);
    assert_eq!(argv(&command), ["--flag", "value", "hello"]);
}

#[test]
fn review_renders_flags_and_prompt() {
    let command = ReviewCommand {
        uncommitted: true,
        base: Some("main".to_owned()),
        ..ReviewCommand::default()
    };
    assert_eq!(
        argv(&command),
        ["review", "--uncommitted", "--base", "main"]
    );
}

#[test]
fn resume_renders_last_flag_and_positionals() {
    let command = ResumeCommand {
        last: true,
        session_id: Some("sess".to_owned()),
        ..ResumeCommand::default()
    };
    assert_eq!(argv(&command), ["resume", "--last", "sess"]);
}

#[test]
fn archive_emits_required_session_positional_and_no_all_flag() {
    let rendered = argv(&ArchiveCommand {
        session: "sess-1".to_owned(),
        ..ArchiveCommand::default()
    });
    assert_eq!(rendered, ["archive", "sess-1"]);
    assert!(
        !rendered.iter().any(|arg| arg == "--all"),
        "archive must not emit the fabricated --all flag"
    );
}

#[test]
fn archive_emits_remote_options_before_session() {
    let command = ArchiveCommand {
        remote: Some("ws://host:1234".to_owned()),
        remote_auth_token_env: Some("CODEX_TOKEN".to_owned()),
        session: "my-session".to_owned(),
        ..ArchiveCommand::default()
    };
    assert_eq!(
        argv(&command),
        [
            "archive",
            "--remote",
            "ws://host:1234",
            "--remote-auth-token-env",
            "CODEX_TOKEN",
            "my-session",
        ]
    );
}

#[test]
fn unarchive_emits_required_session_positional_and_no_all_flag() {
    let rendered = argv(&UnarchiveCommand {
        session: "sess-2".to_owned(),
        ..UnarchiveCommand::default()
    });
    assert_eq!(rendered, ["unarchive", "sess-2"]);
    assert!(!rendered.iter().any(|arg| arg == "--all"));
}

#[test]
fn root_run_remote_takes_a_value() {
    let command = RunCommand {
        options: RunOptions {
            remote: Some("ws://localhost:8080".to_owned()),
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(argv(&command), ["--remote", "ws://localhost:8080"]);
}

#[test]
fn resume_remote_takes_a_value() {
    let command = ResumeCommand {
        options: RunOptions {
            remote: Some("wss://host:9".to_owned()),
            ..RunOptions::default()
        },
        last: true,
        ..ResumeCommand::default()
    };
    assert_eq!(
        argv(&command),
        ["resume", "--remote", "wss://host:9", "--last"]
    );
}

#[test]
fn fork_remote_takes_a_value() {
    let command = ForkCommand {
        options: RunOptions {
            remote: Some("unix://".to_owned()),
            ..RunOptions::default()
        },
        ..ForkCommand::default()
    };
    assert_eq!(argv(&command), ["fork", "--remote", "unix://"]);
}

#[test]
fn review_emits_only_config_family_and_selectors() {
    let command = ReviewCommand {
        global: GlobalConfig {
            config: vec![ConfigOverride::new("model", "o3")],
            enable: vec!["codex_hooks".to_owned()],
            ..GlobalConfig::default()
        },
        strict_config: true,
        uncommitted: true,
        base: Some("main".to_owned()),
        commit: Some("abc123".to_owned()),
        title: Some("My review".to_owned()),
        prompt: Some("look here".to_owned()),
    };
    assert_eq!(
        argv(&command),
        [
            "review",
            "--config",
            "model=o3",
            "--enable",
            "codex_hooks",
            "--strict-config",
            "--uncommitted",
            "--base",
            "main",
            "--commit",
            "abc123",
            "--title",
            "My review",
            "look here",
        ]
    );
}

#[test]
fn exec_resume_narrowed_omits_run_family_flags() {
    let command = ExecResumeCommand {
        options: ExecSubcommandOptions {
            strict_config: true,
            model: Some("gpt-5-codex".to_owned()),
            output_last_message: Some(PathBuf::from("/tmp/last.txt")),
            ..ExecSubcommandOptions::default()
        },
        images: vec![PathBuf::from("a.png")],
        last: true,
        all: true,
        json: true,
        session_id: Some("sid".to_owned()),
        prompt: Some("go".to_owned()),
    };
    assert_eq!(
        argv(&command),
        [
            "exec",
            "resume",
            "--json",
            "--last",
            "--all",
            "--image",
            "a.png",
            "--strict-config",
            "--model",
            "gpt-5-codex",
            "--output-last-message",
            "/tmp/last.txt",
            "sid",
            "go",
        ]
    );
}

#[test]
fn exec_review_emits_selectors_and_narrowed_options() {
    let command = ExecReviewCommand {
        options: ExecSubcommandOptions {
            model: Some("gpt-5-codex".to_owned()),
            ..ExecSubcommandOptions::default()
        },
        json: true,
        uncommitted: true,
        base: Some("main".to_owned()),
        commit: Some("deadbeef".to_owned()),
        title: Some("t".to_owned()),
        prompt: Some("p".to_owned()),
    };
    assert_eq!(
        argv(&command),
        [
            "exec",
            "review",
            "--json",
            "--uncommitted",
            "--base",
            "main",
            "--commit",
            "deadbeef",
            "--title",
            "t",
            "--model",
            "gpt-5-codex",
            "p",
        ]
    );
}

#[test]
fn login_status_appends_status_subcommand() {
    assert_eq!(argv(&LoginCommand::status()), ["login", "status"]);
}

#[test]
fn logout_is_bare() {
    assert_eq!(argv(&LogoutCommand::default()), ["logout"]);
}

#[test]
fn features_enable_names_the_feature() {
    let command = FeaturesCommand::new(FeaturesSubcommand::Enable {
        name: "codex_hooks".to_owned(),
    });
    assert_eq!(argv(&command), ["features", "enable", "codex_hooks"]);
}

#[test]
fn mcp_add_stdio_renders_name_then_env_then_double_dash_command() {
    let command = McpCommand::new(McpSubcommand::Add {
        name: "fs".to_owned(),
        transport: McpTransport::Stdio {
            command: "npx".to_owned(),
            args: vec!["server".to_owned()],
            env: vec![
                ("A".to_owned(), "1".to_owned()),
                ("B".to_owned(), "2".to_owned()),
            ],
        },
    });
    assert_eq!(
        argv(&command),
        [
            "mcp", "add", "fs", "--env", "A=1", "--env", "B=2", "--", "npx", "server"
        ]
    );
}

#[test]
fn mcp_add_stdio_without_env_still_emits_nonempty_command_group() {
    // The stdio transport guarantees a non-empty `-- COMMAND` group: `command`
    // is a required String, so the required-group parse error is unrepresentable.
    let command = McpCommand::new(McpSubcommand::Add {
        name: "fs".to_owned(),
        transport: McpTransport::Stdio {
            command: "my-server".to_owned(),
            args: Vec::new(),
            env: Vec::new(),
        },
    });
    assert_eq!(argv(&command), ["mcp", "add", "fs", "--", "my-server"]);
}

#[test]
fn mcp_add_stdio_env_pair_renders_key_equals_value() {
    // `--env` is a typed KEY=VALUE pair, so a value with no `=` (which codex
    // rejects) is unrepresentable, and an `=` inside the value renders verbatim.
    let command = McpCommand::new(McpSubcommand::Add {
        name: "fs".to_owned(),
        transport: McpTransport::Stdio {
            command: "srv".to_owned(),
            args: Vec::new(),
            env: vec![("PATH".to_owned(), "/a=b".to_owned())],
        },
    });
    assert_eq!(
        argv(&command),
        ["mcp", "add", "fs", "--env", "PATH=/a=b", "--", "srv"]
    );
}

#[test]
fn mcp_add_http_renders_url_and_oauth_flags() {
    let command = McpCommand::new(McpSubcommand::Add {
        name: "http".to_owned(),
        transport: McpTransport::Http {
            url: "https://example.com/mcp".to_owned(),
            bearer_token_env_var: Some("TOKEN_ENV".to_owned()),
            oauth_client_id: Some("client-1".to_owned()),
            oauth_resource: Some("res-1".to_owned()),
        },
    });
    assert_eq!(
        argv(&command),
        [
            "mcp",
            "add",
            "http",
            "--url",
            "https://example.com/mcp",
            "--bearer-token-env-var",
            "TOKEN_ENV",
            "--oauth-client-id",
            "client-1",
            "--oauth-resource",
            "res-1",
        ]
    );
}

#[test]
fn mcp_add_http_minimal_emits_only_url() {
    // The HTTP transport carries no `--env` and no trailing command: neither the
    // both-set group conflict nor a stray stdio-only flag is representable.
    let command = McpCommand::new(McpSubcommand::Add {
        name: "http".to_owned(),
        transport: McpTransport::Http {
            url: "https://h/mcp".to_owned(),
            bearer_token_env_var: None,
            oauth_client_id: None,
            oauth_resource: None,
        },
    });
    assert_eq!(
        argv(&command),
        ["mcp", "add", "http", "--url", "https://h/mcp"]
    );
}

#[test]
fn mcp_list_and_get_render_json_flag() {
    assert_eq!(
        argv(&McpCommand::new(McpSubcommand::List { json: true })),
        ["mcp", "list", "--json"]
    );
    assert_eq!(
        argv(&McpCommand::new(McpSubcommand::Get {
            name: "fs".to_owned(),
            json: true,
        })),
        ["mcp", "get", "--json", "fs"]
    );
}

#[test]
fn mcp_login_renders_scopes_before_name() {
    let command = McpCommand::new(McpSubcommand::Login {
        name: "fs".to_owned(),
        scopes: Some("read,write".to_owned()),
    });
    assert_eq!(
        argv(&command),
        ["mcp", "login", "--scopes", "read,write", "fs"]
    );
}

#[test]
fn plugin_add_renders_marketplace_and_json_before_selector() {
    let command = PluginCommand::new(PluginSubcommand::Add {
        plugin: "sample".to_owned(),
        marketplace: Some("debug".to_owned()),
        json: true,
    });
    assert_eq!(
        argv(&command),
        [
            "plugin",
            "add",
            "--marketplace",
            "debug",
            "--json",
            "sample"
        ]
    );
}

#[test]
fn plugin_list_renders_available_flag() {
    let command = PluginCommand::new(PluginSubcommand::List {
        marketplace: None,
        json: true,
        available: true,
    });
    assert_eq!(argv(&command), ["plugin", "list", "--json", "--available"]);
}

#[test]
fn plugin_marketplace_add_renders_ref_and_sparse() {
    let command = PluginCommand::new(PluginSubcommand::Marketplace(
        PluginMarketplaceSubcommand::Add {
            source: "owner/repo".to_owned(),
            git_ref: Some("main".to_owned()),
            sparse: vec!["plugins/foo".to_owned()],
            json: true,
        },
    ));
    assert_eq!(
        argv(&command),
        [
            "plugin",
            "marketplace",
            "add",
            "--ref",
            "main",
            "--sparse",
            "plugins/foo",
            "--json",
            "owner/repo",
        ]
    );
}

#[test]
fn plugin_marketplace_upgrade_all_omits_positional() {
    let command = PluginCommand::new(PluginSubcommand::Marketplace(
        PluginMarketplaceSubcommand::Upgrade {
            name: None,
            json: false,
        },
    ));
    assert_eq!(argv(&command), ["plugin", "marketplace", "upgrade"]);
}

#[test]
fn remote_control_types_start_subcommand() {
    let command = RemoteControlCommand {
        json: true,
        command: Some(RemoteControlSubcommand::Start),
        ..RemoteControlCommand::default()
    };
    assert_eq!(argv(&command), ["remote-control", "--json", "start"]);
}

#[test]
fn sandbox_renders_added_flags_and_double_dash_command() {
    let command = SandboxCommand {
        allow_unix_sockets: vec!["/tmp/s.sock".to_owned()],
        log_denials: true,
        command: vec!["ls".to_owned()],
        ..SandboxCommand::default()
    };
    assert_eq!(
        argv(&command),
        [
            "sandbox",
            "--allow-unix-socket",
            "/tmp/s.sock",
            "--log-denials",
            "--",
            "ls"
        ]
    );
}

#[test]
fn doctor_renders_ascii_flag() {
    let command = DoctorCommand {
        ascii: true,
        ..DoctorCommand::default()
    };
    assert_eq!(argv(&command), ["doctor", "--ascii"]);
}

#[test]
fn doctor_renders_report_flags() {
    let command = DoctorCommand {
        json: true,
        summary: true,
        ..DoctorCommand::default()
    };
    assert_eq!(argv(&command), ["doctor", "--json", "--summary"]);
}

#[test]
fn completion_shell_is_a_bare_positional() {
    let command = CompletionCommand {
        shell: Some(CompletionShell::Zsh),
        ..CompletionCommand::default()
    };
    assert_eq!(argv(&command), ["completion", "zsh"]);
}

#[test]
fn completion_without_shell_omits_positional() {
    assert_eq!(argv(&CompletionCommand::default()), ["completion"]);
}

#[test]
fn sandbox_command_appends_double_dash_before_command() {
    let command = SandboxCommand {
        command: vec!["ls".to_owned(), "-la".to_owned()],
        ..SandboxCommand::default()
    };
    assert_eq!(argv(&command), ["sandbox", "--", "ls", "-la"]);
}

#[test]
fn local_provider_renders_as_flag_value() {
    let command = ExecCommand {
        common: CommonConfig {
            oss: true,
            local_provider: Some(LocalProvider::Ollama),
            ..CommonConfig::default()
        },
        ..ExecCommand::default()
    };
    assert_eq!(
        argv(&command),
        ["exec", "--oss", "--local-provider", "ollama"]
    );
}

// ----- executor context ------------------------------------------------------

#[test]
fn default_executor_uses_codex() {
    assert_eq!(Codex::default().executable(), OsString::from("codex"));
}

#[test]
fn to_process_carries_cwd_and_env() {
    let codex = Codex::default()
        .with_current_dir("/tmp/work")
        .with_env("CODEX_HOME", "/tmp/home");
    let command = codex.to_process(&ExecCommand::prompt("hi").json());
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
        [("CODEX_HOME".to_owned(), Some("/tmp/home".to_owned()))]
    );
}

#[test]
fn with_env_replaces_existing_key() {
    let codex = Codex::default().with_env("K", "one").with_env("K", "two");
    let envs: Vec<_> = codex
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

const FLAT_STREAM: &str = r#"{"type":"thread.started","thread_id":"th_123"}
{"type":"turn.started"}
{"type":"item.completed","item":{"type":"agent_message","text":"all done"}}
{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":5}}"#;

#[test]
fn parse_collects_object_events_only() {
    let parsed = ExecOutput::parse(FLAT_STREAM);
    assert_eq!(parsed.events().len(), 4);
    assert_eq!(parsed.events()[0].event_type(), EventType::ThreadStarted);
}

#[test]
fn parse_skips_non_json_and_non_object_lines() {
    let stream = "Loading model...\n42\n[1,2,3]\n{\"type\":\"turn.completed\"}\n";
    let parsed = ExecOutput::parse(stream);
    assert_eq!(parsed.events().len(), 1);
    assert_eq!(parsed.events()[0].event_type(), EventType::TurnCompleted);
}

#[test]
fn flat_stream_reports_session_message_status_and_tokens() {
    let parsed = ExecOutput::parse(FLAT_STREAM);
    assert_eq!(parsed.session_id().as_deref(), Some("th_123"));
    assert_eq!(parsed.final_message().as_deref(), Some("all done"));
    assert_eq!(parsed.terminal_status(), Some(TurnStatus::Completed));
    assert_eq!(parsed.total_tokens(), Some(15));
}

#[test]
fn turn_failed_is_not_completed() {
    let stream = r#"{"type":"turn.failed","error":{"message":"boom"}}"#;
    let parsed = ExecOutput::parse(stream);
    let outcome = parsed.turn_outcome().expect("a terminal record");
    assert_eq!(outcome.status, TurnStatus::Failed);
    assert!(!outcome.status.is_completed());
    assert_eq!(outcome.error_message.as_deref(), Some("boom"));
}

#[test]
fn error_event_marks_will_retry() {
    let stream = r#"{"type":"error","message":"rate limited","will_retry":true}"#;
    let parsed = ExecOutput::parse(stream);
    assert!(parsed.will_retry());
    let outcome = parsed.turn_outcome().expect("a terminal record");
    assert_eq!(outcome.status, TurnStatus::Failed);
    assert!(outcome.will_retry);
}

#[test]
fn legacy_msg_wrapped_shapes_are_understood() {
    let stream = r#"{"msg":{"type":"session_configured","session_id":"s_9"}}
{"msg":{"type":"agent_message","message":"legacy hi"}}
{"msg":{"type":"task_complete"}}"#;
    let parsed = ExecOutput::parse(stream);
    assert_eq!(parsed.session_id().as_deref(), Some("s_9"));
    assert_eq!(parsed.final_message().as_deref(), Some("legacy hi"));
    assert_eq!(parsed.terminal_status(), Some(TurnStatus::Completed));
}

#[test]
fn no_terminal_record_yields_no_outcome() {
    let parsed = ExecOutput::parse(r#"{"type":"turn.started"}"#);
    assert!(parsed.turn_outcome().is_none());
}

#[test]
fn try_from_succeeds_when_events_present_even_on_failure_exit() {
    let out = output(FLAT_STREAM, 1);
    let parsed = ExecOutput::try_from(out).expect("events present => Ok");
    assert_eq!(parsed.terminal_status(), Some(TurnStatus::Completed));
}

#[test]
fn try_from_errors_only_when_empty_and_failed() {
    let out = output("not json at all\n", 2);
    let result = ExecOutput::try_from(out);
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
    let out = output("", 0);
    let parsed = ExecOutput::try_from(out).expect("success => Ok even when empty");
    assert!(parsed.events().is_empty());
}

// ----- executor end-to-end (fake CLI) ---------------------------------------

#[cfg(unix)]
#[test]
fn execute_parses_json_stream_from_a_fake_codex() {
    let script = fake_codex(FLAT_STREAM, "", 0);
    let codex = Codex::new(script.path());
    let parsed = async_runtime()
        .block_on(codex.execute(&ExecCommand::prompt("hi").json()))
        .expect("execute succeeds");
    assert_eq!(parsed.session_id().as_deref(), Some("th_123"));
    assert_eq!(parsed.terminal_status(), Some(TurnStatus::Completed));
}

#[cfg(unix)]
#[test]
fn execute_reports_cli_error_on_empty_failure() {
    let script = fake_codex("not json", "boom", 2);
    let codex = Codex::new(script.path());
    let result = async_runtime().block_on(codex.execute(&ExecCommand::prompt("hi").json()));
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
    let script = fake_codex(FLAT_STREAM, "", 0);
    let codex = Codex::new(script.path());
    async_runtime().block_on(async {
        let events: Vec<_> = codex
            .stream(&ExecCommand::prompt("hi").json())
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
                EventType::ThreadStarted,
                EventType::TurnStarted,
                EventType::ItemCompleted,
                EventType::TurnCompleted,
            ]
        );
    });
}

#[cfg(unix)]
#[test]
fn stream_surfaces_failure_exit_after_events() {
    let script = fake_codex(r#"{"type":"turn.started"}"#, "bad exit", 3);
    let codex = Codex::new(script.path());
    async_runtime().block_on(async {
        let mut stream = codex
            .stream(&ExecCommand::prompt("hi").json())
            .expect("spawn stream");
        let first = stream.next().await.expect("one event").expect("event ok");
        assert_eq!(first.event_type(), EventType::TurnStarted);
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
