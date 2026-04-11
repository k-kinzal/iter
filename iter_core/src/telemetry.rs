//! Telemetry helpers that adapt core domain types to `iter_tracing`.

use std::collections::HashMap;

use iter_tracing::{
    SIGNAL_TRACEPARENT_METADATA_KEY, SIGNAL_TRACESTATE_METADATA_KEY, TRACEPARENT, TRACESTATE,
};

use crate::signal::{MetadataKey, MetadataValue, Signal};

/// Inject the current OpenTelemetry context into a [`Signal`].
///
/// This is used by Triggers immediately before publishing a Signal. The
/// Runner later extracts this context and records it as a Span Link on the
/// independent runner trace.
pub fn inject_current_context_into_signal(signal: &mut Signal) {
    let carrier = iter_tracing::current_context_carrier();
    insert_carrier_value(
        signal,
        SIGNAL_TRACEPARENT_METADATA_KEY,
        &carrier,
        TRACEPARENT,
    );
    insert_carrier_value(signal, SIGNAL_TRACESTATE_METADATA_KEY, &carrier, TRACESTATE);
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
    signal: &mut Signal,
    metadata_key: &str,
    carrier: &HashMap<String, String>,
    carrier_key: &str,
) {
    let Some(value) = carrier.get(carrier_key) else {
        return;
    };
    let Ok(key) = MetadataKey::new(metadata_key) else {
        return;
    };
    signal
        .metadata_mut()
        .insert(key, MetadataValue::String(value.clone()));
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
        let mut signal = Signal::new(Metadata::new());
        signal.metadata_mut().insert(
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
}
