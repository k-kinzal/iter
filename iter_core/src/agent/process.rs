//! Shared subprocess helper for running a CLI-backed agent.
//!
//! Every concrete agent in this crate eventually funnels through
//! [`run_command`] (non-interactive/print mode) or the pair
//! [`drive_interactive_child`] + [`drive_interactive_with_finalize`]
//! (hook-based interactive mode) so the exit-status mapping, output-tail
//! bookkeeping, stdin-plumbing, and hook-bundle finalize logic each live
//! in exactly one place.

use std::ffi::{OsStr, OsString};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use crate::process::logs::LogStream;
use crate::process::process_group::{self, ProcessGroup};
use crate::process::stdio::StdioSink;
use crate::{AgentReport, ExitStatus, current_sandbox_prefix};
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

/// Maximum number of bytes of combined stdout/stderr preserved in
/// [`AgentReport::last_output`]. This tail rides along on the
/// `AgentFinished` event for observability — debug UIs and log sinks
/// use it to peek at what the agent printed — so it only needs the
/// recent output, not the entire session log.
pub(crate) const LAST_OUTPUT_TAIL_BYTES: usize = 4096;

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

/// If [`ITER_SANDBOX_COMMAND_PREFIX`](crate::ITER_SANDBOX_COMMAND_PREFIX)
/// is exported by an enclosing
/// [`SandboxWorkspace`](crate::Workspace), splice the decoded argv
/// prefix in front of the caller's `command` and return a new
/// [`Command`] that invokes the original program under that prefix.
///
/// The rebuilt command preserves everything the caller already
/// configured: program, args, environment variables, working
/// directory, process-group inheritance semantics. Stdio is left to
/// the caller — [`run_command`] and
/// [`drive_interactive_child`] set stdio on the returned `Command`
/// after the wrap has been applied.
///
/// When the env var is unset (the common `local`/`clone` workspace
/// case) the function is a pure pass-through — no allocations, no
/// behavior change.
pub(crate) fn apply_sandbox_prefix(command: Command) -> Command {
    let prefix = current_sandbox_prefix();
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

/// Drive a prepared [`tokio::process::Command`] to completion and produce an
/// [`AgentReport`].
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
///    line lands in `log.ndjson`, while keeping a trailing
///    [`LAST_OUTPUT_TAIL_BYTES`] window per stream for
///    [`AgentReport::last_output`].
/// 5. Maps the resulting [`std::process::ExitStatus`] to [`ExitStatus`].
pub(crate) async fn run_command(
    command: Command,
    delivery: PromptDelivery<'_>,
    cancel: CancellationToken,
    sink: Arc<dyn StdioSink>,
) -> Result<AgentReport, AgentError> {
    let mut command = apply_sandbox_prefix(command);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Guarantee the child is killed if this future is dropped on cancel.
        .kill_on_drop(true);

    // Fast-path: if cancellation already fired before we spawn, don't even
    // launch the process.
    if cancel.is_cancelled() {
        return Err(AgentError::Cancelled);
    }

    let mut child = command.spawn()?;
    // Record the spawned tree by its pgid so cancel can reap the entire
    // group (including grandchildren spawned by the agent's tool calls).
    // `kill_on_drop(true)` above stays as a belt-and-suspenders fallback
    // for the direct child, but `group.terminate(...)` below is the
    // primary cancel path.
    let mut group = ProcessGroup::from_child(&child);

    // Take stdin up front so we can write and drop it regardless of delivery
    // mode. Closing stdin via drop is what signals EOF to readers like Claude
    // Code's `--print` loop.
    if let Some(mut stdin) = child.stdin.take() {
        if let PromptDelivery::Stdin(text) = delivery {
            // Write the prompt, but bail out early if cancellation fires
            // mid-write so we don't hang on a stuck reader. `child` is
            // still owned locally; on cancel, dropping it at function exit
            // kicks `kill_on_drop`.
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    group.terminate(AGENT_TERMINATION_GRACE).await;
                    drop(child.wait().await);
                    return Err(AgentError::Cancelled);
                }
                res = stdin.write_all(text.as_bytes()) => res?,
            }
        }
        // Dropping here closes the pipe and delivers EOF.
        drop(stdin);
    }

    // Take stdout/stderr handles so we can drain them concurrently without
    // giving up ownership of `child`. We keep `&mut child` so we can both
    // `wait()` and `start_kill()` it from the same select arm — something
    // `Child::wait_with_output(mut self)` would not permit.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let stdout_sink = sink.clone();
    let stdout_future = async move {
        let mut tail = Vec::new();
        if let Some(s) = stdout {
            tee_lines(s, stdout_sink, Direction::Stdout, &mut tail).await;
        }
        tail
    };
    let stderr_sink = sink.clone();
    let stderr_future = async move {
        let mut tail = Vec::new();
        if let Some(s) = stderr {
            tee_lines(s, stderr_sink, Direction::Stderr, &mut tail).await;
        }
        tail
    };

    // Spawn the tee tasks so their progress survives the cancel branch
    // of the select below. If the cancel arm wins, we still want to
    // give the tee tasks a bounded window to finish forwarding any
    // bytes already sitting in their `BufReader`s into the sink — those
    // are the agent's last words before SIGTERM, and dropping them
    // would leave the operator looking at `iter logs` wondering what
    // the agent was doing when it was killed.
    let stdout_handle = tokio::spawn(stdout_future);
    let stderr_handle = tokio::spawn(stderr_future);

    // Drain stdout/stderr concurrently with waiting for exit, under a
    // biased select on the cancel token so SIGTERM-initiated shutdowns
    // kill the child promptly. The select arms each consume their own
    // copy of the JoinHandles via `Option::take`; whichever arm runs
    // owns and awaits the handles, the other arm sees `None`s and
    // never moves them.
    let mut stdout_handle = Some(stdout_handle);
    let mut stderr_handle = Some(stderr_handle);
    let (status, stdout_tail, stderr_tail) = tokio::select! {
        biased;
        () = cancel.cancelled() => {
            // SIGTERM the whole pgid (agent + sandbox + grandchildren)
            // first, give them `AGENT_TERMINATION_GRACE` to clean up,
            // then SIGKILL anything still alive. Finally reap the
            // direct child to avoid leaking a zombie on short-lived
            // runtimes.
            group.terminate(AGENT_TERMINATION_GRACE).await;
            drop(child.wait().await);
            // Give the tee tasks a bounded window to forward any
            // already-buffered bytes into the sink before we abandon
            // them. Once `child.wait()` resolves the pipes will EOF and
            // both futures finish quickly; a stuck reader is capped at
            // CANCEL_TEE_DRAIN.
            if let Some(h) = stdout_handle.take() {
                drop(tokio::time::timeout(CANCEL_TEE_DRAIN, h).await);
            }
            if let Some(h) = stderr_handle.take() {
                drop(tokio::time::timeout(CANCEL_TEE_DRAIN, h).await);
            }
            return Err(AgentError::Cancelled);
        }
        res = async {
            let status = child.wait().await?;
            // Pipes are now closed: the tee tasks will finish reading
            // any remaining buffered bytes and return.
            let stdout_tail = match stdout_handle.take() {
                Some(h) => h.await.unwrap_or_default(),
                None => Vec::new(),
            };
            let stderr_tail = match stderr_handle.take() {
                Some(h) => h.await.unwrap_or_default(),
                None => Vec::new(),
            };
            Ok::<_, std::io::Error>((status, stdout_tail, stderr_tail))
        } => res?,
    };

    let exit_status = map_exit_status(status);
    let last_output = tail_combined(&stdout_tail, &stderr_tail, LAST_OUTPUT_TAIL_BYTES);

    Ok(AgentReport {
        exit_status,
        last_output,
        turn_count: None,
    })
}

