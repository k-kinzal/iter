//! Output-extraction modes for [`CommandTrigger`](super::CommandTrigger).

use iter_core::{MetadataValue, Queue};
use serde_json::Value;

use super::{CommandTrigger, CommandTriggerError};

/// How [`CommandTrigger`] turns command output into individual records.
///
/// All extractors run inside the iter process — no external binaries
/// are invoked. Users that need richer JSON shaping should run their
/// shaping tool (e.g. `jq -c`) inside the trigger's `run` script and
/// emit a JSON array, then use [`ExtractMode::JsonArray`].
#[derive(Debug, Clone)]
pub enum ExtractMode {
    /// Each non-empty stdout line is a record with a single `line` metadata
    /// field.
    Lines,
    /// Parse the entire stdout as a JSON array of objects. Each object's
    /// top-level fields become metadata.
    JsonArray,
    /// Parse stdout line by line with a regular expression. Named capture
    /// groups become metadata fields.
    Regex(String),
}

impl<Q: Queue + ?Sized + 'static> CommandTrigger<Q> {
    pub(super) fn extract_records(
        &self,
        stdout: &str,
        regex: Option<&regex::Regex>,
    ) -> Result<Vec<Value>, CommandTriggerError<iter_core::queue::QueueError>> {
        match &self.extract {
            ExtractMode::Lines => Ok(stdout
                .lines()
                .filter(|l| !l.is_empty())
                .map(|line| serde_json::json!({ "line": line }))
                .collect()),
            ExtractMode::JsonArray => {
                if stdout.trim().is_empty() {
                    return Ok(Vec::new());
                }
                let parsed: Value = serde_json::from_str(stdout)?;
                match parsed {
                    Value::Array(items) => Ok(items),
                    other @ (Value::Null
                    | Value::Bool(_)
                    | Value::Number(_)
                    | Value::String(_)
                    | Value::Object(_)) => Err(CommandTriggerError::Json(
                        serde::de::Error::custom(format!("expected JSON array, got {other:?}")),
                    )),
                }
            }
            ExtractMode::Regex(_) => {
                let re = regex.expect("regex compiled in run()");
                let mut out = Vec::new();
                for line in stdout.lines() {
                    if let Some(caps) = re.captures(line) {
                        let mut obj = serde_json::Map::new();
                        for name in re.capture_names().flatten() {
                            if let Some(m) = caps.name(name) {
                                obj.insert(name.to_owned(), Value::String(m.as_str().to_owned()));
                            }
                        }
                        if !obj.is_empty() {
                            out.push(Value::Object(obj));
                        }
                    }
                }
                Ok(out)
            }
        }
    }
}

pub(super) fn json_to_metadata_value(v: &Value) -> MetadataValue {
    match v {
        Value::Null => MetadataValue::Null,
        Value::Bool(b) => MetadataValue::Bool(*b),
        Value::Number(n) => n.as_i64().map_or_else(
            || MetadataValue::String(n.to_string()),
            MetadataValue::Integer,
        ),
        Value::String(s) => MetadataValue::String(s.clone()),
        other @ (Value::Array(_) | Value::Object(_)) => MetadataValue::String(other.to_string()),
    }
}

pub(super) fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other @ (Value::Array(_) | Value::Object(_)) => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_to_metadata_value_handles_scalars() {
        assert!(matches!(
            json_to_metadata_value(&serde_json::json!(true)),
            MetadataValue::Bool(true)
        ));
        assert!(matches!(
            json_to_metadata_value(&serde_json::json!(7)),
            MetadataValue::Integer(7)
        ));
        assert!(matches!(
            json_to_metadata_value(&serde_json::json!(null)),
            MetadataValue::Null
        ));
        assert!(matches!(
            json_to_metadata_value(&serde_json::json!("hi")),
            MetadataValue::String(s) if s == "hi"
        ));
    }
}
