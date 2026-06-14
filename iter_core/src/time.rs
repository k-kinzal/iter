//! Runtime ports for nondeterministic time and identifier production.
//!
//! Core code depends on these narrow traits instead of reaching directly for
//! wall-clock time or fresh identifiers. Production callers use the system
//! implementations; tests can inject fixed clocks and deterministic id
//! sequences.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::signal::SignalId;

/// Source of wall-clock time.
pub trait Clock: fmt::Debug + Send + Sync {
    /// Return the current UTC wall-clock instant.
    fn now(&self) -> DateTime<Utc>;

    /// Return the current [`SystemTime`] for filesystem and queue ordering.
    fn system_time(&self) -> SystemTime;
}

/// System-backed [`Clock`].
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        DateTime::<Utc>::from(self.system_time())
    }

    fn system_time(&self) -> SystemTime {
        let elapsed = UNIX_EPOCH.elapsed().map_or(Duration::ZERO, |d| d);
        UNIX_EPOCH + elapsed
    }
}

/// Fixed [`Clock`] for deterministic tests.
#[derive(Debug, Clone)]
pub struct FixedClock {
    now: DateTime<Utc>,
    system_time: SystemTime,
}

impl FixedClock {
    /// Create a fixed clock from a UTC timestamp.
    #[must_use]
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            now,
            system_time: now.into(),
        }
    }

    /// Create a fixed clock from a [`SystemTime`].
    #[must_use]
    pub fn from_system_time(system_time: SystemTime) -> Self {
        Self {
            now: DateTime::<Utc>::from(system_time),
            system_time,
        }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.now
    }

    fn system_time(&self) -> SystemTime {
        self.system_time
    }
}

/// Source of fresh runtime identifiers.
pub trait IdSource: fmt::Debug + Send + Sync {
    /// Return a new signal identifier.
    fn new_id(&self) -> SignalId;
}

/// System-backed [`IdSource`].
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemIdSource;

impl IdSource for SystemIdSource {
    fn new_id(&self) -> SignalId {
        SignalId::from_uuid(Uuid::now_v7())
    }
}

/// Deterministic [`IdSource`] for tests.
#[derive(Debug)]
pub struct DeterministicIdSource {
    next: AtomicU64,
}

impl DeterministicIdSource {
    /// Create a deterministic id source whose first id is derived from `start`.
    #[must_use]
    pub const fn new(start: u64) -> Self {
        Self {
            next: AtomicU64::new(start),
        }
    }
}

impl Default for DeterministicIdSource {
    fn default() -> Self {
        Self::new(1)
    }
}

impl IdSource for DeterministicIdSource {
    fn new_id(&self) -> SignalId {
        let raw = self.next.fetch_add(1, Ordering::Relaxed);
        SignalId::from_uuid(Uuid::from_u128(u128::from(raw)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_clock_returns_pinned_times() {
        let system_time = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let clock = FixedClock::from_system_time(system_time);

        assert_eq!(clock.system_time(), system_time);
        assert_eq!(clock.now(), DateTime::<Utc>::from(system_time));
    }

    #[test]
    fn deterministic_id_source_is_predictable() {
        let ids = DeterministicIdSource::new(7);

        assert_eq!(ids.new_id(), SignalId::from_uuid(Uuid::from_u128(7)));
        assert_eq!(ids.new_id(), SignalId::from_uuid(Uuid::from_u128(8)));
    }
}
