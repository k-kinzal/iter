//! Token types emitted by the lexer.

use crate::ast::Span;

/// A single token paired with its byte span inside the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpannedToken {
    pub(crate) token: Token,
    pub(crate) span: Span,
}

/// Lexical token kinds emitted by the lexer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Token {
    // Punctuation
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    LParen,
    RParen,
    Comma,
    Equals,
    EqEq,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Percent,
    AmpAmp,
    PipePipe,
    Dot,

    // Literals
    String(String),
    Integer(i64),
    Duration(i64), // normalised to seconds
    True,
    False,

    // Identifiers / keywords
    Ident(String),
}

impl Token {
    pub(crate) fn describe(&self) -> String {
        match self {
            Token::LBrace => "`{`".into(),
            Token::RBrace => "`}`".into(),
            Token::LBracket => "`[`".into(),
            Token::RBracket => "`]`".into(),
            Token::LParen => "`(`".into(),
            Token::RParen => "`)`".into(),
            Token::Comma => "`,`".into(),
            Token::Equals => "`=`".into(),
            Token::EqEq => "`==`".into(),
            Token::BangEq => "`!=`".into(),
            Token::Lt => "`<`".into(),
            Token::LtEq => "`<=`".into(),
            Token::Gt => "`>`".into(),
            Token::GtEq => "`>=`".into(),
            Token::Percent => "`%`".into(),
            Token::AmpAmp => "`&&`".into(),
            Token::PipePipe => "`||`".into(),
            Token::Dot => "`.`".into(),
            Token::String(_) => "string literal".into(),
            Token::Integer(_) => "integer literal".into(),
            Token::Duration(_) => "duration literal".into(),
            Token::True => "`true`".into(),
            Token::False => "`false`".into(),
            Token::Ident(name) => format!("identifier `{name}`"),
        }
    }
}
