//! `gemini extensions` — manage Gemini CLI extensions (alias `extension`).
//!
//! The leaf shapes are modeled from each leaf's own `gemini extensions <leaf>
//! --help`: typed positionals plus the documented per-leaf flags, including the
//! fixed-choice `list -o/--output-format`, `config --scope`, and the `new`
//! boilerplate `template`.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{ToArgs, push_enum, push_flag, push_opt};
use crate::values::{ExtensionTemplate, ExtensionsOutputFormat, Scope};

/// `gemini extensions [OPTIONS] <COMMAND>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionsCommand {
    /// `-d, --debug`.
    pub debug: bool,
    /// The extensions subcommand.
    pub command: ExtensionsSubcommand,
}

impl ExtensionsCommand {
    /// Wrap an extensions subcommand with default options.
    #[must_use]
    pub fn new(command: ExtensionsSubcommand) -> Self {
        Self {
            debug: false,
            command,
        }
    }
}

impl ToArgs for ExtensionsCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("extensions".into());
        push_flag(args, self.debug, "--debug");
        self.command.render(args);
    }
}

/// A `gemini extensions` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExtensionsSubcommand {
    /// `extensions install <SOURCE> [OPTIONS]`.
    Install {
        /// Git repository URL or local path.
        source: String,
        /// `--ref <REF>`: the git ref to install from.
        git_ref: Option<String>,
        /// `--auto-update`.
        auto_update: bool,
        /// `--pre-release`.
        pre_release: bool,
        /// `--consent`: acknowledge install risks and skip the prompt.
        consent: bool,
        /// `--skip-settings`: skip the configuration-on-install process.
        skip_settings: bool,
    },
    /// `extensions uninstall [NAMES]... [--all]`.
    Uninstall {
        /// Extension names to uninstall.
        names: Vec<String>,
        /// `--all`: uninstall all installed extensions.
        all: bool,
    },
    /// `extensions list [-o <FORMAT>]`.
    List {
        /// `-o, --output-format <FORMAT>`.
        output_format: Option<ExtensionsOutputFormat>,
    },
    /// `extensions update [<NAME>] [--all]`.
    Update {
        /// Optional extension name (omitted with `--all`).
        name: Option<String>,
        /// `--all`.
        all: bool,
    },
    /// `extensions disable [--scope <SCOPE>] <NAME>`.
    Disable {
        /// Extension name.
        name: String,
        /// `--scope <SCOPE>` (free-form; no fixed choices).
        scope: Option<String>,
    },
    /// `extensions enable [--scope <SCOPE>] <NAME>`.
    Enable {
        /// Extension name.
        name: String,
        /// `--scope <SCOPE>` (free-form; no fixed choices).
        scope: Option<String>,
    },
    /// `extensions link <PATH> [--consent]`.
    Link {
        /// Local path to link.
        path: PathBuf,
        /// `--consent`: acknowledge install risks and skip the prompt.
        consent: bool,
    },
    /// `extensions new <PATH> [TEMPLATE]`.
    New {
        /// Destination path.
        path: PathBuf,
        /// Optional boilerplate template name.
        template: Option<ExtensionTemplate>,
    },
    /// `extensions validate <PATH>`.
    Validate {
        /// Local path to validate.
        path: PathBuf,
    },
    /// `extensions config [NAME] [SETTING] [--scope <SCOPE>]`.
    Config {
        /// Optional extension name.
        name: Option<String>,
        /// Optional setting key.
        setting: Option<String>,
        /// `--scope <SCOPE>`.
        scope: Option<Scope>,
    },
}

impl ExtensionsSubcommand {
    fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Install {
                source,
                git_ref,
                auto_update,
                pre_release,
                consent,
                skip_settings,
            } => {
                args.push("install".into());
                args.push(source.into());
                push_opt(args, "--ref", git_ref.as_deref());
                push_flag(args, *auto_update, "--auto-update");
                push_flag(args, *pre_release, "--pre-release");
                push_flag(args, *consent, "--consent");
                push_flag(args, *skip_settings, "--skip-settings");
            }
            Self::Uninstall { names, all } => {
                args.push("uninstall".into());
                push_flag(args, *all, "--all");
                args.extend(names.iter().map(OsString::from));
            }
            Self::List { output_format } => {
                args.push("list".into());
                push_enum(
                    args,
                    "--output-format",
                    output_format.map(ExtensionsOutputFormat::as_str),
                );
            }
            Self::Update { name, all } => {
                args.push("update".into());
                push_flag(args, *all, "--all");
                if let Some(name) = name {
                    args.push(name.into());
                }
            }
            Self::Disable { name, scope } => {
                args.push("disable".into());
                push_opt(args, "--scope", scope.as_deref());
                args.push(name.into());
            }
            Self::Enable { name, scope } => {
                args.push("enable".into());
                push_opt(args, "--scope", scope.as_deref());
                args.push(name.into());
            }
            Self::Link { path, consent } => {
                args.push("link".into());
                args.push(path.into());
                push_flag(args, *consent, "--consent");
            }
            Self::New { path, template } => {
                args.push("new".into());
                args.push(path.into());
                if let Some(template) = template {
                    args.push(template.as_str().into());
                }
            }
            Self::Validate { path } => {
                args.push("validate".into());
                args.push(path.into());
            }
            Self::Config {
                name,
                setting,
                scope,
            } => {
                args.push("config".into());
                push_enum(args, "--scope", scope.map(Scope::as_str));
                if let Some(name) = name {
                    args.push(name.into());
                }
                if let Some(setting) = setting {
                    args.push(setting.into());
                }
            }
        }
    }
}
