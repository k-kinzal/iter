#![deny(rust_2018_idioms)]
#![allow(unreachable_pub)]

pub mod command;

pub use command::{CommandTrigger, CommandTriggerError, ExtractMode, OnError};
