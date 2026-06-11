//! Oracle parser: a second, independent implementation of the iter language
//! grammar built with `pest_derive`. Its sole purpose is to be diffed
//! against the hand-written implementation in `iter_language::parse_to_cst`.

use iter_language::CstFile;
use pest::Parser;
use pest_derive::Parser;

use super::lowering;

/// Pest-derived parser against `grammar/iter.pest`.
#[derive(Parser)]
#[grammar = "../grammar/iter.pest"]
pub(crate) struct OracleParser;

/// Parse `source` with the oracle parser.
///
/// Returns `(Some(cst), true)` on acceptance and `(None, false)` on
/// rejection. The `bool` flag is `true` ⟺ `cst` is `Some(_)` — it exists as
/// a convenient, redundant "accepted?" signal that callers can compare
/// against the hand-written parser's "no error diagnostics" signal without
/// reaching into the CST.
pub(crate) fn oracle_parse(source: &str) -> (Option<CstFile>, bool) {
    match OracleParser::parse(Rule::file, source) {
        Ok(mut pairs) => {
            // The `file` rule is the top — there is exactly one pair.
            let file_pair = pairs.next().expect("oracle returned no `file` pair");
            let raw = lowering::lower_file(file_pair);
            (Some(raw), true)
        }
        Err(_) => (None, false),
    }
}
