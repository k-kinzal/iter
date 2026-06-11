//! The Queue boundary — the channel that carries [`Signal`](crate::Signal)s
//! into a Runner, and the contract every Signal source writes against.
//!
//! # The boundary contract
//!
//! A **Queue** is the channel a Signal crosses on its way into a Runner. The
//! contract a Signal source connects through *is this boundary itself*, made
//! of three pieces that live here:
//!
//! - the **[`Envelope`]** — a Signal's serialized crossing form on a Queue
//!   (a different concept from a process log line; they share an encoding but
//!   never a home);
//! - the **[`QueueDescriptor`]** — everything another process needs to
//!   *connect* to a Queue (backend + address + resolved params + a
//!   resolved-credential slot), turned into a usable queue by [`connect`];
//! - **enqueue semantics**, including the Trigger's **emission budget**
//!   ([`BudgetedQueue`] — "stop after N, then close to drain").
//!
//! The [`Queue`] trait is dyn-compatible: the runtime queue is always
//! `Arc<dyn Queue>` — whichever backend the definition chose, usable as a
//! Queue. The *closed* set of declarable backends lives at the definition
//! layer (the grammar layer), not here; at run time the boundary is open
//! behavior.
//!
//! # Backends
//!
//! Address/descriptor-connectable backends:
//!
//! - [`drivers::memory::InMemoryQueue`] — single-process default backed by a
//!   `BinaryHeap` and [`tokio::sync::Notify`] (`memory://`, in-process only).
//! - [`drivers::file::FileQueue`] — persistent directory-of-files queue using
//!   POSIX atomic `rename(2)` (`file://`).
//! - `drivers::redis::RedisQueue` — Redis sorted-set queue (`redis://`,
//!   feature `driver-redis`).
//! - `drivers::sqs::SqsQueue` — Amazon SQS (feature `driver-sqs`).
//!
//! Not descriptor-connectable — made only from the full definition:
//!
//! - [`drivers::shell::ShellQueue`] — escape hatch where users supply
//!   `enqueue` / `dequeue` shell commands. It needs the scripts, so it can
//!   only be built from the queue declaration, never connected from a
//!   [`QueueDescriptor`].

pub mod inner;

pub use inner::Queue;

pub mod error;

pub use error::QueueError;

pub mod drivers;

// ─── Re-exports for ergonomic access ────────────────────────────────────

pub use drivers::file::{FileQueue, FileQueueError};
pub use drivers::memory::{InMemoryQueue, InMemoryQueueError};
pub use drivers::shell::{ShellQueue, ShellQueueConfig, ShellQueueError};

#[cfg(feature = "driver-redis")]
pub use drivers::redis::{RedisQueue, RedisQueueError};

#[cfg(feature = "driver-sqs")]
pub use drivers::aws;

#[cfg(feature = "driver-sqs")]
pub use drivers::sqs;

// ─── Shared cross-backend types ─────────────────────────────────────────

pub mod priority;

pub use priority::Priority;

pub mod metadata_source;

pub use metadata_source::{MetadataSource, MissingMetadata};

pub mod envelope;

pub use envelope::{Envelope, EnvelopeError, decode_signal, encode_signal};

pub mod retry;

pub use retry::{RetryMode, RetryPolicy};

pub mod dlq;

pub use dlq::{DlqKind, DlqPolicy, DlqTarget};

// ─── The boundary contract ──────────────────────────────────────────────

pub mod address;

pub use address::{QueueAddress, QueueAddressError};

pub mod descriptor;

pub use descriptor::{QueueDescriptor, ResolvedQueueCredentials, SqsDescriptor};

pub mod connect;

pub use connect::{ConnectError, connect};

pub mod budgeted;

pub use budgeted::BudgetedQueue;
