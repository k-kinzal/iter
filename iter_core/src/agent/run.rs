//! [`AgentRun`] — iter's domain result for a single agent run.
//!
//! This is the **Agent level** of the three-layer agent stack (Command →
//! Driver/Adapter → Agent). It is intentionally minimal: it carries only
//! what iter itself consumes or what its exploration Factors read. The rich,
//! CLI-shaped result lives at the Command level (`drivers/<cli>/command.rs`)
//! and is projected down to this type by each driver acting as an Adapter.
//!
//! There is deliberately **no exit code** here. A successful [`AgentRun`]
//! means "the agent ran"; a non-zero / failed run is an
//! [`AgentError`](crate::agent::AgentError), not an `Ok` carrying a failure
//! field. iter assigns no task-meaning to an exit code, so the exit code
//! never crosses the Adapter boundary into this domain type.

use serde::{Deserialize, Serialize};

/// Final response produced by an Agent.
///
/// The enum serializes without an envelope so Hook templates can use
/// `agent.output` for text and `agent.output.<field>` for structured JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentOutput {
    /// Unstructured final response.
    Text(String),
    /// JSON response validated against the configured schema.
    Json(serde_json::Value),
}

/// Result of one successful agent run, in iter's domain vocabulary.
///
/// Surfaced through
/// [`HookEvent::AgentFinished`](crate::runner::HookEvent::AgentFinished) so event
/// handlers and observers can correlate the run (e.g. against the session
/// it belongs to). The struct is `#[non_exhaustive]` so new Factor-relevant
/// fields can be added without breaking downstream construction.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AgentRun {
    /// Session / conversation id reported by the underlying CLI, when it
    /// exposes one and the driver parsed it from the Command result. Feeds
    /// iter's session-log and continuous-context-persistence Factors, which
    /// key continuity off a stable session identity across runs.
    pub session_id: Option<String>,
    /// Final response exposed to `agent_finished` Hooks, when the driver can
    /// capture one.
    pub output: Option<AgentOutput>,
}

impl AgentRun {
    /// A run that carries no correlation data. Used by drivers whose CLI has
    /// no machine-readable session identity (or whose mode does not surface
    /// one), and by the built-in agents.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// A run carrying the session id the CLI reported.
    #[must_use]
    pub fn with_session_id(session_id: impl Into<String>) -> Self {
        Self {
            session_id: Some(session_id.into()),
            output: None,
        }
    }

    /// Attach an unstructured final response.
    #[must_use]
    pub fn with_text_output(mut self, output: impl Into<String>) -> Self {
        self.output = Some(AgentOutput::Text(output.into()));
        self
    }

    /// Attach a structured final response.
    #[must_use]
    pub fn with_json_output(mut self, output: serde_json::Value) -> Self {
        self.output = Some(AgentOutput::Json(output));
        self
    }
}

pub(crate) fn parse_structured_output(
    schema: &serde_json::Value,
    text: Option<&str>,
) -> Result<AgentOutput, crate::agent::AgentError> {
    let text = text.ok_or_else(|| crate::agent::AgentError::Failed {
        code: None,
        message: "agent produced no final response for `output_schema`".to_owned(),
    })?;
    let value = serde_json::from_str(text).map_err(|error| crate::agent::AgentError::Failed {
        code: None,
        message: format!("agent response is not valid JSON: {error}"),
    })?;
    validate_structured_output(schema, value)
}

pub(crate) fn validate_structured_output(
    schema: &serde_json::Value,
    value: serde_json::Value,
) -> Result<AgentOutput, crate::agent::AgentError> {
    let validator = jsonschema::validator_for(schema).map_err(|error| {
        crate::agent::AgentError::Launch(format!(
            "configured `output_schema` could not be compiled: {error}"
        ))
    })?;
    validator
        .validate(&value)
        .map_err(|error| crate::agent::AgentError::Failed {
            code: None,
            message: format!("agent response does not satisfy `output_schema`: {error}"),
        })?;
    Ok(AgentOutput::Json(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn review_schema() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "decision": {
                    "type": "string",
                    "enum": ["fix", "continue"]
                }
            },
            "required": ["decision"],
            "additionalProperties": false
        })
    }

    #[test]
    fn structured_output_is_parsed_and_validated() {
        let output = parse_structured_output(&review_schema(), Some(r#"{"decision":"fix"}"#))
            .expect("valid structured output");
        assert_eq!(output, AgentOutput::Json(json!({"decision": "fix"})));
    }

    #[test]
    fn structured_output_rejects_schema_mismatch() {
        let error = parse_structured_output(&review_schema(), Some(r#"{"decision":"unknown"}"#))
            .expect_err("enum mismatch");
        assert!(
            error
                .to_string()
                .contains("does not satisfy `output_schema`")
        );
    }

    #[test]
    fn structured_output_rejects_non_json() {
        let error = parse_structured_output(&review_schema(), Some("fix")).expect_err("non-JSON");
        assert!(error.to_string().contains("not valid JSON"));
    }
}
