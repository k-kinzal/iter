//! Small helpers shared by the per-CLI Commands that parse a
//! machine-readable JSON or JSON-lines stream.
//!
//! These are deliberately format-shaped, not CLI-shaped: each Command owns
//! its own result/error structs and field→conclusion logic, but the
//! mechanics of "find the terminal JSON object in a stream" are identical
//! across Claude, Cursor, Codex, Copilot, Gemini, Cline, `OpenCode` and Grok.

use serde_json::Value;

/// Parse `stdout` as a single JSON value (the `--output-format json` /
/// `-o json` contract: the whole stream is one JSON document).
///
/// Returns `None` when the stream is empty or not valid JSON.
pub(crate) fn single_object(stdout: &str) -> Option<Value> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

/// Scan a JSON-lines / stream-json `stdout` and return the **last** line that
/// parses as a JSON object whose `"type"` (or `"kind"`) field equals
/// `marker`.
///
/// This implements the "terminal result event" lookup used by the streaming
/// CLIs: the result lives in the final `result` / `run_result` / turn-status
/// record, which may be preceded by any number of progress events.
pub(crate) fn last_event_of_type(stdout: &str, marker: &str) -> Option<Value> {
    last_event_matching(stdout, |obj| {
        obj.get("type")
            .or_else(|| obj.get("kind"))
            .and_then(Value::as_str)
            == Some(marker)
    })
}

/// Scan a JSON-lines `stdout` and return the **last** object line for which
/// `pred` holds. Lines that do not parse as a JSON object are skipped.
pub(crate) fn last_event_matching<F>(stdout: &str, pred: F) -> Option<Value>
where
    F: Fn(&serde_json::Map<String, Value>) -> bool,
{
    let mut found = None;
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(line)
            && pred(&obj)
        {
            found = Some(Value::Object(obj));
        }
    }
    found
}

/// Return the first object line in a JSON-lines `stdout` for which `pred`
/// holds (earliest occurrence). Used to surface the first error event.
pub(crate) fn first_event_matching<F>(stdout: &str, pred: F) -> Option<Value>
where
    F: Fn(&serde_json::Map<String, Value>) -> bool,
{
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(line)
            && pred(&obj)
        {
            return Some(Value::Object(obj));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_object_parses_whole_stream() {
        let v = single_object(r#"{"a":1,"b":"x"}"#).expect("parse");
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], "x");
    }

    #[test]
    fn single_object_none_for_garbage() {
        assert!(single_object("not json").is_none());
        assert!(single_object("   ").is_none());
    }

    #[test]
    fn last_event_of_type_finds_terminal_record() {
        let stream = concat!(
            "{\"type\":\"progress\",\"n\":1}\n",
            "{\"type\":\"result\",\"is_error\":false,\"session_id\":\"s1\"}\n",
        );
        let v = last_event_of_type(stream, "result").expect("result");
        assert_eq!(v["session_id"], "s1");
    }

    #[test]
    fn last_event_of_type_returns_last_match() {
        let stream = concat!(
            "{\"type\":\"result\",\"n\":1}\n",
            "{\"type\":\"result\",\"n\":2}\n",
        );
        let v = last_event_of_type(stream, "result").expect("result");
        assert_eq!(v["n"], 2);
    }

    #[test]
    fn last_event_of_type_none_when_absent() {
        let stream = "{\"type\":\"progress\"}\n";
        assert!(last_event_of_type(stream, "result").is_none());
    }

    #[test]
    fn first_event_matching_returns_earliest() {
        let stream = concat!(
            "{\"type\":\"error\",\"n\":1}\n",
            "{\"type\":\"error\",\"n\":2}\n",
        );
        let v = first_event_matching(stream, |o| {
            o.get("type").and_then(Value::as_str) == Some("error")
        })
        .expect("error");
        assert_eq!(v["n"], 1);
    }
}
