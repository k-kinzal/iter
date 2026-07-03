//! [`Agent`] — the iter noun that realises the agent cycle.
//!
//! `Agent` is a struct, not a trait: there is exactly one cycle and it is
//! implemented once, here. What varies per CLI is translation, delegated to
//! [`AgentDriver`]; what varies per composition is driver selection,
//! delegated to [`Router`]. The cycle itself — session resolution, prepare,
//! spawn-on-the-workspace, prompt delivery, faithful capture, cooperative
//! termination, interpretation, guaranteed cleanup — never varies.
//!
//! The public face is deliberately minimal:
//!
//! - [`Agent::new`] — birth, from a router. The output sink defaults to a
//!   no-op and is fixed with [`Agent::with_stdio_sink`] when the runner is
//!   built (the process is born with its stdout already redirected; it does
//!   not choose per write).
//! - [`Agent::run_on`] — the spec sentence as a signature: run the agent
//!   *on* the workspace, with a prompt, under a cancellation token.
//!
//! Everything else the old context bag carried has moved to its right home:
//! the workspace seam owns isolation and spawning, drivers own their
//! declared env and hook keys, the runner owns the iteration timeout, and
//! signal correlation rides the ambient
//! [`iter_tracing::iteration_scope`].
//!
//! Two pathways leave a run: the agent's **value** is the workspace's file
//! changes (persisted by workspace teardown's apply-back), and the run's
//! **record** is what flows through here — teed output into the sink,
//! [`AgentRun`] out of [`run_on`](Agent::run_on), exit telemetry onto the
//! current span. `AgentRun` being thin is not a defect; value never travels
//! this path.

use std::path::Path;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::agent::driver::{AgentCommand, AgentDriver};
use crate::agent::router::Router;
use crate::agent::session::SessionIdFile;
use crate::agent::{AgentError, AgentRun, process};
use crate::log::{NoopSink, OutputSink};
use crate::prompt::Prompt;
use crate::workspace::{ActiveWorkspace, StdioMode};

/// The agent bound to one Runner for one exploration. See the [module
/// docs](self).
pub struct Agent {
    router: Box<dyn Router>,
    output: Arc<dyn OutputSink>,
}

impl Agent {
    /// Birth: an agent over the given driver selection. The output sink
    /// defaults to a no-op; the runner's `build()` fixes the real one via
    /// [`with_stdio_sink`](Agent::with_stdio_sink).
    #[must_use]
    pub fn new(router: Box<dyn Router>) -> Self {
        Self {
            router,
            output: Arc::new(NoopSink),
        }
    }

    /// Complete the birth with the destination for the agent's raw child
    /// output (every stdout/stderr line is teed through it into the run
    /// record). Consumes and returns `self`; not called again after the
    /// runner starts.
    #[must_use]
    pub fn with_stdio_sink(mut self, sink: Arc<dyn OutputSink>) -> Self {
        self.output = sink;
        self
    }

    /// Run the agent on the workspace with the given prompt — one iteration's
    /// agent phase.
    ///
    /// Attempts drivers as the router directs: each attempt is one complete
    /// run (prepare → command → spawn → exchange → interpret → cleanup); the
    /// router's [`Route`](crate::agent::Route) decides from the interpreted
    /// error whether another attempt starts. The name of the driver that
    /// carried each attempt is recorded on the current span as
    /// `iter.agent.name` (the final attempt's label wins).
    ///
    /// # Errors
    ///
    /// Returns the last attempt's [`AgentError`] when the router stops
    /// advancing — including [`AgentError::Cancelled`], which no built-in
    /// router advances past.
    ///
    /// # Panics
    ///
    /// Panics if the router violates its contract by yielding no driver on
    /// the first [`Route::next`](crate::agent::Route::next) call — every
    /// built-in router asserts non-emptiness at construction, so this is
    /// unreachable outside a broken custom `Router`.
    pub async fn run_on(
        &self,
        workspace: &dyn ActiveWorkspace,
        prompt: &Prompt,
        cancel: CancellationToken,
    ) -> Result<AgentRun, AgentError> {
        let mut route = self.router.begin();
        let mut last: Option<AgentError> = None;
        loop {
            let Some(driver) = route.next(last.as_ref()) else {
                return Err(last.expect("Router::begin must yield at least one driver"));
            };
            match self.run_driver(driver, workspace, prompt, &cancel).await {
                Ok(run) => return Ok(run),
                Err(err) => last = Some(err),
            }
        }
    }

