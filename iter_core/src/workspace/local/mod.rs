//! [`LocalWorkspace`] — the target directory itself.
//!
//! This is the widest-scope workspace implementation: the agent operates
//! directly against the user's real directory.
//! [`setup`](crate::Workspace::setup) only validates that the path exists
//! and is a directory; [`teardown`](crate::Workspace::teardown) is a
//! no-op because the target directory *is* the source of truth.
//!
//! Use [`LocalWorkspace`] when you want the agent to have full,
//! unmediated access to a directory — for example when iterating on a
//! repository you are already editing by hand.

pub mod error;
pub mod workspace;

pub use error::LocalWorkspaceError;
pub use workspace::LocalWorkspace;
