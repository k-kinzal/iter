//! `cline auth [PROVIDER]` — authenticate a provider and configure its model.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::args::{ToArgs, push_flag, push_opt, push_opt_path};

/// `cline auth [OPTIONS] [PROVIDER]`.
///
/// The `provider` positional is a shorthand for `-p, --provider`; when both are
/// set the CLI honours its own precedence — this builder renders whatever is
/// populated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthCommand {
    /// `-p, --provider <id>`: provider ID.
    pub provider: Option<String>,
    /// `-k, --apikey <key>`: API key.
    pub apikey: Option<String>,
    /// `-m, --modelid <id>`: model ID.
    pub modelid: Option<String>,
    /// `-b, --baseurl <url>`: base URL.
    pub baseurl: Option<String>,
    /// `--azure-api-version <version>`: Azure API version.
    pub azure_api_version: Option<String>,
    /// `--config <dir>`: configuration directory.
    pub config: Option<PathBuf>,
    /// `-c, --cwd <path>`: working directory.
    pub cwd: Option<PathBuf>,
    /// `--data-dir <dir>`: isolated local state directory (enables sandbox
    /// mode).
    pub data_dir: Option<PathBuf>,
    /// `-v, --verbose`: show verbose output.
    pub verbose: bool,
    /// Optional `[PROVIDER]` positional (shorthand for `-p`).
    pub provider_positional: Option<String>,
}

impl ToArgs for AuthCommand {
    fn write_args(&self, args: &mut Vec<OsString>) {
        args.push("auth".into());
        push_opt(args, "--provider", self.provider.as_deref());
        push_opt(args, "--apikey", self.apikey.as_deref());
        push_opt(args, "--modelid", self.modelid.as_deref());
        push_opt(args, "--baseurl", self.baseurl.as_deref());
        push_opt(
            args,
            "--azure-api-version",
            self.azure_api_version.as_deref(),
        );
        push_opt_path(args, "--config", self.config.as_deref());
        push_opt_path(args, "--cwd", self.cwd.as_deref());
        push_opt_path(args, "--data-dir", self.data_dir.as_deref());
        push_flag(args, self.verbose, "--verbose");
        if let Some(provider) = &self.provider_positional {
            args.push(provider.into());
        }
    }
}
