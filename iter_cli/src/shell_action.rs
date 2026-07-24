//! [`ShellAction`] — execute `shell "..."` actions for `on <event> {}`
//! blocks.
//!
//! The action runs a shell command when the dispatcher routes an event it was
//! registered for. Because the [`EventDispatcher`](iter_core::EventDispatcher)
//! routes by [`EventName`](iter_core::EventName), the action itself carries no
//! event-name field — it is a pure action callback.
//!
//! The concrete `on { shell … }` action is an operator-configured side effect
//! (`sh -c`), not one of the six core concepts; it lives in the operator
//! surface (cli) and renders against core's public
//! [`Template`](iter_core::Template) and render views.
//!
//! # Template rendering
//!
//! The command string is compiled once into a [`Template`] and rendered
//! per-event against an [`IterationRenderContext`] — the same render path the
//! runner uses for prompts. Template variables include `{{signal.id}}`,
//! `{{signal.created_at}}`, `{{today}}`, every `{{metadata.*}}` key attached
//! to the signal, and the per-iteration `{{iteration.*}}` snapshot.
//! Signal-less lifecycle events (`runner_starting`, `runner_finished`,
//! `runner_error` raised before a signal was dequeued) render against a
//! [`RunnerRenderContext`] so `{{signal.*}}` and `{{metadata.*}}` are
//! deliberately absent.
//!
//! # Working directory
//!
//! When the triggering event carries a workspace path (everything after
//! `workspace_setup_finished`), the shell command runs with that path as its
//! cwd. Events without a workspace path (`runner_starting`, `runner_finished`,
//! `signal_received`, `workspace_setup_starting`, `runner_error`) inherit the
//! parent's cwd.
//!
//! Shell commands run via `sh -c <cmd>` and inherit the parent's stdio. A
//! non-zero exit status is *logged* but never propagated back to the runner —
//! the [`EventDispatcher`](iter_core::EventDispatcher) contract calls event
//! actions on a best-effort basis.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use iter_core::{
    BoxError, CompletionRenderContext, EventAction, Expr, HookEvent, IterationContext,
    IterationRenderContext, RunnerRenderContext, Signal, Template, TemplateError, VariableStore,
};
use iter_language::{
    ShellActionDef, ShellCaptureDef, ShellCaptureFormat, ShellCaptureMode, ShellCaptureParse,
    ShellCaptureStream,
};
use serde_json::{Value, json};
use tokio::process::Command;
use tracing::warn;

/// Event action that runs a shell command.
///
/// The action holds only the work — the compiled command template and
/// execution logic. Which event it responds to is the dispatcher's
/// responsibility at registration time.
#[derive(Debug, Clone)]
pub(crate) struct ShellAction {
    command_source: String,
    compiled: Template,
    captures: Vec<ShellCaptureDef>,
    variables: VariableStore,
}

impl ShellAction {
    /// Build an action that runs `command` when invoked.
    ///
    /// # Errors
    ///
    /// Returns [`TemplateError::InvalidSyntax`] if `command` is not a valid
    /// Handlebars template.
    pub(crate) fn new(command: impl Into<String>) -> Result<Self, TemplateError> {
        Self::from_def(&ShellActionDef::simple(command), VariableStore::new())
    }

    /// Compile a language-level shell definition against the Runner's shared
    /// variable store.
    pub(crate) fn from_def(
        definition: &ShellActionDef,
        variables: VariableStore,
    ) -> Result<Self, TemplateError> {
        let command_source = definition.script.clone();
        let compiled = Template::compile(command_source.clone())?;
        Ok(Self {
            command_source,
            compiled,
            captures: definition.captures.clone(),
            variables,
        })
    }
}

/// One declared `on <event> [when <expression>] { ... }` handler.
///
/// The condition belongs to the handler, so it is evaluated exactly once
/// against one variable snapshot before any contained action runs.
#[derive(Debug)]
pub(crate) struct ShellEventHandler {
    actions: Vec<ShellAction>,
    condition: Option<Expr>,
    variables: VariableStore,
}

impl ShellEventHandler {
    #[must_use]
    pub(crate) fn new(
        actions: Vec<ShellAction>,
        condition: Option<Expr>,
        variables: VariableStore,
    ) -> Self {
        Self {
            actions,
            condition,
            variables,
        }
    }
}

