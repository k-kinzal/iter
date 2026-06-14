#![deny(rust_2018_idioms)]

pub mod watch;

pub use watch::{ChangeKind, WatchBackend, WatchConfig, WatchTrigger, WatchTriggerError};
