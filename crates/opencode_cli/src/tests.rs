//! Unit tests for argv construction and JSON event-stream parsing.
//!
//! None of these tests invoke opencode's agent; they exercise pure argv
//! builders and the stream parser against synthetic fixtures. The executor
//! end-to-end tests run a fake `opencode` shell script, never the real CLI.

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

use crate::agent::{AgentCommand, AgentSubcommand};
use crate::args::ToArgs;
use crate::auth::{AuthCommand, AuthSubcommand};
use crate::cli::{Error, Opencode};
use crate::db::{DbCommand, DbSubcommand};
use crate::debug::{
    DebugCommand, DebugFileSubcommand, DebugLspSubcommand, DebugRgSubcommand,
    DebugSnapshotSubcommand, DebugSubcommand,
};
use crate::github::{GithubCommand, GithubSubcommand};
use crate::mcp::{McpAuthSubcommand, McpCommand, McpSubcommand};
use crate::ops::{
    AttachCommand, CompletionCommand, ExportCommand, ImportCommand, ModelsCommand, PrCommand,
    StatsCommand, UninstallCommand, UpgradeCommand,
};
use crate::options::{GlobalOptions, ServerOptions};
use crate::output::{EventType, RunOutput};
use crate::run::{RunCommand, RunOptions, TuiCommand};
use crate::server::{AcpCommand, ServeCommand};
use crate::session::{SessionCommand, SessionSubcommand};
use crate::values::{
    AgentMode, Continuation, DbFormat, LogLevel, SessionFormat, StatsModels, UpgradeMethod,
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

/// Write an executable shell script that impersonates `opencode run --format
/// json` by emitting fixed stdout/stderr and exit code, so the executor can be
/// tested without invoking the real CLI.
#[cfg(unix)]
fn fake_opencode(stdout: &str, stderr: &str, code: i32) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("create fake opencode executable");
    write!(
        file,
        "#!/bin/sh\ncat <<'__OC_STDOUT__'\n{stdout}\n__OC_STDOUT__\ncat >&2 <<'__OC_STDERR__'\n{stderr}\n__OC_STDERR__\nexit {code}\n"
    )
    .expect("write fake opencode executable");
    let mut permissions = file
        .as_file()
        .metadata()
        .expect("fake metadata")
        .permissions();
    permissions.set_mode(0o700);
    file.as_file()
        .set_permissions(permissions)
        .expect("make fake opencode executable");
    file
}

// ----- argv construction -----------------------------------------------------

#[test]
fn run_message_renders_run_then_positional() {
    let command = RunCommand::message("do the thing");
    assert_eq!(argv(&command), ["run", "do the thing"]);
}

#[test]
fn run_json_inserts_format_json_before_positional() {
    let command = RunCommand::message("hi").json();
    assert_eq!(argv(&command), ["run", "--format", "json", "hi"]);
}

#[test]
fn run_renders_global_then_options_then_message_in_order() {
    let command = RunCommand {
        global: GlobalOptions {
            print_logs: true,
            log_level: Some(LogLevel::Debug),
        },
        options: RunOptions {
            continuation: Continuation::Continue { fork: false },
            model: Some("anthropic/claude".to_owned()),
            files: vec![PathBuf::from("notes.md")],
            ..RunOptions::default()
        },
        message: vec!["go".to_owned()],
    }
    .json();

    assert_eq!(
        argv(&command),
        [
            "run",
            "--print-logs",
            "--log-level",
            "DEBUG",
            "--continue",
            "--model",
            "anthropic/claude",
            "--format",
            "json",
            "--file",
            "notes.md",
            "go",
        ]
    );
}

#[test]
fn run_continuation_fresh_emits_no_selector() {
    // Fresh carries no `fork` field, so `--fork`-without-selector is
    // unrepresentable; Fresh renders none of the three selector flags.
    let command = RunCommand::message("go");
    let args = argv(&command);
    assert!(
        !args
            .iter()
            .any(|a| a == "--continue" || a == "--session" || a == "--fork"),
        "fresh continuation emits no selector: {args:?}",
    );
    assert_eq!(args, ["run", "go"]);
}

