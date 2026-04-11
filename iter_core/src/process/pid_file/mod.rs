//! Atomic publication and reading of `~/.iter/proc/<id>/pid`.
//!
//! # Why `linkat`-only (rev12 invariant 1)
//!
//! pid publication MUST use `linkat(dirfd, ".pid.tmp", dirfd, "pid", 0)`,
//! never `renameat`. `linkat` with `flag = 0` is *create-fail-if-exists*,
//! which matches the invariant that `pid` is absent during the
//! `Initializing` phase. `renameat` would silently overwrite a stale `pid`
//! file from a previous crash, erasing forensic evidence.
//!
//! # Why typed errors (rev13 Major)
//!
//! Earlier revisions collapsed all pid-write failures into a single
//! `io::Error`. That made the `EEXIST` cases for `.pid.tmp` and `pid`
//! indistinguishable from generic I/O errors at the call site, breaking
//! the rollback routing in `locked_initial_write` /
//! `locked_adoption_write`. [`PublishError`] now distinguishes the two
//! `EEXIST` shapes, plus a generic [`PublishStep`]-tagged `Io` variant.
//!
//! # Layering
//!
//! ```text
//!   syscall              <- internal libc wrappers
//!   identity             <- ProcessIdentity + ParseIdentityError
//!   cleanup              <- pid_residue_predicate, delete_pid_tmp
//!   publish              <- write_atomic_at (uses syscall + identity)
//!   read                 <- read (uses identity + cleanup)
//! ```

mod cleanup;
mod identity;
mod publish;
mod read;
mod syscall;

pub use identity::{ParseIdentityError, ProcessIdentity};
pub use publish::{PublishError, PublishStep, write_atomic_at};
pub use read::{CorruptKind, FileTypeName, PidFileState, SecurityKind, read};

pub(crate) use cleanup::{delete_pid_tmp, pid_residue_predicate};
