//! `--max-signals` and `--shutdown-timeout` — the two flags that
//! decide when the trigger stops.
//!
//! `--max-signals 0` (the default) means "no limit"; the trigger runs
//! until SIGTERM. `--shutdown-timeout` is the grace window granted to
//! in-flight work after SIGTERM before the process force-exits.

use std::time::Duration;

use clap::Args;

#[derive(Debug, Args)]
pub(crate) struct TerminationArgs {
    /// Stop after emitting `N` signals. `0` (the default) means "no limit".
    #[arg(long = "max-signals", value_name = "N", default_value_t = 0)]
    pub(crate) max_signals: u64,

    /// Seconds to wait for a graceful shutdown after SIGTERM.
    #[arg(long = "shutdown-timeout", value_name = "SECS", default_value_t = 10)]
    pub(crate) shutdown_timeout_secs: u64,
}

impl TerminationArgs {
    #[must_use]
    pub(crate) fn shutdown_timeout(&self) -> Duration {
        Duration::from_secs(self.shutdown_timeout_secs)
    }
}