/// Execute one already-rendered hook command.
///
/// Runner and Compose hooks intentionally share this process contract:
/// `sh -c`, null stdin, inherited stdout/stderr, optional cwd, and
/// best-effort handling of non-zero exit statuses.
pub(crate) async fn run_shell_command(
    rendered: &str,
    cwd: Option<&Path>,
    env: &[(String, String)],
) -> Result<(), BoxError> {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(rendered)
        .stdin(Stdio::null())
        .envs(env.iter().cloned());
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    let status = command
        .status()
        .await
        .map_err(|error| -> BoxError { Box::new(error) })?;
    if !status.success() {
        warn!(
            command = %rendered,
            cwd = ?cwd,
            exit = ?status.code(),
            "shell action exited non-zero"
        );
    }
    Ok(())
}

impl EventAction for ShellAction {
    async fn handle(
        &self,
        event: &HookEvent,
        iteration: &IterationContext,
    ) -> Result<(), BoxError> {
        let (signal, cwd) = extract_context(event);
        let render_result = match event {
            HookEvent::RunnerCompleting { completion }
            | HookEvent::RunnerCompleted { completion } => {
                self.compiled
                    .render(&CompletionRenderContext::with_variables(
                        completion,
                        iteration,
                        self.variables.snapshot(),
                    ))
            }
            HookEvent::AgentFinished {
                signal,
                result: Ok(agent),
                ..
            } => {
                let ctx = IterationRenderContext::with_agent_and_variables(
                    signal.as_signal(),
                    iteration,
                    self.variables.snapshot(),
                    agent,
                );
                self.compiled.render(&ctx)
            }
            _ if signal.is_some() => {
                let signal = signal.expect("guarded by is_some");
                let ctx = IterationRenderContext::with_variables(
                    signal,
                    iteration,
                    self.variables.snapshot(),
                );
                self.compiled.render(&ctx)
            }
            _ => self.compiled.render(&RunnerRenderContext::with_variables(
                iteration,
                self.variables.snapshot(),
            )),
        };
        let rendered = match render_result {
            Ok(text) => text,
            Err(err) => {
                warn!(
                    command = %self.command_source,
                    error = %err,
                    "shell action template render failed"
                );
                return Ok(());
            }
        };
        if self.captures.is_empty() {
            run_shell_command(&rendered, cwd.as_deref(), &[]).await?;
        } else {
            let output =
                run_captured_shell_command(&rendered, cwd.as_deref(), &self.captures).await?;
            self.publish_captures(&output)?;
        }
        Ok(())
    }
}

