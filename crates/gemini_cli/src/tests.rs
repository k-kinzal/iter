//! Unit tests for argv construction and JSON / stream-json output parsing.
//!
//! None of these tests invoke the real Gemini CLI; they exercise pure argv
//! builders and the output parsers against synthetic fixtures, plus a fake
//! shell-script executable for the executor end-to-end paths.

use std::ffi::OsString;
#[cfg(unix)]
use std::io::Write as _;
use std::os::unix::process::ExitStatusExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::{ExitStatus, Output as ProcessOutput};

use futures::StreamExt as _;
use pretty_assertions::assert_eq;

use crate::args::ToArgs;
use crate::cli::{Error, Gemini};
use crate::extensions::{ExtensionsCommand, ExtensionsSubcommand};
use crate::gemma::{GemmaCommand, GemmaSubcommand};
use crate::hooks::{HooksCommand, HooksSubcommand};
use crate::mcp::{McpCommand, McpSubcommand};
use crate::output::{GeminiOutput, StreamEventType, StreamOutput};
use crate::run::{RunCommand, RunOptions};
use crate::skills::{SkillsCommand, SkillsSubcommand};
use crate::values::{
    ApprovalMode, ExtensionTemplate, ExtensionsOutputFormat, McpScope, McpTransport, Scope,
    SessionRef, Worktree,
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

/// Write an executable shell script that impersonates `gemini` by emitting
/// fixed stdout/stderr and exit code, so the executor can be tested without
/// invoking the real CLI.
#[cfg(unix)]
fn fake_gemini(stdout: &str, stderr: &str, code: i32) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("create fake gemini executable");
    write!(
        file,
        "#!/bin/sh\ncat <<'__GEMINI_STDOUT__'\n{stdout}\n__GEMINI_STDOUT__\ncat >&2 <<'__GEMINI_STDERR__'\n{stderr}\n__GEMINI_STDERR__\nexit {code}\n"
    )
    .expect("write fake gemini executable");
    let mut permissions = file
        .as_file()
        .metadata()
        .expect("fake metadata")
        .permissions();
    permissions.set_mode(0o700);
    file.as_file()
        .set_permissions(permissions)
        .expect("make fake gemini executable");
    file
}

// ----- argv construction -----------------------------------------------------

#[test]
fn prompt_json_renders_output_format_then_prompt() {
    let command = RunCommand::prompt("do the thing").json();
    assert_eq!(
        argv(&command),
        ["--output-format", "json", "--prompt", "do the thing"]
    );
}

#[test]
fn prompt_without_format_omits_the_selector() {
    let command = RunCommand::prompt("hi");
    assert_eq!(argv(&command), ["--prompt", "hi"]);
}

#[test]
fn query_is_a_bare_trailing_positional() {
    let command = RunCommand::query("explain this repo");
    assert_eq!(argv(&command), ["explain this repo"]);
}

#[test]
fn stream_json_selects_the_stream_format() {
    let command = RunCommand::prompt("go").stream_json();
    assert_eq!(
        argv(&command),
        ["--output-format", "stream-json", "--prompt", "go"]
    );
}

#[test]
fn options_render_before_format_and_prompt_in_stable_order() {
    let command = RunCommand {
        options: RunOptions {
            model: Some("gemini-2.5-pro".to_owned()),
            yolo: true,
            approval_mode: Some(ApprovalMode::AutoEdit),
            include_directories: vec![PathBuf::from("/src")],
            ..RunOptions::default()
        },
        prompt: Some("go".to_owned()),
        ..RunCommand::default()
    }
    .json();

    assert_eq!(
        argv(&command),
        [
            "--model",
            "gemini-2.5-pro",
            "--yolo",
            "--approval-mode",
            "auto_edit",
            "--include-directories",
            "/src",
            "--output-format",
            "json",
            "--prompt",
            "go",
        ]
    );
}