#[test]
fn run_continuation_continue_renders_fork_after_continue() {
    let command = RunCommand {
        options: RunOptions {
            continuation: Continuation::Continue { fork: true },
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(argv(&command), ["run", "--continue", "--fork"]);
}

#[test]
fn run_continuation_session_renders_id_then_fork() {
    let command = RunCommand {
        options: RunOptions {
            continuation: Continuation::Session {
                id: "ses_7".to_owned(),
                fork: true,
            },
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(argv(&command), ["run", "--session", "ses_7", "--fork"]);
}

#[test]
fn run_continuation_continue_session_keeps_both_selectors() {
    // LEAVE: opencode accepts `--continue --session <id>` together (it applies
    // the explicit id), so ContinueSession keeps that valid input representable.
    let command = RunCommand {
        options: RunOptions {
            continuation: Continuation::ContinueSession {
                id: "ses_9".to_owned(),
                fork: false,
            },
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(argv(&command), ["run", "--continue", "--session", "ses_9"]);
}

#[test]
fn tui_continuation_session_forks() {
    let command = TuiCommand {
        continuation: Continuation::Session {
            id: "ses_1".to_owned(),
            fork: true,
        },
        ..TuiCommand::default()
    };
    assert_eq!(argv(&command), ["--session", "ses_1", "--fork"]);
}

#[test]
fn attach_continuation_continue_session_forks() {
    let command = AttachCommand {
        continuation: Continuation::ContinueSession {
            id: "ses_2".to_owned(),
            fork: true,
        },
        url: "http://localhost:4096".to_owned(),
        ..AttachCommand::default()
    };
    assert_eq!(
        argv(&command),
        [
            "attach",
            "--continue",
            "--session",
            "ses_2",
            "--fork",
            "http://localhost:4096",
        ]
    );
}

#[test]
fn root_tui_has_no_subcommand_token() {
    let command = TuiCommand {
        model: Some("m".to_owned()),
        continuation: Continuation::Continue { fork: false },
        prompt: Some("seed".to_owned()),
        project: Some(PathBuf::from("/repo")),
        ..TuiCommand::default()
    };
    assert_eq!(
        argv(&command),
        ["--model", "m", "--continue", "--prompt", "seed", "/repo"]
    );
}

#[test]
fn serve_renders_bind_and_repeated_cors() {
    let command = ServeCommand {
        server: ServerOptions {
            port: Some(8080),
            cors: vec!["a.example".to_owned(), "b.example".to_owned()],
            ..ServerOptions::default()
        },
        ..ServeCommand::default()
    };
    assert_eq!(
        argv(&command),
        [
            "serve",
            "--port",
            "8080",
            "--cors",
            "a.example",
            "--cors",
            "b.example"
        ]
    );
}

#[test]
fn acp_appends_cwd_after_server_options() {
    let command = AcpCommand {
        cwd: Some("/work".to_owned()),
        ..AcpCommand::default()
    };
    assert_eq!(argv(&command), ["acp", "--cwd", "/work"]);
}

#[test]
fn session_delete_names_the_session() {
    let command = SessionCommand::new(SessionSubcommand::Delete {
        session_id: "ses_1".to_owned(),
    });
    assert_eq!(argv(&command), ["session", "delete", "ses_1"]);
}

#[test]
fn auth_login_forwards_optional_url() {
    let command = AuthCommand::new(AuthSubcommand::Login {
        url: Some("https://provider.example".to_owned()),
        provider: None,
        method: None,
    });
    assert_eq!(
        argv(&command),
        ["auth", "login", "https://provider.example"]
    );
}

#[test]
fn auth_login_renders_provider_and_method_before_url() {
    let command = AuthCommand::new(AuthSubcommand::Login {
        url: None,
        provider: Some("anthropic".to_owned()),
        method: Some("console".to_owned()),
    });
    assert_eq!(
        argv(&command),
        [
            "auth",
            "login",
            "--provider",
            "anthropic",
            "--method",
            "console"
        ]
    );
}

#[test]
fn agent_list_is_a_leaf() {
    let command = AgentCommand::new(AgentSubcommand::List);
    assert_eq!(argv(&command), ["agent", "list"]);
}

#[test]
fn agent_create_renders_flags_in_help_order() {
    let command = AgentCommand::new(AgentSubcommand::Create {
        path: Some(".opencode/agent".to_owned()),
        description: Some("does things".to_owned()),
        mode: Some(AgentMode::Subagent),
        tools: Some("bash,read".to_owned()),
        model: Some("anthropic/claude".to_owned()),
    });
    assert_eq!(
        argv(&command),
        [
            "agent",
            "create",
            "--path",
            ".opencode/agent",
            "--description",
            "does things",
            "--mode",
            "subagent",
            "--tools",
            "bash,read",
            "--model",
            "anthropic/claude",
        ]
    );
}

#[test]
fn agent_mode_choices_match_opencode() {
    assert_eq!(AgentMode::All.as_str(), "all");
    assert_eq!(AgentMode::Primary.as_str(), "primary");
    assert_eq!(AgentMode::Subagent.as_str(), "subagent");
}

#[test]
fn mcp_add_forwards_passthrough_args() {
    let command = McpCommand::new(McpSubcommand::Add {
        args: vec![
            "fs".to_owned(),
            "--".to_owned(),
            "npx".to_owned(),
            "server".to_owned(),
        ],
    });
    assert_eq!(argv(&command), ["mcp", "add", "fs", "--", "npx", "server"]);
}

#[test]
fn github_run_renders_event_and_token() {
    let command = GithubCommand::new(GithubSubcommand::Run {
        event: Some("issue_comment".to_owned()),
        token: Some("github_pat_123".to_owned()),
    });
    assert_eq!(
        argv(&command),
        [
            "github",
            "run",
            "--event",
            "issue_comment",
            "--token",
            "github_pat_123"
        ]
    );
}

#[test]
fn mcp_auth_authenticate_forwards_optional_name() {
    let command = McpCommand::new(McpSubcommand::Auth(McpAuthSubcommand::Authenticate {
        name: Some("github".to_owned()),
    }));
    assert_eq!(argv(&command), ["mcp", "auth", "github"]);
}

#[test]
fn mcp_auth_list_is_a_nested_leaf() {
    let command = McpCommand::new(McpSubcommand::Auth(McpAuthSubcommand::List));
    assert_eq!(argv(&command), ["mcp", "auth", "list"]);
}

#[test]
fn session_list_renders_max_count_and_format() {
    let command = SessionCommand::new(SessionSubcommand::List {
        max_count: Some(20),
        format: Some(SessionFormat::Json),
    });
    assert_eq!(
        argv(&command),
        ["session", "list", "--max-count", "20", "--format", "json"]
    );
}

#[test]
fn session_format_choices_match_opencode() {
    assert_eq!(SessionFormat::Table.as_str(), "table");
    assert_eq!(SessionFormat::Json.as_str(), "json");
}

#[test]
fn db_query_is_the_default_command_with_format() {
    let command = DbCommand::new(DbSubcommand::Query {
        query: Some("SELECT 1".to_owned()),
        format: Some(DbFormat::Json),
    });
    assert_eq!(argv(&command), ["db", "--format", "json", "SELECT 1"]);
}

#[test]
fn db_path_is_a_leaf() {
    let command = DbCommand::new(DbSubcommand::Path);
    assert_eq!(argv(&command), ["db", "path"]);
}

#[test]
fn debug_agent_names_the_agent() {
    let command = DebugCommand::new(DebugSubcommand::Agent {
        name: "reviewer".to_owned(),
        tool: None,
        params: None,
    });
    assert_eq!(argv(&command), ["debug", "agent", "reviewer"]);
}

#[test]
fn debug_agent_renders_tool_and_params_before_name() {
    let command = DebugCommand::new(DebugSubcommand::Agent {
        name: "reviewer".to_owned(),
        tool: Some("bash".to_owned()),
        params: Some("{\"command\":\"ls\"}".to_owned()),
    });
    assert_eq!(
        argv(&command),
        [
            "debug",
            "agent",
            "--tool",
            "bash",
            "--params",
            "{\"command\":\"ls\"}",
            "reviewer",
        ]
    );
}

#[test]
fn debug_lsp_diagnostics_names_the_file() {
    let command = DebugCommand::new(DebugSubcommand::Lsp(DebugLspSubcommand::Diagnostics {
        file: "src/main.rs".to_owned(),
    }));
    assert_eq!(
        argv(&command),
        ["debug", "lsp", "diagnostics", "src/main.rs"]
    );
}

#[test]
fn debug_lsp_document_symbols_uses_the_hyphenated_leaf() {
    let command = DebugCommand::new(DebugSubcommand::Lsp(DebugLspSubcommand::DocumentSymbols {
        uri: "file:///x".to_owned(),
    }));
    assert_eq!(
        argv(&command),
        ["debug", "lsp", "document-symbols", "file:///x"]
    );
}

#[test]
fn debug_rg_tree_renders_limit() {
    let command = DebugCommand::new(DebugSubcommand::Rg(DebugRgSubcommand::Tree {
        limit: Some(5),
    }));
    assert_eq!(argv(&command), ["debug", "rg", "tree", "--limit", "5"]);
}

#[test]
fn debug_rg_files_renders_query_and_single_glob() {
    let command = DebugCommand::new(DebugSubcommand::Rg(DebugRgSubcommand::Files {
        query: Some("main".to_owned()),
        glob: Some("*.rs".to_owned()),
        limit: None,
    }));
    assert_eq!(
        argv(&command),
        ["debug", "rg", "files", "--query", "main", "--glob", "*.rs"]
    );
}

#[test]
fn debug_rg_search_renders_repeated_glob_then_limit_then_pattern() {
    let command = DebugCommand::new(DebugSubcommand::Rg(DebugRgSubcommand::Search {
        pattern: "TODO".to_owned(),
        glob: vec!["*.rs".to_owned(), "*.toml".to_owned()],
        limit: Some(10),
    }));
    assert_eq!(
        argv(&command),
        [
            "debug", "rg", "search", "--glob", "*.rs", "--glob", "*.toml", "--limit", "10", "TODO",
        ]
    );
}

#[test]
fn debug_file_read_names_the_path() {
    let command = DebugCommand::new(DebugSubcommand::File(DebugFileSubcommand::Read {
        path: "a.txt".to_owned(),
    }));
    assert_eq!(argv(&command), ["debug", "file", "read", "a.txt"]);
}

#[test]
fn debug_file_tree_omits_optional_dir() {
    let command = DebugCommand::new(DebugSubcommand::File(DebugFileSubcommand::Tree {
        dir: None,
    }));
    assert_eq!(argv(&command), ["debug", "file", "tree"]);
}

#[test]
fn debug_snapshot_diff_names_the_hash() {
    let command = DebugCommand::new(DebugSubcommand::Snapshot(DebugSnapshotSubcommand::Diff {
        hash: "abc".to_owned(),
    }));
    assert_eq!(argv(&command), ["debug", "snapshot", "diff", "abc"]);
}

#[test]
fn completion_is_bare() {
    assert_eq!(argv(&CompletionCommand::default()), ["completion"]);
}

#[test]
fn attach_places_url_after_flags() {
    let command = AttachCommand {
        continuation: Continuation::Continue { fork: false },
        url: "http://localhost:4096".to_owned(),
        ..AttachCommand::default()
    };
    assert_eq!(
        argv(&command),
        ["attach", "--continue", "http://localhost:4096"]
    );
}

#[test]
fn upgrade_renders_method_then_target() {
    let command = UpgradeCommand {
        method: Some(UpgradeMethod::Npm),
        target: Some("0.1.48".to_owned()),
        ..UpgradeCommand::default()
    };
    assert_eq!(argv(&command), ["upgrade", "--method", "npm", "0.1.48"]);
}

#[test]
fn uninstall_renders_flags() {
    let command = UninstallCommand {
        dry_run: true,
        force: true,
        ..UninstallCommand::default()
    };
    assert_eq!(argv(&command), ["uninstall", "--dry-run", "--force"]);
}

#[test]
fn models_renders_verbose_then_provider() {
    let command = ModelsCommand {
        verbose: true,
        provider: Some("anthropic".to_owned()),
        ..ModelsCommand::default()
    };
    assert_eq!(argv(&command), ["models", "--verbose", "anthropic"]);
}

#[test]
fn stats_bare_models_is_value_less() {
    let command = StatsCommand {
        days: Some(7),
        models: Some(StatsModels::All),
        ..StatsCommand::default()
    };
    assert_eq!(argv(&command), ["stats", "--days", "7", "--models"]);
}

#[test]
fn stats_top_models_carries_the_count() {
    let command = StatsCommand {
        models: Some(StatsModels::Top(5)),
        ..StatsCommand::default()
    };
    assert_eq!(argv(&command), ["stats", "--models", "5"]);
}

#[test]
fn export_session_id_is_a_bare_positional() {
    let command = ExportCommand {
        session_id: Some("ses_9".to_owned()),
        ..ExportCommand::default()
    };
    assert_eq!(argv(&command), ["export", "ses_9"]);
}

#[test]
fn import_file_is_a_bare_positional() {
    let command = ImportCommand {
        file: "dump.json".to_owned(),
        ..ImportCommand::default()
    };
    assert_eq!(argv(&command), ["import", "dump.json"]);
}

#[test]
fn pr_renders_the_number_positional() {
    let command = PrCommand {
        number: 42,
        ..PrCommand::default()
    };
    assert_eq!(argv(&command), ["pr", "42"]);
}

// ----- executor context ------------------------------------------------------

#[test]
fn default_executor_uses_opencode() {
    assert_eq!(Opencode::default().executable(), OsString::from("opencode"));
}

#[test]
fn to_process_carries_cwd_and_env() {
    let opencode = Opencode::default()
        .with_current_dir("/tmp/work")
        .with_env("OPENCODE_CONFIG", "/tmp/config");
    let command = opencode.to_process(&RunCommand::message("hi").json());
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
        [("OPENCODE_CONFIG".to_owned(), Some("/tmp/config".to_owned()))]
    );
}

#[test]
fn with_env_replaces_existing_key() {
    let opencode = Opencode::default()
        .with_env("K", "one")
        .with_env("K", "two");
    let envs: Vec<_> = opencode
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

const JSONL_STREAM: &str = r#"{"type":"session","id":"ses_123"}
{"type":"result","text":"all done"}"#;

const SINGLE_OBJECT: &str = r#"{"type":"session","id":"ses_9","text":"whole-stream form"}"#;

#[test]
fn parse_accepts_the_json_lines_form() {
    let parsed = RunOutput::parse(JSONL_STREAM);
    assert_eq!(parsed.events().len(), 2);
    assert_eq!(parsed.events()[0].event_type(), EventType::Session);
    assert_eq!(parsed.events()[1].event_type(), EventType::Result);
}

#[test]
fn parse_accepts_the_single_object_form() {
    let parsed = RunOutput::parse(SINGLE_OBJECT);
    assert_eq!(parsed.events().len(), 1);
    assert_eq!(parsed.session_id().as_deref(), Some("ses_9"));
    assert_eq!(parsed.final_message().as_deref(), Some("whole-stream form"));
}

#[test]
fn parse_skips_non_json_and_non_object_lines() {
    let stream = "Loading...\n42\n[1,2,3]\n{\"type\":\"result\",\"text\":\"ok\"}\n";
    let parsed = RunOutput::parse(stream);
    assert_eq!(parsed.events().len(), 1);
    assert_eq!(parsed.events()[0].event_type(), EventType::Result);
}

#[test]
fn json_lines_reports_session_and_final_message() {
    let parsed = RunOutput::parse(JSONL_STREAM);
    assert_eq!(parsed.session_id().as_deref(), Some("ses_123"));
    assert_eq!(parsed.final_message().as_deref(), Some("all done"));
    assert!(!parsed.is_error());
    assert!(parsed.error().is_none());
}

#[test]
fn session_error_is_the_authoritative_failure_signal() {
    let stream = r#"{"type":"session","id":"ses_1"}
{"type":"session.error","error":{"message":"context length exceeded"}}"#;
    let parsed = RunOutput::parse(stream);
    assert!(parsed.is_error());
    let error = parsed.error().expect("an error event");
    assert_eq!(error.message(), "context length exceeded");
    assert_eq!(parsed.events()[1].event_type(), EventType::SessionError);
}

#[test]
fn result_error_is_recognized_on_the_single_object_form() {
    let stream = r#"{"type":"result.error","error":{"message":"invalid flag"}}"#;
    let parsed = RunOutput::parse(stream);
    assert!(parsed.is_error());
    assert_eq!(
        parsed.error().expect("an error event").into_message(),
        "invalid flag"
    );
}

#[test]
fn unknown_event_types_fall_through_to_other() {
    let parsed = RunOutput::parse(r#"{"type":"tool.invoked","name":"grep"}"#);
    assert_eq!(
        parsed.events()[0].event_type(),
        EventType::Other(Some("tool.invoked".to_owned()))
    );
}

#[test]
fn try_from_succeeds_when_events_present_even_on_failure_exit() {
    // opencode's exit code lies: a non-empty stream is authoritative even when
    // the process exits non-zero.
    let out = output(JSONL_STREAM, 1);
    let parsed = RunOutput::try_from(out).expect("events present => Ok");
    assert_eq!(parsed.session_id().as_deref(), Some("ses_123"));
}

#[test]
fn try_from_errors_only_when_empty_and_failed() {
    let out = output("not json at all\n", 2);
    let result = RunOutput::try_from(out);
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
    let parsed = RunOutput::try_from(out).expect("success => Ok even when empty");
    assert!(parsed.events().is_empty());
}

// ----- executor end-to-end (fake CLI) ---------------------------------------

#[cfg(unix)]
#[test]
fn execute_parses_json_stream_from_a_fake_opencode() {
    let script = fake_opencode(JSONL_STREAM, "", 0);
    let opencode = Opencode::new(script.path());
    let parsed = async_runtime()
        .block_on(opencode.execute(&RunCommand::message("hi").json()))
        .expect("execute succeeds");
    assert_eq!(parsed.session_id().as_deref(), Some("ses_123"));
    assert_eq!(parsed.final_message().as_deref(), Some("all done"));
}

#[cfg(unix)]
#[test]
fn execute_reports_cli_error_on_empty_failure() {
    let script = fake_opencode("not json", "boom", 1);
    let opencode = Opencode::new(script.path());
    let result = async_runtime().block_on(opencode.execute(&RunCommand::message("hi").json()));
    assert!(
        matches!(
            result,
            Err(Error::Cli {
                exit_code: Some(1),
                ..
            })
        ),
        "expected Error::Cli, got {result:?}"
    );
}

#[cfg(unix)]
#[test]
fn stream_yields_events_then_verifies_exit() {
    let script = fake_opencode(JSONL_STREAM, "", 0);
    let opencode = Opencode::new(script.path());
    async_runtime().block_on(async {
        let events: Vec<_> = opencode
            .stream(&RunCommand::message("hi").json())
            .expect("spawn stream")
            .collect()
            .await;
        let types: Vec<_> = events
            .into_iter()
            .map(|event| event.expect("event ok").event_type())
            .collect();
        assert_eq!(types, [EventType::Session, EventType::Result]);
    });
}

#[cfg(unix)]
#[test]
fn stream_surfaces_failure_exit_after_events() {
    let script = fake_opencode(r#"{"type":"session","id":"x"}"#, "bad exit", 3);
    let opencode = Opencode::new(script.path());
    async_runtime().block_on(async {
        let mut stream = opencode
            .stream(&RunCommand::message("hi").json())
            .expect("spawn stream");
        let first = stream.next().await.expect("one event").expect("event ok");
        assert_eq!(first.event_type(), EventType::Session);
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
