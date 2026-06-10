//! Lower the oracle parser's `Pairs<Rule>` tree into the same
//! `iter_language::RawFile` shape the hand-written parser produces.
//!
//! The transformation is intentionally mechanical; any cleverness here
//! would weaken the differential guarantee. Where a choice had to be made
//! (string escape handling, triple-string dedent, duration unit → seconds),
//! we duplicate the hand-written lexer's behavior verbatim — documented at
//! each call site.

use iter_language::{
    RawAction, RawBlock, RawCmpOp, RawEventHandler, RawField, RawFile, RawGuard, RawIdent,
    RawPromptMatchArm, RawRoute, RawSection, RawValue, Span,
};
use pest::iterators::Pair;

use super::parser::Rule;

pub(crate) fn lower_file(pair: Pair<Rule>) -> RawFile {
    assert_eq!(pair.as_rule(), Rule::file);
    let mut sections = Vec::new();
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::section => sections.push(lower_section(inner)),
            Rule::EOI => {}
            other => panic!("unexpected rule under `file`: {other:?}"),
        }
    }
    RawFile { sections }
}

fn lower_section(pair: Pair<Rule>) -> RawSection {
    assert_eq!(pair.as_rule(), Rule::section);
    let inner = first_child(pair);
    match inner.as_rule() {
        Rule::prompt_section => lower_prompt_section(inner),
        Rule::on_section => lower_top_on_section(inner),
        Rule::arg_section => lower_arg_section(inner),
        Rule::block_section => lower_block_section(inner),
        other => panic!("unexpected rule under `section`: {other:?}"),
    }
}

fn lower_arg_section(pair: Pair<Rule>) -> RawSection {
    assert_eq!(pair.as_rule(), Rule::arg_section);
    let span = pair_span(&pair);
    let mut children = pair.into_inner();

    let kw = children.next().expect("kw_arg");
    let keyword_span = pair_span(&kw);

    let name_pair = children.next().expect("arg name ident");
    let name = lower_ident(&name_pair);

    let body = children.next().map(|default_pair| {
        assert_eq!(default_pair.as_rule(), Rule::arg_default);
        let val_pair = first_child(default_pair);
        let value = lower_string_literal(val_pair.clone());
        let value_span = pair_span(&val_pair);
        let field_span = name.span.start..value_span.end;
        RawBlock {
            fields: vec![RawField {
                name: RawIdent {
                    name: "default".to_string(),
                    span: value_span.clone(),
                },
                value: RawValue::String(value, value_span),
                span: field_span,
            }],
            routes: Vec::new(),
            actions: Vec::new(),
            prompt_arms: Vec::new(),
            event_handlers: Vec::new(),
            span: name.span.start..span.end,
        }
    });

    RawSection::Block {
        keyword: "arg".to_string(),
        keyword_span: keyword_span.clone(),
        kind: Some(name),
        kind2: None,
        alias: None,
        body,
        span,
    }
}

// ---------------------------------------------------------------------------
// Block-style sections
// ---------------------------------------------------------------------------

fn lower_block_section(pair: Pair<Rule>) -> RawSection {
    assert_eq!(pair.as_rule(), Rule::block_section);
    let inner = first_child(pair);
    match inner.as_rule() {
        Rule::runner_section => lower_runner_section(inner),
        Rule::kinded_section => lower_kinded_section(inner),
        other => panic!("unexpected rule under `block_section`: {other:?}"),
    }
}

fn lower_runner_section(pair: Pair<Rule>) -> RawSection {
    assert_eq!(pair.as_rule(), Rule::runner_section);
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();
    let kw_pair = inner.next().expect("runner keyword");
    let keyword_span = pair_span(&kw_pair);
    let body = inner.next().map(lower_block);
    RawSection::Block {
        keyword: "runner".into(),
        keyword_span,
        kind: None,
        kind2: None,
        alias: None,
        body,
        span,
    }
}

