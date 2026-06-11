//! Test-only pretty printer for `CstFile`.
//!
//! This is NOT an authoritative formatter for the iter language — it only
//! guarantees that the output re-parses (via both the hand-written and the
//! oracle parsers) into a CST that is `canonicalize`-equal to the input.
//! That property is what makes it useful for generated-input tests: we
//! build a random `CstFile`, print it, re-parse it, and compare shapes.

use iter_language::{
    CstAction, CstBlock, CstCmpOp, CstField, CstFile, CstGuard, CstRoute, CstSection, CstValue,
};

pub(crate) fn pretty(file: &CstFile) -> String {
    let mut out = String::new();
    for (i, section) in file.sections.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        pp_section(&mut out, section, 0);
    }
    out
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn pp_section(out: &mut String, s: &CstSection, depth: usize) {
    match s {
        CstSection::Block {
            keyword,
            kind,
            kind2,
            body,
            ..
        } => {
            indent(out, depth);
            out.push_str(keyword);
            if let Some(k) = kind {
                out.push(' ');
                out.push_str(&k.name);
            }
            if let Some(k) = kind2 {
                out.push(' ');
                out.push_str(&k.name);
            }
            if let Some(b) = body {
                out.push(' ');
                pp_block(out, b, depth);
            }
            out.push('\n');
        }
        CstSection::Prompt { guard, body, .. } => {
            indent(out, depth);
            out.push_str("prompt");
            if let Some(g) = guard {
                out.push_str(" when ");
                pp_guard(out, g, 0);
            }
            out.push(' ');
            pp_string(out, body);
            out.push('\n');
        }
        CstSection::On { event, body, .. } => {
            indent(out, depth);
            out.push_str("on ");
            out.push_str(&event.name);
            out.push(' ');
            pp_block(out, body, depth);
            out.push('\n');
        }
    }
}

fn pp_block(out: &mut String, b: &CstBlock, depth: usize) {
    if b.fields.is_empty() && b.routes.is_empty() && b.actions.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push_str("{\n");
    for f in &b.fields {
        pp_field(out, f, depth + 1);
    }
    for r in &b.routes {
        pp_route(out, r, depth + 1);
    }
    for a in &b.actions {
        pp_action(out, a, depth + 1);
    }
    indent(out, depth);
    out.push('}');
}

fn pp_field(out: &mut String, f: &CstField, depth: usize) {
    indent(out, depth);
    if is_bareword_field_name(&f.name.name) {
        out.push_str(&f.name.name);
    } else {
        pp_string(out, &f.name.name);
    }
    match &f.value {
        CstValue::Block(b) => {
            out.push(' ');
            pp_block(out, b, depth);
            out.push('\n');
        }
        v => {
            out.push_str(" = ");
            pp_value(out, v, depth);
            out.push('\n');
        }
    }
}

fn pp_route(out: &mut String, r: &CstRoute, depth: usize) {
    indent(out, depth);
    out.push_str("on ");
    pp_string(out, &r.event_pattern);
    if let Some(w) = &r.when {
        out.push_str(" when ");
        pp_string(out, w);
    }
    out.push(' ');
    pp_block(out, &r.body, depth);
    out.push('\n');
}

fn pp_action(out: &mut String, a: &CstAction, depth: usize) {
    indent(out, depth);
    out.push_str("shell ");
    pp_string(out, &a.command);
    out.push('\n');
}

fn pp_value(out: &mut String, v: &CstValue, depth: usize) {
    match v {
        CstValue::String(s, _) => pp_string(out, s),
        CstValue::Integer(n, _) => out.push_str(&n.to_string()),
        // Durations are canonicalised to seconds on both sides. Pretty-print
        // back as `<n>s` so the re-parsed value equals the original.
        CstValue::Duration(n, _) => {
            out.push_str(&n.to_string());
            out.push('s');
        }
        CstValue::Bool(b, _) => out.push_str(if *b { "true" } else { "false" }),
        CstValue::Null(_) => out.push_str("null"),
        CstValue::Ident(name, _) => out.push_str(name),
        CstValue::List(items, _) => {
            out.push('[');
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                pp_value(out, it, depth);
            }
            out.push(']');
        }
        CstValue::Block(b) => pp_block(out, b, depth),
        CstValue::Call { name, args, .. } => {
            out.push_str(name);
            out.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                pp_value(out, a, depth);
            }
            out.push(')');
        }
    }
}

/// Whether `name` can be emitted as a bareword identifier without quoting.
///
/// Mirrors the grammar's `ident` rule (and the contextual block-entry
/// keywords that the field-name rule excludes): leading ASCII letter or
/// underscore, then ASCII alphanumerics or underscores, never the literal
/// `on` or `shell` (those would re-route to action / nested-route parsing).
fn is_bareword_field_name(name: &str) -> bool {
    if matches!(name, "on" | "shell") {
        return false;
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn pp_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write as _;
                write!(out, "\\u{{{:x}}}", c as u32).expect("write to String");
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn pp_cmp_op(op: CstCmpOp) -> &'static str {
    match op {
        CstCmpOp::Eq => "==",
        CstCmpOp::Neq => "!=",
        CstCmpOp::Lt => "<",
        CstCmpOp::Le => "<=",
        CstCmpOp::Gt => ">",
        CstCmpOp::Ge => ">=",
    }
}

fn pp_guard(out: &mut String, g: &CstGuard, parent_prec: u8) {
    // Precedence: `||` = 1, `&&` = 2, atom = 3.
    let (prec, sep): (u8, &str) = match g {
        CstGuard::Or(..) => (1, " || "),
        CstGuard::And(..) => (2, " && "),
        _ => (3, ""),
    };
    let needs_parens = prec < parent_prec;
    if needs_parens {
        out.push('(');
    }
    match g {
        CstGuard::MetadataEq { key, value, .. } => {
            out.push_str("metadata.");
            out.push_str(key);
            out.push_str(" == ");
            pp_string(out, value);
        }
        CstGuard::MetadataNeq { key, value, .. } => {
            out.push_str("metadata.");
            out.push_str(key);
            out.push_str(" != ");
            pp_string(out, value);
        }
        CstGuard::IterationCmp {
            field,
            modulus,
            op,
            rhs,
            ..
        } => {
            out.push_str("iteration.");
            out.push_str(field);
            if let Some(m) = modulus {
                out.push_str(" % ");
                out.push_str(&m.to_string());
            }
            out.push(' ');
            out.push_str(pp_cmp_op(*op));
            out.push(' ');
            out.push_str(&rhs.to_string());
        }
        CstGuard::IterationResultEq { value, .. } => {
            out.push_str("iteration.previous_result == ");
            pp_string(out, value);
        }
        CstGuard::IterationResultNeq { value, .. } => {
            out.push_str("iteration.previous_result != ");
            pp_string(out, value);
        }
        CstGuard::Or(l, r, _) | CstGuard::And(l, r, _) => {
            pp_guard(out, l, prec);
            out.push_str(sep);
            pp_guard(out, r, prec);
        }
    }
    if needs_parens {
        out.push(')');
    }
}
