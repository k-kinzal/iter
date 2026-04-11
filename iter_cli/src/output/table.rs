//! Elastic-tab tables for CLI listings.
//!
//! Wraps [`tabwriter`] so subcommands never hand-format columns. Every
//! list-like view flows through this helper — hand-formatted `{:<26}`
//! strings drift the moment a field's max width changes.
//!
//! Output layout:
//!
//! - Two spaces between columns (matches `docker ps` style).
//! - Header row uppercase, written verbatim.
//! - Trailing newline at the end of the rendered block.
//! - No trailing whitespace on any line.

use std::io::Write;

use tabwriter::TabWriter;

use super::stream::cli_println;

/// Elastic table renderer.
pub(crate) struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl Table {
    /// Build a new table with the given header row.
    #[must_use]
    pub(crate) fn new(headers: &[&str]) -> Self {
        Self {
            headers: headers.iter().map(|h| (*h).to_owned()).collect(),
            rows: Vec::new(),
        }
    }

    /// Append a row.
    pub(crate) fn row<I, S>(&mut self, values: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.rows.push(values.into_iter().map(Into::into).collect());
    }

    /// Render the table and write it to stdout.
    pub(crate) fn print(self) {
        let rendered = self.render();
        for line in rendered.lines() {
            cli_println!("{line}");
        }
    }

    /// Render the table to a `String`. Useful in tests.
    #[must_use]
    pub(crate) fn render(self) -> String {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut tw = TabWriter::new(&mut buf).minwidth(0).padding(2);
            writeln!(tw, "{}", self.headers.join("\t")).expect("in-memory write");
            for row in &self.rows {
                writeln!(tw, "{}", row.join("\t")).expect("in-memory write");
            }
            tw.flush().expect("in-memory flush");
        }
        let raw = String::from_utf8(buf).expect("TabWriter produced non-UTF8: unreachable");
        let mut out = String::with_capacity(raw.len());
        for line in raw.lines() {
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_header_only_when_no_rows() {
        let table = Table::new(&["ID", "STATUS"]);
        let out = table.render();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("ID"));
        assert!(lines[0].contains("STATUS"));
    }

    #[test]
    fn aligns_short_and_long_values() {
        let mut t = Table::new(&["ID", "NAME"]);
        t.row(["abc".to_owned(), "alpha".to_owned()]);
        t.row(["xyz1234567".to_owned(), "long-name-here".to_owned()]);
        let out = t.render();
        assert!(
            out.is_ascii(),
            "this alignment test assumes ASCII-only output"
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        let header_id_end = lines[0].find("NAME").expect("NAME header present");
        for line in &lines[1..] {
            assert!(
                line.len() > header_id_end,
                "row shorter than header alignment: {line:?}"
            );
            let prefix = &line[..header_id_end];
            assert!(
                prefix.ends_with("  "),
                "row's first column does not pad to the second-column \
                 boundary with two spaces; prefix={prefix:?}, line={line:?}"
            );
            let second_col_byte = line.as_bytes()[header_id_end];
            assert!(
                second_col_byte != b' ',
                "second column does not start at header_id_end; line={line:?}"
            );
        }
    }

    #[test]
    fn no_trailing_whitespace_on_lines() {
        let mut t = Table::new(&["ID", "NAME"]);
        t.row(["a".to_owned(), "b".to_owned()]);
        let out = t.render();
        for line in out.lines() {
            assert_eq!(
                line.trim_end(),
                line,
                "trailing whitespace on line: {line:?}"
            );
        }
    }

    #[test]
    fn empty_last_cell_has_no_trailing_whitespace() {
        let mut t = Table::new(&["KIND", "NAME", "DETAIL"]);
        t.row(["queue".to_owned(), "default".to_owned(), String::new()]);
        t.row(["service".to_owned(), "api".to_owned(), String::new()]);
        let out = t.render();
        for line in out.lines() {
            assert_eq!(
                line.trim_end(),
                line,
                "trailing whitespace on line: {line:?}"
            );
        }
    }
}
