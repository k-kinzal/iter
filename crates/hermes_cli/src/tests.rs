use pretty_assertions::assert_eq;

use crate::{
    AuthAddOptions, AuthCommand, AuthSubcommand, ChatCommand, ChatOptions, CompletionCommand,
    ConfigCommand, ConfigSubcommand, ContinueMode, CredentialType, Exit, Hermes, LoginCommand,
    LoginProvider, LogoutCommand, LogoutProvider, McpAddOptions, McpAuth, McpCommand, McpSubcommand,
    McpTransport, ModelCommand, NousOauthOptions, RawCommand, RunCommand, RunMode, RunOptions,
    RunOutput, SendCommand, SendOutput, SessionExport, SessionsCommand, SessionsSubcommand, Shell,
    SpotifyAction, StatusCommand, ToArgs, ToolsCommand, ToolsSubcommand, UpdateCommand,
    VersionCommand,
};

// ----- helpers -------------------------------------------------------------

fn argv<C: ToArgs + ?Sized>(command: &C) -> Vec<String> {
    command
        .to_args()
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

/// Build an expected argv vector from string slices.
fn v(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

#[cfg(unix)]
fn exit_status(code: i32) -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt as _;
    std::process::ExitStatus::from_raw(code << 8)
}

#[cfg(unix)]
fn signal_status(signal: i32) -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt as _;
    std::process::ExitStatus::from_raw(signal)
}

