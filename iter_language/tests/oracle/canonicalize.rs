//! Span canonicalisation: replace every `Span` inside a `CstFile` with
//! `0..0` so structural comparison between the hand-written CST and the
//! oracle CST is span-oblivious.
//!
//! Span equality is out of scope for the grammar-spec differential: the two
//! implementations tokenise at slightly different boundaries (the
//! hand-written lexer's span for a duration spans digits + suffix, pest's
//! span for the same literal spans digits + suffix — they happen to agree
//! today but the spec guarantee is shape, not span byte ranges). We
//! compare shape explicitly and leave span equivalence as a non-goal.

use iter_language::{CstBlock, CstField, CstFile, CstGuard, CstIdent, CstSection, CstValue};

pub(crate) fn canonicalize(file: &mut CstFile) {
    for s in &mut file.sections {
        canon_section(s);
    }
}

fn canon_section(s: &mut CstSection) {
    match s {
        CstSection::Block {
            keyword_span,
            kind,
            kind2,
            alias,
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
            if let Some(a) = alias {
                canon_ident(a);
            }
            if let Some(b) = body {
                canon_block(b);
            }
        }
        CstSection::Prompt {
            keyword_span,
            name: _,
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
        CstSection::On {
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

fn canon_block(b: &mut CstBlock) {
    b.span = 0..0;
    for f in &mut b.fields {
        canon_field(f);
    }
    for r in &mut b.routes {
        r.span = 0..0;
        if let Some(sp) = &mut r.when_span {
            *sp = 0..0;
        }
        canon_block(&mut r.body);
    }
    for a in &mut b.actions {
        a.keyword_span = 0..0;
        a.span = 0..0;
    }
}

fn canon_field(f: &mut CstField) {
    f.span = 0..0;
    canon_ident(&mut f.name);
    canon_value(&mut f.value);
}

fn canon_value(v: &mut CstValue) {
    match v {
        CstValue::String(_, s)
        | CstValue::Integer(_, s)
        | CstValue::Duration(_, s)
        | CstValue::Bool(_, s)
        | CstValue::Null(s)
        | CstValue::Ident(_, s) => *s = 0..0,
        CstValue::List(items, s) => {
            *s = 0..0;
            for it in items {
                canon_value(it);
            }
        }
        CstValue::Block(b) => canon_block(b),
        CstValue::Call { args, span, .. } => {
            *span = 0..0;
            for a in args {
                canon_value(a);
            }
        }
    }
}

fn canon_ident(i: &mut CstIdent) {
    i.span = 0..0;
}

fn canon_guard(g: &mut CstGuard) {
    zero_guard_spans(g);
    // Both the hand-written recursive-descent parser and the oracle parser
    // produce left-associated `And`/`Or` chains; `CstFile`s synthesised by
    // proptest strategies can be right-associated. Normalise here so
    // structural comparison is associativity-oblivious for same-kind
    // chains.
    *g = reassociate_left(g.clone());
}

fn zero_guard_spans(g: &mut CstGuard) {
    match g {
        CstGuard::MetadataEq { span, .. } | CstGuard::MetadataNeq { span, .. } => {
            *span = 0..0;
        }
        CstGuard::IterationCmp {
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
        CstGuard::IterationResultEq {
            field_span,
            value_span,
            span,
            ..
        }
        | CstGuard::IterationResultNeq {
            field_span,
            value_span,
            span,
            ..
        } => {
            *field_span = 0..0;
            *value_span = 0..0;
            *span = 0..0;
        }
        CstGuard::And(l, r, s) | CstGuard::Or(l, r, s) => {
            *s = 0..0;
            zero_guard_spans(l);
            zero_guard_spans(r);
        }
    }
}

/// Rebuild a guard tree so every same-operator chain is left-associated.
fn reassociate_left(g: CstGuard) -> CstGuard {
    let flat_or = flatten(&g, GuardOp::Or);
    if flat_or.len() >= 2 {
        return fold_left(flat_or, GuardOp::Or);
    }
    let flat_and = flatten(&g, GuardOp::And);
    if flat_and.len() >= 2 {
        return fold_left(flat_and, GuardOp::And);
    }
    match g {
        CstGuard::And(l, r, s) => CstGuard::And(
            Box::new(reassociate_left(*l)),
            Box::new(reassociate_left(*r)),
            s,
        ),
        CstGuard::Or(l, r, s) => CstGuard::Or(
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

fn flatten(g: &CstGuard, op: GuardOp) -> Vec<CstGuard> {
    let mut out = Vec::new();
    flatten_into(g, op, &mut out);
    out
}

fn flatten_into(g: &CstGuard, op: GuardOp, out: &mut Vec<CstGuard>) {
    match (op, g) {
        (GuardOp::Or, CstGuard::Or(l, r, _)) | (GuardOp::And, CstGuard::And(l, r, _)) => {
            flatten_into(l, op, out);
            flatten_into(r, op, out);
        }
        _ => out.push(g.clone()),
    }
}

fn fold_left(items: Vec<CstGuard>, op: GuardOp) -> CstGuard {
    let mut iter = items.into_iter();
    let first = iter.next().expect("at least one guard");
    let mut acc = reassociate_left(first);
    for next in iter {
        let right = reassociate_left(next);
        let node = match op {
            GuardOp::And => CstGuard::And(Box::new(acc), Box::new(right), 0..0),
            GuardOp::Or => CstGuard::Or(Box::new(acc), Box::new(right), 0..0),
        };
        acc = node;
    }
    acc
}
