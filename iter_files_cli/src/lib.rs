#![deny(rust_2018_idioms)]

pub mod files_trigger;

pub use files_trigger::{FilesSource, FilesTrigger, FilesTriggerError};
