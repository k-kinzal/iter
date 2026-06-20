use std::ffi::OsString;

use crate::args::push_flag;
use crate::values::Switch;

/// `claude install [target]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Install {
    /// Optional target: `stable`, `latest`, or a specific version.
    pub target: Option<String>,
    /// `--force`.
    pub force: Switch,
}

impl Install {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_flag(args, self.force, "--force");
        if let Some(target) = &self.target {
            args.push(target.into());
        }
    }
}
