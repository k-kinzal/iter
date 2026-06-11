//! Shared subprocess primitive for running a CLI-backed agent.
//!
//! This is infrastructure shared by the **Command level** of every driver.
//! It owns the parts of spawning a child that are identical across CLIs —
//! sandbox-prefix wrapping, process-group setup, stdin delivery, stdout/
//! stderr teeing into `log.ndjson`, cancellation, and platform exit-status
//! mapping — and hands back a [`CommandOutput`] (full captured output + a
//! [`RawExit`]). It does **not** interpret that output: turning
//! `(exit, stdout, stderr)` into a CLI-shaped result or error is each
//! per-CLI Command's job (`drivers/<cli>/command.rs`).
//!
//! Two entry points:
//!
//! * [`spawn_capture`] — non-interactive/print mode: pipes stdio, captures
//!   the child's **complete** stdout/stderr so a Command can parse the
//!   machine-readable stream, and returns a [`CommandOutput`].
//! * [`drive_interactive`] / [`drive_interactive_with_finalize`] — hook-based
//!   interactive (TUI) mode: inherits stdio (no capture) and returns just the
//!   [`RawExit`], optionally finalizing a hook bundle first.

use std::ffi::{OsStr, OsString};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use crate::log::{LogStream, OutputSink};
use crate::process_group::{self, ProcessGroup};
use bytes::Bytes;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::AgentError;
use crate::signal::{SignalId, SignalKind};

/// Grace period applied between SIGTERM and SIGKILL when the runner cancels
/// an agent process tree. Five seconds matches the upper bound most CLI
/// agents use for their own shutdown handlers; anything longer just keeps
/// the runner blocked.
///
/// `ITERATION_TIMEOUT_DRAIN_GRACE` (in `agent/inner.rs`) is derived from
/// this constant so the iteration-timeout drain window always exceeds the
/// SIGTERM grace; if you change one, the other follows automatically.
pub(crate) const AGENT_TERMINATION_GRACE: Duration = Duration::from_secs(5);

/// Bound on how long the stdio tee tasks are awaited after the agent
/// has been cancelled. Cancellation kills the child (closing the pipes
/// from the kernel side), which makes the tee `read` calls return
/// promptly; this cap protects against a stuck pipe (e.g. a frozen
/// sandbox host process holding the read end). One second matches the
/// observed worst case in tests and is small enough that operator
/// shutdown latency stays imperceptible.
const CANCEL_TEE_DRAIN: Duration = Duration::from_secs(1);

/// Platform exit disposition of an agent child process, as observed by the
/// shared spawn primitive — *before* any CLI-specific interpretation.
///
/// `Code(0)` is the only success disposition; a non-zero code, a terminating
/// signal, or an indeterminate status are all reported faithfully and left
/// for the Command/Adapter to interpret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RawExit {
    /// Process exited with the given code (`0` = clean exit).
    Code(i32),
    /// Process was terminated by the given signal number.
    Signal(i32),
    /// Platform exposed neither an exit code nor a terminating signal.
    Unknown,
}

impl RawExit {
    /// Map a non-success exit to an [`AgentError`] for Commands whose mode
    /// produces no richer in-band signal (interactive TUI runs, and the
    /// text-only CLIs once their scanners find nothing). Returns `None` for a
    /// clean exit.
    pub(crate) fn into_failure(self) -> Option<AgentError> {
        match self {
            Self::Code(0) => None,
            Self::Code(code) => Some(AgentError::Failed {
                code: Some(code),
                message: format!("agent exited with code {code}"),
            }),
            Self::Signal(sig) => Some(AgentError::TerminatedBySignal(sig)),
            Self::Unknown => Some(AgentError::Failed {
                code: None,
                message: "agent exited with an indeterminate status".to_owned(),
            }),
        }
    }
}

/// Complete captured output of a non-interactive agent child.
///
/// Carries the full stdout and stderr byte streams (so a Command can parse
/// the CLI's machine-readable result, however far into the stream the
/// terminal event lands) plus the platform [`RawExit`]. Nothing is
/// discarded at this layer — lossy projection happens in the Command/Adapter.
#[derive(Debug, Clone)]
pub(crate) struct CommandOutput {
    /// Platform exit disposition.
    pub(crate) exit: RawExit,
    /// Complete captured stdout.
    pub(crate) stdout: Vec<u8>,
    /// Complete captured stderr.
    pub(crate) stderr: Vec<u8>,
}

impl CommandOutput {
    /// Borrow stdout as a UTF-8 string (lossy).
    pub(crate) fn stdout_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.stdout)
    }

    /// Borrow stderr as a UTF-8 string (lossy).
    pub(crate) fn stderr_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.stderr)
    }
}

