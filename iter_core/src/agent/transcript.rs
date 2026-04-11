//! Transcript parsers for the per-agent project-local hook modules.
//!
//! * [`parse_claude_transcript`] — Claude/Codex transcript shape
//!   (`assistant` events with `message.content[].text`).
//! * [`parse_copilot_transcript`] — Copilot transcript shape
//!   (`assistant.message` events with `data.content`).

use std::path::Path;

use serde_json::Value;
use tokio::fs;

use super::AgentError;
use super::hook_lifecycle::{HookCapture, map_hook_io};

/// Parse a Claude-Code-style transcript JSONL file and return the last
/// assistant text message plus the total count of assistant text
/// messages observed.
///
/// Schema (one JSON object per line):
///
/// ```text
/// {"type":"assistant","message":{"content":[{"type":"text","text":"..."}]}}
/// ```
///
/// Used by both `agent::claude::hook` and `agent::codex::hook`, which share
/// the transcript shape.
/// Unknown / malformed lines are skipped silently so the finalize path
/// stays resilient to schema drift in newer CLI releases.
pub(crate) async fn parse_claude_transcript(path: &Path) -> Result<HookCapture, AgentError> {
    let Some(text) = read_transcript_text(path, "read assistant transcript").await? else {
        return Ok(HookCapture {
            last_output: None,
            turn_count: Some(0),
        });
    };

    let mut last: Option<String> = None;
    let mut count: u32 = 0;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        if event.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(content) = event
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for block in content {
            if block.get("type").and_then(Value::as_str) != Some("text") {
                continue;
            }
            if let Some(t) = block.get("text").and_then(Value::as_str) {
                last = Some(t.to_owned());
                count += 1;
            }
        }
    }

    Ok(HookCapture {
        last_output: last,
        turn_count: Some(count),
    })
}

/// Parse a Copilot-style transcript JSONL file and return the last
/// assistant text message plus the total count of assistant text
/// messages observed.
///
/// Schema (one JSON object per line):
///
/// ```text
/// {"type":"assistant.message","data":{"content":"..."}}
/// ```
///
/// Used by `agent::copilot::hook`. Unknown / malformed lines are skipped
/// silently.
pub(crate) async fn parse_copilot_transcript(path: &Path) -> Result<HookCapture, AgentError> {
    let Some(text) = read_transcript_text(path, "read copilot transcript").await? else {
        return Ok(HookCapture {
            last_output: None,
            turn_count: Some(0),
        });
    };

    let mut last: Option<String> = None;
    let mut count: u32 = 0;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        if event.get("type").and_then(Value::as_str) != Some("assistant.message") {
            continue;
        }
        if let Some(content) = event
            .get("data")
            .and_then(|d| d.get("content"))
            .and_then(Value::as_str)
        {
            last = Some(content.to_owned());
            count += 1;
        }
    }

    Ok(HookCapture {
        last_output: last,
        turn_count: Some(count),
    })
}

/// Read the transcript file and return its UTF-8 text body. Returns
/// `Ok(None)` when the file is missing or its bytes are not valid UTF-8
/// — both cases mean "no transcript to parse" rather than a hard error,
/// because interactive CLIs occasionally drop binary debris into
/// transcript paths and we would rather surface an empty capture than
/// abort the whole iteration.
async fn read_transcript_text(path: &Path, op: &'static str) -> Result<Option<String>, AgentError> {
    match fs::read(path).await {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Ok(Some(text)),
            Err(_) => Ok(None),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(map_hook_io(op)(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[tokio::test]
    async fn parse_claude_transcript_returns_last_and_count() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("transcript.jsonl");
        let lines = [
            json!({
                "type": "assistant",
                "message": {"content": [{"type": "text", "text": "first"}]},
            }),
            json!({"type": "user", "message": {"content": []}}),
            json!({
                "type": "assistant",
                "message": {"content": [{"type": "text", "text": "final"}]},
            }),
        ];
        let mut body = String::new();
        for line in &lines {
            body.push_str(&serde_json::to_string(line).unwrap());
            body.push('\n');
        }
        fs::write(&path, body).await.expect("write");

        let capture = parse_claude_transcript(&path).await.expect("parse");
        assert_eq!(capture.last_output.as_deref(), Some("final"));
        assert_eq!(capture.turn_count, Some(2));
    }

    #[tokio::test]
    async fn parse_claude_transcript_missing_file_returns_empty() {
        let tmp = TempDir::new().expect("tmp");
        let capture = parse_claude_transcript(&tmp.path().join("nope.jsonl"))
            .await
            .expect("parse");
        assert!(capture.last_output.is_none());
        assert_eq!(capture.turn_count, Some(0));
    }

    #[tokio::test]
    async fn parse_copilot_transcript_returns_last_and_count() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("copilot.jsonl");
        let lines = [
            json!({
                "type": "assistant.message",
                "data": {"content": "first answer"},
            }),
            json!({"type": "user.input", "data": {"content": "question"}}),
            json!({
                "type": "assistant.message",
                "data": {"content": "second answer"},
            }),
        ];
        let mut body = String::new();
        for line in &lines {
            body.push_str(&serde_json::to_string(line).unwrap());
            body.push('\n');
        }
        fs::write(&path, body).await.expect("write");

        let capture = parse_copilot_transcript(&path).await.expect("parse");
        assert_eq!(capture.last_output.as_deref(), Some("second answer"));
        assert_eq!(capture.turn_count, Some(2));
    }
}