impl EventAction for ShellEventHandler {
    async fn handle(
        &self,
        event: &HookEvent,
        iteration: &IterationContext,
    ) -> Result<(), BoxError> {
        if let Some(condition) = &self.condition {
            let context = expression_context(event, iteration, self.variables.snapshot())?;
            if !condition
                .evaluate_bool(&context)
                .map_err(|error| -> BoxError { Box::new(error) })?
            {
                return Ok(());
            }
        }

        // Preserve best-effort action execution: one action error is reported
        // for this handler, but does not prevent later declared actions.
        let mut first_error = None;
        for action in &self.actions {
            if let Err(error) = action.handle(event, iteration).await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

fn expression_context(
    event: &HookEvent,
    iteration: &IterationContext,
    variables: iter_core::VariableSnapshot,
) -> Result<Value, BoxError> {
    let value = match event {
        HookEvent::RunnerCompleting { completion } | HookEvent::RunnerCompleted { completion } => {
            serde_json::to_value(CompletionRenderContext::with_variables(
                completion, iteration, variables,
            ))
        }
        HookEvent::AgentFinished {
            signal,
            result: Ok(agent),
            ..
        } => serde_json::to_value(IterationRenderContext::with_agent_and_variables(
            signal.as_signal(),
            iteration,
            variables,
            agent,
        )),
        _ => match event.signal() {
            Some(signal) => serde_json::to_value(IterationRenderContext::with_variables(
                signal, iteration, variables,
            )),
            None => serde_json::to_value(RunnerRenderContext::with_variables(iteration, variables)),
        },
    }
    .map_err(|error| -> BoxError { Box::new(error) })?;
    Ok(value)
}

impl ShellAction {
    fn publish_captures(&self, output: &std::process::Output) -> Result<(), BoxError> {
        let mut updates = Vec::with_capacity(self.captures.len());
        for capture in &self.captures {
            let bytes = match capture.stream {
                ShellCaptureStream::Stdout => &output.stdout,
                ShellCaptureStream::Stderr => &output.stderr,
            };
            let current = String::from_utf8(bytes.clone()).map_err(|error| {
                Box::new(CaptureError::InvalidUtf8 {
                    name: capture.name.clone(),
                    source: error,
                }) as BoxError
            })?;
            let text = match capture.mode {
                ShellCaptureMode::Replace => current,
                ShellCaptureMode::Append => {
                    let previous = self
                        .variables
                        .get(&capture.name)
                        .and_then(|value| {
                            value
                                .get("text")
                                .and_then(Value::as_str)
                                .map(ToOwned::to_owned)
                        })
                        .unwrap_or_default();
                    previous + &current
                }
            };
            let value = capture_value(&capture.name, &text, &capture.parse)
                .map_err(|error| Box::new(error) as BoxError)?;
            updates.push((capture.name.clone(), value));
        }
        self.variables.set_many(updates);
        Ok(())
    }
}

async fn run_captured_shell_command(
    rendered: &str,
    cwd: Option<&Path>,
    captures: &[ShellCaptureDef],
) -> Result<std::process::Output, BoxError> {
    let capture_stdout = captures
        .iter()
        .any(|capture| capture.stream == ShellCaptureStream::Stdout);
    let capture_stderr = captures
        .iter()
        .any(|capture| capture.stream == ShellCaptureStream::Stderr);
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(rendered)
        .stdin(Stdio::null())
        .stdout(if capture_stdout {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        .stderr(if capture_stderr {
            Stdio::piped()
        } else {
            Stdio::inherit()
        });
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    let output = command
        .output()
        .await
        .map_err(|error| -> BoxError { Box::new(error) })?;
    if !output.status.success() {
        warn!(
            command = %rendered,
            cwd = ?cwd,
            exit = ?output.status.code(),
            "captured shell action exited non-zero"
        );
    }
    Ok(output)
}

fn capture_value(name: &str, text: &str, parse: &ShellCaptureParse) -> Result<Value, CaptureError> {
    let lines: Vec<Value> = text
        .lines()
        .map(|line| Value::String(line.to_owned()))
        .collect();
    let (format, value) = match parse {
        ShellCaptureParse::Auto => decode_auto(text),
        ShellCaptureParse::Ordered(formats) => {
            let mut failures = Vec::new();
            let mut decoded = None;
            for format in formats {
                match decode(*format, text) {
                    Ok(value) => {
                        decoded = Some((*format, value));
                        break;
                    }
                    Err(message) => failures.push(format!("{}: {message}", format.as_str())),
                }
            }
            decoded.ok_or_else(|| CaptureError::Parse {
                name: name.to_owned(),
                details: failures.join("; "),
            })?
        }
    };
    Ok(json!({
        "text": text,
        "lines": lines,
        "format": format.as_str(),
        "value": value,
    }))
}

fn decode_auto(text: &str) -> (ShellCaptureFormat, Value) {
    if let Ok(value) = decode(ShellCaptureFormat::Json, text)
        && matches!(value, Value::Object(_) | Value::Array(_))
    {
        return (ShellCaptureFormat::Json, value);
    }
    if non_empty_lines(text) >= 2
        && let Ok(value) = decode(ShellCaptureFormat::Ndjson, text)
    {
        return (ShellCaptureFormat::Ndjson, value);
    }
    if let Ok(value) = decode(ShellCaptureFormat::Toml, text)
        && value.as_object().is_some_and(|object| !object.is_empty())
    {
        return (ShellCaptureFormat::Toml, value);
    }
    if let Ok(value) = decode(ShellCaptureFormat::Yaml, text)
        && matches!(value, Value::Object(_) | Value::Array(_))
    {
        return (ShellCaptureFormat::Yaml, value);
    }
    (ShellCaptureFormat::Text, Value::String(text.to_owned()))
}

fn decode(format: ShellCaptureFormat, text: &str) -> Result<Value, String> {
    match format {
        ShellCaptureFormat::Text => Ok(Value::String(text.to_owned())),
        ShellCaptureFormat::Lines => Ok(Value::Array(
            text.lines()
                .map(|line| Value::String(line.to_owned()))
                .collect(),
        )),
        ShellCaptureFormat::Json => serde_json::from_str(text).map_err(|error| error.to_string()),
        ShellCaptureFormat::Ndjson => decode_ndjson(text),
        ShellCaptureFormat::Yaml => serde_yml::from_str(text).map_err(|error| error.to_string()),
        ShellCaptureFormat::Toml => {
            let value: toml::Value = toml::from_str(text).map_err(|error| error.to_string())?;
            serde_json::to_value(value).map_err(|error| error.to_string())
        }
        ShellCaptureFormat::Csv => decode_delimited(text, b','),
        ShellCaptureFormat::Tsv => decode_delimited(text, b'\t'),
    }
}

fn decode_ndjson(text: &str) -> Result<Value, String> {
    let mut values = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value =
            serde_json::from_str(line).map_err(|error| format!("line {}: {error}", index + 1))?;
        values.push(value);
    }
    if values.is_empty() {
        return Err("no non-empty JSON lines".to_owned());
    }
    Ok(Value::Array(values))
}

fn decode_delimited(text: &str, delimiter: u8) -> Result<Value, String> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(false)
        .from_reader(text.as_bytes());
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|error| error.to_string())?;
        rows.push(Value::Array(
            record
                .iter()
                .map(|field| Value::String(field.to_owned()))
                .collect(),
        ));
    }
    if rows.is_empty() {
        return Err("no delimited rows".to_owned());
    }
    Ok(Value::Array(rows))
}

fn non_empty_lines(text: &str) -> usize {
    text.lines().filter(|line| !line.trim().is_empty()).count()
}

#[derive(Debug, thiserror::Error)]
enum CaptureError {
    #[error("capture `{name}` was not valid UTF-8")]
    InvalidUtf8 {
        name: String,
        #[source]
        source: std::string::FromUtf8Error,
    },
    #[error("capture `{name}` did not match any configured parser: {details}")]
    Parse { name: String, details: String },
}

/// Extract the signal + optional workspace-path pair that a shell action
/// should use when processing `event`.
fn extract_context(event: &HookEvent) -> (Option<&Signal>, Option<PathBuf>) {
    match event {
        HookEvent::SignalReceived { signal } | HookEvent::WorkspaceSetupStarting { signal } => {
            (Some(signal.as_signal()), None)
        }
        HookEvent::WorkspaceSetupFinished { signal, path }
        | HookEvent::AgentStarting { signal, path, .. }
        | HookEvent::AgentFinished { signal, path, .. }
        | HookEvent::WorkspaceTeardownStarting { signal, path }
        | HookEvent::WorkspaceTeardownFinished { signal, path } => {
            (Some(signal.as_signal()), Some(path.clone()))
        }
        HookEvent::DequeueFailed { .. }
        | HookEvent::RenderPromptFailed { .. }
        | HookEvent::WorkspaceSetupFailed { .. }
        | HookEvent::AgentRunFailed { .. }
        | HookEvent::WorkspaceTeardownFailed { .. }
        | HookEvent::CompletionConditionFailed { .. }
        | HookEvent::RunnerStarting {}
        | HookEvent::RunnerCompleting { .. }
        | HookEvent::RunnerCompleted { .. }
        | HookEvent::RunnerFinished { .. } => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iter_core::{
        AgentRun, BinaryOp, EventDispatcher, EventName, ExprLiteral, Metadata, MetadataKey,
        MetadataValue, PathSegment, Signal, VariableStore,
    };

    fn iter_ctx() -> IterationContext {
        IterationContext::for_test()
    }

    fn empty_signal() -> Signal {
        Signal::new(Metadata::new())
    }

    fn torndown_event(path: PathBuf) -> HookEvent {
        HookEvent::WorkspaceTeardownFinished {
            signal: empty_signal().into(),
            path,
        }
    }

    fn agent_finished_event(path: PathBuf, run: AgentRun) -> HookEvent {
        HookEvent::AgentFinished {
            signal: empty_signal().into(),
            path,
            result: Ok(run),
        }
    }

    fn decision_is(expected: &str) -> Expr {
        Expr::Binary {
            lhs: Box::new(Expr::Path {
                root: "agent".to_owned(),
                segments: vec![
                    PathSegment::Field("output".to_owned()),
                    PathSegment::Field("decision".to_owned()),
                ],
            }),
            op: BinaryOp::Eq,
            rhs: Box::new(Expr::Literal(ExprLiteral::String(expected.to_owned()))),
        }
    }

    fn capture(
        name: &str,
        stream: ShellCaptureStream,
        mode: ShellCaptureMode,
        parse: ShellCaptureParse,
    ) -> ShellCaptureDef {
        ShellCaptureDef {
            name: name.to_owned(),
            stream,
            mode,
            parse,
        }
    }

    #[tokio::test]
    async fn shell_action_only_runs_on_registered_event() {
        let action = ShellAction::new("true").expect("compile");
        let mut dispatcher = EventDispatcher::new();
        dispatcher.on(EventName::AgentFinished, action);

        dispatcher
            .emit(&torndown_event(PathBuf::from("/tmp")), &iter_ctx())
            .await;
    }

    #[tokio::test]
    async fn shell_action_logs_but_does_not_propagate_nonzero_exit() {
        let action = ShellAction::new("false").expect("compile");
        action
            .handle(&torndown_event(PathBuf::from("/tmp")), &iter_ctx())
            .await
            .expect("must not propagate");
    }

    #[tokio::test]
    async fn capture_publishes_json_for_later_templates() {
        let variables = VariableStore::new();
        let definition = ShellActionDef {
            script: "printf '{\"foo\":1}\\n'".into(),
            captures: vec![capture(
                "context",
                ShellCaptureStream::Stdout,
                ShellCaptureMode::Replace,
                ShellCaptureParse::Auto,
            )],
        };
        let action = ShellAction::from_def(&definition, variables.clone()).expect("compile");
        action
            .handle(&HookEvent::RunnerStarting {}, &iter_ctx())
            .await
            .expect("capture");

        let template = Template::compile("{{var.context.value.foo}}/{{var.context.lines.[0]}}")
            .expect("template");
        let rendered = template
            .render(&RunnerRenderContext::with_variables(
                &iter_ctx(),
                variables.snapshot(),
            ))
            .expect("render captured var");
        assert_eq!(rendered, "1/{\"foo\":1}");
    }

    #[tokio::test]
    async fn append_reparses_the_complete_stream_as_ndjson() {
        let variables = VariableStore::new();
        let replace = ShellActionDef {
            script: "printf '{\"foo\":1}\\n'".into(),
            captures: vec![capture(
                "context",
                ShellCaptureStream::Stdout,
                ShellCaptureMode::Replace,
                ShellCaptureParse::Auto,
            )],
        };
        let append = ShellActionDef {
            script: "printf '{\"foo\":2}\\n'".into(),
            captures: vec![capture(
                "context",
                ShellCaptureStream::Stdout,
                ShellCaptureMode::Append,
                ShellCaptureParse::Auto,
            )],
        };
        ShellAction::from_def(&replace, variables.clone())
            .expect("replace compile")
            .handle(&HookEvent::RunnerStarting {}, &iter_ctx())
            .await
            .expect("replace");
        ShellAction::from_def(&append, variables.clone())
            .expect("append compile")
            .handle(&HookEvent::RunnerStarting {}, &iter_ctx())
            .await
            .expect("append");

        let value = variables.get("context").expect("context");
        assert_eq!(value["format"], "ndjson");
        assert_eq!(value["value"][0]["foo"], 1);
        assert_eq!(value["value"][1]["foo"], 2);
    }

    #[test]
    fn text_and_lines_are_always_available_when_auto_falls_back() {
        let value =
            capture_value("table", "a,b\n1,2\n", &ShellCaptureParse::Auto).expect("text fallback");
        assert_eq!(value["format"], "text");
        assert_eq!(value["text"], "a,b\n1,2\n");
        assert_eq!(value["lines"], json!(["a,b", "1,2"]));
        assert_eq!(value["value"], "a,b\n1,2\n");
    }

    #[test]
    fn auto_does_not_misclassify_empty_or_comment_only_text_as_toml() {
        for source in ["", "# generated later\n"] {
            let value =
                capture_value("context", source, &ShellCaptureParse::Auto).expect("text fallback");
            assert_eq!(value["format"], "text", "source={source:?}");
            assert_eq!(value["value"], source);
        }
    }

    #[test]
    fn every_explicit_format_decodes_to_a_json_shaped_value() {
        let cases = [
            (ShellCaptureFormat::Text, "plain\n", json!("plain\n")),
            (
                ShellCaptureFormat::Lines,
                "alpha\nbeta\n",
                json!(["alpha", "beta"]),
            ),
            (ShellCaptureFormat::Json, "{\"foo\":1}", json!({"foo": 1})),
            (
                ShellCaptureFormat::Ndjson,
                "{\"foo\":1}\n{\"foo\":2}\n",
                json!([{"foo": 1}, {"foo": 2}]),
            ),
            (
                ShellCaptureFormat::Csv,
                "name,count\nalpha,1\n",
                json!([["name", "count"], ["alpha", "1"]]),
            ),
            (
                ShellCaptureFormat::Tsv,
                "name\tcount\nalpha\t1\n",
                json!([["name", "count"], ["alpha", "1"]]),
            ),
            (ShellCaptureFormat::Yaml, "foo: 1\n", json!({"foo": 1})),
            (ShellCaptureFormat::Toml, "foo = 1\n", json!({"foo": 1})),
        ];
        for (format, source, expected) in cases {
            assert_eq!(
                decode(format, source).unwrap_or_else(|error| panic!("{format:?}: {error}")),
                expected,
            );
        }
    }

    #[test]
    fn ordered_parsers_use_the_first_successful_format() {
        let value = capture_value(
            "table",
            "name,count\nalpha,1\n",
            &ShellCaptureParse::Ordered(vec![
                ShellCaptureFormat::Json,
                ShellCaptureFormat::Csv,
                ShellCaptureFormat::Text,
            ]),
        )
        .expect("CSV fallback");

        assert_eq!(value["format"], "csv");
        assert_eq!(value["value"], json!([["name", "count"], ["alpha", "1"]]));
    }

    #[tokio::test]
    async fn stdout_and_stderr_can_be_captured_independently() {
        let variables = VariableStore::new();
        let definition = ShellActionDef {
            script: "printf out; printf err >&2".into(),
            captures: vec![
                capture(
                    "output",
                    ShellCaptureStream::Stdout,
                    ShellCaptureMode::Replace,
                    ShellCaptureParse::Ordered(vec![ShellCaptureFormat::Text]),
                ),
                capture(
                    "errors",
                    ShellCaptureStream::Stderr,
                    ShellCaptureMode::Replace,
                    ShellCaptureParse::Ordered(vec![ShellCaptureFormat::Text]),
                ),
            ],
        };
        ShellAction::from_def(&definition, variables.clone())
            .expect("compile")
            .handle(&HookEvent::RunnerStarting {}, &iter_ctx())
            .await
            .expect("capture");

        assert_eq!(variables.get("output").expect("output")["text"], "out");
        assert_eq!(variables.get("errors").expect("errors")["text"], "err");
    }

    #[tokio::test]
    async fn multi_capture_publish_is_atomic_when_one_parser_fails() {
        let variables = VariableStore::new();
        variables.set("output", json!({"text": "old"}));
        let definition = ShellActionDef {
            script: "printf new; printf not-json >&2".into(),
            captures: vec![
                capture(
                    "output",
                    ShellCaptureStream::Stdout,
                    ShellCaptureMode::Replace,
                    ShellCaptureParse::Ordered(vec![ShellCaptureFormat::Text]),
                ),
                capture(
                    "errors",
                    ShellCaptureStream::Stderr,
                    ShellCaptureMode::Replace,
                    ShellCaptureParse::Ordered(vec![ShellCaptureFormat::Json]),
                ),
            ],
        };
        let error = ShellAction::from_def(&definition, variables.clone())
            .expect("compile")
            .handle(&HookEvent::RunnerStarting {}, &iter_ctx())
            .await
            .expect_err("second parser must fail");

        assert!(error.to_string().contains("capture `errors`"));
        assert_eq!(variables.get("output"), Some(json!({"text": "old"})));
        assert_eq!(variables.get("errors"), None);
    }

    #[tokio::test]
    async fn shell_action_renders_signal_and_metadata_templates() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().to_path_buf();

        let mut metadata = Metadata::new();
        metadata.insert(
            MetadataKey::new("file").expect("key"),
            MetadataValue::String("src/lib.rs".into()),
        );
        let signal = Signal::new(metadata);
        let signal_id = signal.id().to_string();

        let action =
            ShellAction::new("echo {{metadata.file}}:{{signal.id}} > marker.txt").expect("compile");
        action
            .handle(
                &HookEvent::WorkspaceTeardownFinished {
                    signal: signal.into(),
                    path: ws.clone(),
                },
                &iter_ctx(),
            )
            .await
            .expect("action ok");

        let marker = ws.join("marker.txt");
        let contents = std::fs::read_to_string(&marker).expect("marker");
        assert!(
            contents.contains("src/lib.rs"),
            "metadata not rendered: {contents:?}"
        );
        assert!(
            contents.contains(&signal_id),
            "signal.id not rendered: {contents:?}"
        );
    }

    #[tokio::test]
    async fn shell_action_renders_iteration_root() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().to_path_buf();
        let signal = Signal::new(Metadata::new());

        let action = ShellAction::new(
            "echo n={{iteration.count}} prev={{iteration.previous_result}} > iter.txt",
        )
        .expect("compile");
        let iteration = IterationContext::for_count(7);
        action
            .handle(
                &HookEvent::WorkspaceTeardownFinished {
                    signal: signal.into(),
                    path: ws.clone(),
                },
                &iteration,
            )
            .await
            .expect("action ok");

        let contents = std::fs::read_to_string(ws.join("iter.txt")).expect("iter.txt");
        assert!(
            contents.contains("n=7"),
            "iteration.count missing: {contents:?}"
        );
        assert!(
            contents.contains("prev=none"),
            "iteration.previous_result missing: {contents:?}"
        );
    }