/// Failure of the shared spawn primitive itself — *before* a Command gets to
/// interpret any output. Either the run was cancelled, or the child could
/// not be launched / its streams could not be driven.
#[derive(Debug)]
pub(crate) enum SpawnError {
    /// Cancellation fired before or during the run; the child (if any) has
    /// been killed.
    Cancelled,
    /// The child could not be spawned, or an I/O error occurred while
    /// writing the prompt / draining its streams.
    Launch(std::io::Error),
}

impl From<SpawnError> for AgentError {
    fn from(err: SpawnError) -> Self {
        match err {
            SpawnError::Cancelled => Self::Cancelled,
            SpawnError::Launch(io) => Self::Launch(io.to_string()),
        }
    }
}

/// How the prompt should be delivered to the child process.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PromptDelivery<'a> {
    /// Write the prompt to the child's stdin (then close stdin).
    Stdin(&'a str),
    /// The prompt was already embedded in `command.arg(...)` by the caller;
    /// no stdin data is sent and the child's stdin is closed immediately.
    Inline,
}

/// Apply user-declared environment variables to an agent [`Command`].
///
/// Called by each driver *before* iter-managed env injection so that
/// iter-internal variables (trace context, hook state files, etc.) always
/// take precedence over user-declared values with the same name.
pub(crate) fn apply_user_env(command: &mut Command, env: &[(String, String)]) {
    for (key, value) in env {
        command.env(key, value);
    }
}

/// Inject the current `OTel` trace context into an agent process environment.
///
/// This is intentionally opt-in at the driver layer. Agent CLIs differ in
/// whether they read W3C context from environment variables, and injecting a
/// carrier into every subprocess would make unsupported drivers look
/// correlated without the agent actually participating in propagation.
pub(crate) fn inject_trace_context_env(command: &mut Command) -> bool {
    iter_tracing::inject_current_context_env(command)
}

/// Inject the current trace context in the form GitHub Copilot CLI consumes.
///
/// The standalone Copilot CLI 1.0.43 does not read `TRACEPARENT` as an
/// incoming `OTel` carrier. Its SDK reads `COPILOT_TRACE_PARENT` and forwards it
/// to Copilot API calls as `X-Copilot-Traceparent`, so keep this path explicit
/// instead of reusing the generic environment-carrier helper.
pub(crate) fn inject_copilot_trace_parent_env(command: &mut Command) -> bool {
    let Some(traceparent) = iter_tracing::current_traceparent() else {
        return false;
    };
    command.env("COPILOT_TRACE_PARENT", traceparent);
    true
}

/// Add per-iteration attributes to an agent process' `OTel` resource.
///
/// Agent CLIs that produce their own telemetry generally read
/// `OTEL_RESOURCE_ATTRIBUTES` before emitting spans. Since iter launches a
/// fresh agent process for each signal, dynamic identifiers such as
/// `iter.signal.id` are safe and make the agent trace joinable with the
/// runner trace even when the agent starts a separate trace.
pub(crate) fn inject_agent_otel_resource_attrs(
    command: &mut Command,
    signal_id: SignalId,
    signal_kind: SignalKind,
    workspace_path: &Path,
    driver: &'static str,
) {
    let mut attrs = command_or_process_resource_attrs(command);
    attrs.insert("iter.signal.id".to_string(), signal_id.to_string());
    attrs.insert("iter.signal.kind".to_string(), signal_kind.to_string());
    attrs.insert("iter.agent.driver".to_string(), driver.to_string());
    attrs.insert(
        "iter.workspace.path".to_string(),
        absolute_workspace_path(workspace_path),
    );
    if let Some(traceparent) = iter_tracing::current_traceparent()
        && let Some((trace_id, span_id)) = parse_traceparent_ids(&traceparent)
    {
        attrs.insert("iter.parent.trace_id".to_string(), trace_id.to_string());
        attrs.insert("iter.parent.span_id".to_string(), span_id.to_string());
    }
    command.env(
        "OTEL_RESOURCE_ATTRIBUTES",
        iter_tracing::format_resource_attributes(
            attrs
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        ),
    );
}

fn command_or_process_resource_attrs(
    command: &Command,
) -> std::collections::BTreeMap<String, String> {
    let command_value = command
        .as_std()
        .get_envs()
        .find_map(|(key, value)| (key == "OTEL_RESOURCE_ATTRIBUTES").then_some(value));
    let value = match command_value {
        Some(Some(value)) => Some(value.to_string_lossy().into_owned()),
        Some(None) => None,
        None => std::env::var("OTEL_RESOURCE_ATTRIBUTES").ok(),
    };
    value
        .as_deref()
        .map(iter_tracing::parse_resource_attributes)
        .unwrap_or_default()
}