fn lower_kinded_section(pair: Pair<Rule>) -> RawSection {
    assert_eq!(pair.as_rule(), Rule::kinded_section);
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();
    let kw_pair = inner.next().expect("kinded section keyword");
    let keyword_span = pair_span(&kw_pair);
    let keyword = keyword_text(&kw_pair);
    let kind_pair = inner.next().expect("kinded section kind ident");
    let kind = Some(lower_ident(&kind_pair));
    let mut kind2: Option<RawIdent> = None;
    let mut alias: Option<RawIdent> = None;
    let mut body: Option<RawBlock> = None;
    for tail in inner {
        match tail.as_rule() {
            Rule::as_alias => {
                let alias_ident = tail
                    .into_inner()
                    .find(|c| c.as_rule() == Rule::ident)
                    .expect("as_alias contains ident");
                alias = Some(lower_ident(&alias_ident));
            }
            Rule::kind2_with_block => {
                let mut k2_inner = tail.into_inner();
                let k2_ident = k2_inner.next().expect("kind2 ident");
                kind2 = Some(lower_ident(&k2_ident));
                let k2_block = k2_inner.next().expect("kind2 block");
                body = Some(lower_block(k2_block));
            }
            Rule::block => body = Some(lower_block(tail)),
            other => panic!("unexpected kinded_section tail child: {other:?}"),
        }
    }
    RawSection::Block {
        keyword,
        keyword_span,
        kind,
        kind2,
        alias,
        body,
        span,
    }
}

