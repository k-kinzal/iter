//! Cross-poll deduplication helpers for the command trigger.

use serde_json::Value;

/// Canonicalize a JSON value for stable hashing across polls.
pub(super) fn canonicalize(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            // Sort keys to make object representation order-independent.
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let mut s = String::from("{");
            let mut first = true;
            for (k, v) in entries {
                if !first {
                    s.push(',');
                }
                first = false;
                s.push_str(&serde_json::to_string(k).unwrap_or_else(|_| String::from("\"\"")));
                s.push(':');
                s.push_str(&canonicalize(v));
            }
            s.push('}');
            s
        }
        Value::Array(arr) => {
            let mut s = String::from("[");
            let mut first = true;
            for v in arr {
                if !first {
                    s.push(',');
                }
                first = false;
                s.push_str(&canonicalize(v));
            }
            s.push(']');
            s
        }
        other @ (Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)) => {
            other.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_is_key_order_independent() {
        let a = serde_json::json!({"a": 1, "b": 2});
        let b = serde_json::json!({"b": 2, "a": 1});
        assert_eq!(canonicalize(&a), canonicalize(&b));
    }
}
