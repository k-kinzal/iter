//! Shared child-process machinery for the agent cycle.
//!
//! This is plumbing, not a concept: the [`Agent`](crate::agent::Agent)'s
//! private helpers for driving a **spawned** child through one exchange.
//! Creation belongs to the workspace seam
//! ([`ActiveWorkspace::spawn`](crate::workspace::ActiveWorkspace::spawn));
//! everything from birth to reaping lives here:
//!
//! * [`feed_and_capture`] — piped exchange: deliver the stdin payload, tee
//!   stdout/stderr line-by-line into the output sink while accumulating the
//!   **complete** streams, honour cancellation with a graceful
//!   SIGTERM→SIGKILL window, and report the exit faithfully.
//! * [`wait_inherited`] — interactive exchange: the child owns the
//!   terminal; only wait for its exit (same cancellation discipline).
//!
//! Both return a plain [`std::process::Output`] for the driver's
//! [`interpret`](crate::agent::AgentDriver::interpret) to read; signal
//! terminations survive the trip through
//! [`ExitStatus`](std::process::ExitStatus) on Unix.
//!
//! The module also keeps the env-injection helpers drivers opt into while
//! building their commands, and the shared token-limit detector.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::log::{LogStream, OutputSink};
use crate::process_group::ProcessGroup;
use bytes::Bytes;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

use super::AgentError;

/// Grace period applied between SIGTERM and SIGKILL when the runner cancels
/// an agent process tree. Five seconds matches the upper bound most CLI
/// agents use for their own shutdown handlers; anything longer just keeps
/// the runner blocked.
///
/// The runner's iteration-timeout drain window is derived from this constant
/// so it always exceeds the SIGTERM grace; if you change one, the other
/// follows automatically.
pub(crate) const AGENT_TERMINATION_GRACE: Duration = Duration::from_secs(5);

/// Bound on how long the stdio tee tasks are awaited after the agent
/// has been cancelled. Cancellation kills the child (closing the pipes
/// from the kernel side), which makes the tee `read` calls return
/// promptly; this cap protects against a stuck pipe (e.g. a frozen
/// sandbox host process holding the read end). One second matches the
/// observed worst case in tests and is small enough that operator
/// shutdown latency stays imperceptible.
const CANCEL_TEE_DRAIN: Duration = Duration::from_secs(1);

/// Maximum raw stdout/stderr tail recorded in telemetry for each agent stream.
///
/// The full streams are still captured for driver parsing and teed to
/// `log.ndjson` line-by-line, but the structured raw-process telemetry event
/// carries only the last 64 KiB per stream to keep OTel/log payloads bounded.
/// The event also records the complete byte lengths and truncation booleans,
/// so operators can tell when the tail is incomplete. These bytes are not
/// redacted here; they already transit the process log sink unchanged today.
pub(crate) const RAW_AGENT_STDIO_TAIL_BYTES: usize = 64 * 1024;

/// Platform exit disposition of an agent child process — *before* any
/// CLI-specific interpretation.
///
/// `Code(0)` is the only success disposition; a non-zero code, a terminating
/// signal, or an indeterminate status are all reported faithfully and left
/// for the driver's `interpret` to read.
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
    /// Stable label for the platform exit disposition.
    pub(crate) fn disposition(self) -> &'static str {
        match self {
            Self::Code(_) => "exited",
            Self::Signal(_) => "signal",
            Self::Unknown => "unknown",
        }
    }

    /// Process exit code, when the platform reported one.
    pub(crate) fn exit_code(self) -> Option<i32> {
        match self {
            Self::Code(code) => Some(code),
            Self::Signal(_) | Self::Unknown => None,
        }
    }

    /// Map a non-success exit to an [`AgentError`] for drivers whose mode
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

    /// Convert this observed platform exit into an [`std::process::ExitStatus`].
    ///
    /// The capture helpers use this to hand drivers a standard
    /// [`std::process::Output`]; on Unix a signal termination survives the
    /// round trip (`ExitStatusExt::signal`).
    pub(crate) fn into_exit_status(self) -> std::process::ExitStatus {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            match self {
                Self::Code(code) => std::process::ExitStatus::from_raw(code << 8),
                Self::Signal(signal) => std::process::ExitStatus::from_raw(signal),
                Self::Unknown => std::process::ExitStatus::from_raw(1 << 8),
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt;
            match self {
                Self::Code(code) => {
                    let code = u32::try_from(code).unwrap_or(1);
                    std::process::ExitStatus::from_raw(code)
                }
                Self::Signal(_) | Self::Unknown => std::process::ExitStatus::from_raw(1),
            }
        }
    }
}