fn keyword_text(pair: &Pair<Rule>) -> String {
    // `block_keyword` is now a bare ident (any non-reserved word); read its
    // literal text directly. The kw_runner case stays for the dedicated
    // runner_section rule.
    match pair.as_rule() {
        Rule::block_keyword => pair.as_str().to_string(),
        Rule::kw_runner => "runner".into(),
        other => panic!("expected block_keyword, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Prompt
// ---------------------------------------------------------------------------

fn lower_prompt_section(pair: Pair<Rule>) -> RawSection {
    assert_eq!(pair.as_rule(), Rule::prompt_section);
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();
    let kw_pair = inner.next().expect("prompt keyword");
    let keyword_span = pair_span(&kw_pair);

    let mut name: Option<RawIdent> = None;
    let mut guard: Option<RawGuard> = None;
    let mut body_pair: Option<Pair<Rule>> = None;

    for p in inner {
        match p.as_rule() {
            Rule::prompt_as_alias => {
                for c in p.into_inner() {
                    match c.as_rule() {
                        Rule::kw_as => {}
                        Rule::ident => name = Some(lower_ident(&c)),
                        Rule::string_literal => body_pair = Some(c),
                        other => panic!("unexpected rule under `prompt_as_alias`: {other:?}"),
                    }
                }
            }
            Rule::prompt_guard => {
                let guard_pair = p
                    .into_inner()
                    .find(|c| c.as_rule() == Rule::guard)
                    .expect("`prompt_guard` contains a `guard`");
                guard = Some(lower_guard(guard_pair));
            }
            Rule::string_literal => body_pair = Some(p),
            other => panic!("unexpected rule under `prompt_section`: {other:?}"),
        }
    }

    let body_pair = body_pair.expect("prompt body");
    let body_span = pair_span(&body_pair);
    let body = lower_string_literal(body_pair);
    RawSection::Prompt {
        keyword_span,
        name,
        guard,
        body,
        body_span,
        span,
    }
}

fn lower_guard(pair: Pair<Rule>) -> RawGuard {
    assert_eq!(pair.as_rule(), Rule::guard);
    lower_guard_or(first_child(pair))
}

fn lower_guard_or(pair: Pair<Rule>) -> RawGuard {
    assert_eq!(pair.as_rule(), Rule::guard_or);
    let mut inner = pair.into_inner();
    let first = inner.next().expect("guard_or: left operand");
    let mut acc = lower_guard_and(first);
    for next in inner {
        let right = lower_guard_and(next);
        let span = acc.span().start..right.span().end;
        acc = RawGuard::Or(Box::new(acc), Box::new(right), span);
    }
    acc
}

fn lower_guard_and(pair: Pair<Rule>) -> RawGuard {
    assert_eq!(pair.as_rule(), Rule::guard_and);
    let mut inner = pair.into_inner();
    let first = inner.next().expect("guard_and: left operand");
    let mut acc = lower_guard_atom(first);
    for next in inner {
        let right = lower_guard_atom(next);
        let span = acc.span().start..right.span().end;
        acc = RawGuard::And(Box::new(acc), Box::new(right), span);
    }
    acc
}

fn lower_guard_atom(pair: Pair<Rule>) -> RawGuard {
    assert_eq!(pair.as_rule(), Rule::guard_atom);
    let inner = first_child(pair);
    match inner.as_rule() {
        Rule::guard_paren => {
            let or = inner
                .into_inner()
                .find(|c| c.as_rule() == Rule::guard_or)
                .expect("guard_paren contains guard_or");
            lower_guard_or(or)
        }
        Rule::guard_meta => lower_guard_meta(inner),
        Rule::guard_iter => lower_guard_iter(inner),
        other => panic!("unexpected guard_atom child: {other:?}"),
    }
}

fn lower_guard_meta(pair: Pair<Rule>) -> RawGuard {
    assert_eq!(pair.as_rule(), Rule::guard_meta);
    let span = pair_span(&pair);
    let mut key: Option<String> = None;
    let mut op = String::new();
    let mut value: Option<String> = None;
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::kw_metadata => {}
            Rule::ident => key = Some(p.as_str().to_string()),
            Rule::guard_op => op = p.as_str().to_string(),
            Rule::string => value = Some(lower_string_raw(&p)),
            other => panic!("unexpected guard_meta child: {other:?}"),
        }
    }
    let key = key.expect("guard key");
    let value = value.expect("guard value");
    match op.as_str() {
        "==" => RawGuard::MetadataEq { key, value, span },
        "!=" => RawGuard::MetadataNeq { key, value, span },
        other => panic!("unexpected guard op {other:?}"),
    }
}

fn lower_guard_iter(pair: Pair<Rule>) -> RawGuard {
    assert_eq!(pair.as_rule(), Rule::guard_iter);
    let span = pair_span(&pair);
    let mut field: Option<String> = None;
    let mut field_span: Span = 0..0;
    let mut modulus: Option<i64> = None;
    let mut modulus_span: Option<Span> = None;
    let mut op_str = String::new();
    let mut op_span: Span = 0..0;
    let mut rhs: Option<RhsLiteral> = None;
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::kw_iteration => {}
            Rule::ident => {
                field_span = pair_span(&p);
                field = Some(p.as_str().to_string());
            }
            Rule::guard_iter_modulus => {
                let mod_span = pair_span(&p);
                let mut inner = p.into_inner();
                let int_pair = inner
                    .find(|c| c.as_rule() == Rule::integer)
                    .expect("guard_iter_modulus contains integer");
                let int_span = pair_span(&int_pair);
                let int_val = int_pair
                    .as_str()
                    .parse::<i64>()
                    .expect("integer literal parses");
                modulus = Some(int_val);
                modulus_span = Some(int_span);
                let _ = mod_span;
            }
            Rule::guard_iter_op => {
                op_span = pair_span(&p);
                op_str = p.as_str().to_string();
            }
            Rule::guard_iter_rhs => {
                let inner = first_child(p);
                let s = pair_span(&inner);
                rhs = Some(match inner.as_rule() {
                    Rule::integer => {
                        RhsLiteral::Int(inner.as_str().parse::<i64>().expect("integer parses"), s)
                    }
                    Rule::string => RhsLiteral::Str(lower_string_raw(&inner), s),
                    other => panic!("unexpected guard_iter_rhs child: {other:?}"),
                });
            }
            other => panic!("unexpected guard_iter child: {other:?}"),
        }
    }
    let field = field.expect("iteration field");
    let rhs = rhs.expect("iteration rhs");
    let op = match op_str.as_str() {
        "==" => RawCmpOp::Eq,
        "!=" => RawCmpOp::Neq,
        "<" => RawCmpOp::Lt,
        "<=" => RawCmpOp::Le,
        ">" => RawCmpOp::Gt,
        ">=" => RawCmpOp::Ge,
        other => panic!("unexpected iteration op {other:?}"),
    };
    match rhs {
        RhsLiteral::Int(rhs, rhs_span) => RawGuard::IterationCmp {
            field,
            field_span,
            modulus,
            modulus_span,
            op,
            op_span,
            rhs,
            rhs_span,
            span,
        },
        RhsLiteral::Str(value, value_span) => match op {
            RawCmpOp::Eq => RawGuard::IterationResultEq {
                field,
                field_span,
                value,
                value_span,
                span,
            },
            RawCmpOp::Neq => RawGuard::IterationResultNeq {
                field,
                field_span,
                value,
                value_span,
                span,
            },
            // Pest's grammar lets a string RHS combine with any operator;
            // the hand-written parser rejects everything except `==`/`!=`
            // for result strings. Mirror that here so the differential
            // harness reaches identical CSTs on the inputs both
            // implementations accept and identical "unrepresentable" panics
            // would surface a real bug if we ever desynced.
            other => panic!(
                "string RHS only valid for `iteration.previous_result ==/!=`, got op {other:?}"
            ),
        },
    }
}

enum RhsLiteral {
    Int(i64, Span),
    Str(String, Span),
}