#[cfg(unix)]
fn process_output(stdout: &str, stderr: &str, code: i32) -> std::process::Output {
    std::process::Output {
        status: exit_status(code),
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

#[cfg(unix)]
fn async_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .expect("current-thread runtime")
}

/// Single-quote a string for safe embedding in a `/bin/sh` script.
#[cfg(unix)]
fn sq(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Write an executable fake `hermes` that prints fixed stdout/stderr and exits
/// with `code`.
#[cfg(unix)]
fn fake_hermes(stdout: &str, stderr: &str, code: i32) -> tempfile::NamedTempFile {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;

    let mut file = tempfile::NamedTempFile::new().expect("temp file");
    let script = format!(
        "#!/bin/sh\nprintf '%s' {out}\nprintf '%s' {err} 1>&2\nexit {code}\n",
        out = sq(stdout),
        err = sq(stderr),
    );
    file.write_all(script.as_bytes()).expect("write script");
    file.flush().expect("flush script");
    let mut perms = file.as_file().metadata().expect("metadata").permissions();
    perms.set_mode(0o755);
    file.as_file().set_permissions(perms).expect("chmod");
    file
}

// ----- root run argv -------------------------------------------------------

#[test]
fn oneshot_emits_dash_z_then_prompt() {
    // `-z` carries the prompt as its own value; nothing is fed on stdin.
    assert_eq!(argv(&RunCommand::oneshot("hello")), v(&["-z", "hello"]));
}

#[test]
fn oneshot_renders_options_before_the_dash_z_prompt() {
    let mut command = RunCommand::oneshot("go");
    command.options.model = Some("hermes-4".into());
    assert_eq!(argv(&command), v(&["--model", "hermes-4", "-z", "go"]));
}

#[test]
fn continue_most_recent_renders_bare_flag() {
    let mut command = RunCommand::oneshot("go");
    command.options.continue_session = Some(ContinueMode::MostRecent);
    assert_eq!(argv(&command), v(&["--continue", "-z", "go"]));
}

#[test]
fn continue_named_renders_flag_and_name() {
    let mut command = RunCommand::oneshot("go");
    command.options.continue_session = Some(ContinueMode::Named("branch".into()));
    assert_eq!(argv(&command), v(&["--continue", "branch", "-z", "go"]));
}

#[test]
fn tui_modes_render_flag_and_optional_seed() {
    assert_eq!(argv(&RunCommand::tui()), v(&["--tui"]));
    assert_eq!(argv(&RunCommand::tui_prompt("seed")), v(&["--tui", "seed"]));
}

#[test]
fn cli_mode_renders_flag_and_optional_seed() {
    let command = RunCommand {
        mode: RunMode::Cli(Some("seed".into())),
        ..RunCommand::default()
    };
    assert_eq!(argv(&command), v(&["--cli", "seed"]));
}

#[test]
fn oneshot_mode_carries_its_prompt_in_the_variant() {
    // The `-z` prompt rides inside the mode, so OneShot can never be
    // constructed without it and can never desync from a separate field.
    let command = RunCommand {
        mode: RunMode::OneShot("hi".into()),
        ..RunCommand::default()
    };
    assert_eq!(argv(&command), v(&["-z", "hi"]));
}

#[test]
fn interactive_without_prompt_is_flags_only() {
    assert_eq!(argv(&RunCommand::interactive()), Vec::<String>::new());
}

#[test]
fn interactive_seeds_a_bare_positional_prompt() {
    let command = RunCommand {
        mode: RunMode::Interactive(Some("seed".into())),
        ..RunCommand::default()
    };
    assert_eq!(argv(&command), v(&["seed"]));
}

#[test]
fn run_options_render_in_declaration_order() {
    let command = RunCommand {
        mode: RunMode::OneShot("go".into()),
        options: RunOptions {
            model: Some("m".into()),
            provider: Some("p".into()),
            toolsets: Some("web,fs".into()),
            skills: Some("rust".into()),
            resume: Some("sid".into()),
            continue_session: Some(ContinueMode::Named("branch".into())),
            worktree: true,
            accept_hooks: true,
            yolo: true,
            pass_session_id: true,
            ignore_user_config: true,
            ignore_rules: true,
            safe_mode: true,
            dev: true,
        },
    };
    assert_eq!(
        argv(&command),
        v(&[
            "--model", "m", "--provider", "p", "--toolsets", "web,fs", "--skills", "rust",
            "--resume", "sid", "--continue", "branch", "--worktree", "--accept-hooks", "--yolo",
            "--pass-session-id", "--ignore-user-config", "--ignore-rules", "--safe-mode", "--dev",
            "-z", "go",
        ]),
    );
}

// ----- chat argv -----------------------------------------------------------

#[test]
fn chat_quiet_query_renders_query_then_quiet() {
    assert_eq!(
        argv(&ChatCommand::quiet_query("summarize")),
        v(&["chat", "--query", "summarize", "--quiet"]),
    );
}

#[test]
fn chat_interactive_is_bare_subcommand() {
    assert_eq!(argv(&ChatCommand::interactive()), v(&["chat"]));
}

#[test]
fn chat_options_render_query_first_then_options_in_order() {
    let command = ChatCommand {
        query: Some("q".into()),
        options: ChatOptions {
            model: Some("m".into()),
            continue_session: Some(ContinueMode::MostRecent),
            max_turns: Some(5),
            tui: true,
            ..ChatOptions::default()
        },
    };
    assert_eq!(
        argv(&command),
        v(&["chat", "--query", "q", "--model", "m", "--continue", "--max-turns", "5", "--tui"]),
    );
}

// ----- send argv -----------------------------------------------------------

#[test]
fn send_to_renders_target_then_message_positional() {
    assert_eq!(
        argv(&SendCommand::to("telegram:123", "hi")),
        v(&["send", "--to", "telegram:123", "hi"]),
    );
}

#[test]
fn send_json_flag_precedes_the_message_positional() {
    assert_eq!(
        argv(&SendCommand::to("telegram", "hi").with_json()),
        v(&["send", "--to", "telegram", "--json", "hi"]),
    );
}

#[test]
fn send_list_renders_the_list_flag() {
    assert_eq!(argv(&SendCommand::list()), v(&["send", "--list"]));
}

// ----- send --json output --------------------------------------------------

#[test]
fn send_output_reads_success_fields() {
    let out = SendOutput::parse(
        r#"{"ok": true, "target": "telegram:123", "platform": "telegram", "message_id": "42"}"#,
    )
    .expect("valid json");
    assert_eq!(out.is_ok(), Some(true));
    assert_eq!(out.target(), Some("telegram:123"));
    assert_eq!(out.platform(), Some("telegram"));
    assert_eq!(out.message_id(), Some("42"));
    assert_eq!(out.error(), None);
}

#[test]
fn send_output_reads_failure_fields() {
    let out = SendOutput::parse(r#"{"ok": false, "error": "no route"}"#).expect("valid json");
    assert_eq!(out.is_ok(), Some(false));
    assert_eq!(out.error(), Some("no route"));
}

#[test]
fn send_output_tolerates_alternate_spellings() {
    // `success` / `to` / `messageId` are accepted alongside the canonical keys.
    let out = SendOutput::parse(r#"{"success": true, "to": "slack", "messageId": "9"}"#)
        .expect("valid json");
    assert_eq!(out.is_ok(), Some(true));
    assert_eq!(out.target(), Some("slack"));
    assert_eq!(out.message_id(), Some("9"));
}

#[test]
fn send_output_rejects_non_json() {
    let err = SendOutput::parse("not json at all").expect_err("must reject");
    assert!(matches!(err, crate::Error::Json(_)), "got {err:?}");
}

#[cfg(unix)]
#[test]
fn send_output_try_from_process_output() {
    let out = SendOutput::try_from(process_output(r#"{"ok": true}"#, "", 0)).expect("valid json");
    assert_eq!(out.is_ok(), Some(true));
}

// ----- sessions argv -------------------------------------------------------

#[test]
fn sessions_list_is_bare() {
    assert_eq!(argv(&SessionsCommand::list()), v(&["sessions", "list"]));
}

#[test]
fn sessions_export_renders_filters_before_the_output_positional() {
    let command = SessionsCommand::new(SessionsSubcommand::Export {
        output: "out.jsonl".into(),
        source: Some("cli".into()),
        session_id: Some("abc".into()),
    });
    assert_eq!(
        argv(&command),
        v(&["sessions", "export", "--source", "cli", "--session-id", "abc", "out.jsonl"]),
    );
}

#[test]
fn sessions_export_to_stdout_dash() {
    assert_eq!(
        argv(&SessionsCommand::export("-")),
        v(&["sessions", "export", "-"]),
    );
}

#[test]
fn sessions_delete_renders_yes_before_the_id() {
    let command = SessionsCommand::new(SessionsSubcommand::Delete {
        session_id: "sid".into(),
        yes: true,
    });
    assert_eq!(argv(&command), v(&["sessions", "delete", "--yes", "sid"]));
}

#[test]
fn sessions_prune_renders_all_filters() {
    let command = SessionsCommand::new(SessionsSubcommand::Prune {
        older_than: Some(30),
        source: Some("telegram".into()),
        yes: true,
    });
    assert_eq!(
        argv(&command),
        v(&["sessions", "prune", "--older-than", "30", "--source", "telegram", "--yes"]),
    );
}

#[test]
fn sessions_rename_spreads_the_title_words() {
    let command = SessionsCommand::new(SessionsSubcommand::Rename {
        session_id: "sid".into(),
        title: vec!["New".into(), "Title".into()],
    });
    assert_eq!(
        argv(&command),
        v(&["sessions", "rename", "sid", "New", "Title"]),
    );
}

#[test]
fn sessions_repair_and_bare_leaves() {
    let repair = SessionsCommand::new(SessionsSubcommand::Repair {
        check_only: true,
        no_backup: false,
    });
    assert_eq!(argv(&repair), v(&["sessions", "repair", "--check-only"]));
    assert_eq!(
        argv(&SessionsCommand::new(SessionsSubcommand::Stats)),
        v(&["sessions", "stats"]),
    );
    assert_eq!(
        argv(&SessionsCommand::new(SessionsSubcommand::Browse)),
        v(&["sessions", "browse"]),
    );
}

// ----- sessions export JSONL parsing ---------------------------------------

#[test]
fn session_export_parses_object_lines_and_skips_the_rest() {
    let jsonl = concat!(
        "{\"session_id\": \"a\", \"title\": \"First\", \"source\": \"cli\"}\n",
        "not a json line\n",
        "\n",
        "{\"id\": \"b\"}\n",
        "42\n",
    );
    let export = SessionExport::parse(jsonl);
    assert_eq!(export.records().len(), 2);

    let first = &export.records()[0];
    assert_eq!(first.session_id(), Some("a"));
    assert_eq!(first.title(), Some("First"));
    assert_eq!(first.source(), Some("cli"));

    // The second record identifies its session through the `id` alias.
    assert_eq!(export.records()[1].session_id(), Some("b"));
}

#[test]
fn session_export_session_ids_are_deduplicated_in_first_seen_order() {
    let jsonl = concat!(
        "{\"session_id\": \"a\"}\n",
        "{\"session_id\": \"b\"}\n",
        "{\"session_id\": \"a\"}\n",
    );
    assert_eq!(SessionExport::parse(jsonl).session_ids(), vec!["a", "b"]);
}

// ----- mcp argv ------------------------------------------------------------

#[test]
fn mcp_serve_renders_flags() {
    let command = McpCommand::new(McpSubcommand::Serve {
        verbose: true,
        accept_hooks: true,
    });
    assert_eq!(argv(&command), v(&["mcp", "serve", "--verbose", "--accept-hooks"]));
}

#[test]
fn mcp_add_stdio_renders_name_first_env_pairs_and_args_last() {
    let command = McpCommand::new(McpSubcommand::Add(McpAddOptions {
        name: "gh".into(),
        transport: McpTransport::Stdio {
            command: "npx".into(),
            env: vec![("A".into(), "1".into()), ("B".into(), "2".into())],
            args: vec!["-y".into(), "pkg".into()],
        },
        auth: Some(McpAuth::Header),
    }));
    assert_eq!(
        argv(&command),
        v(&[
            "mcp", "add", "gh", "--auth", "header", "--command", "npx", "--env", "A=1", "--env",
            "B=2", "--args", "-y", "pkg",
        ]),
    );
}

#[test]
fn mcp_add_http_and_preset_transports_render_their_flag() {
    // `--env` / `--args` are unreachable on these transports by construction.
    let http = McpCommand::add(
        "remote",
        McpTransport::Http {
            url: "https://x/sse".into(),
        },
    );
    assert_eq!(
        argv(&http),
        v(&["mcp", "add", "remote", "--url", "https://x/sse"]),
    );

    let preset = McpCommand::add("gh", McpTransport::Preset("github".into()));
    assert_eq!(argv(&preset), v(&["mcp", "add", "gh", "--preset", "github"]));
}

#[test]
fn mcp_remove_and_list_argv() {
    assert_eq!(argv(&McpCommand::remove("gh")), v(&["mcp", "remove", "gh"]));
    assert_eq!(argv(&McpCommand::list()), v(&["mcp", "list"]));
}

// ----- auth / login / logout argv ------------------------------------------

#[test]
fn auth_add_renders_options_then_provider_positional() {
    let command = AuthCommand::new(AuthSubcommand::Add(AuthAddOptions {
        provider: "anthropic".into(),
        credential_type: Some(CredentialType::ApiKey),
        label: Some("work".into()),
        api_key: Some("sk-1".into()),
        manual_paste: false,
        oauth: NousOauthOptions::default(),
    }));
    assert_eq!(
        argv(&command),
        v(&["auth", "add", "--type", "api-key", "--label", "work", "--api-key", "sk-1", "anthropic"]),
    );
}

#[test]
fn auth_list_status_remove_reset_logout_spotify_argv() {
    assert_eq!(argv(&AuthCommand::list()), v(&["auth", "list"]));
    assert_eq!(
        argv(&AuthCommand::new(AuthSubcommand::List {
            provider: Some("anthropic".into())
        })),
        v(&["auth", "list", "anthropic"]),
    );
    assert_eq!(argv(&AuthCommand::status("anthropic")), v(&["auth", "status", "anthropic"]));
    assert_eq!(
        argv(&AuthCommand::new(AuthSubcommand::Remove {
            provider: "anthropic".into(),
            target: "work".into(),
        })),
        v(&["auth", "remove", "anthropic", "work"]),
    );
    assert_eq!(
        argv(&AuthCommand::new(AuthSubcommand::Reset {
            provider: "anthropic".into()
        })),
        v(&["auth", "reset", "anthropic"]),
    );
    assert_eq!(
        argv(&AuthCommand::new(AuthSubcommand::Logout {
            provider: "anthropic".into()
        })),
        v(&["auth", "logout", "anthropic"]),
    );
    assert_eq!(
        argv(&AuthCommand::new(AuthSubcommand::Spotify {
            action: Some(SpotifyAction::Login)
        })),
        v(&["auth", "spotify", "login"]),
    );
}

#[test]
fn spotify_action_variants_render_their_tokens() {
    for (action, token) in [
        (SpotifyAction::Login, "login"),
        (SpotifyAction::Status, "status"),
        (SpotifyAction::Logout, "logout"),
    ] {
        assert_eq!(
            argv(&AuthCommand::new(AuthSubcommand::Spotify {
                action: Some(action),
            })),
            v(&["auth", "spotify", token]),
        );
    }
    // The bare `auth spotify` form (no action) stays representable.
    assert_eq!(
        argv(&AuthCommand::new(AuthSubcommand::Spotify { action: None })),
        v(&["auth", "spotify"]),
    );
}

#[test]
fn login_renders_provider_and_oauth_block() {
    assert_eq!(
        argv(&LoginCommand::provider(LoginProvider::OpenAiCodex)),
        v(&["login", "--provider", "openai-codex"]),
    );
    assert_eq!(argv(&LoginCommand::default()), v(&["login"]));

    let command = LoginCommand {
        provider: Some(LoginProvider::Nous),
        oauth: NousOauthOptions {
            portal_url: Some("https://portal".into()),
            no_browser: true,
            timeout: Some(30),
            insecure: true,
            ..NousOauthOptions::default()
        },
    };
    assert_eq!(
        argv(&command),
        v(&[
            "login", "--provider", "nous", "--portal-url", "https://portal", "--no-browser",
            "--timeout", "30", "--insecure",
        ]),
    );
}

#[test]
fn logout_renders_provider_or_nothing() {
    assert_eq!(
        argv(&LogoutCommand::provider(LogoutProvider::Spotify)),
        v(&["logout", "--provider", "spotify"]),
    );
    assert_eq!(argv(&LogoutCommand::default()), v(&["logout"]));
}

// ----- ops argv ------------------------------------------------------------

#[test]
fn config_argv() {
    assert_eq!(argv(&ConfigCommand::show()), v(&["config", "show"]));
    assert_eq!(
        argv(&ConfigCommand::set("model", "gpt")),
        v(&["config", "set", "model", "gpt"]),
    );
    assert_eq!(
        argv(&ConfigCommand::new(ConfigSubcommand::Path)),
        v(&["config", "path"]),
    );
}

#[test]
fn config_set_pair_states_render_positionals_in_order() {
    // key + value.
    assert_eq!(
        argv(&ConfigCommand::new(ConfigSubcommand::Set {
            pair: Some(("model".into(), Some("gpt".into()))),
        })),
        v(&["config", "set", "model", "gpt"]),
    );
    // key only — a value can never appear without its key.
    assert_eq!(
        argv(&ConfigCommand::new(ConfigSubcommand::Set {
            pair: Some(("model".into(), None)),
        })),
        v(&["config", "set", "model"]),
    );
    // neither — bare `config set`.
    assert_eq!(
        argv(&ConfigCommand::new(ConfigSubcommand::Set { pair: None })),
        v(&["config", "set"]),
    );
}

#[test]
fn tools_argv() {
    assert_eq!(argv(&ToolsCommand::list()), v(&["tools", "list"]));
    let enable = ToolsCommand {
        summary: false,
        subcommand: Some(ToolsSubcommand::Enable {
            platform: Some("telegram".into()),
            names: vec!["web".into(), "memory".into()],
        }),
    };
    assert_eq!(
        argv(&enable),
        v(&["tools", "enable", "--platform", "telegram", "web", "memory"]),
    );
    let summary = ToolsCommand {
        summary: true,
        subcommand: None,
    };
    assert_eq!(argv(&summary), v(&["tools", "--summary"]));
}

#[test]
fn model_status_version_argv() {
    let model = ModelCommand {
        refresh: true,
        ..ModelCommand::default()
    };
    assert_eq!(argv(&model), v(&["model", "--refresh"]));
    assert_eq!(argv(&ModelCommand::default()), v(&["model"]));
    assert_eq!(
        argv(&StatusCommand {
            all: true,
            deep: true
        }),
        v(&["status", "--all", "--deep"]),
    );
    assert_eq!(argv(&VersionCommand), v(&["version"]));
}

#[test]
fn update_argv() {
    let command = UpdateCommand {
        gateway: false,
        check: true,
        no_backup: false,
        backup: true,
        yes: true,
        branch: Some("main".into()),
        force: true,
    };
    assert_eq!(
        argv(&command),
        v(&["update", "--check", "--backup", "--yes", "--branch", "main", "--force"]),
    );
}

#[test]
fn completion_argv() {
    assert_eq!(
        argv(&CompletionCommand::shell(Shell::Zsh)),
        v(&["completion", "zsh"]),
    );
    assert_eq!(argv(&CompletionCommand::default()), v(&["completion"]));
}

#[test]
fn raw_command_renders_name_then_verbatim_args() {
    assert_eq!(
        argv(&RawCommand::with_args("doctor", vec!["--verbose".into()])),
        v(&["doctor", "--verbose"]),
    );
    assert_eq!(argv(&RawCommand::new("gateway")), v(&["gateway"]));
}

// ----- typed values --------------------------------------------------------

#[test]
fn value_tokens_match_the_cli_choices() {
    assert_eq!(LoginProvider::Nous.as_str(), "nous");
    assert_eq!(LoginProvider::OpenAiCodex.as_str(), "openai-codex");
    assert_eq!(LoginProvider::XaiOauth.as_str(), "xai-oauth");
    assert_eq!(LogoutProvider::Spotify.as_str(), "spotify");
    assert_eq!(CredentialType::ApiKey.as_str(), "api-key");
    assert_eq!(CredentialType::Oauth.as_str(), "oauth");
    assert_eq!(McpAuth::Header.as_str(), "header");
    assert_eq!(Shell::default().as_str(), "bash");
    assert_eq!(Shell::Fish.as_str(), "fish");
}

// ----- executor cwd/env ----------------------------------------------------

#[test]
fn to_process_sets_cwd_and_env() {
    let hermes = Hermes::new("hermes")
        .with_current_dir("/work")
        .with_env("HERMES_X", "1");
    let process = hermes.to_process(&RunCommand::oneshot("x"));
    let std = process.as_std();
    assert_eq!(std.get_current_dir(), Some(std::path::Path::new("/work")));
    let has_env = std.get_envs().any(|(k, val)| {
        k == std::ffi::OsStr::new("HERMES_X") && val == Some(std::ffi::OsStr::new("1"))
    });
    assert!(has_env, "declared env must be applied");
}

#[test]
fn with_env_replaces_an_existing_key() {
    let hermes = Hermes::new("hermes").with_env("K", "1").with_env("K", "2");
    let count = hermes
        .envs()
        .filter(|(k, _)| *k == std::ffi::OsStr::new("K"))
        .count();
    assert_eq!(count, 1, "the second set must replace, not append");
    let value = hermes
        .envs()
        .find(|(k, _)| *k == std::ffi::OsStr::new("K"))
        .map(|(_, val)| val.to_owned());
    assert_eq!(value, Some(std::ffi::OsString::from("2")));
}

// ----- exit classification -------------------------------------------------

#[cfg(unix)]
#[test]
fn exit_classifies_success_failure_and_signal() {
    assert_eq!(Exit::from_status(exit_status(0)), Exit::Success);
    assert!(Exit::from_status(exit_status(0)).is_success());
    assert_eq!(Exit::from_status(exit_status(2)), Exit::Failure(2));
    assert_eq!(Exit::from_status(exit_status(2)).code(), Some(2));
    assert_eq!(Exit::from_status(signal_status(9)), Exit::Signal(9));
    assert_eq!(Exit::from_status(signal_status(9)).code(), None);
}

#[cfg(unix)]
#[test]
fn run_output_from_process_output_decodes_text_and_exit() {
    let output = RunOutput::from(process_output("answer", "warn", 0));
    assert_eq!(output.stdout(), "answer");
    assert_eq!(output.stderr(), "warn");
    assert!(output.is_success());
    assert_eq!(output.exit(), Exit::Success);
}

#[cfg(unix)]
#[test]
fn into_result_is_ok_on_clean_exit() {
    let output = RunOutput::from(process_output("ok", "", 0));
    let recovered = output.into_result().expect("clean exit is ok");
    assert_eq!(recovered.stdout(), "ok");
}

#[cfg(unix)]
#[test]
fn into_result_errors_on_nonzero_exit() {
    let output = RunOutput::from(process_output("partial", "boom", 1));
    let err = output.into_result().expect_err("nonzero is an error");
    assert!(
        matches!(err, crate::Error::Cli { exit_code: Some(1), .. }),
        "got {err:?}",
    );
}

// ----- fake-CLI end to end -------------------------------------------------

#[cfg(unix)]
#[test]
fn execute_returns_output_without_erroring_on_nonzero() {
    let bin = fake_hermes("final answer", "", 0);
    let hermes = Hermes::new(bin.path());
    let output = async_runtime()
        .block_on(hermes.execute(&RunCommand::oneshot("x")))
        .expect("execute");
    assert_eq!(output.stdout(), "final answer");
    assert!(output.is_success());
}

#[cfg(unix)]
#[test]
fn execute_surfaces_nonzero_as_output_not_error() {
    let bin = fake_hermes("", "bad args", 2);
    let hermes = Hermes::new(bin.path());
    let output = async_runtime()
        .block_on(hermes.execute(&RunCommand::oneshot("x")))
        .expect("execute still succeeds; classification is up to the caller");
    assert_eq!(output.exit(), Exit::Failure(2));
    assert_eq!(output.stderr(), "bad args");
}
