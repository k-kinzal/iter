//! `{{...}}` template-placeholder validation.

use super::Analyzer;
use crate::ast::Span;
use crate::diagnostic::Diagnostic;

impl Analyzer {
    /// Walk a string body looking for `{{...}}` placeholders and ensure each
    /// reference is `metadata.<ident>`, `signal.<ident>`, `event.<path>`,
    /// `iteration.<field>`, or `today`. Unknown references emit warnings
    /// (since the runner ultimately decides) — malformed syntax is an error.
    pub(super) fn validate_template(&mut self, body: &str, span: &Span) {
        let bytes = body.as_bytes();
        let mut i = 0;
        while i + 1 < bytes.len() {
            if bytes[i] == b'{' && bytes[i + 1] == b'{' {
                let start = i;
                i += 2;
                let inner_start = i;
                while i + 1 < bytes.len() && !(bytes[i] == b'}' && bytes[i + 1] == b'}') {
                    i += 1;
                }
                if i + 1 >= bytes.len() {
                    let abs = span.start + start;
                    self.errors.push(Diagnostic::error(
                        abs..span.end.min(span.start + bytes.len()),
                        "unterminated `{{...}}` template placeholder",
                    ));
                    return;
                }
                let inner = &body[inner_start..i].trim();
                if inner.is_empty() {
                    let abs_start = span.start + start;
                    let abs_end = span.start + i + 2;
                    self.errors.push(Diagnostic::error(
                        abs_start..abs_end,
                        "empty `{{}}` template placeholder",
                    ));
                } else if !is_valid_template_ref(inner) {
                    let abs_start = span.start + start;
                    let abs_end = span.start + i + 2;
                    self.errors.push(
                        Diagnostic::error(
                            abs_start..abs_end,
                            format!("invalid template reference `{inner}`"),
                        )
                        .with_hint(
                            "valid forms: `metadata.<key>`, `signal.<field>`, `event.<path>`, `iteration.<field>`, `today`",
                        ),
                    );
                }
                i += 2;
            } else {
                i += 1;
            }
        }
    }
}

fn is_valid_template_ref(text: &str) -> bool {
    let text = text.trim();
    if text == "today" {
        return true;
    }
    let mut parts = text.split('.');
    let Some(head) = parts.next() else {
        return false;
    };
    if !matches!(head, "metadata" | "signal" | "event" | "iteration") {
        return false;
    }
    let mut tail_count = 0;
    for part in parts {
        tail_count += 1;
        if part.is_empty() {
            return false;
        }
        if !part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return false;
        }
    }
    tail_count >= 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_ref_validation() {
        assert!(is_valid_template_ref("metadata.foo"));
        assert!(is_valid_template_ref("signal.id"));
        assert!(is_valid_template_ref("event.repository.full_name"));
        assert!(is_valid_template_ref("iteration.count"));
        assert!(is_valid_template_ref("iteration.previous_outcome"));
        assert!(is_valid_template_ref("today"));
        assert!(!is_valid_template_ref(""));
        assert!(!is_valid_template_ref("metadata"));
        assert!(!is_valid_template_ref("iteration"));
        assert!(!is_valid_template_ref("foo.bar"));
    }
}