    #[tokio::test]
    async fn agent_finished_renders_structured_agent_output() {
        let tmp = tempfile::tempdir().expect("tmp");
        let action =
            ShellAction::new("echo {{agent.output.decision}} > decision.txt").expect("compile");
        action
            .handle(
                &agent_finished_event(
                    tmp.path().to_path_buf(),
                    AgentRun::empty()
                        .with_json_output(json!({"decision": "fix", "notes": ["missing test"]})),
                ),
                &iter_ctx(),
            )
            .await
            .expect("action");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("decision.txt"))
                .expect("decision")
                .trim(),
            "fix",
        );
    }

    #[tokio::test]
    async fn agent_finished_renders_text_agent_output() {
        let tmp = tempfile::tempdir().expect("tmp");
        let action =
            ShellAction::new("printf '%s' '{{agent.output}}' > response.txt").expect("compile");
        action
            .handle(
                &agent_finished_event(
                    tmp.path().to_path_buf(),
                    AgentRun::empty().with_text_output("plain response"),
                ),
                &iter_ctx(),
            )
            .await
            .expect("action");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("response.txt")).expect("response"),
            "plain response",
        );
    }

    #[tokio::test]
    async fn agent_finished_condition_controls_shell_execution() {
        let tmp = tempfile::tempdir().expect("tmp");
        let event = agent_finished_event(
            tmp.path().to_path_buf(),
            AgentRun::empty().with_json_output(json!({"decision": "continue"})),
        );
        ShellEventHandler::new(
            vec![ShellAction::new("touch skipped").expect("compile")],
            Some(decision_is("fix")),
            VariableStore::new(),
        )
        .handle(&event, &iter_ctx())
        .await
        .expect("false condition");
        ShellEventHandler::new(
            vec![ShellAction::new("touch selected").expect("compile")],
            Some(decision_is("continue")),
            VariableStore::new(),
        )
        .handle(&event, &iter_ctx())
        .await
        .expect("true condition");
        assert!(!tmp.path().join("skipped").exists());
        assert!(tmp.path().join("selected").exists());
    }

    #[tokio::test]
    async fn handler_condition_error_is_reported() {
        let tmp = tempfile::tempdir().expect("tmp");
        let score = Expr::Path {
            root: "agent".to_owned(),
            segments: vec![
                PathSegment::Field("output".to_owned()),
                PathSegment::Field("score".to_owned()),
            ],
        };
        let condition = Expr::Binary {
            lhs: Box::new(Expr::Binary {
                lhs: Box::new(score),
                op: BinaryOp::Mod,
                rhs: Box::new(Expr::Literal(ExprLiteral::Integer(2))),
            }),
            op: BinaryOp::Eq,
            rhs: Box::new(Expr::Literal(ExprLiteral::Integer(0))),
        };
        let handler = ShellEventHandler::new(
            vec![ShellAction::new("touch must-not-run").expect("compile")],
            Some(condition),
            VariableStore::new(),
        );
        let event = agent_finished_event(
            tmp.path().to_path_buf(),
            AgentRun::empty().with_json_output(json!({"score": 0.5})),
        );

        let error = handler
            .handle(&event, &iter_ctx())
            .await
            .expect_err("invalid condition operands must be reported");
        assert!(error.to_string().contains("operator %"));
        assert!(!tmp.path().join("must-not-run").exists());
    }

    #[tokio::test]
    async fn handler_condition_is_evaluated_once_before_all_actions() {
        let tmp = tempfile::tempdir().expect("tmp");
        let variables = VariableStore::new();
        variables.set("gate", json!({"value": "run"}));
        let condition = Expr::Binary {
            lhs: Box::new(Expr::Path {
                root: "var".to_owned(),
                segments: vec![
                    PathSegment::Field("gate".to_owned()),
                    PathSegment::Field("value".to_owned()),
                ],
            }),
            op: BinaryOp::Eq,
            rhs: Box::new(Expr::Literal(ExprLiteral::String("run".to_owned()))),
        };
        let update_gate = ShellAction::from_def(
            &ShellActionDef {
                script: "printf stop".to_owned(),
                captures: vec![capture(
                    "gate",
                    ShellCaptureStream::Stdout,
                    ShellCaptureMode::Replace,
                    ShellCaptureParse::Ordered(vec![ShellCaptureFormat::Text]),
                )],
            },
            variables.clone(),
        )
        .expect("compile");
        let second = ShellAction::from_def(
            &ShellActionDef::simple("touch second-ran"),
            variables.clone(),
        )
        .expect("compile");
        let handler = ShellEventHandler::new(
            vec![update_gate, second],
            Some(condition),
            variables.clone(),
        );

        handler
            .handle(&torndown_event(tmp.path().to_path_buf()), &iter_ctx())
            .await
            .expect("handler");

        assert_eq!(variables.get("gate").expect("gate")["value"], "stop");
        assert!(
            tmp.path().join("second-ran").exists(),
            "later actions use the handler's original condition decision"
        );
    }

    #[tokio::test]
    async fn shell_action_lifecycle_event_renders_iteration_only() {
        let action = ShellAction::new("true {{iteration.count}} {{today}}").expect("compile");
        action
            .handle(&HookEvent::RunnerStarting {}, &iter_ctx())
            .await
            .expect("lifecycle action ok");
    }

    #[tokio::test]
    async fn shell_action_lifecycle_event_with_signal_root_is_swallowed() {
        let action = ShellAction::new("echo {{signal.id}}").expect("compile");
        action
            .handle(&HookEvent::RunnerStarting {}, &iter_ctx())
            .await
            .expect("template error must be swallowed");
    }

    #[tokio::test]
    async fn shell_action_template_error_is_logged_not_propagated() {
        let action = ShellAction::new("echo {{metadata.nonexistent}}").expect("compile");
        action
            .handle(&torndown_event(PathBuf::from("/tmp")), &iter_ctx())
            .await
            .expect("template error must be swallowed");
    }

    #[tokio::test]
    async fn shell_action_uses_workspace_path_as_cwd() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().to_path_buf();

        let action = ShellAction::new("pwd > pwd.txt").expect("compile");
        action
            .handle(
                &HookEvent::WorkspaceTeardownFinished {
                    signal: empty_signal().into(),
                    path: ws.clone(),
                },
                &iter_ctx(),
            )
            .await
            .expect("action ok");

        let pwd_contents = std::fs::read_to_string(ws.join("pwd.txt")).expect("pwd file");
        let observed = PathBuf::from(pwd_contents.trim());
        let expected = std::fs::canonicalize(&ws).unwrap_or_else(|_| ws.clone());
        let observed_canon = std::fs::canonicalize(&observed).unwrap_or_else(|_| observed.clone());
        assert_eq!(observed_canon, expected);
    }

    #[tokio::test]
    async fn extract_context_returns_none_for_runner_lifecycle_events() {
        let (signal, cwd) = extract_context(&HookEvent::RunnerStarting {});
        assert!(signal.is_none());
        assert!(cwd.is_none());

        let (signal, cwd) = extract_context(&HookEvent::RunnerFinished {
            reason: iter_core::RunnerTerminationReason::Once,
            iteration_count: 0,
            last_signal_id: None,
            event_handler_error_count: 0,
            observer_error_count: 0,
        });
        assert!(signal.is_none());
        assert!(cwd.is_none());
    }
}
