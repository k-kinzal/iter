use std::ffi::OsString;

use crate::args::{push_flag, push_opt};
use crate::values::Switch;

/// `claude ultrareview [target]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UltraReview {
    /// Optional PR number, PR URL, or base branch.
    pub target: Option<String>,
    /// `--json`.
    pub json: Switch,
    /// `--timeout`.
    pub timeout_minutes: Option<u32>,
}

impl UltraReview {
    pub(crate) fn render(&self, args: &mut Vec<OsString>) {
        push_flag(args, self.json, "--json");
        push_opt(
            args,
            "--timeout",
            self.timeout_minutes.map(|m| m.to_string()).as_deref(),
        );
        if let Some(target) = &self.target {
            args.push(target.into());
        }
    }
}
