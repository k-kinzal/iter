use std::path::PathBuf;
use std::time::Duration;

use pretty_assertions::assert_eq;

use crate::{
    Antigravity, ChangelogCommand, Exit, GoDuration, HelpCommand, ImportSource, InstallCommand,
    ModelsCommand, PluginCommand, PluginSubcommand, RunCommand, RunMode, RunOptions, RunOutput,
    ToArgs, UpdateCommand,
};

// ----- helpers -------------------------------------------------------------

fn argv<C: ToArgs + ?Sized>(command: &C) -> Vec<String> {
    command
        .to_args()
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
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

/// Write an executable fake `agy` that prints fixed stdout/stderr and exits
/// with `code`.
#[cfg(unix)]
fn fake_agy(stdout: &str, stderr: &str, code: i32) -> tempfile::NamedTempFile {
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
fn print_mode_emits_print_flag_then_prompt() {
    assert_eq!(
        argv(&RunCommand::print("hello")),
        vec!["--print".to_owned(), "hello".to_owned()],
    );
}

#[test]
fn print_mode_renders_conversation_before_prompt_flag() {
    let mut command = RunCommand::print("go");
    command.options.conversation = Some("sess-1".into());
    assert_eq!(
        argv(&command),
        vec![
            "--conversation".to_owned(),
            "sess-1".to_owned(),
            "--print".to_owned(),
            "go".to_owned(),
        ],
    );
}

#[test]
fn interactive_mode_seeds_a_bare_positional_prompt() {
    assert_eq!(
        argv(&RunCommand::interactive_prompt("seed")),
        vec!["seed".to_owned()],
    );
}

#[test]
fn interactive_mode_renders_options_before_the_positional() {
    // Go's `flag` parser stops at the first positional, so options must precede
    // the bare prompt or they would be swallowed.
    let mut command = RunCommand::interactive_prompt("seed");
    command.options.conversation = Some("sess-1".into());
    assert_eq!(
        argv(&command),
        vec![
            "--conversation".to_owned(),
            "sess-1".to_owned(),
            "seed".to_owned(),
        ],
    );
}

#[test]
fn interactive_mode_without_a_prompt_is_flags_only() {
    assert_eq!(argv(&RunCommand::interactive()), Vec::<String>::new());
}

#[test]
fn prompt_interactive_mode_emits_its_flag() {
    assert_eq!(
        argv(&RunCommand::prompt_interactive("seed")),
        vec!["--prompt-interactive".to_owned(), "seed".to_owned()],
    );
}

#[test]
fn run_mode_variants_carry_their_prompt_operand() {
    // The prompt operand now lives inside the mode variant, so a struct-literal
    // `RunCommand` cannot select `--print` / `--prompt-interactive` without also
    // supplying the required prompt — the empty-argv hole is unrepresentable.
    assert_eq!(
        argv(&RunCommand {
            mode: RunMode::Print("do it".into()),
            options: RunOptions::default(),
        }),
        vec!["--print".to_owned(), "do it".to_owned()],
    );
    assert_eq!(
        argv(&RunCommand {
            mode: RunMode::PromptInteractive("seed".into()),
            options: RunOptions::default(),
        }),
        vec!["--prompt-interactive".to_owned(), "seed".to_owned()],
    );
    // Interactive's seed prompt stays genuinely optional.
    assert_eq!(
        argv(&RunCommand {
            mode: RunMode::Interactive(Some("seed".into())),
            options: RunOptions::default(),
        }),
        vec!["seed".to_owned()],
    );
    assert_eq!(
        argv(&RunCommand {
            mode: RunMode::Interactive(None),
            options: RunOptions::default(),
        }),
        Vec::<String>::new(),
    );
}

#[test]
fn run_mode_default_is_interactive_without_a_seed() {
    assert_eq!(RunMode::default(), RunMode::Interactive(None));
    assert_eq!(argv(&RunCommand::default()), Vec::<String>::new());
}

#[test]
fn run_options_render_in_declaration_order() {
    let command = RunCommand {
        mode: RunMode::Print("do it".into()),
        options: RunOptions {
            add_dir: vec![PathBuf::from("/a"), PathBuf::from("/b")],
            continue_conversation: true,
            conversation: Some("conv".into()),
            dangerously_skip_permissions: true,
            log_file: Some(PathBuf::from("/tmp/agy.log")),
            model: Some("gemini-3-pro".into()),
            new_project: true,
            print_timeout: Some(GoDuration::from_secs(600)),
            project: Some("proj".into()),
            sandbox: true,
        },
    };
    assert_eq!(
        argv(&command),
        vec![
            "--add-dir".to_owned(),
            "/a".to_owned(),
            "--add-dir".to_owned(),
            "/b".to_owned(),
            "--continue".to_owned(),
            "--conversation".to_owned(),
            "conv".to_owned(),
            "--dangerously-skip-permissions".to_owned(),
            "--log-file".to_owned(),
            "/tmp/agy.log".to_owned(),
            "--model".to_owned(),
            "gemini-3-pro".to_owned(),
            "--new-project".to_owned(),
            "--print-timeout".to_owned(),
            "10m0s".to_owned(),
            "--project".to_owned(),
            "proj".to_owned(),
            "--sandbox".to_owned(),
            "--print".to_owned(),
            "do it".to_owned(),
        ],
    );
}

// ----- Go duration ---------------------------------------------------------

#[test]
fn go_duration_renders_parse_duration_compatible_strings() {
    assert_eq!(GoDuration::from_secs(0).render(), "0s");
    assert_eq!(GoDuration::from_secs(45).render(), "45s");
    assert_eq!(GoDuration::from_secs(90).render(), "1m30s");
    assert_eq!(GoDuration::from_secs(300).render(), "5m0s");
    assert_eq!(GoDuration::from_secs(600).render(), "10m0s");
    assert_eq!(GoDuration::from_secs(3600).render(), "1h0m0s");
    assert_eq!(GoDuration::from_secs(3661).render(), "1h1m1s");
    assert_eq!(GoDuration::from_millis(500).render(), "500ms");
    assert_eq!(
        GoDuration::new(Duration::from_millis(1500)).render(),
        "1.5s"
    );
}

// ----- plugin --------------------------------------------------------------

#[test]
fn plugin_list_argv() {
    assert_eq!(
        argv(&PluginCommand::list()),
        vec!["plugin".to_owned(), "list".to_owned()],
    );
}

#[test]
fn plugin_install_argv() {
    assert_eq!(
        argv(&PluginCommand::install("fmt@marketplace")),
        vec![
            "plugin".to_owned(),
            "install".to_owned(),
            "fmt@marketplace".to_owned(),
        ],
    );
}

#[test]
fn plugin_uninstall_argv() {
    assert_eq!(
        argv(&PluginCommand::uninstall("fmt")),
        vec![
            "plugin".to_owned(),
            "uninstall".to_owned(),
            "fmt".to_owned()
        ],
    );
}

#[test]
fn plugin_import_with_source_argv() {
    let command = PluginCommand::new(PluginSubcommand::Import {
        source: Some(ImportSource::Gemini),
    });
    assert_eq!(
        argv(&command),
        vec![
            "plugin".to_owned(),
            "import".to_owned(),
            "gemini".to_owned()
        ],
    );
}

#[test]
fn plugin_import_claude_source_renders_claude() {
    // The import source is a fixed choice set; `ImportSource::Claude` renders
    // the exact `claude` token and no other string is representable.
    let command = PluginCommand::new(PluginSubcommand::Import {
        source: Some(ImportSource::Claude),
    });
    assert_eq!(
        argv(&command),
        vec![
            "plugin".to_owned(),
            "import".to_owned(),
            "claude".to_owned()
        ],
    );
}

#[test]
fn plugin_import_without_source_omits_positional() {
    let command = PluginCommand::new(PluginSubcommand::Import { source: None });
    assert_eq!(
        argv(&command),
        vec!["plugin".to_owned(), "import".to_owned()]
    );
}

#[test]
fn plugin_enable_disable_validate_argv() {
    assert_eq!(
        argv(&PluginCommand::new(PluginSubcommand::Enable {
            name: "fmt".into()
        })),
        vec!["plugin".to_owned(), "enable".to_owned(), "fmt".to_owned()],
    );
    assert_eq!(
        argv(&PluginCommand::new(PluginSubcommand::Disable {
            name: "fmt".into()
        })),
        vec!["plugin".to_owned(), "disable".to_owned(), "fmt".to_owned()],
    );
    assert_eq!(
        argv(&PluginCommand::new(PluginSubcommand::Validate {
            path: Some("./plug".into())
        })),
        vec![
            "plugin".to_owned(),
            "validate".to_owned(),
            "./plug".to_owned(),
        ],
    );
}

#[test]
fn plugin_link_and_help_argv() {
    assert_eq!(
        argv(&PluginCommand::new(PluginSubcommand::Link {
            marketplace: "mp".into(),
            target: "tgt".into(),
        })),
        vec![
            "plugin".to_owned(),
            "link".to_owned(),
            "mp".to_owned(),
            "tgt".to_owned(),
        ],
    );
    assert_eq!(
        argv(&PluginCommand::new(PluginSubcommand::Help)),
        vec!["plugin".to_owned(), "help".to_owned()],
    );
}

// ----- ops subcommands -----------------------------------------------------

#[test]
fn install_argv_with_flags() {
    let command = InstallCommand {
        dir: Some(PathBuf::from("/opt/agy")),
        skip_aliases: true,
        skip_path: true,
    };
    assert_eq!(
        argv(&command),
        vec![
            "install".to_owned(),
            "--dir".to_owned(),
            "/opt/agy".to_owned(),
            "--skip-aliases".to_owned(),
            "--skip-path".to_owned(),
        ],
    );
}

#[test]
fn bare_subcommands_argv() {
    assert_eq!(argv(&ModelsCommand), vec!["models".to_owned()]);
    assert_eq!(argv(&UpdateCommand), vec!["update".to_owned()]);
    assert_eq!(argv(&ChangelogCommand), vec!["changelog".to_owned()]);
    assert_eq!(argv(&HelpCommand::default()), vec!["help".to_owned()]);
    assert_eq!(
        argv(&HelpCommand::for_subcommand("plugin")),
        vec!["help".to_owned(), "plugin".to_owned()],
    );
}

// ----- executor cwd/env ----------------------------------------------------

#[test]
fn to_process_sets_cwd_and_env() {
    let agy = Antigravity::new("agy")
        .with_current_dir("/work")
        .with_env("AGY_X", "1");
    let process = agy.to_process(&RunCommand::print("x"));
    let std = process.as_std();
    assert_eq!(std.get_current_dir(), Some(std::path::Path::new("/work")));
    let has_env = std
        .get_envs()
        .any(|(k, v)| k == std::ffi::OsStr::new("AGY_X") && v == Some(std::ffi::OsStr::new("1")));
    assert!(has_env, "declared env must be applied");
}

#[test]
fn with_env_replaces_an_existing_key() {
    let agy = Antigravity::new("agy")
        .with_env("K", "1")
        .with_env("K", "2");
    let count = agy
        .envs()
        .filter(|(k, _)| *k == std::ffi::OsStr::new("K"))
        .count();
    assert_eq!(count, 1, "the second set must replace, not append");
    let value = agy
        .envs()
        .find(|(k, _)| *k == std::ffi::OsStr::new("K"))
        .map(|(_, v)| v.to_owned());
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
        matches!(
            err,
            crate::Error::Cli {
                exit_code: Some(1),
                ..
            }
        ),
        "got {err:?}",
    );
}

// ----- fake-CLI end to end -------------------------------------------------

#[cfg(unix)]
#[test]
fn execute_returns_output_without_erroring_on_nonzero() {
    let bin = fake_agy("final answer", "", 0);
    let agy = Antigravity::new(bin.path());
    let output = async_runtime()
        .block_on(agy.execute(&RunCommand::print("x")))
        .expect("execute");
    assert_eq!(output.stdout(), "final answer");
    assert!(output.is_success());
}

#[cfg(unix)]
#[test]
fn execute_surfaces_nonzero_as_output_not_error() {
    let bin = fake_agy("", "unknown flag", 2);
    let agy = Antigravity::new(bin.path());
    let output = async_runtime()
        .block_on(agy.execute(&RunCommand::print("x")))
        .expect("execute still succeeds; classification is up to the caller");
    assert_eq!(output.exit(), Exit::Failure(2));
    assert_eq!(output.stderr(), "unknown flag");
}
