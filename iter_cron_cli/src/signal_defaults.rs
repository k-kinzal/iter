//! `--priority` and `--metadata` — the two flags that shape every signal this
//! trigger publishes: the operator (clap) surface for the core
//! [`iter_core::signal::defaults`] helper.
//!
//! `--priority` is a closed enum mapped to [`iter_core::Priority`].
//! `--metadata KEY=VALUE` accepts repeated entries; the `KEY=VALUE` parsing
//! lives in core ([`base_metadata`]/[`parse_metadata_pairs`]) so the five
//! trigger binaries do not re-spell it.

use clap::{Args, ValueEnum};
use iter_core::Priority;
use iter_core::signal::defaults::{MetadataPairError, base_metadata, parse_metadata_pairs};
use iter_core::{Metadata, MetadataKey};

use crate::error::{IntoExitCode, exit_codes};

impl IntoExitCode for MetadataPairError {
    fn exit_code(&self) -> i32 {
        exit_codes::USER_INPUT
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum PriorityArg {
    Low,
    Normal,
    High,
    Critical,
}

impl PriorityArg {
    #[allow(dead_code)]
    fn into_priority(self) -> Priority {
        match self {
            Self::Low => Priority::LOW,
            Self::Normal => Priority::NORMAL,
            Self::High => Priority::HIGH,
            Self::Critical => Priority::CRITICAL,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct SignalDefaultsArgs {
    /// Priority assigned to every emitted signal.
    #[arg(long, value_enum, default_value_t = PriorityArg::Normal)]
    pub(crate) priority: PriorityArg,

    /// Static `KEY=VALUE` metadata attached to every emitted signal.
    /// May be repeated.
    #[arg(long = "metadata", value_name = "KEY=VALUE")]
    pub(crate) metadata: Vec<String>,
}

impl SignalDefaultsArgs {
    #[allow(dead_code)]
    #[must_use]
    pub(crate) fn priority_value(&self) -> Priority {
        self.priority.into_priority()
    }

    #[allow(dead_code)]
    pub(crate) fn base_metadata(&self) -> Result<Metadata, MetadataPairError> {
        base_metadata(&self.metadata)
    }

    #[allow(dead_code)]
    pub(crate) fn base_metadata_pairs(
        &self,
    ) -> Result<Vec<(MetadataKey, String)>, MetadataPairError> {
        parse_metadata_pairs(&self.metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use iter_core::MetadataValue;

    #[derive(Debug, Parser)]
    struct Probe {
        #[command(flatten)]
        args: SignalDefaultsArgs,
    }

    #[test]
    fn parses_metadata_pairs() {
        let probe = Probe::parse_from([
            "probe",
            "--metadata",
            "source=manual",
            "--metadata",
            "tag=alpha",
        ]);
        let meta = probe.args.base_metadata().expect("metadata");
        let key = MetadataKey::new("source").expect("key");
        assert_eq!(
            meta.get(&key),
            Some(&MetadataValue::String("manual".into()))
        );
    }

    #[test]
    fn metadata_without_equals_errors() {
        let probe = Probe::parse_from(["probe", "--metadata", "no-eq-here"]);
        let err = probe.args.base_metadata().expect_err("must fail");
        assert!(err.to_string().contains("KEY=VALUE"));
    }

    #[test]
    fn priority_default_is_normal() {
        let probe = Probe::parse_from(["probe"]);
        assert_eq!(probe.args.priority_value(), Priority::NORMAL);
    }

    #[test]
    fn priority_high_resolves() {
        let probe = Probe::parse_from(["probe", "--priority", "high"]);
        assert_eq!(probe.args.priority_value(), Priority::HIGH);
    }
}