/// Borrowed view of a completed run's output for a driver's `interpret`:
/// the platform exit re-read as [`RawExit`] plus the raw byte streams.
pub(crate) struct RawOutput<'a> {
    /// Platform exit disposition.
    pub(crate) exit: RawExit,
    /// Complete captured stdout (empty for inherited-stdio runs).
    pub(crate) stdout: &'a [u8],
    /// Complete captured stderr (empty for inherited-stdio runs).
    pub(crate) stderr: &'a [u8],
}

impl<'a> From<&'a std::process::Output> for RawOutput<'a> {
    fn from(output: &'a std::process::Output) -> Self {
        Self {
            exit: map_exit_status(output.status),
            stdout: &output.stdout,
            stderr: &output.stderr,
        }
    }
}

impl RawOutput<'_> {
    /// Borrow stdout as a UTF-8 string (lossy).
    pub(crate) fn stdout_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(self.stdout)
    }

    /// Borrow stderr as a UTF-8 string (lossy).
    pub(crate) fn stderr_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(self.stderr)
    }
}

fn raw_stdio_tail_lossy(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(RAW_AGENT_STDIO_TAIL_BYTES);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

struct NullableExitCode(Option<i32>);

impl std::fmt::Display for NullableExitCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(code) => write!(f, "{code}"),
            None => f.write_str("null"),
        }
    }
}

