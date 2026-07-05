//! Unit tests for argv construction and JSONL output parsing.
//!
//! None of these tests invoke Copilot; they exercise pure argv builders and
//! the stream parser against synthetic fixtures.

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
use crate::cli::{Copilot, Error};
use crate::mcp::{McpAddOptions, McpCommand, McpSubcommand, McpTransport};
use crate::ops::{
    CompletionCommand, HelpCommand, InitCommand, LoginCommand, UpdateCommand, VersionCommand,
};
use crate::output::{EventType, RunOutput};
use crate::plugin::{PluginCommand, PluginSubcommand};
use crate::run::{RunCommand, RunOptions, ShareTarget};
use crate::values::{Mode, SessionSelector, Shell, Toggle, UpdateChannel};

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

/// Write an executable shell script that impersonates
/// `copilot --output-format json` by emitting fixed stdout/stderr and exit
/// code, so the executor can be tested without invoking the real CLI.
#[cfg(unix)]
fn fake_copilot(stdout: &str, stderr: &str, code: i32) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("create fake copilot executable");
    write!(
        file,
        "#!/bin/sh\ncat <<'__COPILOT_STDOUT__'\n{stdout}\n__COPILOT_STDOUT__\ncat >&2 <<'__COPILOT_STDERR__'\n{stderr}\n__COPILOT_STDERR__\nexit {code}\n"
    )
    .expect("write fake copilot executable");
    let mut permissions = file
        .as_file()
        .metadata()
        .expect("fake metadata")
        .permissions();
    permissions.set_mode(0o700);
    file.as_file()
        .set_permissions(permissions)
        .expect("make fake copilot executable");
    file
}

// ----- argv construction: root run -------------------------------------------

#[test]
fn run_prompt_json_renders_prompt_then_output_format() {
    let command = RunCommand::prompt("do the thing").json();
    assert_eq!(
        argv(&command),
        ["--prompt", "do the thing", "--output-format", "json"]
    );
}

#[test]
fn headless_shape_is_prompt_allow_all_tools_then_json() {
    // The exact argv the driver's headless mode builds.
    let command = RunCommand {
        options: RunOptions {
            prompt: Some("x".to_owned()),
            allow_all_tools: true,
            ..RunOptions::default()
        },
        ..RunCommand::default()
    }
    .json();
    assert_eq!(
        argv(&command),
        [
            "--prompt",
            "x",
            "--allow-all-tools",
            "--output-format",
            "json"
        ]
    );
}