#[derive(Copy, Clone)]
enum Direction {
    Stdout,
    Stderr,
}

/// Tee one piped stream from the child line-by-line into `sink` (so every
/// line reaches `log.ndjson`) while maintaining a trailing
/// [`LAST_OUTPUT_TAIL_BYTES`] window in `tail` for [`AgentReport::last_output`].
///
/// Sink errors are swallowed (the agent run must not abort just because the
/// log writer is gone); read errors end the loop early. After EOF the
/// sink's *per-stream* partial buffer is flushed via
/// [`StdioSink::flush_stream`] so any final unterminated bytes the agent
/// emitted (no trailing newline before exit) surface as their own NDJSON
/// record without disturbing the counterpart pipe's still-active partial
/// — using the global [`StdioSink::flush`] here would prematurely drain
/// the other stream mid-record.
async fn tee_lines<R>(reader: R, sink: Arc<dyn StdioSink>, direction: Direction, tail: &mut Vec<u8>)
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
                // bytes (treating the unterminated fragment the same way
                // we treat a no-newline EOF) before ending the loop, so
                // we don't silently drop the agent's last output on a
                // pipe error.
                if !line.is_empty() {
                    push_tail(tail, &line, LAST_OUTPUT_TAIL_BYTES);
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
                // Push the raw bytes (newline included) into the tail
                // ring so substring matches against AgentReport.last_output
                // see the same shape they did under `read_to_end`.
                push_tail(tail, &line, LAST_OUTPUT_TAIL_BYTES);
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

/// Append `chunk` to `tail`, then trim from the front so `tail.len() <= cap`.
fn push_tail(tail: &mut Vec<u8>, chunk: &[u8], cap: usize) {
    tail.extend_from_slice(chunk);
    if tail.len() > cap {
        let drop = tail.len() - cap;
        tail.drain(..drop);
    }
}

/// Check whether an agent's output contains patterns indicating a
/// context-window or token-limit error. Returns `Some(detail)` with the
/// matched fragment when detected, `None` otherwise.
///
/// This is inherently heuristic — each CLI surfaces the error differently.
/// Patterns are intentionally conservative to avoid false positives.
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
/// resulting platform status onto [`ExitStatus`].
///
/// Unlike [`run_command`], this helper assumes the caller has already
/// configured stdio (typically `Stdio::inherit()` so the TUI renders to
/// the parent terminal) and does **not** touch stdin. Hook-bundle
/// lifecycle is the caller's responsibility — pair this with
/// [`drive_interactive_with_finalize`] to get both concerns handled in a
/// single place.
///
/// The four hook-capable agents (Claude, Codex, Copilot, Gemini) each
/// used to carry a byte-identical copy of this function; it now lives
/// here exactly once.
pub(crate) async fn drive_interactive_child(
    command: Command,
    cancel: &CancellationToken,
) -> Result<ExitStatus, AgentError> {
    let mut command = apply_sandbox_prefix(command);
    if cancel.is_cancelled() {
        return Err(AgentError::Cancelled);
    }

    let mut child = command.spawn()?;
    let mut group = ProcessGroup::from_child(&child);

    let status = tokio::select! {
        biased;
        () = cancel.cancelled() => {
            group.terminate(AGENT_TERMINATION_GRACE).await;
            drop(child.wait().await);
            return Err(AgentError::Cancelled);
        }
        res = child.wait() => res?,
    };

    Ok(map_exit_status(status))
}

/// Drive a pre-configured interactive child to completion, then finalize
/// the hook bundle regardless of whether the child succeeded or errored.
///
/// This helper centralises the "interactive run + hook finalize" skeleton
/// that the four hook-capable agents (Claude, Codex, Copilot, Gemini)
/// used to each inline as ~30 lines of near-identical code. The shared
/// contract is:
///
/// 1. Run the child via [`drive_interactive_child`]. Record the result but
///    do not propagate it yet — the bundle must still be finalized.
/// 2. Await the caller-supplied `finalize` future (typically
///    `bundle.finalize()`, which consumes the bundle and restores
///    backed-up config files).
/// 3. If finalize failed, surface whichever error is *causal*: the run
///    error if the run itself failed, otherwise the finalize error. This
///    ordering ensures we never silently swallow a run failure just to
///    surface a cleanup failure instead.
/// 4. Otherwise unwrap the run result into an [`AgentReport`].
///
/// The caller is responsible for everything up to this point: installing
/// the bundle, building the command, and inheriting stdio. The finalize
/// future is passed in (rather than the bundle itself) so this helper
/// can stay bundle-type-agnostic — each agent's `HookBundle` is a
/// different concrete type with a different internal shape, but
/// `finalize(self) -> impl Future<Output = Result<(), _>>` is uniform
/// across all four.
pub(crate) async fn drive_interactive_with_finalize<Fut>(
    command: Command,
    cancel: CancellationToken,
    finalize: Fut,
) -> Result<AgentReport, AgentError>
where
    Fut: Future<Output = Result<(), AgentError>> + Send,
{
    let run_result = drive_interactive_child(command, &cancel).await;

    if let Err(finalize_err) = finalize.await {
        return Err(match run_result {
            Err(run_err) => run_err,
            Ok(_) => finalize_err,
        });
    }

    let exit_status = run_result?;

    Ok(AgentReport {
        exit_status,
        last_output: None,
        turn_count: None,
    })
}

/// Map a platform [`std::process::ExitStatus`] onto [`ExitStatus`].
///
/// On Unix a process may terminate via a signal without ever producing an
/// exit code; `Command::status.code()` returns `None` in that case and we
/// consult `ExitStatusExt::signal()` to synthesize [`ExitStatus::Signal`].
fn map_exit_status(status: std::process::ExitStatus) -> ExitStatus {
    if status.success() {
        return ExitStatus::Success;
    }
    if let Some(code) = status.code() {
        return ExitStatus::Failure(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return ExitStatus::Signal(sig);
        }
    }
    ExitStatus::Unknown
}

/// Build the [`AgentReport::last_output`] window by concatenating stdout and
/// stderr (in that order), then returning the trailing `max_bytes` — UTF-8
/// lossy-decoded so callers receive a valid Rust [`String`].
///
/// Returns `None` when the combined streams are empty.
fn tail_combined(stdout: &[u8], stderr: &[u8], max_bytes: usize) -> Option<String> {
    if stdout.is_empty() && stderr.is_empty() {
        return None;
    }
    let mut combined = Vec::with_capacity(stdout.len() + stderr.len());
    combined.extend_from_slice(stdout);
    combined.extend_from_slice(stderr);

    // Trim from the front so we keep *trailing* bytes. We cannot naively slice
    // at `len - max_bytes` because it could land mid-UTF-8-codepoint; instead
    // we slice and then run a lossy decode which replaces malformed prefixes
    // with U+FFFD. This is acceptable for the tail buffer since it's used for
    // substring matches, not structural parsing.
    let start = combined.len().saturating_sub(max_bytes);
    let slice = &combined[start..];
    Some(String::from_utf8_lossy(slice).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    #[test]
    fn tail_combined_returns_none_when_empty() {
        assert_eq!(tail_combined(&[], &[], 16), None);
    }

    #[test]
    fn tail_combined_concatenates_stdout_then_stderr() {
        let out = tail_combined(b"hello ", b"world", 64).expect("some");
        assert_eq!(out, "hello world");
    }

    #[test]
    fn tail_combined_keeps_only_trailing_bytes() {
        let stdout = vec![b'a'; 2000];
        let stderr = vec![b'b'; 3000];
        let out = tail_combined(&stdout, &stderr, 100).expect("some");
        assert_eq!(out.len(), 100);
        // All trailing bytes should be from stderr.
        assert!(out.chars().all(|c| c == 'b'));
    }

    #[test]
    fn tail_combined_handles_lossy_utf8_prefix() {
        // Start with an invalid leading byte, then valid ASCII. The replacement
        // character should appear but the valid suffix should survive.
        let stdout = [0xFF, b'o', b'k'];
        let out = tail_combined(&stdout, &[], 64).expect("some");
        assert!(out.ends_with("ok"));
    }

    #[test]
    fn push_tail_truncates_from_front() {
        let mut tail = Vec::new();
        push_tail(&mut tail, b"abcdef", 4);
        assert_eq!(tail, b"cdef");
        push_tail(&mut tail, b"GH", 4);
        assert_eq!(tail, b"efGH");
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
                "service.namespace=iter,iter.compose.project=obsidianvault",
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
            attrs.get("iter.compose.project"),
            Some(&"obsidianvault".to_string())
        );
        assert_eq!(attrs.get("iter.signal.id"), Some(&signal_id.to_string()));
        assert_eq!(attrs.get("iter.signal.kind"), Some(&"work".to_string()));
        assert_eq!(attrs.get("iter.agent.driver"), Some(&"copilot".to_string()));
        assert_eq!(
            attrs.get("iter.workspace.path"),
            Some(&tmp.path().canonicalize().unwrap().display().to_string())
        );
    }

    // `apply_sandbox_prefix` reads a process-wide env var; the two cases
    // below must be serialised, so they share a `#[serial_test]`-style
    // mutex via `std::sync::Mutex`.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn apply_sandbox_prefix_passes_through_when_env_unset() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: the lock above guarantees no other test in this
        // module observes the env var concurrently.
        unsafe {
            std::env::remove_var(crate::ITER_SANDBOX_COMMAND_PREFIX);
        }
        let mut original = Command::new("/bin/echo");
        original.arg("hi");
        let wrapped = apply_sandbox_prefix(original);
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
    impl StdioSink for RecordingSink {
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
        // Per Codex round-4 finding 1: BufReader::read_until may return
        // Err with bytes already buffered into `line` from a successful
        // prior poll. The tee loop must forward those bytes (and push
        // them into the tail ring) before breaking — otherwise an agent
        // whose pipe errors mid-line silently loses its final output.
        let reader = ChunkThenErr {
            chunk: Some(b"partial-no-newline".to_vec()),
            err: Some(std::io::Error::other("pipe broken mid-read")),
        };
        // Hold the concrete recorder via a strong Arc so we can read its
        // captured writes after `tee_lines` returns; the sink reference
        // passed to `tee_lines` is the same Arc up-cast to the trait
        // object.
        let recorder = Arc::new(RecordingSink::default());
        let sink: Arc<dyn StdioSink> = recorder.clone();
        let mut tail = Vec::new();

        tee_lines(reader, sink, Direction::Stdout, &mut tail).await;

        let stdout_writes = recorder.stdout.lock().await;
        assert_eq!(
            stdout_writes.len(),
            1,
            "the buffered partial must be forwarded as a single write before break"
        );
        assert_eq!(stdout_writes[0], b"partial-no-newline");
        assert_eq!(
            tail, b"partial-no-newline",
            "the tail ring must observe the same bytes for AgentReport.last_output"
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
    fn apply_sandbox_prefix_splices_prefix_when_env_set() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let encoded = format!("sandbox-exec{sep}-f{sep}/tmp/p.sb", sep = '\x1f');
        // SAFETY: serialised via ENV_LOCK.
        unsafe {
            std::env::set_var(crate::ITER_SANDBOX_COMMAND_PREFIX, &encoded);
        }
        let mut original = Command::new("/bin/echo");
        original.arg("hi").arg("there");
        let wrapped = apply_sandbox_prefix(original);
        // Restore before assertions run so a panic does not leak state.
        // SAFETY: serialised via ENV_LOCK.
        unsafe {
            std::env::remove_var(crate::ITER_SANDBOX_COMMAND_PREFIX);
        }
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
}
