//! Hand-written lexer state machine.

use super::token::{SpannedToken, Token};
use crate::diagnostic::Diagnostic;

/// Lex `source` into a token stream and a list of diagnostics. The token
/// stream is always returned, even on errors, with broken regions skipped.
pub(crate) fn lex(source: &str) -> (Vec<SpannedToken>, Vec<Diagnostic>) {
    let mut lexer = Lexer::new(source);
    lexer.run();
    (lexer.tokens, lexer.errors)
}

struct Lexer<'a> {
    source: &'a str,
    bytes: &'a [u8],
    pos: usize,
    tokens: Vec<SpannedToken>,
    errors: Vec<Diagnostic>,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            pos: 0,
            tokens: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn run(&mut self) {
        while self.pos < self.bytes.len() {
            let start = self.pos;
            let b = self.bytes[self.pos];
            match b {
                // whitespace
                b' ' | b'\t' | b'\r' | b'\n' => {
                    self.pos += 1;
                }
                // comment
                b'#' => {
                    while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                        self.pos += 1;
                    }
                }
                b'{' => self.push(start, Token::LBrace, 1),
                b'}' => self.push(start, Token::RBrace, 1),
                b'[' => self.push(start, Token::LBracket, 1),
                b']' => self.push(start, Token::RBracket, 1),
                b'(' => self.push(start, Token::LParen, 1),
                b')' => self.push(start, Token::RParen, 1),
                b',' => self.push(start, Token::Comma, 1),
                b'.' => self.push(start, Token::Dot, 1),
                b'=' => {
                    if self.peek_at(1) == Some(b'=') {
                        self.push(start, Token::EqEq, 2);
                    } else if self.peek_at(1) == Some(b'>') {
                        self.push(start, Token::FatArrow, 2);
                    } else {
                        self.push(start, Token::Equals, 1);
                    }
                }
                b'!' => {
                    if self.peek_at(1) == Some(b'=') {
                        self.push(start, Token::BangEq, 2);
                    } else {
                        self.errors.push(Diagnostic::error(
                            start..start + 1,
                            "unexpected character `!`",
                        ));
                        self.pos += 1;
                    }
                }
                // Numeric comparison operators are added with longest-match
                // disambiguation so `<=` and `>=` are not split into two
                // tokens. They are currently only consumed by the
                // `iteration.<field> <op> N` guard form, but are emitted
                // unconditionally because lexing must stay context-free.
                b'<' => {
                    if self.peek_at(1) == Some(b'=') {
                        self.push(start, Token::LtEq, 2);
                    } else {
                        self.push(start, Token::Lt, 1);
                    }
                }
                b'>' => {
                    if self.peek_at(1) == Some(b'=') {
                        self.push(start, Token::GtEq, 2);
                    } else {
                        self.push(start, Token::Gt, 1);
                    }
                }
                b'%' => self.push(start, Token::Percent, 1),
                b'&' => {
                    if self.peek_at(1) == Some(b'&') {
                        self.push(start, Token::AmpAmp, 2);
                    } else {
                        self.errors.push(Diagnostic::error(
                            start..start + 1,
                            "unexpected character `&`; did you mean `&&`?",
                        ));
                        self.pos += 1;
                    }
                }
                b'|' => {
                    if self.peek_at(1) == Some(b'|') {
                        self.push(start, Token::PipePipe, 2);
                    } else {
                        self.errors.push(Diagnostic::error(
                            start..start + 1,
                            "unexpected character `|`; did you mean `||`?",
                        ));
                        self.pos += 1;
                    }
                }
                b'"' => {
                    if self.peek_at(1) == Some(b'"') && self.peek_at(2) == Some(b'"') {
                        self.lex_triple_string(start);
                    } else {
                        self.lex_string(start);
                    }
                }
                _ if b.is_ascii_digit() => self.lex_number_or_duration(start),
                _ if is_ident_start(b) => self.lex_ident(start),
                _ => {
                    // Skip the codepoint, not just the byte, to avoid splitting UTF-8.
                    let ch_len = utf8_char_len(b);
                    let end = (start + ch_len).min(self.bytes.len());
                    let bad = &self.source[start..end];
                    self.errors.push(Diagnostic::error(
                        start..end,
                        format!("unexpected character `{bad}`"),
                    ));
                    self.pos = end;
                }
            }
        }
    }

    fn push(&mut self, start: usize, token: Token, len: usize) {
        let end = start + len;
        self.tokens.push(SpannedToken {
            token,
            span: start..end,
        });
        self.pos = end;
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn lex_string(&mut self, start: usize) {
        // start is on the opening "
        self.pos += 1;
        let mut value = String::new();
        loop {
            if self.pos >= self.bytes.len() {
                self.errors.push(
                    Diagnostic::error(start..self.pos, "unterminated string literal")
                        .with_hint("strings are delimited by `\"`"),
                );
                return;
            }
            let b = self.bytes[self.pos];
            match b {
                b'"' => {
                    self.pos += 1;
                    self.tokens.push(SpannedToken {
                        token: Token::String(value),
                        span: start..self.pos,
                    });
                    return;
                }
                b'\n' => {
                    self.errors.push(
                        Diagnostic::error(
                            start..self.pos,
                            "unterminated string literal: newline before closing `\"`",
                        )
                        .with_hint(
                            "use a triple-quoted string `\"\"\"...\"\"\"` for multi-line content",
                        ),
                    );
                    return;
                }
                b'\\' => {
                    self.pos += 1;
                    if self.pos >= self.bytes.len() {
                        self.errors.push(Diagnostic::error(
                            start..self.pos,
                            "unterminated escape sequence in string",
                        ));
                        return;
                    }
                    let esc = self.bytes[self.pos];
                    self.pos += 1;
                    match esc {
                        b'"' => value.push('"'),
                        b'\\' => value.push('\\'),
                        b'n' => value.push('\n'),
                        b't' => value.push('\t'),
                        b'r' => value.push('\r'),
                        b'0' => value.push('\0'),
                        b'u' => {
                            if !self.lex_unicode_escape(start, &mut value) {
                                return;
                            }
                        }
                        other => {
                            self.errors.push(Diagnostic::error(
                                self.pos - 2..self.pos,
                                format!("unknown escape sequence `\\{}`", other as char),
                            ));
                        }
                    }
                }
                _ => {
                    let ch_len = utf8_char_len(b);
                    let end = (self.pos + ch_len).min(self.bytes.len());
                    value.push_str(&self.source[self.pos..end]);
                    self.pos = end;
                }
            }
        }
    }

    fn lex_unicode_escape(&mut self, start: usize, value: &mut String) -> bool {
        if self.peek_at(0) != Some(b'{') {
            self.errors.push(Diagnostic::error(
                start..self.pos,
                "expected `{` after `\\u` in string escape",
            ));
            return true;
        }
        self.pos += 1;
        let hex_start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos] != b'}' {
            self.pos += 1;
        }
        if self.pos >= self.bytes.len() {
            self.errors.push(Diagnostic::error(
                start..self.pos,
                "unterminated `\\u{...}` escape",
            ));
            return false;
        }
        let hex = &self.source[hex_start..self.pos];
        self.pos += 1; // consume }
        match u32::from_str_radix(hex, 16) {
            Ok(code) => match char::from_u32(code) {
                Some(c) => value.push(c),
                None => self.errors.push(Diagnostic::error(
                    hex_start - 2..self.pos,
                    format!("invalid Unicode scalar `U+{hex}`"),
                )),
            },
            Err(_) => self.errors.push(Diagnostic::error(
                hex_start - 2..self.pos,
                format!("invalid hex digits in Unicode escape: `{hex}`"),
            )),
        }
        true
    }

    fn lex_triple_string(&mut self, start: usize) {
        self.pos += 3;
        let body_start = self.pos;
        loop {
            if self.pos >= self.bytes.len() {
                self.errors.push(Diagnostic::error(
                    start..self.pos,
                    "unterminated triple-quoted string literal",
                ));
                return;
            }
            if self.bytes[self.pos] == b'"'
                && self.peek_at(1) == Some(b'"')
                && self.peek_at(2) == Some(b'"')
            {
                let body_end = self.pos;
                self.pos += 3;
                let raw = &self.source[body_start..body_end];
                let value = dedent_triple(raw);
                self.tokens.push(SpannedToken {
                    token: Token::String(value),
                    span: start..self.pos,
                });
                return;
            }
            self.pos += 1;
        }
    }

    fn lex_number_or_duration(&mut self, start: usize) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        let digits_end = self.pos;
        // duration suffix?
        if let Some(&b) = self.bytes.get(self.pos) {
            if matches!(b, b's' | b'm' | b'h' | b'd') {
                let suffix_start = self.pos;
                self.pos += 1;
                let digits = &self.source[start..digits_end];
                let n: i64 = if let Ok(n) = digits.parse() {
                    n
                } else {
                    self.errors.push(Diagnostic::error(
                        start..self.pos,
                        format!("invalid integer literal `{digits}`"),
                    ));
                    return;
                };
                let secs = match self.bytes[suffix_start] {
                    b's' => n,
                    b'm' => n * 60,
                    b'h' => n * 60 * 60,
                    b'd' => n * 60 * 60 * 24,
                    _ => unreachable!(),
                };
                self.tokens.push(SpannedToken {
                    token: Token::Duration(secs),
                    span: start..self.pos,
                });
                return;
            }
        }
        let digits = &self.source[start..digits_end];
        match digits.parse::<i64>() {
            Ok(n) => self.tokens.push(SpannedToken {
                token: Token::Integer(n),
                span: start..digits_end,
            }),
            Err(_) => {
                self.errors.push(Diagnostic::error(
                    start..digits_end,
                    format!("invalid integer literal `{digits}`"),
                ));
            }
        }
    }

    fn lex_ident(&mut self, start: usize) {
        while self.pos < self.bytes.len() && is_ident_continue(self.bytes[self.pos]) {
            self.pos += 1;
        }
        let text = &self.source[start..self.pos];
        let token = match text {
            "true" => Token::True,
            "false" => Token::False,
            _ => Token::Ident(text.to_string()),
        };
        self.tokens.push(SpannedToken {
            token,
            span: start..self.pos,
        });
    }
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn utf8_char_len(b: u8) -> usize {
    match b {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

/// De-indent a triple-quoted string body. The opening `"""` may be followed
/// by a leading newline; the closing `"""` may be preceded by trailing
/// indentation. The minimum leading-whitespace prefix common to every
/// non-empty line is removed, and a leading and trailing blank line are
/// trimmed.
fn dedent_triple(raw: &str) -> String {
    // Trim a single leading newline if present.
    let trimmed = raw.strip_prefix('\n').unwrap_or(raw);
    // Trim a single trailing newline + indent if present (the closing """
    // sits on its own indented line).
    let mut lines: Vec<&str> = trimmed.split('\n').collect();
    // Find the minimum indent across non-empty lines, but excluding the last
    // line if it is empty / whitespace only (it usually contains the indent
    // before the closing """).
    let last_is_blank = lines
        .last()
        .is_some_and(|l| l.chars().all(|c| c == ' ' || c == '\t'));
    let indent = lines
        .iter()
        .enumerate()
        .filter(|(i, l)| !(l.trim().is_empty() || (last_is_blank && *i == lines.len() - 1)))
        .map(|(_, l)| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);

    if last_is_blank {
        lines.pop();
    }

    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line.len() >= indent {
            out.push_str(&line[indent..]);
        } else {
            out.push_str(line.trim_start());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex_kinds(s: &str) -> Vec<Token> {
        let (tokens, errors) = lex(s);
        assert!(errors.is_empty(), "lexer produced errors: {errors:?}");
        tokens.into_iter().map(|t| t.token).collect()
    }

    #[test]
    fn empty_input() {
        assert!(lex_kinds("").is_empty());
    }

    #[test]
    fn keywords_and_idents() {
        assert_eq!(
            lex_kinds("queue memory"),
            vec![Token::Ident("queue".into()), Token::Ident("memory".into())]
        );
    }

    #[test]
    fn punctuation_and_operators() {
        let toks = lex_kinds("{ } = == != && || ( ) , .");
        assert_eq!(
            toks,
            vec![
                Token::LBrace,
                Token::RBrace,
                Token::Equals,
                Token::EqEq,
                Token::BangEq,
                Token::AmpAmp,
                Token::PipePipe,
                Token::LParen,
                Token::RParen,
                Token::Comma,
                Token::Dot,
            ]
        );
    }

    #[test]
    fn comparison_and_modulus_tokens_are_longest_match() {
        // `<=` and `>=` must not be split into `<`/`=` + `=`. The order of
        // alternatives matters because the lexer is hand-written; this test
        // pins the longest-match behaviour.
        let toks = lex_kinds("< <= > >= %");
        assert_eq!(
            toks,
            vec![
                Token::Lt,
                Token::LtEq,
                Token::Gt,
                Token::GtEq,
                Token::Percent
            ]
        );
    }

    #[test]
    fn comparison_tokens_split_when_no_equals_follows() {
        // Confirm `<` and `>` still tokenise on their own when not part of
        // a `<=` / `>=` digram. Adjacent unrelated characters (here `5`)
        // must not be consumed.
        let toks = lex_kinds("< 5 > 5");
        assert_eq!(
            toks,
            vec![Token::Lt, Token::Integer(5), Token::Gt, Token::Integer(5),]
        );
    }

    #[test]
    fn string_with_escapes() {
        assert_eq!(
            lex_kinds(r#""hello\n\"world\"""#),
            vec![Token::String("hello\n\"world\"".into())]
        );
    }

    #[test]
    fn duration() {
        assert_eq!(
            lex_kinds("5s 10m 2h"),
            vec![
                Token::Duration(5),
                Token::Duration(600),
                Token::Duration(7200),
            ]
        );
    }

    #[test]
    fn comment_skipped() {
        assert_eq!(
            lex_kinds("# this is a comment\nqueue memory"),
            vec![Token::Ident("queue".into()), Token::Ident("memory".into())]
        );
    }

    #[test]
    fn unterminated_string_is_error() {
        let (_tokens, errors) = lex(r#""oops"#);
        assert!(errors.iter().any(|d| d.message.contains("unterminated")));
    }

    #[test]
    fn triple_string_dedents() {
        let src = "\"\"\"\n  hello\n  world\n\"\"\"";
        let toks = lex_kinds(src);
        assert_eq!(toks, vec![Token::String("hello\nworld".into())]);
    }

    #[test]
    fn unicode_escape() {
        assert_eq!(
            lex_kinds(r#""\u{1F600}""#),
            vec![Token::String("\u{1F600}".into())]
        );
    }
}
