//! Event pattern matching, guard evaluation, and metadata rendering for the
//! webhook trigger.

use serde_json::Value;
use serde_json::json;

use iter_core::template::TemplateError;
use iter_core::{Metadata, MetadataValue};

use super::config::CompiledSubscription;

/// Render the compiled metadata templates of `subscription` against the
/// webhook `event` body.
///
/// Each template is rendered with the context `{"event": event}` so source
/// templates written as `{{event.x.y}}` resolve the same dotted paths as
/// under the previous ad-hoc implementation.
pub(super) fn render_metadata(
    subscription: &CompiledSubscription,
    event: &Value,
) -> Result<Metadata, TemplateError> {
    let mut metadata = Metadata::new();
    let ctx = json!({ "event": event });
    for (key, template) in &subscription.metadata {
        let rendered = template.render(&ctx)?;
        metadata.insert(key.clone(), MetadataValue::String(rendered));
    }
    Ok(metadata)
}

/// Match a `foo.bar` style event pattern against an actual event key.
///
/// Each `*` segment is a wildcard for that segment.
pub(crate) fn event_pattern_matches(pattern: &str, value: &str) -> bool {
    let pat_parts: Vec<&str> = pattern.split('.').collect();
    let val_parts: Vec<&str> = value.split('.').collect();
    if pat_parts.len() != val_parts.len() {
        return false;
    }
    pat_parts
        .iter()
        .zip(val_parts.iter())
        .all(|(p, v)| *p == "*" || *p == *v)
}

/// Evaluate a guard expression of the form
/// `{{event.path.to.field}} == 'literal'`.
///
/// Anything not matching that exact shape is treated as `true` for v1.
///
/// This is intentionally a hand-rolled mini-evaluator — `when` is not yet
/// expressible as a proper AST in the language layer, so handlebars'
/// placeholder grammar on its own is not sufficient here. Extending this
/// beyond equality against a literal is blocked on an AST-level redesign of
/// `when` (tracked separately from the template refactor).
pub(crate) fn evaluate_guard(expr: &str, event: &Value) -> bool {
    let trimmed = expr.trim();
    let Some((lhs, rhs)) = trimmed.split_once("==") else {
        return true;
    };
    let lhs = lhs.trim();
    let rhs = rhs.trim();
    let Some(literal) = strip_quotes(rhs) else {
        return true;
    };
    let Some(path_inner) = strip_template(lhs) else {
        return true;
    };
    let Some(path) = path_inner.strip_prefix("event.") else {
        return true;
    };
    let actual = lookup(event, path).map(value_to_string).unwrap_or_default();
    actual == literal
}

fn strip_quotes(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')
            || (bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"'))
    {
        Some(&s[1..s.len() - 1])
    } else {
        None
    }
}

fn strip_template(s: &str) -> Option<&str> {
    let trimmed = s.trim();
    let inner = trimmed.strip_prefix("{{")?.strip_suffix("}}")?;
    Some(inner.trim())
}

/// Look up a dotted path inside a JSON value, honouring array index
/// segments the same way the legacy `{{event.xs.1}}` placeholder did.
fn lookup<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = root;
    for segment in path.split('.') {
        if segment.is_empty() {
            return None;
        }
        cur = match cur {
            Value::Object(map) => map.get(segment)?,
            Value::Array(arr) => {
                let idx: usize = segment.parse().ok()?;
                arr.get(idx)?
            }
            _ => return None,
        };
    }
    Some(cur)
}

/// Convert a JSON scalar to the string it should substitute in as.
fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_pattern_exact_match() {
        assert!(event_pattern_matches("issues.opened", "issues.opened"));
        assert!(!event_pattern_matches("issues.opened", "issues.closed"));
    }

    #[test]
    fn event_pattern_wildcard() {
        assert!(event_pattern_matches("issues.*", "issues.opened"));
        assert!(event_pattern_matches("*.opened", "issues.opened"));
        assert!(!event_pattern_matches("issues.*", "pull_request.opened"));
    }

    #[test]
    fn guard_equality_match() {
        let event = json!({ "action": "opened" });
        assert!(evaluate_guard("{{event.action}} == 'opened'", &event));
        assert!(!evaluate_guard("{{event.action}} == 'closed'", &event));
    }

    #[test]
    fn guard_unsupported_returns_true() {
        let event = json!({ "action": "opened" });
        assert!(evaluate_guard("anything goes", &event));
    }

    #[test]
    fn guard_missing_path_compares_to_empty() {
        let event = json!({ "other": "x" });
        // Missing path renders to "" which equals ''
        assert!(evaluate_guard("{{event.missing}} == ''", &event));
    }

    #[test]
    fn guard_array_index_lookup() {
        let event = json!({ "xs": [10, 20, 30] });
        assert!(evaluate_guard("{{event.xs.1}} == '20'", &event));
    }
}
