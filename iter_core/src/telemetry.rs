//! Telemetry helpers that adapt core domain types to `iter_tracing`.

use std::collections::HashMap;

use iter_tracing::{
    SIGNAL_TRACEPARENT_METADATA_KEY, SIGNAL_TRACESTATE_METADATA_KEY, TRACEPARENT, TRACESTATE,
};

use crate::signal::{MetadataKey, MetadataValue, Signal};

/// Inject the current OpenTelemetry context into a [`Signal`], returning the
/// derived signal.
///
/// This is used by Triggers immediately before publishing a Signal. Because a
/// `Signal` is immutable after construction, the carrier is folded in by
/// constructing a new signal (see [`Signal::with_metadata_value`]) rather than
/// mutating in place. The Runner later extracts this context and records it as
/// a Span Link on the independent runner trace.
#[must_use]
pub fn inject_current_context_into_signal(signal: Signal) -> Signal {
    let carrier = iter_tracing::current_context_carrier();
    let signal = insert_carrier_value(
        signal,
        SIGNAL_TRACEPARENT_METADATA_KEY,
        &carrier,
        TRACEPARENT,
    );
    insert_carrier_value(signal, SIGNAL_TRACESTATE_METADATA_KEY, &carrier, TRACESTATE)
}

/// Extract the trigger span context stored on a [`Signal`].
#[must_use]
pub fn span_context_from_signal(signal: &Signal) -> Option<iter_tracing::SpanContext> {
    let mut carrier = HashMap::new();
    insert_metadata_carrier_value(
        &mut carrier,
        signal,
        SIGNAL_TRACEPARENT_METADATA_KEY,
        TRACEPARENT,
    );
    insert_metadata_carrier_value(
        &mut carrier,
        signal,
        SIGNAL_TRACESTATE_METADATA_KEY,
        TRACESTATE,
    );
    iter_tracing::extract_span_context(&carrier)
}

fn insert_carrier_value(
    signal: Signal,
    metadata_key: &str,
    carrier: &HashMap<String, String>,
    carrier_key: &str,
) -> Signal {
    let Some(value) = carrier.get(carrier_key) else {
        return signal;
    };
    let Ok(key) = MetadataKey::new(metadata_key) else {
        return signal;
    };
    signal.with_metadata_value(key, MetadataValue::String(value.clone()))
}

fn insert_metadata_carrier_value(
    carrier: &mut HashMap<String, String>,
    signal: &Signal,
    metadata_key: &str,
    carrier_key: &str,
) {
    let Ok(key) = MetadataKey::new(metadata_key) else {
        return;
    };
    if let Some(MetadataValue::String(value)) = signal.metadata().get(&key) {
        carrier.insert(carrier_key.to_string(), value.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::Metadata;

    #[test]
    fn extracts_span_context_from_signal_metadata() {
        iter_tracing::install_trace_context_propagator();
        let signal = Signal::new(Metadata::new()).with_metadata_value(
            MetadataKey::new(SIGNAL_TRACEPARENT_METADATA_KEY).unwrap(),
            MetadataValue::String(
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_string(),
            ),
        );

        let span_context = span_context_from_signal(&signal).expect("valid span context");

        assert!(span_context.is_valid());
        assert!(span_context.is_remote());
    }

    #[test]
    fn ignores_missing_signal_context() {
        iter_tracing::install_trace_context_propagator();
        let signal = Signal::new(Metadata::new());

        assert!(span_context_from_signal(&signal).is_none());
    }

    #[test]
    fn inject_without_active_span_returns_signal_unchanged() {
        iter_tracing::install_trace_context_propagator();
        let original = Signal::new(Metadata::new());
        let (id, kind, created_at, metadata_len) = (
            original.id(),
            original.kind(),
            original.created_at(),
            original.metadata().len(),
        );

        // Move the signal in — exercising the real consume-and-return path,
        // not a clone. With no active span there is no carrier to fold in, so
        // the builder hands back an otherwise-identical signal.
        let injected = inject_current_context_into_signal(original);

        assert_eq!(injected.id(), id);
        assert_eq!(injected.kind(), kind);
        assert_eq!(injected.created_at(), created_at);
        assert_eq!(injected.metadata().len(), metadata_len);
    }
}
