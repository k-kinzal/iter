//! Runner-scoped dynamic variables published by lifecycle actions.
//!
//! A [`VariableStore`] is shared by the Runner's prompt renderer and its
//! operator-provided event actions. Actions publish fully-formed JSON-shaped
//! values after they complete; subsequent actions and prompt renders observe
//! a point-in-time [`VariableSnapshot`] below the `var.*` template root.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use serde::Serialize;
use serde_json::Value;

/// Immutable template-render snapshot of all currently published variables.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(transparent)]
pub struct VariableSnapshot(BTreeMap<String, Value>);

impl VariableSnapshot {
    /// Borrow one variable by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    /// Returns `true` when no variables have been published.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Cloneable Runner-scoped store backing the `var.*` template root.
#[derive(Debug, Clone, Default)]
pub struct VariableStore {
    inner: Arc<RwLock<BTreeMap<String, Value>>>,
}

impl VariableStore {
    /// Construct an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Capture an immutable point-in-time view for one template render.
    #[must_use]
    pub fn snapshot(&self) -> VariableSnapshot {
        VariableSnapshot(self.read().clone())
    }

    /// Clone one currently published variable.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Value> {
        self.read().get(name).cloned()
    }

    /// Publish or replace one variable atomically.
    pub fn set(&self, name: impl Into<String>, value: Value) {
        self.write().insert(name.into(), value);
    }

    /// Publish several variables under one write lock.
    ///
    /// A template snapshot observes either the old set or the complete new
    /// set, never a partially published multi-capture shell action.
    pub fn set_many(&self, values: impl IntoIterator<Item = (String, Value)>) {
        self.write().extend(values);
    }

    fn read(&self) -> RwLockReadGuard<'_, BTreeMap<String, Value>> {
        match self.inner.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn write(&self) -> RwLockWriteGuard<'_, BTreeMap<String, Value>> {
        match self.inner.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn snapshots_are_point_in_time_views() {
        let store = VariableStore::new();
        store.set("context", json!({"value": 1}));
        let first = store.snapshot();
        store.set("context", json!({"value": 2}));

        assert_eq!(first.get("context"), Some(&json!({"value": 1})));
        assert_eq!(store.snapshot().get("context"), Some(&json!({"value": 2})));
    }

    #[test]
    fn set_many_publishes_every_value() {
        let store = VariableStore::new();
        store.set_many([
            ("stdout".to_owned(), json!({"text": "out"})),
            ("stderr".to_owned(), json!({"text": "err"})),
        ]);

        let snapshot = store.snapshot();
        assert_eq!(snapshot.get("stdout"), Some(&json!({"text": "out"})));
        assert_eq!(snapshot.get("stderr"), Some(&json!({"text": "err"})));
    }
}
