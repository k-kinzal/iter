//! Hook installation for [`HermesAgent`](super::HermesAgent)'s
//! interactive mode.
//!
//! Hermes has a rich JSON-based hook system with 14+ event types
//! (`pre_tool_call`, `post_tool_call`, `on_session_end`, etc.).
//! For output capture iter only needs `on_session_end`, but the
//! basic driver must be proven first.
//!
//! This module is intentionally empty. Implement a `HookBundle` here
//! once the basic driver is stable and hook-based output capture is
//! needed.