/// Emit the raw-process telemetry event and record the exit attributes on
/// the current span. Fired once per child on every path — clean exit,
/// failure, cancellation, and even launch failure (with `Unknown`).
pub(crate) fn record_raw_agent_process(exit: RawExit, stdout: &[u8], stderr: &[u8]) {
    let exit_code = exit.exit_code();
    let exit_disposition = exit.disposition();
    let span = tracing::Span::current();
    if let Some(code) = exit_code {
        span.record("iter.agent.exit_code", code);
    }
    span.record("iter.agent.exit_disposition", exit_disposition);

    tracing::info!(
        target: "iter::agent",
        {
            "iter.agent.event" = "raw_process_output",
            "iter.agent.exit_code" = %NullableExitCode(exit_code),
            "iter.agent.exit_disposition" = exit_disposition,
            "iter.agent.stdout.bytes" = stdout.len() as u64,
            "iter.agent.stderr.bytes" = stderr.len() as u64,
            "iter.agent.stdout.tail" = %raw_stdio_tail_lossy(stdout),
            "iter.agent.stderr.tail" = %raw_stdio_tail_lossy(stderr),
            "iter.agent.stdout.tail_truncated" = stdout.len() > RAW_AGENT_STDIO_TAIL_BYTES,
            "iter.agent.stderr.tail_truncated" = stderr.len() > RAW_AGENT_STDIO_TAIL_BYTES,
        },
        "agent raw process output captured"
    );
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
///
/// The signal correlation attributes are read from the ambient
/// [`iter_tracing::iteration_scope`] the runner opens around each iteration;
/// outside any scope (standalone driver tests, direct library use) they are
/// simply omitted.
pub(crate) fn inject_agent_otel_resource_attrs(
    command: &mut Command,
    workspace_path: &Path,
    driver: &'static str,
) {
    let mut attrs = command_or_process_resource_attrs(command);
    if let Some(iteration) = iter_tracing::current_iteration_attrs() {
        attrs.insert("iter.signal.id".to_string(), iteration.signal_id);
        attrs.insert("iter.signal.kind".to_string(), iteration.signal_kind);
    } else {
        tracing::debug!(
            target: "iter::agent",
            driver,
            "no iteration scope active; omitting iter.signal.* resource attributes",
        );
    }
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

/// Drive a **spawned** piped child through one exchange: feed the stdin
/// payload, tee stdout/stderr into `sink` while capturing the complete
/// streams, and report the exit as a standard [`std::process::Output`].
///
/// Cancellation terminates the child's whole process group (SIGTERM, a
/// [`AGENT_TERMINATION_GRACE`] window, then SIGKILL), drains the tee tasks
/// for up to [`CANCEL_TEE_DRAIN`] so the agent's last words still reach the
/// sink, and returns [`AgentError::Cancelled`].
///
/// # Errors
///
/// Returns [`AgentError::Cancelled`] on cooperative cancellation and
/// [`AgentError::Launch`] when the child's streams cannot be driven.
pub(crate) async fn feed_and_capture(
    mut child: Child,
    stdin: Option<&str>,
    cancel: &CancellationToken,
    sink: &Arc<dyn OutputSink>,
) -> Result<std::process::Output, AgentError> {
    // Record the spawned tree by its pgid so cancel can reap the entire
    // group (including grandchildren spawned by the agent's tool calls).
    let mut group = ProcessGroup::from_child(&child);

    // Take stdin up front so we can write and drop it regardless of delivery
    // mode. Closing stdin via drop is what signals EOF to readers like Claude
    // Code's `--print` loop.
    if let Some(mut stdin_pipe) = child.stdin.take() {
        if let Some(text) = stdin {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    group.terminate(AGENT_TERMINATION_GRACE).await;
                    let exit = child
                        .wait()
                        .await
                        .map(map_exit_status)
                        .unwrap_or(RawExit::Unknown);
                    // The tee tasks have not been spawned yet, so there is
                    // no captured output to attach — but the once-per-child
                    // raw-telemetry contract still holds on this path.
                    record_raw_agent_process(exit, &[], &[]);
                    return Err(AgentError::Cancelled);
                }
                res = stdin_pipe.write_all(text.as_bytes()) => {
                    if let Err(err) = res {
                        group.terminate(AGENT_TERMINATION_GRACE).await;
                        drop(child.wait().await);
                        record_raw_agent_process(RawExit::Unknown, &[], &[]);
                        return Err(AgentError::Launch(err.to_string()));
                    }
                }
            }
        }
        // Dropping here closes the pipe and delivers EOF.
        drop(stdin_pipe);
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
    let mut stdout_handle = Some(tokio::spawn(stdout_future));
    let mut stderr_handle = Some(tokio::spawn(stderr_future));

    let (status, stdout_buf, stderr_buf) = tokio::select! {
        biased;
        () = cancel.cancelled() => {
            group.terminate(AGENT_TERMINATION_GRACE).await;
            let exit = child
                .wait()
                .await
                .map(map_exit_status)
                .unwrap_or(RawExit::Unknown);
            let stdout_buf = drain_tee_on_cancel(stdout_handle.take()).await;
            let stderr_buf = drain_tee_on_cancel(stderr_handle.take()).await;
            record_raw_agent_process(exit, &stdout_buf, &stderr_buf);
            return Err(AgentError::Cancelled);
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
        } => match res {
            Ok(output) => output,
            Err(err) => {
                record_raw_agent_process(RawExit::Unknown, &[], &[]);
                return Err(AgentError::Launch(err.to_string()));
            }
        },
    };

    let exit = map_exit_status(status);
    record_raw_agent_process(exit, &stdout_buf, &stderr_buf);
    Ok(std::process::Output {
        status,
        stdout: stdout_buf,
        stderr: stderr_buf,
    })
}

/// Wait out a **spawned** child whose stdio is inherited (interactive TUI).
///
/// Nothing is captured — the child owns the terminal; only the exit status
/// speaks, returned inside an [`std::process::Output`] with empty streams.
/// Cancellation follows the same group-terminate discipline as
/// [`feed_and_capture`].
///
/// # Errors
///
/// Returns [`AgentError::Cancelled`] on cooperative cancellation and
/// [`AgentError::Launch`] when the child cannot be awaited.
pub(crate) async fn wait_inherited(
    mut child: Child,
    cancel: &CancellationToken,
) -> Result<std::process::Output, AgentError> {
    let mut group = ProcessGroup::from_child(&child);

    let status = tokio::select! {
        biased;
        () = cancel.cancelled() => {
            group.terminate(AGENT_TERMINATION_GRACE).await;
            let exit = child
                .wait()
                .await
                .map(map_exit_status)
                .unwrap_or(RawExit::Unknown);
            record_raw_agent_process(exit, &[], &[]);
            return Err(AgentError::Cancelled);
        }
        res = child.wait() => match res {
            Ok(status) => status,
            Err(err) => {
                record_raw_agent_process(RawExit::Unknown, &[], &[]);
                return Err(AgentError::Launch(err.to_string()));
            }
        },
    };

    record_raw_agent_process(map_exit_status(status), &[], &[]);
    Ok(std::process::Output {
        status,
        stdout: Vec::new(),
        stderr: Vec::new(),
    })
}

async fn drain_tee_on_cancel(handle: Option<tokio::task::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    let Some(handle) = handle else {
        return Vec::new();
    };
    match tokio::time::timeout(CANCEL_TEE_DRAIN, handle).await {
        Ok(Ok(buf)) => buf,
        Ok(Err(_)) | Err(_) => Vec::new(),
    }
}

#[derive(Copy, Clone)]
enum Direction {
    Stdout,
    Stderr,
}

/// Tee one piped stream from the child line-by-line into `sink` (so every
/// line reaches `log.ndjson`) while accumulating the **complete** byte stream
/// in `buf` for the driver to parse.
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
/// the primary success/fail classifier for the text-only drivers
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

/// Map a platform [`std::process::ExitStatus`] onto [`RawExit`].
///
/// On Unix a process may terminate via a signal without ever producing an
/// exit code; `Command::status.code()` returns `None` in that case and we
/// consult `ExitStatusExt::signal()` to synthesize [`RawExit::Signal`].
pub(crate) fn map_exit_status(status: std::process::ExitStatus) -> RawExit {
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
    use std::sync::Mutex;

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

    /// The plan's risk item: signal terminations must survive the
    /// `RawExit` → `ExitStatus` → `RawExit` round trip that carries a run's
    /// exit into the driver's `interpret`.
    #[cfg(unix)]
    #[test]
    fn raw_exit_exit_status_round_trip_preserves_code_and_signal() {
        for exit in [RawExit::Code(0), RawExit::Code(7), RawExit::Signal(9)] {
            assert_eq!(map_exit_status(exit.into_exit_status()), exit);
        }
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

    // The OTel resource-attribute tests mutate the process-wide
    // `OTEL_RESOURCE_ATTRIBUTES` env var; this mutex ensures they never race.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
        let mut command = Command::new("/bin/echo");

        inject_agent_otel_resource_attrs(&mut command, tmp.path(), "copilot");

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
        assert_eq!(attrs.get("iter.agent.driver"), Some(&"copilot".to_string()));
        assert_eq!(
            attrs.get("iter.workspace.path"),
            Some(&tmp.path().canonicalize().unwrap().display().to_string())
        );
        // Outside any iteration scope, the signal attributes are omitted.
        assert_eq!(attrs.get("iter.signal.id"), None);
        assert_eq!(attrs.get("iter.signal.kind"), None);
    }

    /// Sync test + hand-built runtime so the env lock is held in the sync
    /// frame, never across an await point inside an async body.
    #[test]
    fn inject_agent_otel_resource_attrs_reads_iteration_scope() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().expect("tempdir");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let attrs = runtime.block_on(iter_tracing::iteration_scope(
            iter_tracing::IterationAttrs::new("sig-abc", "work"),
            async {
                let mut command = Command::new("/bin/echo");
                inject_agent_otel_resource_attrs(&mut command, tmp.path(), "claude");
                command_or_process_resource_attrs(&command)
            },
        ));
        assert_eq!(attrs.get("iter.signal.id"), Some(&"sig-abc".to_string()));
        assert_eq!(attrs.get("iter.signal.kind"), Some(&"work".to_string()));
    }

    #[test]
    fn raw_stdio_tail_is_bounded_to_last_bytes() {
        let mut bytes = vec![b'a'; RAW_AGENT_STDIO_TAIL_BYTES + 32];
        bytes.extend_from_slice(b"tail-marker");

        let tail = raw_stdio_tail_lossy(&bytes);

        assert_eq!(tail.len(), RAW_AGENT_STDIO_TAIL_BYTES);
        assert!(tail.ends_with("tail-marker"));
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
            "the capture buffer must observe the same bytes for the driver to parse"
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
}
