//! `process` — the on-disk view of a running iter process.
//!
//! A process record is the durable control-plane view of one running
//! runner. The registry owns record creation, status files expose lifecycle
//! state, pid files bind records back to OS processes, and shutdown/adoption
//! code treats those files as the synchronization boundary between parent
//! and child processes.
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
// (flock-protected status writes, name locks, bootstrap tokens, adoption
// glue, session caches) and intentionally do not appear in the public
// API surface. Items that *should* be public are re-exported individually
// via `pub use` below.
pub(crate) mod adoption;
pub(crate) mod bootstrap_token;
pub(crate) mod name_lock;
pub(crate) mod session;
pub mod signal;
pub(crate) mod status_file;

// Public-facing modules. Each one carries a focused responsibility and is
// addressable as `crate::process::<module>` for callers that prefer the
// inner path over the flat re-exports below.
pub mod error;
pub mod handle;
pub mod id;
pub mod interrupt;
pub mod log;
pub mod metadata;
pub mod observer;
pub mod paths;
pub mod pid_file;
pub mod proc_info;
pub mod record;
pub mod registry;
pub mod runtime;
pub mod shutdown;
pub mod spawner;
pub mod status;

pub use adoption::adopt_from_argv;
pub use error::{
    AdoptError, LockedSectionError, ObserverError, ProcessError, RegistryError, Result,
    SecondaryStatusWriteResult, StartupError, TokenCorruptKind,
};
pub use handle::{BOOTSTRAP_GRACE_ENV, ProcessHandle, bootstrap_grace};
pub use id::{BootstrapToken, Pid, ProcessId};
pub use interrupt::{Interrupt, install_signal_handlers};
pub use log::{
    DEFAULT_LOG_BUFFER, LogSender, OutputPolicy, ProcessLogSink, ProcessOutput, global_log_sender,
    install_global_log_sender, open_output,
};
pub use metadata::ProcessMetadata;
pub use observer::{
    DEFAULT_LIFECYCLE_BUFFER, LIFECYCLE_BUFFER_ENV, LIFECYCLE_TARGET, LifecycleObserver,
};
pub use paths::{DIR_MODE, FILE_MODE, LOCKS_SUBDIR, ProcPaths, proc_root_default};
pub use pid_file::{
    CorruptKind, FileTypeName, PidFileState, ProcessIdentity, PublishError, PublishStep,
    SecurityKind,
};
pub use proc_info::{
    ProcessStartTime, current_identity, identity_for, pid_in_process_table,
    process_is_alive_with_start_time, process_start_time,
};
pub use record::{ProcessRecord, list_default, list_under};
pub use registry::{MetadataDraft, ProcessRegistry, RegisterError};
pub use runtime::{FinalizeReport, ProcessRuntime};
pub use shutdown::{BoxError as ShutdownBoxError, ProcessTerminationReason, ShutdownController};
pub use signal::{SignalDelivery, signal_identity, signal_pid_kill, signal_pid_term};
pub use spawner::{
    DetachedSpec, SpawnError, UnmanagedChild, spawn_detached, spawn_unmanaged_detached,
};
pub use status::{
    CorruptStatusError, CorruptStatusKind, ProcessStatus, TransitionResult, UnknownStatusToken,
    is_allowed,
};
