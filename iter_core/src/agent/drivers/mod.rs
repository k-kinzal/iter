//! Agent driver implementations.
//!
//! Each subdirectory is a self-contained driver that implements
//! [`crate::agent::Agent`]. All drivers are currently process-based
//! (no external SDK dependencies) and always compiled.

pub mod antigravity;
pub mod claude;
pub mod cline;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod gemini;
pub mod generic;
pub mod hermes;
pub mod opencode;
