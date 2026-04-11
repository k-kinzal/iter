//! `build_runner_config` — translate the Iterfile's `runner` section and the
//! CLI-supplied `--once` flag into a [`RunnerConfig`].
//!
//! The iter DSL has no termination-condition clause: the Runner's loop is
//! Signal-driven, so shutdown is authored into the Trigger side (stop
//! producing signals, or produce a dedicated shutdown signal). The only
//! termination condition the Runner itself honours is `--once`, which this
//! function plumbs through from the CLI to the [`RunnerConfig`].
//!
//! `continue_on_error` and `behavior` come from the Iterfile's `runner { }`
//! block. iter ships no project-shaped default for either: whether one bad
//! signal stops the whole loop and whether the runner parks on its queue or
//! synthesises iterations are project-policy calls, not iter calls.

use std::time::Duration;

use iter_core::{RunnerBehavior, RunnerConfig};
use iter_language::{RunnerBehavior as DslRunnerBehavior, RunnerDecl};

/// Build a [`RunnerConfig`] from a [`RunnerDecl`] plus the CLI `--once` flag.
///
/// `once` is plumbed through here (rather than mutated by the caller) so the
/// composition layer is the single source of truth for "what does the runner
/// loop think the termination conditions are?".
///
/// # Panics
///
/// Panics if `runner.iteration_timeout_secs` is non-positive — a contract
/// violation that the semantic layer (`iter_language::semantic::runner`)
/// catches before lowering. See the inline comment for the rationale.
#[must_use]
pub fn build_runner_config(runner: &RunnerDecl, once: bool) -> RunnerConfig {
    RunnerConfig {
        once,
        continue_on_error: runner.continue_on_error,
        behavior: lower_behavior(&runner.behavior),
        iteration_timeout: runner.iteration_timeout_secs.map(|s| {
            // The semantic layer (`iter_language::semantic::runner`) rejects
            // `iteration_timeout_secs <= 0` before lowering, so a non-positive
            // value here is a contract violation by an upstream caller of
            // `build_runner_config` (which is `pub`).  Treat it as such: a
            // silent fallback to either `None` (unbounded) or
            // `Duration::ZERO` (immediate timeout) would just trade one
            // kind of breakage for another.  Surface the violation directly.
            Duration::from_secs(u64::try_from(s).expect(
                "iteration_timeout_secs must be positive (the semantic layer \
                 enforces this; if you reached this panic you constructed a \
                 RunnerDecl directly without going through the language pipeline)",
            ))
        }),
    }
}

fn lower_behavior(behavior: &DslRunnerBehavior) -> RunnerBehavior {
    match behavior {
        DslRunnerBehavior::Wait => RunnerBehavior::Wait,
        DslRunnerBehavior::Loop { delay_secs } => RunnerBehavior::Loop {
            delay: delay_secs
                .and_then(|s| u64::try_from(s).ok())
                .map(Duration::from_secs),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn once_flag_propagates() {
        let decl = RunnerDecl {
            continue_on_error: false,
            behavior: DslRunnerBehavior::Wait,
            iteration_timeout_secs: None,
        };
        let config = build_runner_config(&decl, true);
        assert!(config.once);
    }

    #[test]
    fn continue_on_error_is_plumbed_through_when_false() {
        let decl = RunnerDecl {
            continue_on_error: false,
            behavior: DslRunnerBehavior::Wait,
            iteration_timeout_secs: None,
        };
        let config = build_runner_config(&decl, false);
        assert!(!config.continue_on_error);
    }

    #[test]
    fn continue_on_error_is_plumbed_through_when_true() {
        let decl = RunnerDecl {
            continue_on_error: true,
            behavior: DslRunnerBehavior::Wait,
            iteration_timeout_secs: None,
        };
        let config = build_runner_config(&decl, false);
        assert!(config.continue_on_error);
    }

    #[test]
    fn wait_behavior_lowers_to_wait() {
        let decl = RunnerDecl {
            continue_on_error: false,
            behavior: DslRunnerBehavior::Wait,
            iteration_timeout_secs: None,
        };
        let config = build_runner_config(&decl, false);
        assert_eq!(config.behavior, RunnerBehavior::Wait);
    }

    #[test]
    fn loop_behavior_without_delay_lowers_to_loop_none() {
        let decl = RunnerDecl {
            continue_on_error: false,
            behavior: DslRunnerBehavior::Loop { delay_secs: None },
            iteration_timeout_secs: None,
        };
        let config = build_runner_config(&decl, false);
        assert_eq!(config.behavior, RunnerBehavior::Loop { delay: None });
    }

    #[test]
    fn loop_behavior_with_delay_lowers_to_loop_some() {
        let decl = RunnerDecl {
            continue_on_error: false,
            behavior: DslRunnerBehavior::Loop {
                delay_secs: Some(30),
            },
            iteration_timeout_secs: None,
        };
        let config = build_runner_config(&decl, false);
        assert_eq!(
            config.behavior,
            RunnerBehavior::Loop {
                delay: Some(Duration::from_secs(30)),
            }
        );
    }

    #[test]
    fn iteration_timeout_none_lowers_to_none() {
        let decl = RunnerDecl {
            continue_on_error: true,
            behavior: DslRunnerBehavior::Wait,
            iteration_timeout_secs: None,
        };
        let config = build_runner_config(&decl, false);
        assert_eq!(config.iteration_timeout, None);
    }

    #[test]
    fn iteration_timeout_some_lowers_to_duration() {
        let decl = RunnerDecl {
            continue_on_error: true,
            behavior: DslRunnerBehavior::Wait,
            iteration_timeout_secs: Some(900),
        };
        let config = build_runner_config(&decl, false);
        assert_eq!(config.iteration_timeout, Some(Duration::from_secs(900)));
    }

    #[test]
    fn iteration_timeout_large_value_preserved() {
        let decl = RunnerDecl {
            continue_on_error: true,
            behavior: DslRunnerBehavior::Wait,
            iteration_timeout_secs: Some(3_600_000),
        };
        let config = build_runner_config(&decl, false);
        assert_eq!(
            config.iteration_timeout,
            Some(Duration::from_secs(3_600_000))
        );
    }
}