    /// One driver's complete run. After a successful `prepare`, `cleanup` is
    /// awaited on **every** path — success, failure, and cancellation — with
    /// causal error precedence: a run error outranks a cleanup error, and a
    /// cleanup error surfaces only when the run itself succeeded.
    async fn run_driver(
        &self,
        driver: &dyn AgentDriver,
        workspace: &dyn ActiveWorkspace,
        prompt: &Prompt,
        cancel: &CancellationToken,
    ) -> Result<AgentRun, AgentError> {
        // Record which driver carries this attempt; on a fallback sequence
        // the final attempt's label wins (more informative than a static
        // "router").
        tracing::Span::current().record("iter.agent.name", driver.kind().label());

        if cancel.is_cancelled() {
            return Err(AgentError::Cancelled);
        }
        let path = workspace.path();

        // Session continuity is a driver fact (where the token persists) but
        // an agent responsibility (resolving it is async filesystem work the
        // sync `command()` must not do).
        let session = match driver.session_file() {
            Some(file) => Some(SessionIdFile::new(file.to_path_buf()).resolve(path).await?),
            None => None,
        };

        driver.prepare(path).await?;
        let run_result = self
            .run_prepared(driver, workspace, path, prompt, session.as_deref(), cancel)
            .await;
        let cleanup_result = driver.cleanup(path).await;

        match (run_result, cleanup_result) {
            (Err(run_err), _) => Err(run_err),
            (Ok(run), Ok(())) => Ok(run),
            (Ok(_), Err(cleanup_err)) => Err(cleanup_err),
        }
    }