#[test]
fn worktree_auto_is_a_bare_flag() {
    let command = RunCommand {
        options: RunOptions {
            worktree: Some(Worktree::Auto),
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(argv(&command), ["--worktree"]);
}

#[test]
fn worktree_named_carries_its_value() {
    let command = RunCommand {
        options: RunOptions {
            worktree: Some(Worktree::named("spike")),
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(argv(&command), ["--worktree", "spike"]);
}

#[test]
fn repeatable_flags_render_once_per_value() {
    let command = RunCommand {
        options: RunOptions {
            extensions: vec!["a".to_owned(), "b".to_owned()],
            allowed_mcp_server_names: vec!["fs".to_owned()],
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(
        argv(&command),
        [
            "--allowed-mcp-server-names",
            "fs",
            "--extensions",
            "a",
            "--extensions",
            "b",
        ]
    );
}

#[test]
fn resume_latest_renders_the_latest_selector() {
    let command = RunCommand {
        options: RunOptions {
            resume: Some(SessionRef::Latest),
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(argv(&command), ["--resume", "latest"]);
}

#[test]
fn resume_index_renders_the_bare_number() {
    let command = RunCommand {
        options: RunOptions {
            resume: Some(SessionRef::Index(5)),
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(argv(&command), ["--resume", "5"]);
    assert_eq!(SessionRef::Latest.as_arg(), "latest");
    assert_eq!(SessionRef::Index(5).as_arg(), "5");
}

#[test]
fn delete_session_renders_a_numeric_index() {
    let command = RunCommand {
        options: RunOptions {
            delete_session: Some(3),
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(argv(&command), ["--delete-session", "3"]);
}

fn mcp_add_bare(name: &str, command_or_url: &str, args: Vec<String>) -> McpSubcommand {
    McpSubcommand::Add {
        name: name.to_owned(),
        command_or_url: command_or_url.to_owned(),
        scope: None,
        transport: None,
        env: Vec::new(),
        header: Vec::new(),
        timeout: None,
        trust: false,
        description: None,
        include_tools: Vec::new(),
        exclude_tools: Vec::new(),
        args,
    }
}

#[test]
fn mcp_add_forwards_name_url_and_passthrough_args() {
    let command = McpCommand::new(mcp_add_bare(
        "fs",
        "npx",
        vec!["-y".to_owned(), "server".to_owned()],
    ));
    assert_eq!(argv(&command), ["mcp", "add", "fs", "npx", "-y", "server"]);
}

#[test]
fn mcp_add_types_scope_transport_and_repeatable_flags() {
    let command = McpCommand::new(McpSubcommand::Add {
        name: "http-srv".to_owned(),
        command_or_url: "https://mcp.example/api".to_owned(),
        scope: Some(McpScope::User),
        transport: Some(McpTransport::Http),
        env: vec![("KEY".to_owned(), "value".to_owned())],
        header: vec![("Authorization".to_owned(), "Bearer abc".to_owned())],
        timeout: Some(5000),
        trust: true,
        description: Some("example server".to_owned()),
        include_tools: vec!["read".to_owned()],
        exclude_tools: vec!["write".to_owned()],
        args: Vec::new(),
    });
    assert_eq!(
        argv(&command),
        [
            "mcp",
            "add",
            "--scope",
            "user",
            "--transport",
            "http",
            "--env",
            "KEY=value",
            "--header",
            "Authorization: Bearer abc",
            "--timeout",
            "5000",
            "--trust",
            "--description",
            "example server",
            "--include-tools",
            "read",
            "--exclude-tools",
            "write",
            "http-srv",
            "https://mcp.example/api",
        ]
    );
}

#[test]
fn mcp_add_env_and_header_render_typed_separators() {
    let command = McpCommand::new(McpSubcommand::Add {
        name: "srv".to_owned(),
        command_or_url: "https://mcp.example".to_owned(),
        scope: None,
        transport: None,
        env: vec![
            ("API_KEY".to_owned(), "abc123".to_owned()),
            ("REGION".to_owned(), "us-east-1".to_owned()),
        ],
        header: vec![("X-Api-Key".to_owned(), "abc: 123".to_owned())],
        timeout: None,
        trust: false,
        description: None,
        include_tools: Vec::new(),
        exclude_tools: Vec::new(),
        args: Vec::new(),
    });
    // env joins with `=`; header joins with `: ` — a value containing its own
    // colon still renders unambiguously because key and value are typed apart.
    assert_eq!(
        argv(&command),
        [
            "mcp",
            "add",
            "--env",
            "API_KEY=abc123",
            "--env",
            "REGION=us-east-1",
            "--header",
            "X-Api-Key: abc: 123",
            "srv",
            "https://mcp.example",
        ]
    );
}

#[test]
fn mcp_scope_choices_render_as_documented() {
    assert_eq!(McpScope::User.as_str(), "user");
    assert_eq!(McpScope::Project.as_str(), "project");
}

#[test]
fn mcp_transport_choices_render_as_documented() {
    assert_eq!(McpTransport::Stdio.as_str(), "stdio");
    assert_eq!(McpTransport::Sse.as_str(), "sse");
    assert_eq!(McpTransport::Http.as_str(), "http");
}

#[test]
fn mcp_remove_types_scope_before_name() {
    let command = McpCommand::new(McpSubcommand::Remove {
        name: "fs".to_owned(),
        scope: Some(McpScope::Project),
    });
    assert_eq!(
        argv(&command),
        ["mcp", "remove", "--scope", "project", "fs"]
    );
}

#[test]
fn mcp_disable_types_session_after_name() {
    let command = McpCommand::new(McpSubcommand::Disable {
        name: "fs".to_owned(),
        session: true,
    });
    assert_eq!(argv(&command), ["mcp", "disable", "fs", "--session"]);
}

#[test]
fn mcp_enable_session_is_a_bare_flag() {
    let command = McpCommand::new(McpSubcommand::Enable {
        name: "fs".to_owned(),
        session: true,
    });
    assert_eq!(argv(&command), ["mcp", "enable", "fs", "--session"]);
}

#[test]
fn mcp_debug_precedes_the_leaf() {
    let command = McpCommand {
        debug: true,
        command: McpSubcommand::List,
    };
    assert_eq!(argv(&command), ["mcp", "--debug", "list"]);
}

#[test]
fn extensions_install_renders_source_then_flags() {
    let command = ExtensionsCommand::new(ExtensionsSubcommand::Install {
        source: "https://example/ext".to_owned(),
        git_ref: None,
        auto_update: true,
        pre_release: false,
        consent: false,
        skip_settings: false,
    });
    assert_eq!(
        argv(&command),
        ["extensions", "install", "https://example/ext", "--auto-update"]
    );
}

#[test]
fn extensions_install_types_ref_consent_and_skip_settings() {
    let command = ExtensionsCommand::new(ExtensionsSubcommand::Install {
        source: "acme/ext".to_owned(),
        git_ref: Some("v1.2.3".to_owned()),
        auto_update: false,
        pre_release: true,
        consent: true,
        skip_settings: true,
    });
    assert_eq!(
        argv(&command),
        [
            "extensions",
            "install",
            "acme/ext",
            "--ref",
            "v1.2.3",
            "--pre-release",
            "--consent",
            "--skip-settings",
        ]
    );
}

#[test]
fn extensions_uninstall_types_all_before_names() {
    let command = ExtensionsCommand::new(ExtensionsSubcommand::Uninstall {
        names: vec!["one".to_owned(), "two".to_owned()],
        all: true,
    });
    assert_eq!(
        argv(&command),
        ["extensions", "uninstall", "--all", "one", "two"]
    );
}

#[test]
fn extensions_list_types_output_format_choices() {
    let command = ExtensionsCommand::new(ExtensionsSubcommand::List {
        output_format: Some(ExtensionsOutputFormat::Json),
    });
    assert_eq!(
        argv(&command),
        ["extensions", "list", "--output-format", "json"]
    );
    assert_eq!(ExtensionsOutputFormat::Text.as_str(), "text");
    assert_eq!(ExtensionsOutputFormat::Json.as_str(), "json");
}

#[test]
fn extensions_link_types_consent() {
    let command = ExtensionsCommand::new(ExtensionsSubcommand::Link {
        path: PathBuf::from("/opt/ext"),
        consent: true,
    });
    assert_eq!(
        argv(&command),
        ["extensions", "link", "/opt/ext", "--consent"]
    );
}

#[test]
fn extensions_new_types_template_positional() {
    let command = ExtensionsCommand::new(ExtensionsSubcommand::New {
        path: PathBuf::from("/opt/ext"),
        template: Some(ExtensionTemplate::McpServer),
    });
    assert_eq!(
        argv(&command),
        ["extensions", "new", "/opt/ext", "mcp-server"]
    );
    assert_eq!(ExtensionTemplate::CustomCommands.as_str(), "custom-commands");
    assert_eq!(ExtensionTemplate::ExcludeTools.as_str(), "exclude-tools");
    assert_eq!(ExtensionTemplate::Hooks.as_str(), "hooks");
    assert_eq!(ExtensionTemplate::Policies.as_str(), "policies");
    assert_eq!(ExtensionTemplate::Skills.as_str(), "skills");
    assert_eq!(ExtensionTemplate::ThemesExample.as_str(), "themes-example");
}

#[test]
fn extensions_config_types_scope_before_positionals() {
    let command = ExtensionsCommand::new(ExtensionsSubcommand::Config {
        name: Some("ext".to_owned()),
        setting: Some("key".to_owned()),
        scope: Some(Scope::Workspace),
    });
    assert_eq!(
        argv(&command),
        ["extensions", "config", "--scope", "workspace", "ext", "key"]
    );
}

#[test]
fn extensions_update_all_places_flag_before_optional_name() {
    let command = ExtensionsCommand::new(ExtensionsSubcommand::Update {
        name: None,
        all: true,
    });
    assert_eq!(argv(&command), ["extensions", "update", "--all"]);
}

#[test]
fn skills_install_renders_source_scope_path_and_consent() {
    let command = SkillsCommand::new(SkillsSubcommand::Install {
        source: "acme/skill".to_owned(),
        scope: Some(Scope::User),
        path: Some(PathBuf::from("/opt/skill")),
        consent: true,
    });
    assert_eq!(
        argv(&command),
        [
            "skills",
            "install",
            "acme/skill",
            "--scope",
            "user",
            "--path",
            "/opt/skill",
            "--consent",
        ]
    );
}

#[test]
fn skills_disable_types_scope_choices_after_name() {
    let command = SkillsCommand::new(SkillsSubcommand::Disable {
        name: "my-skill".to_owned(),
        scope: Some(Scope::Workspace),
    });
    assert_eq!(
        argv(&command),
        ["skills", "disable", "my-skill", "--scope", "workspace"]
    );
    assert_eq!(Scope::User.as_str(), "user");
    assert_eq!(Scope::Workspace.as_str(), "workspace");
}

#[test]
fn skills_link_types_scope_and_consent() {
    let command = SkillsCommand::new(SkillsSubcommand::Link {
        path: PathBuf::from("/opt/skill"),
        scope: Some(Scope::User),
        consent: true,
    });
    assert_eq!(
        argv(&command),
        ["skills", "link", "/opt/skill", "--scope", "user", "--consent"]
    );
}

#[test]
fn skills_uninstall_scope_is_typed() {
    let command = SkillsCommand::new(SkillsSubcommand::Uninstall {
        name: "my-skill".to_owned(),
        scope: Some(Scope::Workspace),
    });
    assert_eq!(
        argv(&command),
        ["skills", "uninstall", "my-skill", "--scope", "workspace"]
    );
}

#[test]
fn hooks_migrate_is_a_bare_leaf() {
    let command = HooksCommand::new(HooksSubcommand::Migrate { from_claude: false });
    assert_eq!(argv(&command), ["hooks", "migrate"]);
}

#[test]
fn hooks_migrate_types_from_claude() {
    let command = HooksCommand::new(HooksSubcommand::Migrate { from_claude: true });
    assert_eq!(argv(&command), ["hooks", "migrate", "--from-claude"]);
}

#[test]
fn gemma_status_types_port() {
    let command = GemmaCommand::new(GemmaSubcommand::Status { port: Some(9379) });
    assert_eq!(argv(&command), ["gemma", "status", "--port", "9379"]);
}

#[test]
fn gemma_status_without_port_is_a_bare_verb() {
    let command = GemmaCommand::new(GemmaSubcommand::Status { port: None });
    assert_eq!(argv(&command), ["gemma", "status"]);
}

#[test]
fn gemma_setup_types_full_flag_set() {
    let command = GemmaCommand::new(GemmaSubcommand::Setup {
        port: Some(1234),
        skip_model: true,
        start: true,
        force: true,
        consent: true,
    });
    assert_eq!(
        argv(&command),
        [
            "gemma",
            "setup",
            "--port",
            "1234",
            "--skip-model",
            "--start",
            "--force",
            "--consent",
        ]
    );
}

#[test]
fn gemma_logs_types_lines_and_follow() {
    let command = GemmaCommand::new(GemmaSubcommand::Logs {
        lines: Some(50),
        follow: true,
    });
    assert_eq!(
        argv(&command),
        ["gemma", "logs", "--lines", "50", "--follow"]
    );
}

#[test]
fn gemma_setup_start_false_emits_the_no_start_negation() {
    // Default-true `--start`: `false` must emit `--no-start` so the caller can
    // run setup without launching the LiteRT server.
    let command = GemmaCommand::new(GemmaSubcommand::Setup {
        port: None,
        skip_model: false,
        start: false,
        force: false,
        consent: false,
    });
    assert_eq!(argv(&command), ["gemma", "setup", "--no-start"]);
}

#[test]
fn gemma_logs_follow_false_emits_the_no_follow_negation() {
    // Default-true `--follow` (when `--lines` is omitted): `false` must emit
    // `--no-follow` so the tail-without-follow state is expressible.
    let command = GemmaCommand::new(GemmaSubcommand::Logs {
        lines: None,
        follow: false,
    });
    assert_eq!(argv(&command), ["gemma", "logs", "--no-follow"]);
}

// ----- executor context ------------------------------------------------------

#[test]
fn default_executor_uses_gemini() {
    assert_eq!(Gemini::default().executable(), OsString::from("gemini"));
}

#[test]
fn to_process_carries_cwd_and_env() {
    let gemini = Gemini::default()
        .with_current_dir("/tmp/work")
        .with_env("GEMINI_HOME", "/tmp/home");
    let command = gemini.to_process(&RunCommand::prompt("hi").json());
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
        [("GEMINI_HOME".to_owned(), Some("/tmp/home".to_owned()))]
    );
}

#[test]
fn with_env_replaces_existing_key() {
    let gemini = Gemini::default().with_env("K", "one").with_env("K", "two");
    let envs: Vec<_> = gemini
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
    let mut gemini = Gemini::default().with_env("A", "1").with_env("B", "2");
    gemini.remove_env("A");
    let keys: Vec<_> = gemini
        .envs()
        .map(|(key, _)| key.to_string_lossy().into_owned())
        .collect();
    assert_eq!(keys, ["B".to_owned()]);
}

// ----- single-JSON output parsing --------------------------------------------

const TERMINAL_RECORD: &str = r#"{"response":"all done","session_id":"sess_1","stats":{"tokens":{"input":10,"output":5,"total":15}}}"#;

#[test]
fn parse_reads_response_session_and_tokens() {
    let parsed = GeminiOutput::parse(TERMINAL_RECORD).expect("a terminal record");
    assert_eq!(parsed.response().as_deref(), Some("all done"));
    assert_eq!(parsed.session_id().as_deref(), Some("sess_1"));
    let tokens = parsed.tokens();
    assert_eq!(tokens.input, Some(10));
    assert_eq!(tokens.output, Some(5));
    assert_eq!(tokens.total, Some(15));
    assert!(!parsed.is_error());
}

#[test]
fn parse_of_empty_stdout_is_none() {
    assert!(GeminiOutput::parse("   \n").is_none());
}

#[test]
fn parse_of_non_json_is_none() {
    assert!(GeminiOutput::parse("Loading model...").is_none());
}

#[test]
fn error_field_is_read_and_flags_failure() {
    let stdout = r#"{"error":{"type":"ContextWindowExceededError","message":"too big","code":42}}"#;
    let parsed = GeminiOutput::parse(stdout).expect("a record");
    assert!(parsed.is_error());
    let error = parsed.error().expect("an error object");
    assert_eq!(error.error_type.as_deref(), Some("ContextWindowExceededError"));
    assert_eq!(error.message.as_deref(), Some("too big"));
    assert_eq!(error.code, Some(42));
}

#[test]
fn null_error_field_is_not_an_error() {
    let parsed = GeminiOutput::parse(r#"{"response":"ok","error":null}"#).expect("a record");
    assert!(!parsed.is_error());
    assert!(parsed.error().is_none());
}

#[test]
fn try_from_returns_record_even_on_failure_exit() {
    let out = output(TERMINAL_RECORD, 1);
    let parsed = GeminiOutput::try_from(out).expect("record present => Ok");
    assert_eq!(parsed.response().as_deref(), Some("all done"));
}

#[test]
fn try_from_errors_only_when_empty_and_failed() {
    let out = output("not json at all", 2);
    let result = GeminiOutput::try_from(out);
    assert!(
        matches!(result, Err(Error::Cli { exit_code: Some(2), .. })),
        "expected Error::Cli with exit code 2, got {result:?}"
    );
}

#[test]
fn try_from_succeeds_on_empty_but_successful() {
    let parsed = GeminiOutput::try_from(output("", 0)).expect("success => Ok even when empty");
    assert!(parsed.response().is_none());
    assert!(!parsed.is_error());
}

// ----- stream-json output parsing --------------------------------------------

const EVENT_STREAM: &str = r#"{"type":"assistant","response":"partial"}
{"type":"result","response":"all done","session_id":"sess_2","stats":{"tokens":{"input":3,"output":7,"total":10}}}"#;

#[test]
fn stream_parse_collects_object_lines_only() {
    let stream = "Loading...\n42\n[1,2,3]\n{\"type\":\"result\"}\n";
    let parsed = StreamOutput::parse(stream);
    assert_eq!(parsed.events().len(), 1);
}

#[test]
fn stream_reports_last_response_session_and_tokens() {
    let parsed = StreamOutput::parse(EVENT_STREAM);
    assert_eq!(parsed.events().len(), 2);
    assert_eq!(parsed.response().as_deref(), Some("all done"));
    assert_eq!(parsed.session_id().as_deref(), Some("sess_2"));
    assert_eq!(parsed.tokens().total, Some(10));
    assert!(parsed.error().is_none());
}

#[test]
fn stream_event_marker_is_preserved_as_other() {
    let parsed = StreamOutput::parse(EVENT_STREAM);
    assert_eq!(
        parsed.events()[0].event_type(),
        StreamEventType::Other(Some("assistant".to_owned()))
    );
}

#[test]
fn stream_error_field_surfaces() {
    let stream = r#"{"type":"error","error":{"message":"boom","code":7}}"#;
    let parsed = StreamOutput::parse(stream);
    let error = parsed.error().expect("an error object");
    assert_eq!(error.message.as_deref(), Some("boom"));
    assert_eq!(error.code, Some(7));
}

#[test]
fn stream_try_from_errors_only_when_empty_and_failed() {
    let result = StreamOutput::try_from(output("garbage", 2));
    assert!(
        matches!(result, Err(Error::Cli { exit_code: Some(2), .. })),
        "expected Error::Cli, got {result:?}"
    );
}

// ----- executor end-to-end (fake CLI) ---------------------------------------

#[cfg(unix)]
#[test]
fn execute_parses_json_record_from_a_fake_gemini() {
    let script = fake_gemini(TERMINAL_RECORD, "", 0);
    let gemini = Gemini::new(script.path());
    let parsed = async_runtime()
        .block_on(gemini.execute(&RunCommand::prompt("hi").json()))
        .expect("execute succeeds");
    assert_eq!(parsed.session_id().as_deref(), Some("sess_1"));
    assert_eq!(parsed.response().as_deref(), Some("all done"));
}

#[cfg(unix)]
#[test]
fn execute_reports_cli_error_on_empty_failure() {
    let script = fake_gemini("not json", "boom", 2);
    let gemini = Gemini::new(script.path());
    let result = async_runtime().block_on(gemini.execute(&RunCommand::prompt("hi").json()));
    assert!(
        matches!(result, Err(Error::Cli { exit_code: Some(2), .. })),
        "expected Error::Cli, got {result:?}"
    );
}

#[cfg(unix)]
#[test]
fn stream_yields_events_then_verifies_exit() {
    let script = fake_gemini(EVENT_STREAM, "", 0);
    let gemini = Gemini::new(script.path());
    async_runtime().block_on(async {
        let events: Vec<_> = gemini
            .stream(&RunCommand::prompt("hi").stream_json())
            .expect("spawn stream")
            .collect()
            .await;
        let markers: Vec<_> = events
            .into_iter()
            .map(|event| event.expect("event ok").marker().map(str::to_owned))
            .collect();
        assert_eq!(
            markers,
            [Some("assistant".to_owned()), Some("result".to_owned())]
        );
    });
}

#[cfg(unix)]
#[test]
fn stream_surfaces_failure_exit_after_events() {
    let script = fake_gemini(r#"{"type":"assistant"}"#, "bad exit", 3);
    let gemini = Gemini::new(script.path());
    async_runtime().block_on(async {
        let mut stream = gemini
            .stream(&RunCommand::prompt("hi").stream_json())
            .expect("spawn stream");
        let first = stream.next().await.expect("one event").expect("event ok");
        assert_eq!(
            first.event_type(),
            StreamEventType::Other(Some("assistant".to_owned()))
        );
        let tail = stream.next().await.expect("exit verdict");
        assert!(
            matches!(tail, Err(Error::Cli { exit_code: Some(3), .. })),
            "expected Error::Cli on non-zero exit, got {tail:?}"
        );
    });
}
