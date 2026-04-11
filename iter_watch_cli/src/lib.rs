#![deny(rust_2018_idioms)]
#![allow(unreachable_pub)]

pub mod watch;

pub use watch::{WatchBackend, WatchConfig, WatchTrigger, WatchTriggerError};
