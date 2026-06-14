//! Per-subcommand dispatch handlers.
//!
//! Each function here corresponds to one variant of
//! [`crate::cli::Command`]. They are deliberately thin: argv parsing happens
//! in `cli.rs`, composition happens in the CLI's composition layer
//! ([`crate::compose`], [`crate::iterfile`], and the per-noun start
//! modules), and these handlers do little more than wire those two halves
//! together and forward the result.

pub(crate) mod attach;
pub(crate) mod compose;
pub(crate) mod enqueue;
pub(crate) mod load;
pub(crate) mod proc;
pub(crate) mod run;
pub(crate) mod validate;

pub(crate) use attach::{AttachError, attach, status_exit_code};
pub(crate) use compose::{
    ComposeCmdError, ComposeUpError, run_compose_config, run_compose_down, run_compose_ls,
    run_compose_ps, run_compose_up, run_compose_validate,
};
pub(crate) use enqueue::{EnqueueCmdError, run_enqueue};
pub(crate) use proc::{
    ProcessCmdError, run_discard, run_inspect, run_kill, run_logs, run_promote, run_ps, run_rm,
    run_stop,
};
pub(crate) use run::{RunCmdError, run_run};
pub(crate) use validate::{ValidateCmdError, run_validate};
