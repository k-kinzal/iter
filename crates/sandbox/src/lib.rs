//! Cross-platform sandbox policy applied to process commands.
//!
//! Public types describe sandbox intent. Platform-specific command wrapping is
//! a private implementation detail selected by the compilation target.
#![doc = include_str!("../README.md")]

mod error;
mod platform;
mod policy;
pub mod std;
pub mod tokio;

pub use error::Error;
pub use policy::{EnvironmentPolicy, FilesystemPolicy, NetworkPolicy, Policy, ProcessPolicy};
