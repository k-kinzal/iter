//! [`Router`] — uniform driver selection for the agent cycle.
//!
//! An [`Agent`](crate::agent::Agent) always holds exactly one `Router`;
//! there is no "driver or router" choice on the Agent's public face. A
//! single-driver agent holds a [`SingleAgentRouter`] — a router that does
//! not route.
//!
//! Routing is a **cycle-level** policy: it consumes the *interpreted*
//! domain error of one complete run (e.g.
//! [`AgentError::TokenLimit`](crate::agent::AgentError::TokenLimit)) and
//! decides whether another complete run should start with a different
//! driver. That is why it cannot be an
//! [`AgentDriver`](crate::agent::AgentDriver): a driver translates exactly
//! one run and has no channel to request another.
//!
//! The selection protocol is the [`Route`] cursor: [`Router::begin`] opens
//! one run's cursor, and [`Route::next`] picks the next driver given the
//! previous attempt's failure. The execution loop stays in
//! [`Agent::run_on`](crate::agent::Agent::run_on) — the cycle's body — so a
//! router never touches a process.
//!
//! The named-pair enumeration (`Vec<(String, Box<dyn AgentDriver>)>`) is
//! kept un-flattened so each driver stays individually addressable in
//! routing logs.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::agent::driver::AgentDriver;
use crate::agent::{AgentError, FallbackClass};

/// Failure classes that trigger fallback to the next driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackTriggers {
    /// Fall back on every agent failure class except cancellation.
    AnyFailure,
    /// Fall back only on the listed failure classes.
    Only(HashSet<FallbackClass>),
}

impl Default for FallbackTriggers {
    fn default() -> Self {
        Self::AnyFailure
    }
}

impl FallbackTriggers {
    /// Return whether this trigger set includes the class.
    #[must_use]
    pub fn contains(&self, class: FallbackClass) -> bool {
        match self {
            Self::AnyFailure => true,
            Self::Only(classes) => classes.contains(&class),
        }
    }
}

/// Driver selection policy for one exploration. See the [module
/// docs](self).
pub trait Router: Send + Sync {
    /// Open one run's selection cursor. The first
    /// [`next(None)`](Route::next) call is guaranteed to yield a driver.
    fn begin(&self) -> Box<dyn Route<'_> + '_>;
}

/// One run's selection cursor: given the previous attempt's failure, pick
/// the next driver — or `None` to stop, in which case the agent returns the
/// last error.
///
/// Cancellation ([`AgentError::Cancelled`]) has no fallback class, so every
/// built-in router stops on it — cooperative shutdown always propagates.
///
/// `Send` because the cursor lives across the awaits of
/// [`Agent::run_on`](crate::agent::Agent::run_on), whose future must be
/// spawnable onto a runtime (compose runs services as tasks).
pub trait Route<'r>: Send {
    /// Pick the driver for the next attempt. `last` is `None` on the first
    /// call and the previous attempt's error afterwards.
    fn next(&mut self, last: Option<&AgentError>) -> Option<&'r dyn AgentDriver>;
}

/// The router that does not route: exactly one driver, one attempt.
pub struct SingleAgentRouter {
    driver: Box<dyn AgentDriver>,
}

impl SingleAgentRouter {
    /// Wrap a single driver.
    #[must_use]
    pub fn new(driver: Box<dyn AgentDriver>) -> Self {
        Self { driver }
    }
}

impl Router for SingleAgentRouter {
    fn begin(&self) -> Box<dyn Route<'_> + '_> {
        Box::new(OneShotRoute {
            driver: Some(self.driver.as_ref()),
        })
    }
}

/// Cursor yielding exactly one driver, with no retry.
struct OneShotRoute<'r> {
    driver: Option<&'r dyn AgentDriver>,
}

impl<'r> Route<'r> for OneShotRoute<'r> {
    fn next(&mut self, _last: Option<&AgentError>) -> Option<&'r dyn AgentDriver> {
        self.driver.take()
    }
}

/// Try the drivers in declaration order; on configured failure classes,
/// advance to the next.
pub struct FallbackRouter {
    agents: Vec<(String, Box<dyn AgentDriver>)>,
    triggers: FallbackTriggers,
}

impl FallbackRouter {
    /// Construct a fallback router over the given named drivers.
    ///
    /// # Panics
    ///
    /// Panics if `agents` is empty.
    #[must_use]
    pub fn new(agents: Vec<(String, Box<dyn AgentDriver>)>, triggers: FallbackTriggers) -> Self {
        assert!(
            !agents.is_empty(),
            "FallbackRouter requires at least one driver"
        );
        Self { agents, triggers }
    }
}

impl Router for FallbackRouter {
    fn begin(&self) -> Box<dyn Route<'_> + '_> {
        Box::new(FallbackRoute {
            agents: &self.agents,
            triggers: &self.triggers,
            index: 0,
        })
    }
}

struct FallbackRoute<'r> {
    agents: &'r [(String, Box<dyn AgentDriver>)],
    triggers: &'r FallbackTriggers,
    index: usize,
}

