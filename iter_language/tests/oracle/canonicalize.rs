//! Span canonicalisation: replace every `Span` inside a `RawFile` with
//! `0..0` so structural comparison between the hand-written CST and the
//! oracle CST is span-oblivious.
//!
//! Span equality is out of scope for the grammar-spec differential: the two
//! implementations tokenise at slightly different boundaries (the
//! hand-written lexer's span for a duration spans digits + suffix, pest's
//! span for the same literal spans digits + suffix — they happen to agree
//! today but the spec guarantee is shape, not span byte ranges). We
//! compare shape explicitly and leave span equivalence as a non-goal.

use iter_language::{RawBlock, RawField, RawFile, RawGuard, RawIdent, RawSection, RawValue};

pub(crate) fn canonicalize(file: &mut RawFile) {
    for s in &mut file.sections {
        canon_section(s);
    }
}

fn canon_section(s: &mut RawSection) {
    match s {
        RawSection::Block {
            keyword_span,
            kind,
            kind2,
            body,
            span,
            keyword: _,
        } => {
            *keyword_span = 0..0;
            *span = 0..0;
            if let Some(k) = kind {
                canon_ident(k);
            }
            if let Some(k) = kind2 {
                canon_ident(k);
            }
            if let Some(b) = body {
                canon_block(b);
            }
        }
        RawSection::Prompt {
            keyword_span,
            guard,
            body_span,
            span,
            body: _,
        } => {
            *keyword_span = 0..0;
            *body_span = 0..0;
            *span = 0..0;
            if let Some(g) = guard {
                canon_guard(g);
            }
        }
        RawSection::On {
            keyword_span,
            event,
            body,
            span,
        } => {
            *keyword_span = 0..0;
            *span = 0..0;
            canon_ident(event);
            canon_block(body);
        }
    }
}

fn canon_block(b: &mut RawBlock) {
    b.span = 0..0;
    for f in &mut b.fields {
        canon_field(f);
    }
    for r in &mut b.routes {
        r.span = 0..0;
        canon_block(&mut r.body);
    }
    for a in &mut b.actions {
        a.keyword_span = 0..0;
        a.span = 0..0;
    }
}

fn canon_field(f: &mut RawField) {
    f.span = 0..0;
    canon_ident(&mut f.name);
    canon_value(&mut f.value);
}

fn canon_value(v: &mut RawValue) {
    match v {
        RawValue::String(_, s)
        | RawValue::Integer(_, s)
        | RawValue::Duration(_, s)
        | RawValue::Bool(_, s)
        | RawValue::Ident(_, s) => *s = 0..0,
        RawValue::List(items, s) => {
            *s = 0..0;
            for it in items {
                canon_value(it);
            }
        }
        RawValue::Block(b) => canon_block(b),
        RawValue::Call { args, span, .. } => {
            *span = 0..0;
            for a in args {
                canon_value(a);
            }
        }
    }
}

fn canon_ident(i: &mut RawIdent) {
    i.span = 0..0;
}

fn canon_guard(g: &mut RawGuard) {
    zero_guard_spans(g);
    // Both the hand-written recursive-descent parser and the oracle parser
    // produce left-associated `And`/`Or` chains; `RawFile`s synthesised by
    // proptest strategies can be right-associated. Normalise here so
    // structural comparison is associativity-oblivious for same-kind
    // chains.
    *g = reassociate_left(g.clone());
}

fn zero_guard_spans(g: &mut RawGuard) {
    match g {
        RawGuard::MetadataEq { span, .. } | RawGuard::MetadataNeq { span, .. } => {
            *span = 0..0;
        }
        RawGuard::IterationCmp {
            field_span,
            modulus_span,
            op_span,
            rhs_span,
            span,
            ..
        } => {
            *field_span = 0..0;
            if let Some(m) = modulus_span {
                *m = 0..0;
            }
            *op_span = 0..0;
            *rhs_span = 0..0;
            *span = 0..0;
        }
        RawGuard::IterationOutcomeEq {
            field_span,
            value_span,
            span,
            ..
        }
        | RawGuard::IterationOutcomeNeq {
            field_span,
            value_span,
            span,
            ..
        } => {
            *field_span = 0..0;
            *value_span = 0..0;
            *span = 0..0;
        }
        RawGuard::And(l, r, s) | RawGuard::Or(l, r, s) => {
            *s = 0..0;
            zero_guard_spans(l);
            zero_guard_spans(r);
        }
    }
}

/// Rebuild a guard tree so every same-operator chain is left-associated.
fn reassociate_left(g: RawGuard) -> RawGuard {
    let flat_or = flatten(&g, GuardOp::Or);
    if flat_or.len() >= 2 {
        return fold_left(flat_or, GuardOp::Or);
    }
    let flat_and = flatten(&g, GuardOp::And);
    if flat_and.len() >= 2 {
        return fold_left(flat_and, GuardOp::And);
    }
    match g {
        RawGuard::And(l, r, s) => RawGuard::And(
            Box::new(reassociate_left(*l)),
            Box::new(reassociate_left(*r)),
            s,
        ),
        RawGuard::Or(l, r, s) => RawGuard::Or(
            Box::new(reassociate_left(*l)),
            Box::new(reassociate_left(*r)),
            s,
        ),
        leaf => leaf,
    }
}

#[derive(Copy, Clone)]
enum GuardOp {
    And,
    Or,
}

fn flatten(g: &RawGuard, op: GuardOp) -> Vec<RawGuard> {
    let mut out = Vec::new();
    flatten_into(g, op, &mut out);
    out
}

fn flatten_into(g: &RawGuard, op: GuardOp, out: &mut Vec<RawGuard>) {
    match (op, g) {
        (GuardOp::Or, RawGuard::Or(l, r, _)) | (GuardOp::And, RawGuard::And(l, r, _)) => {
            flatten_into(l, op, out);
            flatten_into(r, op, out);
        }
        _ => out.push(g.clone()),
    }
}

fn fold_left(items: Vec<RawGuard>, op: GuardOp) -> RawGuard {
    let mut iter = items.into_iter();
    let first = iter.next().expect("at least one guard");
    let mut acc = reassociate_left(first);
    for next in iter {
        let right = reassociate_left(next);
        let node = match op {
            GuardOp::And => RawGuard::And(Box::new(acc), Box::new(right), 0..0),
            GuardOp::Or => RawGuard::Or(Box::new(acc), Box::new(right), 0..0),
        };
        acc = node;
    }
    acc
}