// ---------------------------------------------------------------------------
// Top-level `on <event>` handler
// ---------------------------------------------------------------------------

fn lower_top_on_section(pair: Pair<Rule>) -> RawSection {
    assert_eq!(pair.as_rule(), Rule::on_section);
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();
    let kw_pair = inner.next().expect("on keyword");
    let keyword_span = pair_span(&kw_pair);
    let event_pair = inner.next().expect("event ident");
    let event = lower_ident(&event_pair);
    let block_pair = inner.next().expect("event handler block");
    let body = lower_block(block_pair);
    RawSection::On {
        keyword_span,
        event,
        body,
        span,
    }
}

// ---------------------------------------------------------------------------
// Blocks
// ---------------------------------------------------------------------------

fn lower_block(pair: Pair<Rule>) -> RawBlock {
    assert_eq!(pair.as_rule(), Rule::block);
    let span = pair_span(&pair);
    let mut fields = Vec::new();
    let mut routes = Vec::new();
    let mut actions = Vec::new();
    let mut prompt_arms = Vec::new();
    let mut event_handlers = Vec::new();
    for entry in pair.into_inner() {
        match entry.as_rule() {
            Rule::block_entry => {
                let inner = first_child(entry);
                match inner.as_rule() {
                    Rule::prompt_match_default_arm => {
                        prompt_arms.push(lower_prompt_match_default_arm(inner));
                    }
                    Rule::prompt_match_guard_arm => {
                        prompt_arms.push(lower_prompt_match_guard_arm(inner));
                    }
                    Rule::nested_event_handler => {
                        event_handlers.push(lower_nested_event_handler(inner));
                    }
                    Rule::nested_route => routes.push(lower_nested_route(inner)),
                    Rule::action => actions.push(lower_action(inner)),
                    Rule::field => fields.push(lower_field(inner)),
                    other => panic!("unexpected block_entry child: {other:?}"),
                }
            }
            other => panic!("unexpected rule inside block: {other:?}"),
        }
    }
    RawBlock {
        fields,
        routes,
        actions,
        prompt_arms,
        event_handlers,
        span,
    }
}

fn lower_prompt_match_default_arm(pair: Pair<Rule>) -> RawPromptMatchArm {
    assert_eq!(pair.as_rule(), Rule::prompt_match_default_arm);
    let span = pair_span(&pair);
    let value_pair = pair
        .into_inner()
        .find(|c| c.as_rule() == Rule::prompt_arm_value)
        .expect("prompt_match_default_arm contains prompt_arm_value");
    let value = lower_prompt_arm_value(value_pair);
    RawPromptMatchArm {
        guard: None,
        value,
        span,
    }
}

fn lower_prompt_match_guard_arm(pair: Pair<Rule>) -> RawPromptMatchArm {
    assert_eq!(pair.as_rule(), Rule::prompt_match_guard_arm);
    let span = pair_span(&pair);
    let mut guard_out: Option<RawGuard> = None;
    let mut value_out: Option<RawValue> = None;
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::guard => guard_out = Some(lower_guard(p)),
            Rule::prompt_arm_value => value_out = Some(lower_prompt_arm_value(p)),
            other => panic!("unexpected prompt_match_guard_arm child: {other:?}"),
        }
    }
    RawPromptMatchArm {
        guard: guard_out,
        value: value_out.expect("prompt_match_guard_arm value"),
        span,
    }
}

fn lower_prompt_arm_value(pair: Pair<Rule>) -> RawValue {
    assert_eq!(pair.as_rule(), Rule::prompt_arm_value);
    let inner = first_child(pair);
    let span = pair_span(&inner);
    match inner.as_rule() {
        Rule::string_literal => RawValue::String(lower_string_literal(inner), span),
        Rule::ident => RawValue::Ident(inner.as_str().to_string(), span),
        other => panic!("unexpected prompt_arm_value child: {other:?}"),
    }
}

fn lower_nested_event_handler(pair: Pair<Rule>) -> RawEventHandler {
    assert_eq!(pair.as_rule(), Rule::nested_event_handler);
    let span = pair_span(&pair);
    let mut event: Option<RawIdent> = None;
    let mut body: Option<RawBlock> = None;
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::kw_on => {}
            Rule::ident => event = Some(lower_ident(&p)),
            Rule::block => body = Some(lower_block(p)),
            other => panic!("unexpected nested_event_handler child: {other:?}"),
        }
    }
    RawEventHandler {
        event: event.expect("nested_event_handler event"),
        body: body.expect("nested_event_handler body"),
        span,
    }
}

