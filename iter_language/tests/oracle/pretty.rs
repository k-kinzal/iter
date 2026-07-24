//! Test-only pretty printer for `CstFile`.
//!
//! This is NOT an authoritative formatter for the iter language — it only
//! guarantees that the output re-parses (via both the hand-written and the
//! oracle parsers) into a CST that is `canonicalize`-equal to the input.
//! That property is what makes it useful for generated-input tests: we
//! build a random `CstFile`, print it, re-parse it, and compare shapes.

use iter_language::{
    CstAction, CstActionBody, CstBinaryOp, CstBlock, CstExprLiteral, CstField, CstFile, CstGuard,
    CstPathSegment, CstRoute, CstSection, CstValue,
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
    if b.fields.is_empty()
        && b.conditions.is_empty()
        && b.routes.is_empty()
        && b.actions.is_empty()
        && b.captures.is_empty()
        && b.prompt_arms.is_empty()
        && b.event_handlers.is_empty()
    {
        out.push_str("{}");
        return;
    }
    out.push_str("{\n");
    for f in &b.fields {
        pp_field(out, f, depth + 1);
    }
    for condition in &b.conditions {
        indent(out, depth + 1);
        out.push_str("condition ");
        out.push_str(&condition.kind.name);
        out.push_str(" as ");
        out.push_str(&condition.name.name);
        out.push(' ');
        pp_block(out, &condition.body, depth + 1);
        out.push('\n');
    }
    for r in &b.routes {
        pp_route(out, r, depth + 1);
    }
    for a in &b.actions {
        pp_action(out, a, depth + 1);
    }
    for capture in &b.captures {
        indent(out, depth + 1);
        out.push_str("capture ");
        out.push_str(&capture.name.name);
        out.push(' ');
        pp_block(out, &capture.body, depth + 1);
        out.push('\n');
    }
    for arm in &b.prompt_arms {
        indent(out, depth + 1);
        if let Some(guard) = &arm.guard {
            pp_guard(out, guard, 0);
        } else {
            out.push('_');
        }
        out.push_str(" => ");
        pp_value(out, &arm.value, depth + 1);
        out.push('\n');
    }
    for handler in &b.event_handlers {
        indent(out, depth + 1);
        out.push_str("on ");
        out.push_str(&handler.event.name);
        if let Some(condition) = &handler.condition {
            out.push_str(" when ");
            pp_guard(out, condition, 0);
        }
        out.push(' ');
        pp_block(out, &handler.body, depth + 1);
        out.push('\n');
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
        v @ (CstValue::String(..)
        | CstValue::Integer(..)
        | CstValue::Duration(..)
        | CstValue::Bool(..)
        | CstValue::Null(_)
        | CstValue::Ident(..)
        | CstValue::List(..)
        | CstValue::Call { .. }) => {
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
    match &a.body {
        CstActionBody::Shorthand { script, .. } => {
            out.push_str("shell ");
            pp_string(out, script);
        }
        CstActionBody::Block(block) => {
            out.push_str("shell ");
            pp_block(out, block, depth);
        }
        CstActionBody::Enqueue(block) => {
            out.push_str("enqueue ");
            pp_block(out, block, depth);
        }
    }
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
/// `on`, `shell`, `condition`, or `capture` (those would re-route to a
/// contextual nested declaration).
fn is_bareword_field_name(name: &str) -> bool {
    if matches!(name, "on" | "shell" | "condition" | "capture") {
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

fn pp_binary_op(op: CstBinaryOp) -> &'static str {
    match op {
        CstBinaryOp::Or => "||",
        CstBinaryOp::And => "&&",
        CstBinaryOp::Eq => "==",
        CstBinaryOp::Neq => "!=",
        CstBinaryOp::Lt => "<",
        CstBinaryOp::Le => "<=",
        CstBinaryOp::Gt => ">",
        CstBinaryOp::Ge => ">=",
        CstBinaryOp::Mod => "%",
    }
}

fn pp_guard(out: &mut String, g: &CstGuard, parent_prec: u8) {
    let prec = match g {
        CstGuard::Binary { op, .. } => match op {
            CstBinaryOp::Or => 1,
            CstBinaryOp::And => 2,
            CstBinaryOp::Eq
            | CstBinaryOp::Neq
            | CstBinaryOp::Lt
            | CstBinaryOp::Le
            | CstBinaryOp::Gt
            | CstBinaryOp::Ge => 3,
            CstBinaryOp::Mod => 4,
        },
        CstGuard::Literal { .. } | CstGuard::Path { .. } => 5,
    };
    let needs_parens = prec < parent_prec;
    if needs_parens {
        out.push('(');
    }
    match g {
        CstGuard::Literal { value, .. } => match value {
            CstExprLiteral::String(value) => pp_string(out, value),
            CstExprLiteral::Integer(value) => out.push_str(&value.to_string()),
            CstExprLiteral::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            CstExprLiteral::Null => out.push_str("null"),
        },
        CstGuard::Path { root, segments, .. } => {
            out.push_str(&root.name);
            for segment in segments {
                match segment {
                    CstPathSegment::Field(field) => {
                        out.push('.');
                        out.push_str(&field.name);
                    }
                    CstPathSegment::Index(index, _) => {
                        out.push('[');
                        out.push_str(&index.to_string());
                        out.push(']');
                    }
                }
            }
        }
        CstGuard::Binary { lhs, op, rhs, .. } => {
            pp_guard(out, lhs, prec);
            out.push(' ');
            out.push_str(pp_binary_op(*op));
            out.push(' ');
            pp_guard(out, rhs, prec + 1);
        }
    }
    if needs_parens {
        out.push(')');
    }
}