    /// The exchange between `prepare` and `cleanup`: translate out, spawn on
    /// the workspace, feed and capture (or just wait), translate back.
    async fn run_prepared(
        &self,
        driver: &dyn AgentDriver,
        workspace: &dyn ActiveWorkspace,
        path: &Path,
        prompt: &Prompt,
        session: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<AgentRun, AgentError> {
        let AgentCommand {
            process: command,
            stdin,
            io,
        } = driver.command(path, prompt, session)?;

        // The Inherit invariant (see `AgentCommand`): a stdin payload under
        // terminal inheritance has nowhere to go — `wait_inherited` never
        // feeds it. Surface a driver that breaks this in debug/tests rather
        // than dropping the prompt silently.
        debug_assert!(
            !(matches!(io, StdioMode::Inherit) && stdin.is_some()),
            "AgentCommand invariant violated: Inherit mode must carry no stdin \
             (it would be silently dropped)"
        );

        // Fast-path: if cancellation already fired, don't launch at all.
        if cancel.is_cancelled() {
            return Err(AgentError::Cancelled);
        }

        // The workspace bears the birth: isolation, cwd, stdio, process
        // group. The agent takes the child from there to the grave.
        let child = match workspace.spawn(command, io) {
            Ok(child) => child,
            Err(err) => {
                process::record_raw_agent_process(process::RawExit::Unknown, &[], &[]);
                return Err(AgentError::Launch(err.to_string()));
            }
        };

        let output = match io {
            StdioMode::Piped => {
                process::feed_and_capture(child, stdin.as_deref(), cancel, &self.output).await?
            }
            StdioMode::Inherit => process::wait_inherited(child, cancel).await?,
        };

        driver.interpret(&output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentKind;
    use crate::agent::process::RawOutput;
    use crate::agent::router::{FallbackRouter, FallbackTriggers, RotateRouter, SingleAgentRouter};
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Which pre-run step of the cycle the driver refuses at.
    enum FailStage {
        Prepare,
        Command,
    }

    /// Skeleton-observing driver: records the order of its calls and can be
    /// configured to fail at each step. Its command is a real trivial shell
    /// invocation so the whole spawn/capture path is exercised.
    struct RecordingDriver {
        calls: Arc<Mutex<Vec<&'static str>>>,
        /// Refuse at this pre-run step, when set.
        fail_stage: Option<FailStage>,
        fail_cleanup: bool,
        /// Shell exit code the command's child reports.
        exit_code: i32,
        /// Use a nonexistent program so the spawn itself fails.
        unspawnable: bool,
        /// Cancel this token from inside `command()` — simulates the runner
        /// cancelling between prepare and spawn.
        cancel_on_command: Option<CancellationToken>,
    }

    impl RecordingDriver {
        fn new(calls: Arc<Mutex<Vec<&'static str>>>) -> Self {
            Self {
                calls,
                fail_stage: None,
                fail_cleanup: false,
                exit_code: 0,
                unspawnable: false,
                cancel_on_command: None,
            }
        }
    }

    #[async_trait::async_trait]
    impl AgentDriver for RecordingDriver {
        fn command(
            &self,
            _path: &Path,
            _prompt: &Prompt,
            _session: Option<&str>,
        ) -> Result<AgentCommand, AgentError> {
            self.calls.lock().unwrap().push("command");
            if let Some(token) = &self.cancel_on_command {
                token.cancel();
            }
            if matches!(self.fail_stage, Some(FailStage::Command)) {
                return Err(AgentError::Launch("command refused".to_owned()));
            }
            let mut process = if self.unspawnable {
                tokio::process::Command::new("/definitely/not/a/binary/iter-test")
            } else {
                let mut c = tokio::process::Command::new("sh");
                c.arg("-c").arg(format!("exit {}", self.exit_code));
                c
            };
            // Verify the workspace overwrites this; harmless either way.
            process.env("ITER_TEST_MARKER", "1");
            Ok(AgentCommand {
                process,
                stdin: None,
                io: StdioMode::Piped,
            })
        }

        fn interpret(&self, output: &std::process::Output) -> Result<AgentRun, AgentError> {
            self.calls.lock().unwrap().push("interpret");
            match RawOutput::from(output).exit.into_failure() {
                None => Ok(AgentRun::empty()),
                Some(err) => Err(err),
            }
        }

        async fn prepare(&self, _path: &Path) -> Result<(), AgentError> {
            self.calls.lock().unwrap().push("prepare");
            if matches!(self.fail_stage, Some(FailStage::Prepare)) {
                return Err(AgentError::Launch("prepare refused".to_owned()));
            }
            Ok(())
        }

        async fn cleanup(&self, _path: &Path) -> Result<(), AgentError> {
            self.calls.lock().unwrap().push("cleanup");
            if self.fail_cleanup {
                return Err(AgentError::Failed {
                    code: None,
                    message: "cleanup refused".to_owned(),
                });
            }
            Ok(())
        }

        fn kind(&self) -> AgentKind {
            AgentKind::Generic
        }
    }

    async fn active_workspace(dir: &TempDir) -> Box<dyn ActiveWorkspace> {
        let mut ws = crate::workspace::LocalWorkspace::new(dir.path());
        crate::workspace::Workspace::setup(&mut ws, CancellationToken::new())
            .await
            .expect("setup")
    }

    fn agent_over(driver: RecordingDriver) -> Agent {
        Agent::new(Box::new(SingleAgentRouter::new(Box::new(driver))))
    }

    #[tokio::test]
    async fn cycle_calls_in_order_on_success() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let dir = TempDir::new().expect("tempdir");
        let active = active_workspace(&dir).await;
        let agent = agent_over(RecordingDriver::new(calls.clone()));
        let prompt = Prompt::from("x");

        agent
            .run_on(&*active, &prompt, CancellationToken::new())
            .await
            .expect("run ok");

        assert_eq!(
            *calls.lock().unwrap(),
            vec!["prepare", "command", "interpret", "cleanup"],
        );
    }

    #[tokio::test]
    async fn cleanup_runs_when_command_fails() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let dir = TempDir::new().expect("tempdir");
        let active = active_workspace(&dir).await;
        let mut driver = RecordingDriver::new(calls.clone());
        driver.fail_stage = Some(FailStage::Command);
        let agent = agent_over(driver);
        let prompt = Prompt::from("x");

        let err = agent
            .run_on(&*active, &prompt, CancellationToken::new())
            .await
            .expect_err("command failure surfaces");
        assert!(matches!(err, AgentError::Launch(_)));
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["prepare", "command", "cleanup"]
        );
    }

