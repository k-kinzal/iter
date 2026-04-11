//! Per-path debouncer used by [`WatchTrigger`](super::WatchTrigger) when
//! `per_file = true`.
//!
//! The debouncer remembers the last admitted timestamp for each path. A new
//! event for the same path is admitted only when its timestamp is at least
//! `cooldown` after the previous admit. With every admit we opportunistically
//! prune entries older than `2 * cooldown` so the table cannot grow unbounded.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::time::Instant;

/// Per-path debounce table. `cooldown = None` disables suppression entirely
/// (every event is admitted; the table stays empty).
#[derive(Debug, Default)]
pub(super) struct PathDebouncer {
    last_admit: HashMap<PathBuf, Instant>,
}

impl PathDebouncer {
    /// Returns `true` when the event for `path` at `now` should be emitted.
    /// On `true`, the entry is recorded and stale entries are pruned.
    pub(super) fn admit(&mut self, path: &Path, now: Instant, cooldown: Option<Duration>) -> bool {
        let Some(cd) = cooldown else {
            return true;
        };
        if let Some(last) = self.last_admit.get(path) {
            if now.duration_since(*last) < cd {
                return false;
            }
        }
        self.last_admit.insert(path.to_path_buf(), now);
        let ttl = cd.saturating_mul(2);
        self.last_admit.retain(|_, t| now.duration_since(*t) <= ttl);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[tokio::test(start_paused = true)]
    async fn no_cooldown_admits_everything() {
        let mut d = PathDebouncer::default();
        let p = Path::new("/x");
        let t0 = Instant::now();
        assert!(d.admit(p, t0, None));
        assert!(d.admit(p, t0, None));
        assert!(d.last_admit.is_empty(), "no entry should be recorded");
    }

    #[tokio::test(start_paused = true)]
    async fn within_cooldown_suppresses_repeat() {
        let mut d = PathDebouncer::default();
        let p = Path::new("/x");
        let t0 = Instant::now();
        let cd = Duration::from_secs(5);
        assert!(d.admit(p, t0, Some(cd)));
        // Second event 1s later — within cooldown.
        let t1 = t0 + Duration::from_secs(1);
        assert!(!d.admit(p, t1, Some(cd)));
    }

    #[tokio::test(start_paused = true)]
    async fn after_cooldown_admits_again() {
        let mut d = PathDebouncer::default();
        let p = Path::new("/x");
        let t0 = Instant::now();
        let cd = Duration::from_secs(5);
        assert!(d.admit(p, t0, Some(cd)));
        let t_late = t0 + cd + Duration::from_millis(1);
        assert!(d.admit(p, t_late, Some(cd)));
    }

    #[tokio::test(start_paused = true)]
    async fn distinct_paths_are_independent() {
        let mut d = PathDebouncer::default();
        let t0 = Instant::now();
        let cd = Duration::from_secs(5);
        assert!(d.admit(Path::new("/a"), t0, Some(cd)));
        assert!(d.admit(Path::new("/b"), t0, Some(cd)));
    }

    #[tokio::test(start_paused = true)]
    async fn prune_drops_stale_entries() {
        let mut d = PathDebouncer::default();
        let cd = Duration::from_secs(1);
        let t0 = Instant::now();
        d.admit(Path::new("/old"), t0, Some(cd));
        // Admit a fresh path far in the future — old entry should be pruned.
        let t_future = t0 + Duration::from_secs(10);
        d.admit(Path::new("/new"), t_future, Some(cd));
        assert!(!d.last_admit.contains_key(Path::new("/old")));
        assert!(d.last_admit.contains_key(Path::new("/new")));
    }
}