fn absolute_workspace_path(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

fn parse_traceparent_ids(traceparent: &str) -> Option<(&str, &str)> {
    let mut parts = traceparent.split('-');
    let version = parts.next()?;
    let trace_id = parts.next()?;
    let span_id = parts.next()?;
    let flags = parts.next()?;
    if parts.next().is_some()
        || version.len() != 2
        || trace_id.len() != 32
        || span_id.len() != 16
        || flags.len() != 2
        || trace_id == "00000000000000000000000000000000"
        || span_id == "0000000000000000"
    {
        return None;
    }
    Some((trace_id, span_id))
}

/// Splice the sandbox argv `prefix` in front of the caller's `command` and
/// return a new [`Command`] that invokes the original program under that
/// prefix.
///
/// The `prefix` is the typed command-construction data the runner reads from
/// the active workspace via
/// [`Workspace::sandbox_command_prefix`](crate::workspace::Workspace::sandbox_command_prefix)
/// and threads onto the agent invocation — it is **not** read from the
/// process environment.
///
/// The rebuilt command preserves everything the caller already
/// configured *that can be introspected*: program, args, environment
/// variables, working directory, process-group inheritance semantics.
///
/// Two child attributes have no `std`/`tokio` getter and therefore
/// **cannot** be carried across the rebuild — stdio disposition and
/// `kill_on_drop`. The spawning helper re-asserts both on the returned
/// command after the wrap: [`spawn_capture`] sets piped stdio +
/// `kill_on_drop(true)`, [`drive_interactive`] inherits stdio (the default)
/// and sets `kill_on_drop(true)`. Callers must not rely on either attribute
/// surviving a non-empty-prefix wrap.
///
/// When `prefix` is empty (the common `local`/`clone` workspace case) the
/// function is a pure pass-through — the caller's command is returned
/// verbatim, so any stdio / `kill_on_drop` it set is retained.
pub(crate) fn apply_sandbox_prefix(command: Command, prefix: &[OsString]) -> Command {
    if prefix.is_empty() {
        // Pass-through: the caller's command runs verbatim, but we still
        // install the process-group attribute so the resulting child is
        // the leader of its own group. This is what
        // `ProcessGroup::from_child` later relies on to `killpg` the
        // whole tree on cancel.
        let mut command = command;
        process_group::configure(&mut command);
        return command;
    }

    let std_cmd = command.as_std();
    let program = std_cmd.get_program().to_os_string();
    let args: Vec<OsString> = std_cmd.get_args().map(OsStr::to_os_string).collect();
    let envs: Vec<(OsString, Option<OsString>)> = std_cmd
        .get_envs()
        .map(|(k, v)| (k.to_os_string(), v.map(OsStr::to_os_string)))
        .collect();
    let cwd: Option<PathBuf> = std_cmd.get_current_dir().map(Path::to_path_buf);

    let mut wrapped = Command::new(&prefix[0]);
    for part in prefix.iter().skip(1) {
        wrapped.arg(part);
    }
    wrapped.arg(&program);
    for arg in &args {
        wrapped.arg(arg);
    }
    // Re-apply envs. `None` as the value carries over the `env_remove`
    // semantics the caller originally expressed.
    for (key, value) in envs {
        match value {
            Some(v) => {
                wrapped.env(key, v);
            }
            None => {
                wrapped.env_remove(key);
            }
        }
    }
    if let Some(cwd) = cwd {
        wrapped.current_dir(cwd);
    }
    // Install the process-group attribute on the rebuilt command so the
    // sandbox host (bwrap / sandbox-exec) becomes the group leader; every
    // descendant — including the inner program and the tools it spawns —
    // inherits the same pgid and can be reaped in one `killpg` call.
    process_group::configure(&mut wrapped);
    wrapped
}

/// Drive a prepared [`tokio::process::Command`] to completion and capture its
/// **complete** stdout/stderr plus platform exit status into a
/// [`CommandOutput`].
///
/// The caller is responsible for pre-populating the command with its program
/// name, arguments, working directory, and environment variables. This
/// helper only:
///
/// 1. Configures stdio (stdin piped, stdout/stderr piped).
/// 2. Spawns the child.
/// 3. Writes the prompt on stdin when `delivery` is
///    [`PromptDelivery::Stdin`], then closes stdin so the child sees EOF.
/// 4. Tees the child's stdout/stderr line-by-line through `sink` so every
///    line lands in `log.ndjson`, while accumulating the full byte streams
///    for the Command to parse.
/// 5. Maps the resulting [`std::process::ExitStatus`] to [`RawExit`].
///
/// Interpretation of `(exit, stdout, stderr)` — success vs. failure, error
/// class, session id — is **not** done here; that is the per-CLI Command's
/// job. This function only fails for spawn/cancel reasons ([`SpawnError`]).
pub(crate) async fn spawn_capture(
    command: Command,
    delivery: PromptDelivery<'_>,
    cancel: CancellationToken,
    sink: Arc<dyn OutputSink>,
    sandbox_prefix: &[OsString],
) -> Result<CommandOutput, SpawnError> {
    let mut command = apply_sandbox_prefix(command, sandbox_prefix);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Guarantee the child is killed if this future is dropped on cancel.
        .kill_on_drop(true);

    // Fast-path: if cancellation already fired before we spawn, don't even
    // launch the process.
    if cancel.is_cancelled() {
        return Err(SpawnError::Cancelled);
    }

    let mut child = command.spawn().map_err(SpawnError::Launch)?;
    // Record the spawned tree by its pgid so cancel can reap the entire
    // group (including grandchildren spawned by the agent's tool calls).
    let mut group = ProcessGroup::from_child(&child);

    // Take stdin up front so we can write and drop it regardless of delivery
    // mode. Closing stdin via drop is what signals EOF to readers like Claude
    // Code's `--print` loop.
    if let Some(mut stdin) = child.stdin.take() {
        if let PromptDelivery::Stdin(text) = delivery {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    group.terminate(AGENT_TERMINATION_GRACE).await;
                    drop(child.wait().await);
                    return Err(SpawnError::Cancelled);
                }
                res = stdin.write_all(text.as_bytes()) => res.map_err(SpawnError::Launch)?,
            }
        }
        // Dropping here closes the pipe and delivers EOF.
        drop(stdin);
    }

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let stdout_sink = sink.clone();
    let stdout_future = async move {
        let mut buf = Vec::new();
        if let Some(s) = stdout {
            tee_lines(s, stdout_sink, Direction::Stdout, &mut buf).await;
        }
        buf
    };
    let stderr_sink = sink.clone();
    let stderr_future = async move {
        let mut buf = Vec::new();
        if let Some(s) = stderr {
            tee_lines(s, stderr_sink, Direction::Stderr, &mut buf).await;
        }
        buf
    };

    // Spawn the tee tasks so their progress survives the cancel branch of the
    // select below. If the cancel arm wins, we still give the tee tasks a
    // bounded window to flush already-buffered bytes — the agent's last words
    // before SIGTERM.
    let stdout_handle = tokio::spawn(stdout_future);
    let stderr_handle = tokio::spawn(stderr_future);

    let mut stdout_handle = Some(stdout_handle);
    let mut stderr_handle = Some(stderr_handle);
    let (status, stdout_buf, stderr_buf) = tokio::select! {
        biased;
        () = cancel.cancelled() => {
            group.terminate(AGENT_TERMINATION_GRACE).await;
            drop(child.wait().await);
            if let Some(h) = stdout_handle.take() {
                drop(tokio::time::timeout(CANCEL_TEE_DRAIN, h).await);
            }
            if let Some(h) = stderr_handle.take() {
                drop(tokio::time::timeout(CANCEL_TEE_DRAIN, h).await);
            }
            return Err(SpawnError::Cancelled);
        }
        res = async {
            let status = child.wait().await?;
            let stdout_buf = match stdout_handle.take() {
                Some(h) => h.await.unwrap_or_default(),
                None => Vec::new(),
            };
            let stderr_buf = match stderr_handle.take() {
                Some(h) => h.await.unwrap_or_default(),
                None => Vec::new(),
            };
            Ok::<_, std::io::Error>((status, stdout_buf, stderr_buf))
        } => res.map_err(SpawnError::Launch)?,
    };

    Ok(CommandOutput {
        exit: map_exit_status(status),
        stdout: stdout_buf,
        stderr: stderr_buf,
    })
}