impl<'r> Route<'r> for FallbackRoute<'r> {
    fn next(&mut self, last: Option<&AgentError>) -> Option<&'r dyn AgentDriver> {
        if self.index > 0 {
            // Deciding whether to continue past a failed attempt.
            let err = last?;
            match err.fallback_class() {
                Some(class) if self.triggers.contains(class) => {
                    let (failed_name, _) = &self.agents[self.index - 1];
                    tracing::warn!(
                        target: "iter::agent_router",
                        agent = failed_name.as_str(),
                        index = self.index - 1,
                        class = class.label(),
                        "agent failed, trying next",
                    );
                }
                // Cancellation (no class) and non-triggering classes stop
                // the route; the agent returns this error as-is.
                _ => return None,
            }
        }
        let (_, driver) = self.agents.get(self.index)?;
        self.index += 1;
        Some(driver.as_ref())
    }
}

/// Rotate through the drivers round-robin across runs. A failed attempt is
/// not retried within the run.
pub struct RotateRouter {
    agents: Vec<(String, Box<dyn AgentDriver>)>,
    counter: AtomicUsize,
}

impl RotateRouter {
    /// Construct a rotate router over the given named drivers.
    ///
    /// # Panics
    ///
    /// Panics if `agents` is empty.
    #[must_use]
    pub fn new(agents: Vec<(String, Box<dyn AgentDriver>)>) -> Self {
        assert!(
            !agents.is_empty(),
            "RotateRouter requires at least one driver"
        );
        Self {
            agents,
            counter: AtomicUsize::new(0),
        }
    }
}

impl Router for RotateRouter {
    fn begin(&self) -> Box<dyn Route<'_> + '_> {
        let index = self.counter.fetch_add(1, Ordering::Relaxed) % self.agents.len();
        Box::new(OneShotRoute {
            driver: Some(self.agents[index].1.as_ref()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::driver::AgentCommand;
    use crate::agent::{AgentKind, AgentRun};
    use crate::prompt::Prompt;
    use std::path::Path;

    /// Inert driver for pure selection-protocol tests — never actually run.
    struct MockDriver;

    #[async_trait::async_trait]
    impl AgentDriver for MockDriver {
        fn command(
            &self,
            _path: &Path,
            _prompt: &Prompt,
            _session: Option<&str>,
        ) -> Result<AgentCommand, AgentError> {
            Err(AgentError::Launch("mock driver is never run".to_owned()))
        }

        fn interpret(&self, _output: &std::process::Output) -> Result<AgentRun, AgentError> {
            Ok(AgentRun::empty())
        }

        fn kind(&self) -> AgentKind {
            AgentKind::Noop
        }
    }

    fn named(name: &'static str) -> (String, Box<dyn AgentDriver>) {
        (name.to_owned(), Box::new(MockDriver))
    }

    #[test]
    fn single_route_yields_exactly_once() {
        let router = SingleAgentRouter::new(Box::new(MockDriver));
        let mut route = router.begin();
        assert!(route.next(None).is_some(), "first next must yield");
        assert!(
            route
                .next(Some(&AgentError::Failed {
                    code: Some(1),
                    message: "boom".into()
                }))
                .is_none(),
            "single router never retries",
        );
    }

    #[test]
    fn fallback_advances_on_triggering_class() {
        let router =
            FallbackRouter::new(vec![named("a"), named("b")], FallbackTriggers::AnyFailure);
        let mut route = router.begin();
        assert!(route.next(None).is_some());
        let err = AgentError::TokenLimit("context window".into());
        assert!(
            route.next(Some(&err)).is_some(),
            "AnyFailure must advance past a token limit",
        );
        assert!(
            route.next(Some(&err)).is_none(),
            "exhausted route must stop",
        );
    }

    #[test]
    fn fallback_stops_on_cancelled() {
        let router =
            FallbackRouter::new(vec![named("a"), named("b")], FallbackTriggers::AnyFailure);
        let mut route = router.begin();
        assert!(route.next(None).is_some());
        assert!(
            route.next(Some(&AgentError::Cancelled)).is_none(),
            "cancellation has no fallback class and must stop the route",
        );
    }

    #[test]
    fn fallback_stops_on_non_triggering_class() {
        let router = FallbackRouter::new(
            vec![named("a"), named("b")],
            FallbackTriggers::Only(HashSet::from([FallbackClass::TokenLimit])),
        );
        let mut route = router.begin();
        assert!(route.next(None).is_some());
        let launch = AgentError::Launch("no binary".into());
        assert!(
            route.next(Some(&launch)).is_none(),
            "Only(TokenLimit) must not advance past a launch failure",
        );

        let mut route = router.begin();
        assert!(route.next(None).is_some());
        let limit = AgentError::TokenLimit("context window".into());
        assert!(
            route.next(Some(&limit)).is_some(),
            "Only(TokenLimit) must advance past a token limit",
        );
    }

    #[test]
    fn rotate_cycles_across_begins_and_never_retries() {
        let router = RotateRouter::new(vec![named("a"), named("b")]);
        for _ in 0..3 {
            let mut route = router.begin();
            assert!(route.next(None).is_some());
            let err = AgentError::Failed {
                code: Some(1),
                message: "boom".into(),
            };
            assert!(route.next(Some(&err)).is_none(), "rotate never retries");
        }
        assert_eq!(router.counter.load(Ordering::Relaxed), 3);
    }
}
