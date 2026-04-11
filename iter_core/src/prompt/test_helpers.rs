//! Shared test helpers for the prompt submodules.

use crate::signal::Signal;
use crate::signal::metadata::{Metadata, MetadataKey, MetadataValue};

use super::guard::PromptGuard;

pub(super) fn signal_with(metadata: Metadata) -> Signal {
    Signal::new(metadata)
}

pub(super) fn signal_with_kind(kind: &str) -> Signal {
    let mut meta = Metadata::new();
    meta.insert(
        MetadataKey::new("kind").unwrap(),
        MetadataValue::String(kind.into()),
    );
    signal_with(meta)
}

pub(super) fn guard_kind_eq(value: &str) -> PromptGuard {
    PromptGuard::MetadataEq {
        key: "kind".into(),
        value: value.into(),
    }
}