#[derive(Copy, Clone)]
enum Direction {
    Stdout,
    Stderr,
}

/// Tee one piped stream from the child line-by-line into `sink` (so every
/// line reaches `log.ndjson`) while accumulating the **complete** byte stream
/// in `buf` for the Command to parse.
///
/// Sink errors are swallowed (the agent run must not abort just because the
/// log writer is gone); read errors end the loop early. After EOF the
/// sink's *per-stream* partial buffer is flushed via
/// [`OutputSink::flush_stream`] so any final unterminated bytes surface as
/// their own NDJSON record without disturbing the counterpart pipe's still-
/// active partial.
async fn tee_lines<R>(reader: R, sink: Arc<dyn OutputSink>, direction: Direction, buf: &mut Vec<u8>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf_reader = BufReader::new(reader);
    let mut line = Vec::new();
    loop {
        line.clear();
        match buf_reader.read_until(b'\n', &mut line).await {
            Ok(0) => break,
            Err(err) => {
                // `read_until` may return Err with bytes already buffered
                // into `line` from a successful prior poll. Forward those
                // bytes (and keep them in `buf`) before ending the loop, so
                // we don't silently drop the agent's last output on a pipe
                // error.
                if !line.is_empty() {
                    buf.extend_from_slice(&line);
                    let chunk = Bytes::copy_from_slice(&line);
                    let send_res = match direction {
                        Direction::Stdout => sink.write_stdout(chunk).await,
                        Direction::Stderr => sink.write_stderr(chunk).await,
                    };
                    if let Err(send_err) = send_res {
                        tracing::warn!(
                            target: "iter::agent",
                            error = %send_err,
                            direction = match direction { Direction::Stdout => "stdout", Direction::Stderr => "stderr" },
                            "agent stdio sink rejected partial line on read error; continuing"
                        );
                    }
                }
                tracing::warn!(
                    target: "iter::agent",
                    error = %err,
                    direction = match direction { Direction::Stdout => "stdout", Direction::Stderr => "stderr" },
                    "agent pipe read error; ending tee"
                );
                break;
            }
            Ok(_) => {
                buf.extend_from_slice(&line);
                let chunk = Bytes::copy_from_slice(&line);
                let res = match direction {
                    Direction::Stdout => sink.write_stdout(chunk).await,
                    Direction::Stderr => sink.write_stderr(chunk).await,
                };
                if let Err(err) = res {
                    tracing::warn!(
                        target: "iter::agent",
                        error = %err,
                        direction = match direction { Direction::Stdout => "stdout", Direction::Stderr => "stderr" },
                        "agent stdio sink rejected line; continuing"
                    );
                }
            }
        }
    }
    let stream = match direction {
        Direction::Stdout => LogStream::Stdout,
        Direction::Stderr => LogStream::Stderr,
    };
    if let Err(err) = sink.flush_stream(stream).await {
        tracing::warn!(
            target: "iter::agent",
            error = %err,
            direction = match direction { Direction::Stdout => "stdout", Direction::Stderr => "stderr" },
            "agent stdio sink stream flush failed at EOF; trailing partial line may be lost"
        );
    }
}

