//! `runner_policy_from_def` — translate the Iterfile's `runner` section and the
//! CLI-supplied `--once` flag into a [`RunnerPolicy`].
//!
//! Repetition/error policy remains separate from the Runner's ordered
//! first-class completion conditions. This module lowers both surfaces;
//! `--once` remains a CLI-owned one-iteration completion boundary.
//!
//! `continue_on_error` and `behavior` come from the Iterfile's `runner { }`
//! block. iter ships no project-shaped default for either: whether one bad
//! signal stops the whole loop and whether the runner parks on its queue or
//! synthesises iterations are project-policy calls, not iter calls.

use std::time::Duration;

use iter_core::{
    CompletionCondition, CompletionConditionErrorPolicy, RunnerPolicy, SignalAcquisition,
};
use iter_language::{
    CompletionConditionDef, CompletionConditionErrorPolicy as DslCompletionErrorPolicy, RunnerDef,
    SignalAcquisition as DslSignalAcquisition,
};

/// Build a [`RunnerPolicy`] from a [`RunnerDef`] plus the CLI `--once` flag.
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
pub(crate) fn runner_policy_from_def(runner: &RunnerDef, once: bool) -> RunnerPolicy {
    RunnerPolicy {
        once,
        continue_on_error: runner.continue_on_error,
        behavior: lower_behavior(&runner.behavior),
        iteration_timeout: runner.iteration_timeout_secs.map(|s| {
            Duration::from_secs(u64::try_from(s).expect(
                "iteration_timeout_secs must be positive (the semantic layer \
                 enforces this; if you reached this panic you constructed a \
                 RunnerDef directly without going through the language pipeline)",
            ))
        }),
    }
}

/// Lower the runner's ordered completion declaration into runtime conditions.
#[must_use]
pub(crate) fn completion_conditions_from_def(runner: &RunnerDef) -> Vec<CompletionCondition> {
    runner
        .completion
        .as_ref()
        .map(|completion| {
            completion
                .conditions
                .iter()
                .map(|condition| match &condition.node {
                    CompletionConditionDef::Iterations { name, max } => {
                        CompletionCondition::Iterations {
                            name: name.clone(),
                            max: *max,
                        }
                    }
                    CompletionConditionDef::Shell {
                        name,
                        run,
                        timeout_secs,
                        on_error,
                    } => CompletionCondition::Shell {
                        name: name.clone(),
                        command: run.clone(),
                        timeout: Duration::from_secs(*timeout_secs),
                        on_error: match on_error {
                            DslCompletionErrorPolicy::Abort => {
                                CompletionConditionErrorPolicy::Abort
                            }
                            DslCompletionErrorPolicy::Continue => {
                                CompletionConditionErrorPolicy::Continue
                            }
                        },
                    },
                    CompletionConditionDef::Elapsed {
                        name,
                        duration_secs,
                    } => CompletionCondition::Elapsed {
                        name: name.clone(),
                        duration: Duration::from_secs(*duration_secs),
                    },
                    CompletionConditionDef::Deadline { name, at } => {
                        let at = chrono::DateTime::parse_from_rfc3339(at)
                            .expect("semantic analyzer validated completion deadline")
                            .with_timezone(&chrono::Utc);
                        CompletionCondition::Deadline {
                            name: name.clone(),
                            at,
                        }
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn lower_behavior(behavior: &DslSignalAcquisition) -> SignalAcquisition {
    match behavior {
        DslSignalAcquisition::Wait => SignalAcquisition::Wait,
        DslSignalAcquisition::Synthesize { delay_secs } => SignalAcquisition::Synthesize {
            delay: delay_secs
                .and_then(|s| u64::try_from(s).ok())
                .map(Duration::from_secs),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iter_language::{PromptExpr, PromptValue};

    fn test_runner(
        continue_on_error: bool,
        behavior: DslSignalAcquisition,
        iteration_timeout_secs: Option<i64>,
    ) -> RunnerDef {
        RunnerDef {
            name: None,
            agent: String::new(),
            workspace: String::new(),
            queue: None,
            continue_on_error,
            behavior,
            iteration_timeout_secs,
            completion: None,
            prompt: PromptExpr::Single(PromptValue::Inline(String::new())),
            events: Vec::new(),
        }
    }

    #[test]
    fn once_flag_propagates() {
        let decl = test_runner(false, DslSignalAcquisition::Wait, None);
        let policy = runner_policy_from_def(&decl, true);
        assert!(policy.once);
    }

    #[test]
    fn continue_on_error_is_plumbed_through_when_false() {
        let decl = test_runner(false, DslSignalAcquisition::Wait, None);
        let policy = runner_policy_from_def(&decl, false);
        assert!(!policy.continue_on_error);
    }

    #[test]
    fn continue_on_error_is_plumbed_through_when_true() {
        let decl = test_runner(true, DslSignalAcquisition::Wait, None);
        let policy = runner_policy_from_def(&decl, false);
        assert!(policy.continue_on_error);
    }

    #[test]
    fn wait_behavior_lowers_to_wait() {
        let decl = test_runner(false, DslSignalAcquisition::Wait, None);
        let policy = runner_policy_from_def(&decl, false);
        assert_eq!(policy.behavior, SignalAcquisition::Wait);
    }

    #[test]
    fn loop_behavior_without_delay_lowers_to_loop_none() {
        let decl = test_runner(
            false,
            DslSignalAcquisition::Synthesize { delay_secs: None },
            None,
        );
        let policy = runner_policy_from_def(&decl, false);
        assert_eq!(
            policy.behavior,
            SignalAcquisition::Synthesize { delay: None }
        );
    }

    #[test]
    fn loop_behavior_with_delay_lowers_to_loop_some() {
        let decl = test_runner(
            false,
            DslSignalAcquisition::Synthesize {
                delay_secs: Some(30),
            },
            None,
        );
        let policy = runner_policy_from_def(&decl, false);
        assert_eq!(
            policy.behavior,
            SignalAcquisition::Synthesize {
                delay: Some(Duration::from_secs(30)),
            }
        );
    }

    #[test]
    fn iteration_timeout_none_lowers_to_none() {
        let decl = test_runner(true, DslSignalAcquisition::Wait, None);
        let policy = runner_policy_from_def(&decl, false);
        assert_eq!(policy.iteration_timeout, None);
    }

    #[test]
    fn iteration_timeout_some_lowers_to_duration() {
        let decl = test_runner(true, DslSignalAcquisition::Wait, Some(900));
        let policy = runner_policy_from_def(&decl, false);
        assert_eq!(policy.iteration_timeout, Some(Duration::from_secs(900)));
    }

    #[test]
    fn iteration_timeout_large_value_preserved() {
        let decl = test_runner(true, DslSignalAcquisition::Wait, Some(3_600_000));
        let policy = runner_policy_from_def(&decl, false);
        assert_eq!(
            policy.iteration_timeout,
            Some(Duration::from_secs(3_600_000))
        );
    }
}
