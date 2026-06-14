#![deny(rust_2018_idioms)]

pub mod command;

pub use command::{CommandTrigger, CommandTriggerError, ExtractMode, OnError};