/// Check whether an agent's output contains patterns indicating a
/// context-window or token-limit error. Returns `Some(detail)` with the
/// matched fragment when detected, `None` otherwise.
///
/// This is inherently heuristic — each CLI surfaces the error differently.
/// Patterns are intentionally conservative to avoid false positives. It is
/// the primary success/fail classifier for the text-only Commands
/// (Antigravity, Hermes `-z`) and a fallback refiner for the JSON ones.
pub(crate) fn detect_token_limit(output: &str) -> Option<String> {
    const PATTERNS: &[&str] = &[
        "context window",
        "token limit",
        "context length exceeded",
        "maximum context length",
        "too many tokens",
    ];
    let lower = output.to_ascii_lowercase();
    for pattern in PATTERNS {
        if let Some(pos) = lower.find(pattern) {
            let raw_start = pos.saturating_sub(40);
            let raw_end = (pos + pattern.len() + 40).min(output.len());
            let start = (0..=raw_start)
                .rev()
                .find(|&i| output.is_char_boundary(i))
                .unwrap_or(0);
            let end = (raw_end..=output.len())
                .find(|&i| output.is_char_boundary(i))
                .unwrap_or(output.len());
            return Some(output[start..end].to_string());
        }
    }
    None
}

/// Drive an interactive child to completion (or cancellation) and map the
/// resulting platform status onto [`RawExit`].
///
/// Unlike [`spawn_capture`], this helper assumes the caller has already
/// configured stdio (typically `Stdio::inherit()` so the TUI renders to the
/// parent terminal) and does **not** touch stdin or capture output. Hook-
/// bundle lifecycle is the caller's responsibility — pair this with
/// [`drive_interactive_with_finalize`] to get both concerns handled in a
/// single place.
pub(crate) async fn drive_interactive(
    command: Command,
    cancel: &CancellationToken,
    sandbox_prefix: &[OsString],
) -> Result<RawExit, SpawnError> {
    let mut command = apply_sandbox_prefix(command, sandbox_prefix);
    // Re-assert kill-on-drop after the wrap: a non-empty prefix rebuilds the
    // command, and `kill_on_drop` cannot be read back to carry it over. The
    // caller already set it on the pre-wrap command (preserved in the
    // pass-through case); this guarantees it on the wrapped command too, so a
    // dropped future kills the sandbox host directly even if the process-group
    // teardown does not run. Mirrors `spawn_capture`.
    command.kill_on_drop(true);
    if cancel.is_cancelled() {
        return Err(SpawnError::Cancelled);
    }

    let mut child = command.spawn().map_err(SpawnError::Launch)?;
    let mut group = ProcessGroup::from_child(&child);

    let status = tokio::select! {
        biased;
        () = cancel.cancelled() => {
            group.terminate(AGENT_TERMINATION_GRACE).await;
            drop(child.wait().await);
            return Err(SpawnError::Cancelled);
        }
        res = child.wait() => res.map_err(SpawnError::Launch)?,
    };

    Ok(map_exit_status(status))
}

