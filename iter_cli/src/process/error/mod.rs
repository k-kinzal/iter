//! Error surface for the `process` subsystem.
//!
//! # Layering
//!
//! - [`ProcessError`] is the *outer* type — what every public `async fn` in
//!   the subsystem ultimately returns. It is the type the CLI sees.
//! - [`StartupError`] / [`AdoptError`] are the *inner*, locked-section types
//!   for `locked_initial_write` and `locked_adoption_write`. They are wrapped
//!   into `ProcessError::{Startup,Adopt}(Box<…>)` on the way out.
//! - [`LockedSectionError`] carries the failures the *shared* critical
//!   section can raise (pid-file publication, status writeback, fsync, the
//!   environmental I/O surrounding them). Both [`StartupError`] and
//!   [`AdoptError`] embed it via a `LockedSection(_)` variant so the two
//!   surfaces cannot drift apart.
//! - [`RegistryError`] is for the `name_lock` / `registry` layer.
//! - [`ObserverError`] is for the lifecycle observer.
//!
//! # Recursive type cycle (rev17)
//!
//! `LockedSectionError::Io(ProcessError)` carries the *closure-internal*
//! environmental I/O failures (flock acquire/release, mutex poisoning) up
//! into the typed outer error. Conversely `ProcessError::Startup(_)` carries
//! the typed outer error down to the CLI as a single shape. Without
//! indirection these two payloads form a recursive sized type which Rust
//! cannot lay out, so `ProcessError` boxes the outer side
//! (`Startup(Box<StartupError>)` / `Adopt(Box<AdoptError>)`).
//!
//! Boxing is on the *outer* (CLI-facing, low-frequency) side so the hot path
//! inside `with_locked_status` (where `LockedSectionError::Io(ProcessError)`
//! is constructed every time `flock` fails) does not allocate.
//!
//! # File layout
//!
//! ```text
//!   token            <- TokenCorruptKind (leaf)
//!   secondary        <- SecondaryStatusWrite (leaf)
//!   process          <- ProcessError (outer, references Box<startup|adopt>)
//!   locked_section   <- LockedSectionError (references ProcessError + secondary)
//!   startup          <- StartupError (references LockedSectionError)
//!   adopt            <- AdoptError (references LockedSectionError + token)
//!   registry         <- RegistryError (independent)
//!   observer         <- ObserverError (independent)
//! ```

mod adopt;
mod locked_section;
mod observer;
mod process;
mod registry;
mod secondary;
mod startup;
mod token;

pub(crate) use adopt::AdoptError;
pub(crate) use locked_section::LockedSectionError;
pub(crate) use observer::ObserverError;
pub(crate) use process::ProcessError;
pub(crate) use registry::RegistryError;
pub(crate) use secondary::SecondaryStatusWrite;
pub(crate) use startup::StartupError;
pub(crate) use token::TokenCorruptKind;

// Cross-family conversions — see module-level docs and rev17 history.
//
// Up → down: typed inner error is wrapped at the CLI boundary. Boxing here
// breaks the otherwise-recursive sized type and concentrates allocations on
// the cold path.
impl From<StartupError> for ProcessError {
    fn from(e: StartupError) -> Self {
        ProcessError::Startup(Box::new(e))
    }
}

impl From<AdoptError> for ProcessError {
    fn from(e: AdoptError) -> Self {
        ProcessError::Adopt(Box::new(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_error_has_known_size() {
        // The whole point of `Box<StartupError>` / `Box<AdoptError>` is to
        // break the recursive sized-type cycle. Verify the outer enum
        // actually has a finite size by computing it.
        let _: usize = size_of::<ProcessError>();
    }

    #[test]
    fn round_trip_startup_to_process_to_startup_via_io() {
        let inner: StartupError = LockedSectionError::JoinPanic.into();
        let outer: ProcessError = inner.into();
        match outer {
            ProcessError::Startup(boxed) => match *boxed {
                StartupError::LockedSection(LockedSectionError::JoinPanic) => {}
                _ => panic!("variant mismatch"),
            },
            _ => panic!("outer variant mismatch"),
        }
    }

    #[test]
    fn process_error_lifts_into_startup_locked_section_io() {
        let pe = ProcessError::StatusFilePoisoned;
        let lifted: StartupError = pe.into();
        match lifted {
            StartupError::LockedSection(LockedSectionError::Io(
                ProcessError::StatusFilePoisoned,
            )) => {}
            _ => panic!("expected StartupError::LockedSection(Io(StatusFilePoisoned))"),
        }
    }

    #[test]
    fn process_error_lifts_into_adopt_locked_section_io() {
        let pe = ProcessError::StatusFilePoisoned;
        let lifted: AdoptError = pe.into();
        match lifted {
            AdoptError::LockedSection(LockedSectionError::Io(ProcessError::StatusFilePoisoned)) => {
            }
            _ => panic!("expected AdoptError::LockedSection(Io(StatusFilePoisoned))"),
        }
    }
}
