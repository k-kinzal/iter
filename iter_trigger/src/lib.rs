//! SDK for iter trigger CLIs — Queue connection and Signal emission.
//!
//! This crate provides the shared knowledge that every trigger CLI needs:
//! "connect to an iter Queue and emit Signals into it." It is an SDK, not
//! a framework — it does not own `main()`, does not provide Clap definitions,
//! and does not control the execution loop. Each CLI owns its execution and
//! calls into this SDK.
//!
//! # Key types
//!
//! - [`Trigger`] — holds a queue handle, emits signals, tracks count.
//! - [`TriggerConfig`] — signal defaults and termination policy.
//! - [`QueueLoader`] — connects to a queue by URL.
//! - [`TriggerEvent`] — per-emission metadata builder.
//!
//! # Non-dependencies
//!
//! This crate depends on `iter_core` (for queue drivers) but does **not**
//! depend on `iter_language` or `iter_compose`. The compose layer resolves
//! declarations and passes resolved URLs to trigger CLIs.

mod counting_queue;
mod queue_loader;
pub mod shutdown;
mod trigger;

pub use counting_queue::CountingQueue;
pub use queue_loader::{QueueHandle, QueueHandleError, QueueLoadError, QueueLoader};
pub use shutdown::{ShutdownError, install_shutdown_handler};
pub use trigger::{EmitError, Trigger, TriggerConfig, TriggerEvent};

pub use iter_core::queue::{self, Priority, Queue};
pub use iter_core::signal::{Metadata, MetadataKey, MetadataValue, Signal, SignalId, SignalKind};
