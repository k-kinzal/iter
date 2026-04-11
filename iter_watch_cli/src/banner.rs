//! `--name` and `--quiet` — the two flags that govern the trigger's
//! presentation in startup / shutdown banners.
//!
//! `--name` is the identifier embedded in banners and signal metadata
//! (defaults to `<binary>#<pid>` when absent). `--quiet` suppresses the
//! banners themselves; tracing output is unaffected.

use clap::Args;

#[derive(Debug, Args)]
pub(crate) struct BannerArgs {
    /// Identifier for this trigger instance, recorded in logs and signal
    /// metadata.
    #[arg(long, value_name = "NAME")]
    pub(crate) name: Option<String>,

    /// Suppress the startup / shutdown banner on stderr. Tracing output
    /// is unaffected — control verbosity with `--log-level`.
    #[arg(short = 'q', long = "quiet", default_value_t = false)]
    pub(crate) quiet: bool,
}

impl BannerArgs {
    #[must_use]
    pub(crate) fn instance_name(&self, binary: &str) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("{binary}#{}", std::process::id()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct Probe {
        #[command(flatten)]
        args: BannerArgs,
    }

    #[test]
    fn instance_name_falls_back_to_binary_and_pid() {
        let probe = Probe::parse_from(["probe"]);
        let name = probe.args.instance_name("iter-cron");
        assert!(name.starts_with("iter-cron#"));
    }

    #[test]
    fn quiet_defaults_to_false() {
        let probe = Probe::parse_from(["probe"]);
        assert!(!probe.args.quiet);
    }

    #[test]
    fn quiet_parses() {
        let probe = Probe::parse_from(["probe", "--quiet"]);
        assert!(probe.args.quiet);
    }
}