#[test]
fn run_renders_model_mode_dirs_and_attached_tool_in_order() {
    let command = RunCommand {
        options: RunOptions {
            prompt: Some("go".to_owned()),
            model: Some("gpt-5.2".to_owned()),
            mode: Some(Mode::Plan),
            add_dirs: vec![PathBuf::from("/src")],
            allow_tools: vec!["shell".to_owned()],
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(
        argv(&command),
        [
            "--prompt",
            "go",
            "--model",
            "gpt-5.2",
            "--mode",
            "plan",
            "--add-dir",
            "/src",
            "--allow-tool=shell",
        ]
    );
}

#[test]
fn resume_selector_renders_bare_and_attached_forms() {
    let bare = RunCommand {
        options: RunOptions {
            resume: Some(SessionSelector::Prompt),
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(argv(&bare), ["--resume"]);

    let specific = RunCommand {
        options: RunOptions {
            resume: Some(SessionSelector::reference("0cb916d")),
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(argv(&specific), ["--resume=0cb916d"]);
}

#[test]
fn toggles_render_attached_but_stream_takes_a_space() {
    let command = RunCommand {
        options: RunOptions {
            mouse: Some(Toggle::On),
            bash_env: Some(Toggle::Off),
            stream: Some(Toggle::On),
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(
        argv(&command),
        ["--mouse=on", "--stream", "on", "--bash-env=off"]
    );
}

#[test]
fn share_renders_bare_and_attached_path() {
    let bare = RunCommand {
        options: RunOptions {
            share: Some(ShareTarget::Default),
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(argv(&bare), ["--share"]);

    let path = RunCommand {
        options: RunOptions {
            share: Some(ShareTarget::Path(PathBuf::from("/tmp/s.md"))),
            ..RunOptions::default()
        },
        ..RunCommand::default()
    };
    assert_eq!(argv(&path), ["--share=/tmp/s.md"]);
}

#[test]
fn interactive_prompt_is_bare_positional_after_raw_args() {
    let command = RunCommand::interactive_prompt("seed").with_args(["agent"]);
    assert_eq!(argv(&command), ["agent", "seed"]);
}

// ----- argv construction: mcp ------------------------------------------------

#[test]
fn mcp_add_remote_renders_transport_then_name_then_url() {
    let command = McpCommand::new(McpSubcommand::Add {
        name: "notion".to_owned(),
        transport: McpTransport::Http {
            url: "https://mcp.notion.com/mcp".to_owned(),
        },
        options: McpAddOptions::default(),
    });
    assert_eq!(
        argv(&command),
        [
            "mcp",
            "add",
            "--transport",
            "http",
            "notion",
            "https://mcp.notion.com/mcp",
        ]
    );
}

#[test]
fn mcp_add_sse_transport_renders_its_own_name() {
    // The `sse` remote transport requires a URL just like `http`, and emits
    // its own `--transport sse` token (a closed hole: previously the transport
    // and the URL were independent, so `--transport sse` with no URL was
    // representable).
    let command = McpCommand::new(McpSubcommand::Add {
        name: "events".to_owned(),
        transport: McpTransport::Sse {
            url: "https://mcp.example.com/sse".to_owned(),
        },
        options: McpAddOptions::default(),
    });
    assert_eq!(
        argv(&command),
        [
            "mcp",
            "add",
            "--transport",
            "sse",
            "events",
            "https://mcp.example.com/sse",
        ]
    );
}

#[test]
fn mcp_add_local_passes_command_after_double_dash() {
    // The default `stdio` transport carries a *required* command; it emits no
    // `--transport` flag (stdio is Copilot's default) and places the command
    // after `--`.
    let command = McpCommand::new(McpSubcommand::Add {
        name: "fs".to_owned(),
        transport: McpTransport::Stdio {
            command: "npx".to_owned(),
            args: vec!["server".to_owned()],
            env: Vec::new(),
        },
        options: McpAddOptions::default(),
    });
    assert_eq!(argv(&command), ["mcp", "add", "fs", "--", "npx", "server"]);
}

#[test]
fn mcp_add_stdio_env_renders_typed_key_value_pairs() {
    // `--env` is now typed as (key, value) pairs on the stdio transport and
    // rendered `KEY=VALUE`, so a malformed non-pair value is unrepresentable.
    let command = McpCommand::new(McpSubcommand::Add {
        name: "github".to_owned(),
        transport: McpTransport::Stdio {
            command: "npx".to_owned(),
            args: vec![
                "-y".to_owned(),
                "@modelcontextprotocol/server-github".to_owned(),
            ],
            env: vec![(
                "GITHUB_PERSONAL_ACCESS_TOKEN".to_owned(),
                "ghp_xxx".to_owned(),
            )],
        },
        options: McpAddOptions::default(),
    });
    assert_eq!(
        argv(&command),
        [
            "mcp",
            "add",
            "--env",
            "GITHUB_PERSONAL_ACCESS_TOKEN=ghp_xxx",
            "github",
            "--",
            "npx",
            "-y",
            "@modelcontextprotocol/server-github",
        ]
    );
}

#[test]
fn mcp_add_shared_options_render_between_flags_and_name() {
    // Header/tools/timeout/json/show-secrets are shared across transports and
    // render after the transport flags and before the name.
    let command = McpCommand::new(McpSubcommand::Add {
        name: "notion".to_owned(),
        transport: McpTransport::Http {
            url: "https://mcp.notion.com/mcp".to_owned(),
        },
        options: McpAddOptions {
            headers: vec!["Authorization: Bearer tok".to_owned()],
            tools: Some("*".to_owned()),
            timeout_ms: Some(5000),
            show_secrets: true,
            json: true,
        },
    });
    assert_eq!(
        argv(&command),
        [
            "mcp",
            "add",
            "--transport",
            "http",
            "--header",
            "Authorization: Bearer tok",
            "--tools",
            "*",
            "--timeout",
            "5000",
            "--show-secrets",
            "--json",
            "notion",
            "https://mcp.notion.com/mcp",
        ]
    );
}

#[test]
fn mcp_get_places_flags_before_name() {
    let command = McpCommand::new(McpSubcommand::Get {
        name: "github".to_owned(),
        json: true,
        show_secrets: false,
    });
    assert_eq!(argv(&command), ["mcp", "get", "--json", "github"]);
}

#[test]
fn mcp_list_and_remove_render() {
    assert_eq!(
        argv(&McpCommand::new(McpSubcommand::List { json: true })),
        ["mcp", "list", "--json"]
    );
    assert_eq!(
        argv(&McpCommand::new(McpSubcommand::Remove {
            name: "github".to_owned(),
        })),
        ["mcp", "remove", "github"]
    );
}

// ----- argv construction: plugin ---------------------------------------------

#[test]
fn plugin_install_names_the_source() {
    let command = PluginCommand::new(PluginSubcommand::Install {
        source: "owner/repo".to_owned(),
    });
    assert_eq!(argv(&command), ["plugin", "install", "owner/repo"]);
}

#[test]
fn plugin_update_all_renders_flag_without_name() {
    let command = PluginCommand::new(PluginSubcommand::Update {
        all: true,
        name: None,
    });
    assert_eq!(argv(&command), ["plugin", "update", "--all"]);
}

#[test]
fn plugin_marketplace_forwards_passthrough_args() {
    let command = PluginCommand::new(PluginSubcommand::Marketplace {
        args: vec!["browse".to_owned(), "copilot-plugins".to_owned()],
    });
    assert_eq!(
        argv(&command),
        ["plugin", "marketplace", "browse", "copilot-plugins"]
    );
}

// ----- argv construction: ops ------------------------------------------------

#[test]
fn completion_shell_is_a_required_positional() {
    assert_eq!(
        argv(&CompletionCommand::new(Shell::Zsh)),
        ["completion", "zsh"]
    );
}

#[test]
fn login_forwards_host() {
    let command = LoginCommand {
        host: Some("https://example.ghe.com".to_owned()),
    };
    assert_eq!(
        argv(&command),
        ["login", "--host", "https://example.ghe.com"]
    );
}

#[test]
fn update_channel_is_a_bare_positional() {
    let command = UpdateCommand {
        channel: Some(UpdateChannel::Prerelease),
    };
    assert_eq!(argv(&command), ["update", "prerelease"]);
    assert_eq!(argv(&UpdateCommand::default()), ["update"]);
}

#[test]
fn version_and_init_are_bare() {
    assert_eq!(argv(&VersionCommand), ["version"]);
    assert_eq!(argv(&InitCommand), ["init"]);
}

#[test]
fn help_optionally_names_a_topic() {
    let command = HelpCommand {
        topic: Some("permissions".to_owned()),
    };
    assert_eq!(argv(&command), ["help", "permissions"]);
    assert_eq!(argv(&HelpCommand::default()), ["help"]);
}

// ----- executor context ------------------------------------------------------

#[test]
fn default_executor_uses_copilot() {
    assert_eq!(Copilot::default().executable(), OsString::from("copilot"));
}

#[test]
fn to_process_carries_cwd_and_env() {
    let copilot = Copilot::default()
        .with_current_dir("/tmp/work")
        .with_env("COPILOT_HOME", "/tmp/home");
    let command = copilot.to_process(&RunCommand::prompt("hi").json());
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
        [("COPILOT_HOME".to_owned(), Some("/tmp/home".to_owned()))]
    );
}

#[test]
fn with_env_replaces_existing_key() {
    let copilot = Copilot::default().with_env("K", "one").with_env("K", "two");
    let envs: Vec<_> = copilot
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

const RESULT_STREAM: &str = r#"{"type":"session.started","sessionId":"sess-1"}
{"type":"result","sessionId":"sess-1","exitCode":0,"usage":{"premiumRequests":2}}"#;

#[test]
fn parse_collects_object_events_only() {
    let parsed = RunOutput::parse(RESULT_STREAM);
    assert_eq!(parsed.events().len(), 2);
    assert_eq!(parsed.events()[1].event_type(), EventType::Result);
}

#[test]
fn parse_skips_non_json_and_non_object_lines() {
    let stream = "Loading...\n42\n[1,2,3]\n{\"type\":\"result\"}\n";
    let parsed = RunOutput::parse(stream);
    assert_eq!(parsed.events().len(), 1);
    assert_eq!(parsed.events()[0].event_type(), EventType::Result);
}

#[test]
fn result_record_exposes_session_exit_and_usage() {
    let parsed = RunOutput::parse(RESULT_STREAM);
    let record = parsed.result().expect("a result record");
    assert_eq!(record.session_id.as_deref(), Some("sess-1"));
    assert_eq!(record.exit_code, Some(0));
    assert_eq!(record.premium_requests(), Some(2));
    assert_eq!(parsed.session_id().as_deref(), Some("sess-1"));
    assert!(parsed.session_error().is_none());
}

#[test]
fn session_error_record_exposes_type_code_and_status() {
    let stream = r#"{"type":"session.error","errorType":"quota_exceeded","errorCode":"quota","statusCode":402}"#;
    let parsed = RunOutput::parse(stream);
    let record = parsed.session_error().expect("a session.error record");
    assert_eq!(record.error_type.as_deref(), Some("quota_exceeded"));
    assert_eq!(record.error_code.as_deref(), Some("quota"));
    assert_eq!(record.status_code, Some(402));
    assert!(parsed.result().is_none());
}

#[test]
fn both_terminal_records_are_exposed_when_both_appear() {
    // Classification (which one wins) is the caller's job; the crate surfaces
    // both records faithfully.
    let stream = concat!(
        "{\"type\":\"result\",\"sessionId\":\"s\",\"exitCode\":0}\n",
        "{\"type\":\"session.error\",\"errorType\":\"quota\",\"statusCode\":402}\n",
    );
    let parsed = RunOutput::parse(stream);
    assert!(parsed.result().is_some());
    assert!(parsed.session_error().is_some());
}

#[test]
fn try_from_succeeds_when_events_present_even_on_failure_exit() {
    let parsed = RunOutput::try_from(output(RESULT_STREAM, 1)).expect("events present => Ok");
    assert_eq!(parsed.session_id().as_deref(), Some("sess-1"));
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
fn execute_parses_json_stream_from_a_fake_copilot() {
    let script = fake_copilot(RESULT_STREAM, "", 0);
    let copilot = Copilot::new(script.path());
    let parsed = async_runtime()
        .block_on(copilot.execute(&RunCommand::prompt("hi").json()))
        .expect("execute succeeds");
    assert_eq!(parsed.session_id().as_deref(), Some("sess-1"));
}

#[cfg(unix)]
#[test]
fn execute_reports_cli_error_on_empty_failure() {
    let script = fake_copilot("not json", "boom", 2);
    let copilot = Copilot::new(script.path());
    let result = async_runtime().block_on(copilot.execute(&RunCommand::prompt("hi").json()));
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
    let script = fake_copilot(RESULT_STREAM, "", 0);
    let copilot = Copilot::new(script.path());
    async_runtime().block_on(async {
        let events: Vec<_> = copilot
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
                EventType::Other(Some("session.started".to_owned())),
                EventType::Result,
            ]
        );
    });
}

#[cfg(unix)]
#[test]
fn stream_surfaces_failure_exit_after_events() {
    let script = fake_copilot(r#"{"type":"session.started"}"#, "bad exit", 3);
    let copilot = Copilot::new(script.path());
    async_runtime().block_on(async {
        let mut stream = copilot
            .stream(&RunCommand::prompt("hi").json())
            .expect("spawn stream");
        let first = stream.next().await.expect("one event").expect("event ok");
        assert_eq!(
            first.event_type(),
            EventType::Other(Some("session.started".to_owned()))
        );
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
