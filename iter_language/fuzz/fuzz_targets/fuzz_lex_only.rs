//! Lexer-focused fuzz target.
//!
//! The hand-written lexer is not part of `iter_language`'s public surface,
//! so we exercise it by driving `iter_language::parse_to_cst` through
//! inputs biased towards token-shaped bytes (operators, whitespace, short
//! digit-or-letter runs). libFuzzer's coverage-guided exploration takes
//! care of reaching lexical corner cases (unterminated strings, mixed
//! digit/letter runs, `\u{...}` escape edge cases, triple-quoted string
//! state machines) from this seed distribution.

#![no_main]

use libfuzzer_sys::fuzz_target;

const TOKEN_ALPHABET: &[u8] = b" \t\n#\"\\{}[]()=!&|,.0123456789abcdefghijklmnopqrstuvwxyz_smhd";

fn normalise(data: &[u8]) -> String {
    // Map every input byte into `TOKEN_ALPHABET` so we never emit invalid
    // UTF-8 and the lexer reliably sees interesting tokens.
    let mut out = String::with_capacity(data.len());
    for &b in data {
        let c = TOKEN_ALPHABET[(b as usize) % TOKEN_ALPHABET.len()];
        out.push(c as char);
    }
    out
}

fuzz_target!(|data: &[u8]| {
    let src = normalise(data);
    let _ = iter_language::parse_to_cst(&src);
});
