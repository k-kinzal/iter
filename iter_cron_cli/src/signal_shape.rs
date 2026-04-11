//! `--priority` and `--metadata` — the two flags that shape every
//! signal this trigger publishes.
//!
//! `--priority` is a closed enum mapped to [`iter_core::Priority`].
//! `--metadata KEY=VALUE` accepts repeated entries and parses into a
//! [`Metadata`] map (or a `Vec<(MetadataKey, String)>` when the caller
//! needs the pairs directly).

use clap::{Args, ValueEnum};
use iter_core::{Metadata, MetadataKey, MetadataValue, Priority};
use thiserror::Error;

use crate::error::{IntoExitCode, exit_codes};

#[derive(Debug, Error)]
pub(crate) enum MetadataParseError {
    #[error("--metadata expects KEY=VALUE, got `{0}`")]
    MissingEquals(String),
    #[error("invalid metadata key `{0}`")]
    InvalidKey(String),
}

impl IntoExitCode for MetadataParseError {
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
pub(crate) struct SignalShapeArgs {
    /// Priority assigned to every emitted signal.
    #[arg(long, value_enum, default_value_t = PriorityArg::Normal)]
    pub(crate) priority: PriorityArg,

    /// Static `KEY=VALUE` metadata attached to every emitted signal.
    /// May be repeated.
    #[arg(long = "metadata", value_name = "KEY=VALUE")]
    pub(crate) metadata: Vec<String>,
}

impl SignalShapeArgs {
    #[allow(dead_code)]
    #[must_use]
    pub(crate) fn priority_value(&self) -> Priority {
        self.priority.into_priority()
    }

    #[allow(dead_code)]
    pub(crate) fn base_metadata(&self) -> Result<Metadata, MetadataParseError> {
        let mut out = Metadata::new();
        for (key, value) in self.base_metadata_pairs()? {
            out.insert(key, MetadataValue::String(value));
        }
        Ok(out)
    }

    pub(crate) fn base_metadata_pairs(
        &self,
    ) -> Result<Vec<(MetadataKey, String)>, MetadataParseError> {
        let mut out = Vec::with_capacity(self.metadata.len());
        for entry in &self.metadata {
            let (k, v) = entry
                .split_once('=')
                .ok_or_else(|| MetadataParseError::MissingEquals(entry.clone()))?;
            let key =
                MetadataKey::new(k).map_err(|_| MetadataParseError::InvalidKey(k.to_owned()))?;
            out.push((key, v.to_owned()));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct Probe {
        #[command(flatten)]
        args: SignalShapeArgs,
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
