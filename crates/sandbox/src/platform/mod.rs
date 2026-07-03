//! Private target-specific command wrapping.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

use std::process::Command;

use crate::Error;
use crate::policy::Policy;

#[cfg(target_os = "linux")]
pub(crate) fn wrap(policy: &Policy, target: &Command) -> Result<Command, Error> {
    linux::wrap(policy, target)
}

#[cfg(target_os = "macos")]
pub(crate) fn wrap(policy: &Policy, target: &Command) -> Result<Command, Error> {
    macos::wrap(policy, target)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn wrap(policy: &Policy, target: &Command) -> Result<Command, Error> {
    let _ = (policy, target);
    Err(Error::UnsupportedPlatform)
}