    #[tokio::test]
    async fn cleanup_runs_when_spawn_fails() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let dir = TempDir::new().expect("tempdir");
        let active = active_workspace(&dir).await;
        let mut driver = RecordingDriver::new(calls.clone());
        driver.unspawnable = true;
        let agent = agent_over(driver);
        let prompt = Prompt::from("x");

        let err = agent
            .run_on(&*active, &prompt, CancellationToken::new())
            .await
            .expect_err("spawn failure surfaces");
        assert!(matches!(err, AgentError::Launch(_)));
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["prepare", "command", "cleanup"]
        );
    }

    #[tokio::test]
    async fn cleanup_runs_when_cancelled_mid_run() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let dir = TempDir::new().expect("tempdir");
        let active = active_workspace(&dir).await;
        let token = CancellationToken::new();
        let mut driver = RecordingDriver::new(calls.clone());
        // The token fires while the driver assembles its command — i.e.
        // after prepare succeeded, before the spawn. Cleanup must still run.
        driver.cancel_on_command = Some(token.clone());
        let agent = agent_over(driver);
        let prompt = Prompt::from("x");

        let err = agent
            .run_on(&*active, &prompt, token)
            .await
            .expect_err("cancelled");
        assert!(matches!(err, AgentError::Cancelled));
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["prepare", "command", "cleanup"]
        );
    }

    #[tokio::test]
    async fn cleanup_does_not_run_when_prepare_fails() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let dir = TempDir::new().expect("tempdir");
        let active = active_workspace(&dir).await;
        let mut driver = RecordingDriver::new(calls.clone());
        driver.fail_stage = Some(FailStage::Prepare);
        let agent = agent_over(driver);
        let prompt = Prompt::from("x");

        let err = agent
            .run_on(&*active, &prompt, CancellationToken::new())
            .await
            .expect_err("prepare failure surfaces");
        assert!(matches!(err, AgentError::Launch(_)));
        assert_eq!(*calls.lock().unwrap(), vec!["prepare"]);
    }

    #[tokio::test]
    async fn run_error_outranks_cleanup_error() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let dir = TempDir::new().expect("tempdir");
        let active = active_workspace(&dir).await;
        let mut driver = RecordingDriver::new(calls.clone());
        driver.exit_code = 7;
        driver.fail_cleanup = true;
        let agent = agent_over(driver);
        let prompt = Prompt::from("x");

        let err = agent
            .run_on(&*active, &prompt, CancellationToken::new())
            .await
            .expect_err("run failure surfaces");
        assert!(
            matches!(err, AgentError::Failed { code: Some(7), .. }),
            "run error must win over cleanup error, got {err:?}",
        );
    }

    #[tokio::test]
    async fn cleanup_error_surfaces_when_run_succeeded() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let dir = TempDir::new().expect("tempdir");
        let active = active_workspace(&dir).await;
        let mut driver = RecordingDriver::new(calls.clone());
        driver.fail_cleanup = true;
        let agent = agent_over(driver);
        let prompt = Prompt::from("x");

        let err = agent
            .run_on(&*active, &prompt, CancellationToken::new())
            .await
            .expect_err("cleanup failure surfaces");
        assert!(
            matches!(err, AgentError::Failed { code: None, ref message } if message.contains("cleanup")),
        );
    }

    #[tokio::test]
    async fn pre_cancelled_token_short_circuits_before_prepare() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let dir = TempDir::new().expect("tempdir");
        let active = active_workspace(&dir).await;
        let agent = agent_over(RecordingDriver::new(calls.clone()));
        let prompt = Prompt::from("x");
        let token = CancellationToken::new();
        token.cancel();

        // The first cancel check sits before session/prepare; with a
        // pre-cancelled token no driver step runs at all... except that the
        // check is per-attempt inside run_driver, which runs before prepare.
        let err = agent
            .run_on(&*active, &prompt, token)
            .await
            .expect_err("cancelled");
        assert!(matches!(err, AgentError::Cancelled));
        assert!(
            calls.lock().unwrap().is_empty(),
            "no driver step may run under a pre-cancelled token, got {:?}",
            calls.lock().unwrap(),
        );
    }

    #[tokio::test]
    async fn fallback_router_advances_to_success() {
        let calls_a = Arc::new(Mutex::new(Vec::new()));
        let calls_b = Arc::new(Mutex::new(Vec::new()));
        let dir = TempDir::new().expect("tempdir");
        let active = active_workspace(&dir).await;

        let mut failing = RecordingDriver::new(calls_a.clone());
        failing.exit_code = 1;
        let succeeding = RecordingDriver::new(calls_b.clone());
        let agent = Agent::new(Box::new(FallbackRouter::new(
            vec![
                ("a".into(), Box::new(failing)),
                ("b".into(), Box::new(succeeding)),
            ],
            FallbackTriggers::AnyFailure,
        )));
        let prompt = Prompt::from("x");

        agent
            .run_on(&*active, &prompt, CancellationToken::new())
            .await
            .expect("fallback reaches the succeeding driver");
        assert!(
            calls_a.lock().unwrap().contains(&"interpret"),
            "first driver must have fully run",
        );
        assert!(
            calls_b.lock().unwrap().contains(&"interpret"),
            "second driver must have fully run",
        );
    }

    #[tokio::test]
    async fn fallback_router_exhaustion_returns_last_error() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let dir = TempDir::new().expect("tempdir");
        let active = active_workspace(&dir).await;

        let mut a = RecordingDriver::new(calls.clone());
        a.exit_code = 3;
        let mut b = RecordingDriver::new(calls.clone());
        b.exit_code = 7;
        let agent = Agent::new(Box::new(FallbackRouter::new(
            vec![("a".into(), Box::new(a)), ("b".into(), Box::new(b))],
            FallbackTriggers::AnyFailure,
        )));
        let prompt = Prompt::from("x");

        let err = agent
            .run_on(&*active, &prompt, CancellationToken::new())
            .await
            .expect_err("both fail");
        assert!(
            matches!(err, AgentError::Failed { code: Some(7), .. }),
            "the LAST attempt's error must surface, got {err:?}",
        );
    }

    #[tokio::test]
    async fn rotate_router_picks_one_driver_per_run() {
        let calls_a = Arc::new(Mutex::new(Vec::new()));
        let calls_b = Arc::new(Mutex::new(Vec::new()));
        let dir = TempDir::new().expect("tempdir");
        let active = active_workspace(&dir).await;

        let agent = Agent::new(Box::new(RotateRouter::new(vec![
            ("a".into(), Box::new(RecordingDriver::new(calls_a.clone()))),
            ("b".into(), Box::new(RecordingDriver::new(calls_b.clone()))),
        ])));
        let prompt = Prompt::from("x");

        for _ in 0..2 {
            agent
                .run_on(&*active, &prompt, CancellationToken::new())
                .await
                .expect("run ok");
        }
        assert_eq!(
            calls_a.lock().unwrap().len(),
            calls_b.lock().unwrap().len(),
            "two runs must rotate across both drivers",
        );
    }
}
