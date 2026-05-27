use crate::ast::{
    ComposeRoot, ComposeTriggerOverride, NamedQueue, QueueRef, ServiceSource, Span, Spanned,
};
use crate::diagnostic::Diagnostic;

pub(super) fn resolve_queue_refs(root: &mut ComposeRoot) -> Result<(), Diagnostic> {
    let queue_count = root.queues.len();
    let single_queue_name = if queue_count == 1 {
        Some(root.queues[0].node.name.clone())
    } else {
        None
    };

    for service in &mut root.services {
        let queue_slot: &mut Option<QueueRef> = match &mut service.node.source {
            ServiceSource::Build { queue, .. } => queue,
            ServiceSource::Inline(inline) => &mut inline.queue,
        };
        check_or_default(
            queue_slot,
            single_queue_name.as_deref(),
            queue_count,
            &service.span,
        )?;
        validate_named(queue_slot.as_ref(), &root.queues, &service.span)?;
    }

    for trigger in &mut root.triggers {
        let mut slot = Some(std::mem::replace(
            &mut trigger.node.target,
            QueueRef::Anonymous,
        ));
        check_or_default(
            &mut slot,
            single_queue_name.as_deref(),
            queue_count,
            &trigger.span,
        )?;
        validate_named(slot.as_ref(), &root.queues, &trigger.span)?;
        trigger.node.target = slot.unwrap_or(QueueRef::Anonymous);
    }

    for compose in &root.composes {
        for queue_ref in compose.node.queues.values() {
            validate_named(Some(queue_ref), &root.queues, &compose.span)?;
        }
        for svc_override in compose.node.services.values() {
            if let Some(ref queue_ref) = svc_override.queue {
                validate_named(Some(queue_ref), &root.queues, &compose.span)?;
            }
        }
        for trig_override in compose.node.triggers.values() {
            if let ComposeTriggerOverride::Override {
                target: Some(queue_ref),
            } = trig_override
            {
                validate_named(Some(queue_ref), &root.queues, &compose.span)?;
            }
        }
    }

    Ok(())
}

fn check_or_default(
    slot: &mut Option<QueueRef>,
    single_queue_name: Option<&str>,
    queue_count: usize,
    span: &Span,
) -> Result<(), Diagnostic> {
    let needs_default = matches!(slot, None | Some(QueueRef::Anonymous));
    if needs_default {
        if let Some(name) = single_queue_name {
            *slot = Some(QueueRef::Named(name.to_owned()));
            return Ok(());
        }
        return Err(Diagnostic::error(
            span.clone(),
            if queue_count == 0 {
                "compose.iter declares no `queue` blocks; add one or qualify the binding"
            } else {
                "queue reference omitted but compose.iter declares more than one queue"
            },
        ));
    }
    Ok(())
}

fn validate_named(
    slot: Option<&QueueRef>,
    queues: &[Spanned<NamedQueue>],
    span: &Span,
) -> Result<(), Diagnostic> {
    if let Some(QueueRef::Named(name)) = slot {
        if !queues.iter().any(|q| &q.node.name == name) {
            return Err(Diagnostic::error(
                span.clone(),
                format!("queue `{name}` is not declared in this compose.iter"),
            ));
        }
    }
    Ok(())
}