/// Drive a pre-configured interactive child to completion, then finalize the
/// hook bundle regardless of whether the child succeeded or errored, and
/// return the child's [`RawExit`].
///
/// 1. Run the child via [`drive_interactive`]. Record the result but do not
///    propagate it yet — the bundle must still be finalized.
/// 2. Await the caller-supplied `finalize` future (typically
///    `bundle.finalize()`).
/// 3. If finalize failed, surface whichever error is *causal*: the run error
///    if the run itself failed, otherwise the finalize error.
/// 4. Otherwise return the child's [`RawExit`] for the caller to interpret.
pub(crate) async fn drive_interactive_with_finalize<Fut>(
    command: Command,
    cancel: CancellationToken,
    sandbox_prefix: &[OsString],
    finalize: Fut,
) -> Result<RawExit, AgentError>
where
    Fut: Future<Output = Result<(), AgentError>> + Send,
{
    let run_result = drive_interactive(command, &cancel, sandbox_prefix).await;

    if let Err(finalize_err) = finalize.await {
        return Err(match run_result {
            Err(run_err) => run_err.into(),
            Ok(_) => finalize_err,
        });
    }

    Ok(run_result?)
}

/// Map a platform [`std::process::ExitStatus`] onto [`RawExit`].
///
/// On Unix a process may terminate via a signal without ever producing an
/// exit code; `Command::status.code()` returns `None` in that case and we
/// consult `ExitStatusExt::signal()` to synthesize [`RawExit::Signal`].
fn map_exit_status(status: std::process::ExitStatus) -> RawExit {
    if let Some(code) = status.code() {
        return RawExit::Code(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return RawExit::Signal(sig);
        }
    }
    RawExit::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    #[test]
    fn raw_exit_into_failure_maps_each_disposition() {
        assert!(RawExit::Code(0).into_failure().is_none());
        assert!(matches!(
            RawExit::Code(7).into_failure(),
            Some(AgentError::Failed { code: Some(7), .. })
        ));
        assert!(matches!(
            RawExit::Signal(9).into_failure(),
            Some(AgentError::TerminatedBySignal(9))
        ));
        assert!(matches!(
            RawExit::Unknown.into_failure(),
            Some(AgentError::Failed { code: None, .. })
        ));
    }

    #[test]
    fn resource_attribute_roundtrip_escapes_separators() {
        let attrs = [
            ("service.name", "iter"),
            ("iter.workspace.path", "/tmp/a,b=c\\d"),
        ];
        let encoded = iter_tracing::format_resource_attributes(attrs);
        assert_eq!(
            iter_tracing::parse_resource_attributes(&encoded).get("iter.workspace.path"),
            Some(&"/tmp/a,b=c\\d".to_string())
        );
    }

    #[test]
    fn parse_traceparent_ids_rejects_invalid_context() {
        assert_eq!(
            parse_traceparent_ids("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
            Some(("4bf92f3577b34da6a3ce929d0e0e4736", "00f067aa0ba902b7"))
        );
        assert_eq!(
            parse_traceparent_ids("00-00000000000000000000000000000000-00f067aa0ba902b7-01"),
            None
        );
        assert_eq!(parse_traceparent_ids("not-a-traceparent"), None);
    }

    #[test]
    fn inject_agent_otel_resource_attrs_preserves_static_attrs() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: serialised via ENV_LOCK.
        unsafe {
            std::env::set_var(
                "OTEL_RESOURCE_ATTRIBUTES",
                "service.namespace=iter,deployment.environment=staging",
            );
        }
        let tmp = tempfile::tempdir().expect("tempdir");
        let signal_id = SignalId::new();
        let mut command = Command::new("/bin/echo");

        inject_agent_otel_resource_attrs(
            &mut command,
            signal_id,
            SignalKind::Work,
            tmp.path(),
            "copilot",
        );

        // SAFETY: serialised via ENV_LOCK.
        unsafe {
            std::env::remove_var("OTEL_RESOURCE_ATTRIBUTES");
        }

        let attrs = command_or_process_resource_attrs(&command);
        assert_eq!(attrs.get("service.namespace"), Some(&"iter".to_string()));
        assert_eq!(
            attrs.get("deployment.environment"),
            Some(&"staging".to_string())
        );
        assert_eq!(attrs.get("iter.signal.id"), Some(&signal_id.to_string()));
        assert_eq!(attrs.get("iter.signal.kind"), Some(&"work".to_string()));
        assert_eq!(attrs.get("iter.agent.driver"), Some(&"copilot".to_string()));
        assert_eq!(
            attrs.get("iter.workspace.path"),
            Some(&tmp.path().canonicalize().unwrap().display().to_string())
        );
    }

    // Both the OTel resource-attribute test and the legacy-protocol
    // regression test below mutate process-wide env vars; this mutex ensures
    // they never race each other. The remaining prefix-wrapping tests touch no
    // environment at all and take no lock.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn apply_sandbox_prefix_passes_through_when_empty() {
        // The verbatim (`local`/`clone`) case: an empty invocation prefix
        // leaves the caller's program and args untouched.
        let mut original = Command::new("/bin/echo");
        original.arg("hi");
        let wrapped = apply_sandbox_prefix(original, &[]);
        let std_cmd = wrapped.as_std();
        assert_eq!(std_cmd.get_program(), "/bin/echo");
        let args: Vec<_> = std_cmd.get_args().map(OsStr::to_os_string).collect();
        assert_eq!(args, vec![OsString::from("hi")]);
    }

    /// Minimal async reader that yields one chunk of bytes, then returns
    /// an `io::Error` on the next poll. Used to simulate a pipe that
    /// errors out after the agent has emitted its final partial line.
    struct ChunkThenErr {
        chunk: Option<Vec<u8>>,
        err: Option<std::io::Error>,
    }

    impl tokio::io::AsyncRead for ChunkThenErr {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if let Some(chunk) = self.chunk.take() {
                buf.put_slice(&chunk);
                std::task::Poll::Ready(Ok(()))
            } else if let Some(err) = self.err.take() {
                std::task::Poll::Ready(Err(err))
            } else {
                std::task::Poll::Ready(Ok(()))
            }
        }
    }

    /// Recording sink: captures every `write_stdout` / `write_stderr` call so
    /// tests can assert on the bytes that surfaced.
    #[derive(Default)]
    struct RecordingSink {
        stdout: tokio::sync::Mutex<Vec<Vec<u8>>>,
        stderr: tokio::sync::Mutex<Vec<Vec<u8>>>,
    }

    #[async_trait]
    impl OutputSink for RecordingSink {
        async fn write_stdout(&self, bytes: Bytes) -> std::io::Result<()> {
            self.stdout.lock().await.push(bytes.to_vec());
            Ok(())
        }
        async fn write_stderr(&self, bytes: Bytes) -> std::io::Result<()> {
            self.stderr.lock().await.push(bytes.to_vec());
            Ok(())
        }
    }

    #[tokio::test]
    async fn tee_lines_flushes_buffered_bytes_on_read_error() {
        // BufReader::read_until may return Err with bytes already buffered
        // into `line`. The tee loop must forward those bytes (and keep them
        // in `buf`) before breaking — otherwise an agent whose pipe errors
        // mid-line silently loses its final output.
        let reader = ChunkThenErr {
            chunk: Some(b"partial-no-newline".to_vec()),
            err: Some(std::io::Error::other("pipe broken mid-read")),
        };
        let recorder = Arc::new(RecordingSink::default());
        let sink: Arc<dyn OutputSink> = recorder.clone();
        let mut buf = Vec::new();

        tee_lines(reader, sink, Direction::Stdout, &mut buf).await;

        let stdout_writes = recorder.stdout.lock().await;
        assert_eq!(
            stdout_writes.len(),
            1,
            "the buffered partial must be forwarded as a single write before break"
        );
        assert_eq!(stdout_writes[0], b"partial-no-newline");
        assert_eq!(
            buf, b"partial-no-newline",
            "the capture buffer must observe the same bytes for the Command to parse"
        );
    }

    #[test]
    fn detect_token_limit_finds_known_patterns() {
        assert!(detect_token_limit("Error: context window exceeded for this model").is_some());
        assert!(detect_token_limit("token limit reached, please reduce input").is_some());
        assert!(detect_token_limit("context length exceeded").is_some());
        assert!(detect_token_limit("maximum context length is 128000 tokens").is_some());
        assert!(detect_token_limit("too many tokens in the request").is_some());
    }

    #[test]
    fn detect_token_limit_returns_none_for_unrelated_output() {
        assert!(detect_token_limit("successfully completed").is_none());
        assert!(detect_token_limit("error: file not found").is_none());
        assert!(detect_token_limit("").is_none());
    }

    #[test]
    fn detect_token_limit_handles_multibyte_utf8_context() {
        let prefix = "é".repeat(30);
        let input = format!("{prefix}context window exceeded");
        let detail = detect_token_limit(&input).expect("should match");
        assert!(detail.contains("context window"));
    }

    #[test]
    fn apply_sandbox_prefix_splices_invocation_prefix() {
        // Wrapping is driven entirely by the prefix argument — the typed
        // command-construction data the runner threads onto the agent
        // invocation. No environment is consulted.
        let prefix = vec![
            OsString::from("sandbox-exec"),
            OsString::from("-f"),
            OsString::from("/tmp/p.sb"),
        ];
        let mut original = Command::new("/bin/echo");
        original.arg("hi").arg("there");
        let wrapped = apply_sandbox_prefix(original, &prefix);
        let std_cmd = wrapped.as_std();
        assert_eq!(std_cmd.get_program(), "sandbox-exec");
        let args: Vec<_> = std_cmd.get_args().map(OsStr::to_os_string).collect();
        assert_eq!(
            args,
            vec![
                OsString::from("-f"),
                OsString::from("/tmp/p.sb"),
                OsString::from("/bin/echo"),
                OsString::from("hi"),
                OsString::from("there"),
            ]
        );
    }

    #[test]
    fn apply_sandbox_prefix_ignores_legacy_env_protocol() {
        // Regression guard: the sandbox prefix used to cross from workspace
        // setup to command launch through the `ITER_SANDBOX_COMMAND_PREFIX`
        // process-global env var. That protocol is gone — wrapping reads only
        // the invocation argument. Even with the old variable set to a hostile
        // value, an empty prefix passes through verbatim and a real prefix
        // wins.
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: serialised via ENV_LOCK so no concurrent reader observes it.
        unsafe {
            std::env::set_var("ITER_SANDBOX_COMMAND_PREFIX", "bwrap\x1f--bind\x1f/evil");
        }

        let mut verbatim = Command::new("/bin/echo");
        verbatim.arg("hi");
        let passthrough = apply_sandbox_prefix(verbatim, &[]);

        let prefix = vec![OsString::from("sandbox-exec"), OsString::from("/tmp/ok.sb")];
        let wrapped = apply_sandbox_prefix(Command::new("/bin/echo"), &prefix);

        // SAFETY: serialised via ENV_LOCK; restore before assertions so a
        // panic cannot leak state into the OTel test.
        unsafe {
            std::env::remove_var("ITER_SANDBOX_COMMAND_PREFIX");
        }

        assert_eq!(passthrough.as_std().get_program(), "/bin/echo");
        assert_eq!(wrapped.as_std().get_program(), "sandbox-exec");
    }

    #[test]
    fn concurrent_prefixes_do_not_cross_contaminate() {
        // Acceptance: concurrent runners cannot race through process-global
        // sandbox-prefix state, because no such global exists. Each call
        // carries its own prefix; wrapping many commands in parallel yields a
        // result that reflects only that call's prefix.
        use std::thread;
        let handles: Vec<_> = (0..8)
            .map(|i| {
                thread::spawn(move || {
                    let prefix = vec![
                        OsString::from(format!("sandbox-{i}")),
                        OsString::from("-f"),
                        OsString::from(format!("/tmp/{i}.sb")),
                    ];
                    let wrapped = apply_sandbox_prefix(Command::new("/bin/echo"), &prefix);
                    let std_cmd = wrapped.as_std();
                    let program = std_cmd.get_program().to_os_string();
                    let args: Vec<OsString> =
                        std_cmd.get_args().map(OsStr::to_os_string).collect();
                    (i, program, args)
                })
            })
            .collect();
        for handle in handles {
            let (i, program, args) = handle.join().expect("worker thread");
            // Both the program *and* every arg must reflect only this thread's
            // prefix — a partial isolation that co-mingled the argv tail would
            // pass a program-only assertion.
            assert_eq!(program, OsString::from(format!("sandbox-{i}")));
            assert_eq!(
                args,
                vec![
                    OsString::from("-f"),
                    OsString::from(format!("/tmp/{i}.sb")),
                    OsString::from("/bin/echo"),
                ]
            );
        }
    }
}
