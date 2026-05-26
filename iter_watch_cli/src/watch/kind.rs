//! Change-event classification for the watch trigger.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use notify::event::EventKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Created,
    Modified,
    Removed,
}

impl ChangeKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Modified => "modified",
            Self::Removed => "removed",
        }
    }

    #[must_use]
    pub fn from_event_kind(kind: EventKind) -> Option<Self> {
        match kind {
            EventKind::Create(_) => Some(Self::Created),
            EventKind::Modify(_) => Some(Self::Modified),
            EventKind::Remove(_) => Some(Self::Removed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ChangeRecord {
    pub(super) path: PathBuf,
    pub(super) kind: ChangeKind,
    pub(super) timestamp: DateTime<Utc>,
}
