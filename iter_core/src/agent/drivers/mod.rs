//! Agent driver implementations.
//!
//! Each subdirectory is a self-contained driver that implements
//! [`crate::agent::AgentDriver`] — one CLI, one bidirectional translator.
//! All drivers are process-based and always compiled.

pub mod antigravity;
pub mod claude_code;
pub mod cline;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod fake;
pub mod gemini;
pub mod generic;
pub mod grok;
pub mod hermes;
pub mod noop;
pub mod opencode;
