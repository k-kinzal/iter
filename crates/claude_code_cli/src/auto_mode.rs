use std::ffi::OsString;

use crate::args::push_opt;

/// `claude auto-mode ...`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AutoMode {
    /// `auto-mode config`.
    Config,
    /// `auto-mode critique`.
    Critique(AutoModeCritique),
    /// `auto-mode defaults`.
    Defaults,
    /// `auto-mode help [command]`.
    Help(Option<AutoModeHelpCommand>),
}

impl AutoMode {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        match self {
            Self::Config => args.push("config".into()),
            Self::Critique(command) => {
                args.push("critique".into());
                command.render(args);
            }
            Self::Defaults => args.push("defaults".into()),
            Self::Help(command) => {
                args.push("help".into());
                if let Some(command) = command {
                    args.push(command.as_str().into());
                }
            }
        }
    }
}

/// `claude auto-mode help`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AutoModeHelpCommand {
    /// `config`.
    Config,
    /// `critique`.
    Critique,
    /// `defaults`.
    Defaults,
}

impl AutoModeHelpCommand {
    #[must_use]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Critique => "critique",
            Self::Defaults => "defaults",
        }
    }
}

/// `claude auto-mode critique`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutoModeCritique {
    /// `--model`.
    pub model: Option<String>,
}

impl AutoModeCritique {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_opt(args, "--model", self.model.as_deref());
    }
}
