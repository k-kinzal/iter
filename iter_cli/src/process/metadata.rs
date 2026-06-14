//! `meta.json` schema for `~/.iter/proc/<id>/`.
//!
//! Holds everything `iter ps` / `iter inspect` needs to render a row without
//! reaching for any other side-file: name, Iterfile path, subcommand verb,
//! started-at, full argv, env overrides, and the `--debug` flag. The current
//! lifecycle status lives in `<dir>/status` (not here) so it can be updated
//! atomically without rewriting the JSON document.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::process::id::ProcessId;

/// Full deserialised contents of `<id>/meta.json`.
///
/// The file is written exactly once at session creation; subsequent updates
/// happen by writing siblings (`status`, `pid`) rather than rewriting the
/// JSON. This keeps the metadata path read-only on the hot path of
/// `iter ps`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProcessMetadata {
    /// Owner id (also the directory name).
    pub(crate) id: ProcessId,
    /// Human-friendly registered name.
    pub(crate) name: String,
    /// Absolute path to the `Iterfile` that was loaded.
    pub(crate) iterfile: PathBuf,
    /// CLI subcommand verb (`"run"`, `"compose up"`, …).
    pub(crate) subcommand: String,
    /// Wall-clock time at which the session was created (the moment
    /// `<dir>/status` first contained `initializing`).
    pub(crate) started_at: DateTime<Utc>,
    /// Full argv handed to the child process, *excluding* `argv[0]`.
    pub(crate) args: Vec<String>,
    /// Environment overrides merged on top of the inherited environment at
    /// spawn time. Sensitive values are NOT redacted at this layer; callers
    /// passing secrets are expected to filter before construction.
    pub(crate) env: Vec<(String, String)>,
    /// `--debug` was active for this session.
    pub(crate) debug: bool,
    /// Parent process id for child records spawned by an orchestrator
    /// (e.g. `iter compose up` spawning services / triggers). `None` for
    /// top-level invocations. `#[serde(default)]` so existing on-disk
    /// `meta.json` files without this field continue to deserialise.
    #[serde(default)]
    pub(crate) parent_id: Option<ProcessId>,
    /// Free-form labels attached to the process at registration time.
    ///
    /// Keys in the `iter.<feature>.<key>` namespace are reserved for
    /// internal use (e.g. compose stores `iter.compose.project`,
    /// `iter.compose.service`, `iter.compose.orchestrator_pid`,
    /// `iter.compose.orchestrator_start_time`). Anything outside the
    /// `iter.*` namespace is available to user-side tooling.
    /// `#[serde(default)]` keeps older `meta.json` files (which do not
    /// have this field) loadable.
    #[serde(default)]
    pub(crate) labels: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_round_trips_through_json() {
        let id = ProcessId::generate();
        let mut labels = BTreeMap::new();
        labels.insert("iter.compose.project".into(), "demo".into());
        labels.insert("iter.compose.service".into(), "worker".into());
        let meta = ProcessMetadata {
            id,
            name: "alpha".into(),
            iterfile: PathBuf::from("/tmp/Iterfile"),
            subcommand: "run".into(),
            started_at: Utc::now(),
            args: vec!["run".into(), "--debug".into()],
            env: vec![("FOO".into(), "bar".into())],
            debug: true,
            parent_id: None,
            labels,
        };
        let bytes = serde_json::to_vec(&meta).expect("serialize");
        let parsed: ProcessMetadata = serde_json::from_slice(&bytes).expect("deserialize");
        assert_eq!(parsed, meta);
    }

    #[test]
    fn metadata_without_labels_field_still_deserialises() {
        // Older meta.json files written before the labels field existed
        // must continue to load (defaults to an empty BTreeMap).
        let id = ProcessId::generate();
        let json = serde_json::json!({
            "id": id.to_string(),
            "name": "legacy",
            "iterfile": "/tmp/Iterfile",
            "subcommand": "run",
            "started_at": Utc::now().to_rfc3339(),
            "args": ["run"],
            "env": [],
            "debug": false,
        });
        let meta: ProcessMetadata = serde_json::from_value(json).expect("legacy meta");
        assert!(meta.labels.is_empty());
        assert!(meta.parent_id.is_none());
    }
}