fn lower_nested_route(pair: Pair<Rule>) -> RawRoute {
    assert_eq!(pair.as_rule(), Rule::nested_route);
    let span = pair_span(&pair);
    let mut event_pattern: Option<String> = None;
    let mut when: Option<String> = None;
    let mut when_span: Option<Span> = None;
    let mut body: Option<RawBlock> = None;
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::kw_on => {}
            Rule::string if event_pattern.is_none() => {
                event_pattern = Some(lower_string_raw(&p));
            }
            Rule::route_when => {
                let s = p
                    .into_inner()
                    .find(|c| c.as_rule() == Rule::string)
                    .expect("route_when contains string");
                when_span = Some(pair_span(&s));
                when = Some(lower_string_raw(&s));
            }
            Rule::block => body = Some(lower_block(p)),
            other => panic!("unexpected nested_route child: {other:?}"),
        }
    }
    RawRoute {
        event_pattern: event_pattern.expect("route event pattern"),
        when,
        when_span,
        body: body.expect("route body"),
        span,
    }
}

fn lower_action(pair: Pair<Rule>) -> RawAction {
    assert_eq!(pair.as_rule(), Rule::action);
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();
    let kw_pair = inner.next().expect("shell keyword");
    let keyword_span = pair_span(&kw_pair);
    let cmd_pair = inner.next().expect("action command string");
    let command = lower_string_raw(&cmd_pair);
    RawAction {
        keyword_span,
        command,
        span,
    }
}

fn lower_field(pair: Pair<Rule>) -> RawField {
    assert_eq!(pair.as_rule(), Rule::field);
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();
    let name_pair = inner.next().expect("field name");
    let name = lower_field_name(name_pair);
    let rhs = inner.next().expect("field rhs");
    let value = lower_field_rhs(rhs);
    RawField { name, value, span }
}

fn lower_field_name(pair: Pair<Rule>) -> RawIdent {
    assert_eq!(pair.as_rule(), Rule::field_name);
    let span = pair_span(&pair);
    let inner = pair
        .into_inner()
        .find(|c| matches!(c.as_rule(), Rule::ident | Rule::string))
        .expect("field_name wraps an ident or string");
    match inner.as_rule() {
        Rule::ident => lower_ident(&inner),
        Rule::string => RawIdent {
            name: lower_string_raw(&inner),
            span,
        },
        other => panic!("unexpected field_name child: {other:?}"),
    }
}

