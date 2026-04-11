//! Diagnostics produced by the parser and semantic analyzer.

use crate::ast::Span;
use ariadne::{Color, Label, Report, ReportKind, Source};

/// Severity of a [`Diagnostic`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// A blocking issue. The parser refuses to return a successful AST when
    /// any error-severity diagnostic is present.
    Error,
    /// A non-blocking warning. The AST is still returned to callers.
    Warning,
}

impl Severity {
    fn report_kind(self) -> ReportKind<'static> {
        match self {
            Severity::Error => ReportKind::Error,
            Severity::Warning => ReportKind::Warning,
        }
    }
}

/// A single diagnostic produced during lexing, parsing, or semantic analysis.
///
/// Diagnostics carry a [`Severity`], a source [`Span`], a human-readable
/// `message`, and an optional `hint` describing how to fix the problem.
/// Use [`Diagnostic::report`] to obtain a pretty, source-annotated rendering
/// powered by `ariadne`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Severity of the diagnostic.
    pub severity: Severity,
    /// Byte range inside the source the diagnostic refers to.
    pub span: Span,
    /// Primary, human-readable message.
    pub message: String,
    /// Optional hint that suggests a fix.
    pub hint: Option<String>,
}

impl Diagnostic {
    /// Build a new error diagnostic.
    pub fn error(span: Span, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            span,
            message: message.into(),
            hint: None,
        }
    }

    /// Build a new warning diagnostic.
    pub fn warning(span: Span, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            span,
            message: message.into(),
            hint: None,
        }
    }

    /// Attach an optional hint to this diagnostic, returning `self`.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Render this diagnostic into a human-friendly, source-annotated string.
    ///
    /// `source_name` is used as the file label in the output (e.g.
    /// `"path/to/file"` — any display label works). `source` must be the
    /// original source text.
    ///
    /// # Panics
    ///
    /// Panics only if writing to an in-memory `Vec<u8>` buffer fails, which
    /// the standard library guarantees cannot happen for non-OOM cases.
    #[must_use]
    pub fn report(&self, source_name: &str, source: &str) -> String {
        // Clamp the span so an out-of-bounds value (e.g. EOF after recovery)
        // does not panic ariadne.
        let max = source.len();
        let start = self.span.start.min(max);
        let end = self.span.end.min(max).max(start);
        let span = start..end;

        let color = match self.severity {
            Severity::Error => Color::Red,
            Severity::Warning => Color::Yellow,
        };

        let mut builder = Report::build(self.severity.report_kind(), source_name, start)
            .with_message(&self.message)
            .with_label(
                Label::new((source_name, span))
                    .with_message(&self.message)
                    .with_color(color),
            );
        if let Some(hint) = &self.hint {
            builder = builder.with_help(hint);
        }
        let report = builder.finish();

        let mut buf: Vec<u8> = Vec::new();
        // ariadne returns Result for I/O errors; writing to a Vec cannot fail.
        report
            .write((source_name, Source::from(source)), &mut buf)
            .expect("writing diagnostic to in-memory buffer must not fail");
        // Strip ANSI escape codes so snapshot tests are stable across
        // platforms and TTY detection. The escape characters interleave with
        // text: `\x1b[<params>m`. We strip them with a tiny inline scanner.
        strip_ansi(&String::from_utf8_lossy(&buf))
    }
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if let Some('[') = chars.peek() {
                chars.next();
                while let Some(&n) = chars.peek() {
                    chars.next();
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
                continue;
            }
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_is_stripped() {
        let stripped = strip_ansi("\x1b[31mhello\x1b[0m world");
        assert_eq!(stripped, "hello world");
    }

    #[test]
    fn report_renders_without_panic() {
        let source = "queue redis { }\n";
        let diag = Diagnostic::error(0..15, "queue redis requires `url`")
            .with_hint("add `url = \"redis://...\"`");
        let rendered = diag.report("path/to/file", source);
        assert!(rendered.contains("queue redis requires"));
        assert!(rendered.contains("path/to/file"));
    }
}
