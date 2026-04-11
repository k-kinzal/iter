#![deny(rust_2018_idioms)]
#![allow(unreachable_pub)]

pub mod files_trigger;

pub use files_trigger::{FilesSource, FilesTrigger, FilesTriggerError};