fn lower_field_rhs(pair: Pair<Rule>) -> RawValue {
    assert_eq!(pair.as_rule(), Rule::field_rhs);
    let inner = first_child(pair);
    match inner.as_rule() {
        Rule::block => RawValue::Block(lower_block(inner)),
        Rule::value => lower_value(inner),
        other => panic!("unexpected field_rhs child: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

fn lower_value(pair: Pair<Rule>) -> RawValue {
    assert_eq!(pair.as_rule(), Rule::value);
    let inner = first_child(pair);
    let span = pair_span(&inner);
    match inner.as_rule() {
        Rule::call => lower_call(inner),
        Rule::list => lower_list(inner),
        Rule::block => RawValue::Block(lower_block(inner)),
        Rule::string_literal => RawValue::String(lower_string_literal(inner), span),
        Rule::duration => RawValue::Duration(lower_duration(&inner), span),
        Rule::integer => RawValue::Integer(lower_integer(&inner), span),
        Rule::boolean => RawValue::Bool(inner.as_str() == "true", span),
        Rule::null => RawValue::Null(span),
        Rule::ident => RawValue::Ident(inner.as_str().to_string(), span),
        other => panic!("unexpected value child: {other:?}"),
    }
}

fn lower_call(pair: Pair<Rule>) -> RawValue {
    assert_eq!(pair.as_rule(), Rule::call);
    let span = pair_span(&pair);
    let mut inner = pair.into_inner();
    let name_pair = inner.next().expect("call name");
    let name = name_pair.as_str().to_string();
    let mut args = Vec::new();
    for p in inner {
        if p.as_rule() == Rule::call_args {
            for v in p.into_inner() {
                if v.as_rule() == Rule::value {
                    args.push(lower_value(v));
                }
            }
        }
    }
    RawValue::Call { name, args, span }
}

fn lower_list(pair: Pair<Rule>) -> RawValue {
    assert_eq!(pair.as_rule(), Rule::list);
    let span = pair_span(&pair);
    let mut items = Vec::new();
    for p in pair.into_inner() {
        if p.as_rule() == Rule::list_items {
            for v in p.into_inner() {
                if v.as_rule() == Rule::value {
                    items.push(lower_value(v));
                }
            }
        }
    }
    RawValue::List(items, span)
}

fn lower_integer(pair: &Pair<Rule>) -> i64 {
    pair.as_str()
        .parse()
        .expect("integer literal must be parsable as i64 — grammar guarantees ASCII_DIGIT+")
}

/// Duration normalisation MUST match the hand-written lexer at
/// `iter_language/src/lexer.rs:lex_number_or_duration`:
///   `s` → n, `m` → n*60, `h` → n*3600, `d` → n*86400.
fn lower_duration(pair: &Pair<Rule>) -> i64 {
    let text = pair.as_str();
    let (digits, suffix) = text.split_at(text.len() - 1);
    let n: i64 = digits.parse().expect("duration digits must parse as i64");
    match suffix {
        "s" => n,
        "m" => n * 60,
        "h" => n * 60 * 60,
        "d" => n * 60 * 60 * 24,
        other => panic!("unexpected duration suffix {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Strings
// ---------------------------------------------------------------------------

fn lower_string_literal(pair: Pair<Rule>) -> String {
    assert_eq!(pair.as_rule(), Rule::string_literal);
    let inner = first_child(pair);
    match inner.as_rule() {
        Rule::triple_string => lower_triple_string(&inner),
        Rule::string => lower_string_raw(&inner),
        other => panic!("unexpected string_literal child: {other:?}"),
    }
}

/// Unescape a `"..."` string literal exactly as the hand-written lexer does.
fn lower_string_raw(pair: &Pair<Rule>) -> String {
    assert_eq!(pair.as_rule(), Rule::string);
    let raw = pair.as_str();
    // Strip leading and trailing `"` — pest only matches balanced quotes.
    let body = &raw[1..raw.len() - 1];
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let esc = chars
            .next()
            .expect("well-formed escape — grammar guarantees");
        match esc {
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            '0' => out.push('\0'),
            'u' => {
                // Expected form: u{HEX+} — grammar only accepts this shape,
                // so pattern-match it without defensive error paths.
                let open = chars.next();
                assert_eq!(open, Some('{'), "grammar guarantees `{{` after `\\u`");
                let mut hex = String::new();
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                    hex.push(c);
                }
                let code =
                    u32::from_str_radix(&hex, 16).expect("grammar guarantees ASCII_HEX_DIGIT+");
                if let Some(c) = char::from_u32(code) {
                    out.push(c);
                }
                // If the code point is not a valid scalar, the hand-written
                // lexer emits a diagnostic and drops it; we drop it here
                // too so the resulting string matches.
            }
            other => {
                // Unknown escapes: the hand-written lexer emits a
                // diagnostic and does not push anything. We replicate that
                // "do nothing" behavior to keep the CST equivalent for
                // inputs both parsers accept. Inputs with unknown escapes
                // produce lexer errors on the hand-written side → they are
                // handled as reject-vs-accept divergences elsewhere.
                let _ = other;
            }
        }
    }
    out
}

/// Dedent a triple-quoted string body identically to
/// `iter_language/src/lexer.rs::dedent_triple`.
fn lower_triple_string(pair: &Pair<Rule>) -> String {
    assert_eq!(pair.as_rule(), Rule::triple_string);
    let raw = pair.as_str();
    // Strip leading/trailing `"""`.
    let body = &raw[3..raw.len() - 3];
    dedent_triple(body)
}

fn dedent_triple(raw: &str) -> String {
    let trimmed = raw.strip_prefix('\n').unwrap_or(raw);
    let mut lines: Vec<&str> = trimmed.split('\n').collect();
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

// ---------------------------------------------------------------------------
// Identifiers / utilities
// ---------------------------------------------------------------------------

fn lower_ident(pair: &Pair<Rule>) -> RawIdent {
    assert_eq!(pair.as_rule(), Rule::ident);
    let span = pair_span(pair);
    RawIdent {
        name: pair.as_str().to_string(),
        span,
    }
}

fn pair_span(pair: &Pair<Rule>) -> Span {
    let s = pair.as_span();
    s.start()..s.end()
}

fn first_child(pair: Pair<Rule>) -> Pair<Rule> {
    pair.into_inner()
        .next()
        .expect("rule has at least one child")
}
