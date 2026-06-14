//! `process` — the CLI-owned on-disk view of a running iter process.
//!
//! A process record is the durable control-plane view of one running
//! runner. The registry owns record creation, status files expose lifecycle
//! state, pid files bind records back to OS processes, and shutdown/adoption
//! code treats those files as the synchronization boundary between parent
//! and child processes.
//!
//! This is the `iter ps` / `iter inspect` record layer. It is distinct from
//! `iter_core::process_group`, the generic OS-process-tree primitive used by
//! core agent and queue drivers.
//!
//! Layering (current set; later phases plug in on top):
//!
//! ```text
//!   error  status  id            <- leaves
//!   paths  proc_info  metadata   <- depend on leaves
//!   pid_file                     <- depends on paths + proc_info
//! ```
//!
//! Public re-exports are kept narrow so the rest of the crate (and the CLI)
//! can refer to `crate::process::ProcessId`, `crate::process::ProcessStatus`,
//! `crate::process::ProcessError`, etc., without reaching into each module
//! by name.

// Internal primitives. These modules implement crate-internal invariants
// (flock-protected status writes, name locks, adoption tokens, adoption
// glue, session caches) and intentionally do not appear in the public
// API surface. Items that *should* be public are re-exported individually
// via `pub use` below.
pub(crate) mod adoption;
pub(crate) mod adoption_token;
pub(crate) mod name_lock;
pub(crate) mod session;
pub(crate) mod status_file;

// Public-facing modules. Each one carries a focused responsibility and is
// addressable as `crate::process::<module>` for callers that prefer the
// inner path over the flat re-exports below.
pub(crate) mod error;
pub(crate) mod handle;
pub(crate) mod id;
pub(crate) mod interrupt;
pub(crate) mod lifetime_lock;
pub(crate) mod log;
pub(crate) mod metadata;
pub(crate) mod observer;
pub(crate) mod paths;
pub(crate) mod pid_file;
pub(crate) mod posix_signal;
pub(crate) mod proc_info;
pub(crate) mod record;
pub(crate) mod registry;
pub(crate) mod runtime;
pub(crate) mod spawner;
pub(crate) mod status;

pub(crate) use adoption::adopt_from_argv;
pub(crate) use error::{
    AdoptError, LockedSectionError, ObserverError, ProcessError, RegistryError,
    SecondaryStatusWrite, StartupError, TokenCorruptKind,
};
pub(crate) use handle::{BOOTSTRAP_GRACE_ENV, ProcessHandle, bootstrap_grace};
pub(crate) use id::{AdoptionToken, Pid, ProcessId};
pub(crate) use interrupt::ShutdownIntent;
pub(crate) use lifetime_lock::LifetimeLock;
pub(crate) use log::{
    DEFAULT_LOG_BUFFER, LogSender, OutputPolicy, ProcessLogSink, ProcessOutput, global_log_sender,
    install_global_log_sender, open_output,
};
pub(crate) use metadata::ProcessMetadata;
pub(crate) use observer::{
    DEFAULT_LIFECYCLE_BUFFER, LIFECYCLE_BUFFER_ENV, LIFECYCLE_TARGET, LifecycleObserver,
};
pub(crate) use paths::{DIR_MODE, FILE_MODE, LOCKS_SUBDIR, ProcPaths, proc_root_default};
pub(crate) use pid_file::{
    CorruptKind, FileTypeName, PidFileState, ProcessIdentity, PublishError, PublishStep,
    SecurityKind,
};
pub(crate) use posix_signal::{PosixSignal, signal_identity, signal_pid_kill, signal_pid_term};
pub(crate) use proc_info::{
    ProcessStartTime, current_identity, identity_for, pid_in_process_table,
    process_is_alive_with_start_time, process_start_time,
};
pub(crate) use record::{ProcessRecord, list_default, list_under};
pub(crate) use registry::{MetadataDraft, ProcessRegistry, RegisterError};
pub(crate) use runtime::ProcessRuntime;
pub(crate) use spawner::{
    DetachedSpec, SpawnError, UnmanagedChild, spawn_detached, spawn_unmanaged_detached,
};
pub(crate) use status::{
    CorruptStatusError, CorruptStatusKind, ProcessStatus, StatusTransition, UnknownStatusToken,
    is_allowed,
};
