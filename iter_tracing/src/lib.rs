//! OpenTelemetry setup and propagation helpers shared by iter crates.
//!
//! The crate intentionally stays independent of every other iter crate so they
//! can share the same W3C trace context and OTLP wiring without forming
//! dependency cycles. It owns telemetry *mechanism* only — reading the standard
//! `OTEL_*` environment contract, building exporters, and propagating context.
//! Deciding a telemetry *value* (such as the `service.name`) is policy each
//! binary owns; this crate never invents one.

use std::collections::{BTreeMap, HashMap};
use std::io;

use opentelemetry::global;
use opentelemetry::logs::LogRecord as _;
use opentelemetry::propagation::{Extractor, Injector};
pub use opentelemetry::trace::SpanContext;
use opentelemetry::trace::{Status, TraceContextExt, TracerProvider as _};
use opentelemetry::{Context, InstrumentationScope, KeyValue};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::{LogProcessor, SdkLogRecord, SdkLoggerProvider};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tokio::process::Command;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;

/// Default OTLP HTTP endpoint used when telemetry is explicitly enabled but no
/// endpoint is supplied by config or environment.
pub const DEFAULT_OTLP_HTTP_ENDPOINT: &str = "http://localhost:4318";

/// Internal switch a parent process sets to enable telemetry in a spawned
/// child process.
pub const ITER_OTEL_ENABLED: &str = "ITER_OTEL_ENABLED";

/// W3C trace context carrier key.
pub const TRACEPARENT: &str = "traceparent";

/// W3C trace state carrier key.
pub const TRACESTATE: &str = "tracestate";

/// Metadata key that carries the W3C `traceparent` across a process boundary,
/// preserving the parent span context.
pub const SIGNAL_TRACEPARENT_METADATA_KEY: &str = "iter.otel.traceparent";

/// Metadata key that carries W3C `tracestate` across a process boundary.
pub const SIGNAL_TRACESTATE_METADATA_KEY: &str = "iter.otel.tracestate";

/// OpenTelemetry protocol supported by iter's direct OTLP exporter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtelProtocol {
    /// OTLP over HTTP/protobuf.
    HttpProtobuf,
}

/// Resolved runtime OTLP configuration: endpoints and resource attributes
/// after the standard `OTEL_*` environment contract has been read.
#[derive(Debug, Clone)]
pub struct OtelRuntimeConfig {
    /// `service.name` for the current process, or `None` when the owning
    /// binary has not supplied one. This crate never invents a value; an
    /// absent name falls through to the OpenTelemetry SDK default.
    pub service_name: Option<String>,
    /// OTLP endpoint. For `http/protobuf`, either the collector base endpoint
    /// or the full `/v1/traces` endpoint is accepted.
    pub endpoint: String,
    /// OTLP logs endpoint. For `http/protobuf`, either the collector base
    /// endpoint or the full `/v1/logs` endpoint is accepted.
    pub logs_endpoint: String,
    /// Extra resource attributes attached to the tracer provider.
    pub resource_attributes: BTreeMap<String, String>,
    /// Export protocol.
    pub protocol: OtelProtocol,
}

impl OtelRuntimeConfig {
    /// Build a config from environment variables.
    ///
    /// Returns `None` unless telemetry is explicitly enabled by
    /// `ITER_OTEL_ENABLED=true`, `OTEL_TRACES_EXPORTER=otlp`, or
    /// `OTEL_LOGS_EXPORTER=otlp`.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let enabled = std::env::var(ITER_OTEL_ENABLED)
            .ok()
            .is_some_and(|v| v == "true")
            || std::env::var("OTEL_TRACES_EXPORTER")
                .ok()
                .is_some_and(|v| v.split(',').any(|part| part.trim() == "otlp"))
            || std::env::var("OTEL_LOGS_EXPORTER")
                .ok()
                .is_some_and(|v| v.split(',').any(|part| part.trim() == "otlp"));
        if !enabled {
            return None;
        }
        Some(Self {
            service_name: std::env::var("OTEL_SERVICE_NAME").ok(),
            endpoint: resolve_traces_endpoint(None),
            logs_endpoint: resolve_logs_endpoint(None),
            resource_attributes: parse_resource_attributes_env(),
            protocol: OtelProtocol::HttpProtobuf,
        })
    }
}

/// Resolve the OTLP traces endpoint: an explicit override wins, then the
/// standard `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` /
/// `OTEL_EXPORTER_OTLP_ENDPOINT` environment contract, falling back to
/// [`DEFAULT_OTLP_HTTP_ENDPOINT`].
///
/// This is the single home for the traces endpoint fallback chain; every
/// binary that builds an [`OtelRuntimeConfig`] resolves through it rather than
/// re-encoding the precedence.
#[must_use]
pub fn resolve_traces_endpoint(explicit: Option<String>) -> String {
    explicit
        .or_else(|| std::env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").ok())
        .or_else(|| std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok())
        .unwrap_or_else(|| DEFAULT_OTLP_HTTP_ENDPOINT.to_string())
}

/// Resolve the OTLP logs endpoint: an explicit override wins, then the standard
/// `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` / `OTEL_EXPORTER_OTLP_ENDPOINT`
/// environment contract, falling back to [`DEFAULT_OTLP_HTTP_ENDPOINT`].
///
/// The single home for the logs endpoint fallback chain (see
/// [`resolve_traces_endpoint`]).
#[must_use]
pub fn resolve_logs_endpoint(explicit: Option<String>) -> String {
    explicit
        .or_else(|| std::env::var("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT").ok())
        .or_else(|| std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok())
        .unwrap_or_else(|| DEFAULT_OTLP_HTTP_ENDPOINT.to_string())
}

/// Keeps process-global telemetry resources alive until shutdown.
pub struct TelemetryGuard {
    tracer_provider: Option<SdkTracerProvider>,
    logger_provider: Option<SdkLoggerProvider>,
}

impl TelemetryGuard {
    /// Create a guard that owns the providers.
    #[must_use]
    pub fn new(tracer_provider: SdkTracerProvider, logger_provider: SdkLoggerProvider) -> Self {
        Self {
            tracer_provider: Some(tracer_provider),
            logger_provider: Some(logger_provider),
        }
    }

    /// Create an empty no-op guard.
    #[must_use]
    pub fn noop() -> Self {
        Self {
            tracer_provider: None,
            logger_provider: None,
        }
    }

    /// Release and shut down the providers early.
    pub fn shutdown(&mut self) {
        if let Some(provider) = self.logger_provider.take() {
            drop(provider.shutdown());
        }
        if let Some(provider) = self.tracer_provider.take() {
            drop(provider.shutdown());
        }
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Install W3C trace context propagation globally for this process.
pub fn install_trace_context_propagator() {
    global::set_text_map_propagator(TraceContextPropagator::new());
}

/// Build an SDK tracer provider for the supplied runtime config.
///
/// # Errors
///
/// Returns an exporter build error when the endpoint/protocol combination
/// cannot create an OTLP exporter.
pub fn build_tracer_provider(
    config: &OtelRuntimeConfig,
) -> Result<SdkTracerProvider, opentelemetry_otlp::ExporterBuildError> {
    match config.protocol {
        OtelProtocol::HttpProtobuf => {}
    }

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(trace_endpoint(&config.endpoint))
        .build()?;

    Ok(SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource(config))
        .build())
}

/// Build an SDK logger provider for the supplied runtime config.
///
/// # Errors
///
/// Returns an exporter build error when the endpoint/protocol combination
/// cannot create an OTLP log exporter.
pub fn build_logger_provider(
    config: &OtelRuntimeConfig,
) -> Result<SdkLoggerProvider, opentelemetry_otlp::ExporterBuildError> {
    match config.protocol {
        OtelProtocol::HttpProtobuf => {}
    }

    let exporter = opentelemetry_otlp::LogExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(logs_endpoint(&config.logs_endpoint))
        .build()?;

    Ok(SdkLoggerProvider::builder()
        .with_log_processor(TraceContextAttributesProcessor)
        .with_batch_exporter(exporter)
        .with_resource(resource(config))
        .build())
}

/// Build the tracing layer that exports spans to OpenTelemetry.
#[must_use]
pub fn otel_layer<S>(
    provider: &SdkTracerProvider,
    instrumentation_name: &'static str,
) -> impl Layer<S> + Send + Sync + 'static
where
    S: tracing::Subscriber + for<'span> LookupSpan<'span> + Send + Sync,
{
    let tracer = provider.tracer(instrumentation_name);
    tracing_opentelemetry::layer()
        .with_tracer(tracer)
        .with_error_events_to_status(true)
        .with_error_records_to_exceptions(true)
}

/// Build the tracing layer that exports `tracing` events to OpenTelemetry
/// logs while preserving trace/span IDs from the active `OTel` span.
///
/// The OTel-internal targets (the exporter, transport, and bridge crates) are
/// filtered out here so their own diagnostics never recurse back into the log
/// pipeline. This is the single home for that predicate; callers attach the
/// layer as-is.
#[must_use]
pub fn otel_log_layer<S>(provider: &SdkLoggerProvider) -> impl Layer<S> + Send + Sync + 'static
where
    S: tracing::Subscriber + for<'span> LookupSpan<'span> + Send + Sync,
{
    OpenTelemetryTracingBridge::new(provider).with_filter(filter_fn(otel_log_target_enabled))
}

/// Install a stderr tracing subscriber for a standalone binary.
///
/// `otel` is the caller-resolved OTLP configuration (or `None` to leave export
/// disabled). When present, this also installs trace and log export layers and
/// returns a guard that keeps the providers alive. The caller owns the
/// `service.name` decision — this crate does not invent one.
#[must_use]
pub fn install_stderr_subscriber(
    env_filter: EnvFilter,
    json: bool,
    otel: Option<OtelRuntimeConfig>,
) -> TelemetryGuard {
    install_trace_context_propagator();
    let stderr_layer: Box<dyn Layer<tracing_subscriber::Registry> + Send + Sync> = if json {
        fmt::layer().json().with_writer(io::stderr).boxed()
    } else {
        fmt::layer().with_writer(io::stderr).boxed()
    };
    install_stderr_subscriber_inner(env_filter, stderr_layer, otel)
}

fn install_stderr_subscriber_inner(
    env_filter: EnvFilter,
    stderr_layer: Box<dyn Layer<tracing_subscriber::Registry> + Send + Sync>,
    otel: Option<OtelRuntimeConfig>,
) -> TelemetryGuard {
    let Some(otel_config) = otel else {
        let _installed = tracing_subscriber::registry()
            .with(stderr_layer)
            .with(env_filter)
            .try_init()
            .is_ok();
        return TelemetryGuard::noop();
    };

    let tracer_provider = match build_tracer_provider(&otel_config) {
        Ok(provider) => provider,
        Err(err) => {
            eprintln!("warning: failed to initialize OpenTelemetry trace exporter: {err}");
            let _installed = tracing_subscriber::registry()
                .with(stderr_layer)
                .with(env_filter)
                .try_init()
                .is_ok();
            return TelemetryGuard::noop();
        }
    };
    let logger_provider = match build_logger_provider(&otel_config) {
        Ok(provider) => provider,
        Err(err) => {
            eprintln!("warning: failed to initialize OpenTelemetry log exporter: {err}");
            let _installed = tracing_subscriber::registry()
                .with(stderr_layer)
                .with(env_filter)
                .try_init()
                .is_ok();
            return TelemetryGuard::noop();
        }
    };

    // "iter" names the shared tracing instrumentation scope (this library
    // itself), not a service; the SDK `service.name` comes from the resolved
    // config's resource.
    let otel_trace_layer = otel_layer(&tracer_provider, "iter");
    let otel_log_layer = otel_log_layer(&logger_provider);
    let _installed = tracing_subscriber::registry()
        .with(stderr_layer)
        .with(otel_log_layer)
        .with(otel_trace_layer)
        .with(env_filter)
        .try_init()
        .is_ok();

    TelemetryGuard::new(tracer_provider, logger_provider)
}

fn otel_log_target_enabled(metadata: &tracing::Metadata<'_>) -> bool {
    let target = metadata.target();
    !matches!(
        target.split("::").next(),
        Some(
            "opentelemetry"
                | "opentelemetry_sdk"
                | "opentelemetry_otlp"
                | "opentelemetry_appender_tracing"
                | "tracing_opentelemetry"
                | "hyper"
                | "h2"
                | "tonic"
                | "reqwest"
        )
    )
}

fn resource(config: &OtelRuntimeConfig) -> Resource {
    // When no name was supplied, leave the SDK's own default rather than
    // inventing one here — deciding the value is the binary's policy.
    let mut resource = match &config.service_name {
        Some(service_name) => Resource::builder().with_service_name(service_name.clone()),
        None => Resource::builder(),
    };
    for (key, value) in &config.resource_attributes {
        resource = resource.with_attribute(KeyValue::new(key.clone(), value.clone()));
    }
    resource.build()
}

#[derive(Debug)]
struct TraceContextAttributesProcessor;

impl LogProcessor for TraceContextAttributesProcessor {
    fn emit(&self, data: &mut SdkLogRecord, _instrumentation: &InstrumentationScope) {
        let Some(context) = data.trace_context() else {
            return;
        };
        let trace_id = context.trace_id.to_string();
        let span_id = context.span_id.to_string();
        data.add_attribute("trace_id", trace_id);
        data.add_attribute("span_id", span_id);
    }

    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }
}

/// Convert a base OTLP endpoint into the trace export endpoint.
#[must_use]
pub fn trace_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim_end_matches('/');
    if trimmed.ends_with("/v1/traces") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1/traces")
    }
}

/// Convert a base OTLP endpoint into the log export endpoint.
#[must_use]
pub fn logs_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim_end_matches('/');
    if trimmed.ends_with("/v1/logs") {
        trimmed.to_string()
    } else if let Some(base) = trimmed.strip_suffix("/v1/traces") {
        format!("{base}/v1/logs")
    } else {
        format!("{trimmed}/v1/logs")
    }
}

/// Parse `OTEL_RESOURCE_ATTRIBUTES` into a stable map.
#[must_use]
pub fn parse_resource_attributes_env() -> BTreeMap<String, String> {
    std::env::var("OTEL_RESOURCE_ATTRIBUTES")
        .ok()
        .into_iter()
        .flat_map(|raw| parse_resource_attributes(&raw))
        .collect()
}

/// Parse an `OTel` resource attribute list.
///
/// The format follows the environment variable convention where entries are
/// comma-separated `key=value` pairs and separators can be backslash-escaped.
#[must_use]
pub fn parse_resource_attributes(input: &str) -> BTreeMap<String, String> {
    split_escaped(input, ',')
        .into_iter()
        .filter_map(|entry| {
            let (key, value) = split_once_escaped(&entry, '=')?;
            let key = unescape_resource_attribute_part(key.trim());
            if key.is_empty() {
                return None;
            }
            Some((key, unescape_resource_attribute_part(value.trim())))
        })
        .collect()
}

/// Format resource attributes for `OTEL_RESOURCE_ATTRIBUTES`.
#[must_use]
pub fn format_resource_attributes<'a>(
    attrs: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> String {
    attrs
        .into_iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                escape_resource_attribute_part(key),
                escape_resource_attribute_part(value)
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn split_once_escaped(input: &str, needle: char) -> Option<(&str, &str)> {
    let mut escaped = false;
    for (idx, ch) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == needle {
            return Some((&input[..idx], &input[idx + ch.len_utf8()..]));
        }
    }
    None
}

fn split_escaped(input: &str, needle: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for ch in input.chars() {
        if escaped {
            current.push('\\');
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == needle {
            out.push(current);
            current = String::new();
            continue;
        }
        current.push(ch);
    }
    if escaped {
        current.push('\\');
    }
    out.push(current);
    out
}

fn escape_resource_attribute_part(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            ',' | '=' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

fn unescape_resource_attribute_part(input: &str) -> String {
    let mut out = String::new();
    let mut escaped = false;
    for ch in input.chars() {
        if escaped {
            out.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        out.push(ch);
    }
    if escaped {
        out.push('\\');
    }
    out
}

/// Inject the current tracing span's OpenTelemetry context into a text-map
/// carrier.
pub fn inject_current_context(carrier: &mut impl Injector) {
    let context = tracing::Span::current().context();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, carrier);
    });
}

/// Extract an OpenTelemetry context from a text-map carrier.
#[must_use]
pub fn extract_context(carrier: &impl Extractor) -> Context {
    global::get_text_map_propagator(|propagator| propagator.extract(carrier))
}

/// Extract a valid remote span context from a carrier.
#[must_use]
pub fn extract_span_context(carrier: &impl Extractor) -> Option<SpanContext> {
    let context = extract_context(carrier);
    let span = context.span();
    let span_context = span.span_context();
    span_context.is_valid().then(|| span_context.clone())
}

/// Force a tracing span to begin a new OpenTelemetry trace.
pub fn set_span_as_trace_root(span: &tracing::Span) {
    if let Err(err) = span.set_parent(Context::new()) {
        tracing::debug!(error = %err, "failed to set OpenTelemetry span parent");
    }
}

/// Add a Span Link from `span` to a span context from another trace.
pub fn add_span_link(span: &tracing::Span, span_context: SpanContext) {
    span.add_link(span_context);
}

/// Mark a span as failed and attach a compact exception event.
///
/// `source` names which operation produced the error (the caller's
/// `ErrorSource` wire form, e.g. `"workspace_setup"`); it is attached as the
/// `iter.error.source` attribute.
pub fn record_span_error(span: &tracing::Span, source: &'static str, message: &str) {
    span.set_status(Status::error(message.to_owned()));
    span.add_event(
        "exception",
        vec![
            KeyValue::new("exception.message", message.to_owned()),
            KeyValue::new("iter.error.source", source),
        ],
    );
}

/// Return a carrier map containing the current span context.
#[must_use]
pub fn current_context_carrier() -> HashMap<String, String> {
    let mut carrier = HashMap::new();
    inject_current_context(&mut carrier);
    carrier
}

/// Return the current W3C `traceparent` value, if the active span has a valid
/// OpenTelemetry context.
#[must_use]
pub fn current_traceparent() -> Option<String> {
    current_context_carrier().remove(TRACEPARENT)
}

/// Inject the current span context into a child process environment.
///
/// The W3C keys are inserted as both canonical lowercase carrier names and
/// uppercase environment-variable aliases so agents can support either
/// convention.
///
/// Returns `true` when a valid `traceparent` was available for injection.
#[must_use]
pub fn inject_current_context_env(command: &mut Command) -> bool {
    let carrier = current_context_carrier();
    let has_traceparent = carrier.contains_key(TRACEPARENT);
    for (key, value) in carrier {
        command.env(&key, &value);
        command.env(key.to_ascii_uppercase(), value);
    }
    has_traceparent
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::{SpanId, TraceId, logs::AnyValue};
    use opentelemetry_sdk::{
        logs::InMemoryLogExporter,
        trace::{InMemorySpanExporterBuilder, SdkTracerProvider},
    };
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn trace_endpoint_accepts_base_or_trace_endpoint() {
        assert_eq!(
            trace_endpoint("http://collector:4318"),
            "http://collector:4318/v1/traces"
        );
        assert_eq!(
            trace_endpoint("http://collector:4318/v1/traces"),
            "http://collector:4318/v1/traces"
        );
    }

    #[test]
    fn logs_endpoint_accepts_base_log_or_trace_endpoint() {
        assert_eq!(
            logs_endpoint("http://collector:4318"),
            "http://collector:4318/v1/logs"
        );
        assert_eq!(
            logs_endpoint("http://collector:4318/v1/logs"),
            "http://collector:4318/v1/logs"
        );
        assert_eq!(
            logs_endpoint("http://collector:4318/v1/traces"),
            "http://collector:4318/v1/logs"
        );
    }

    #[test]
    fn resource_attribute_roundtrip_escapes_separators() {
        let encoded = format_resource_attributes([
            ("service.name", "iter"),
            ("iter.workspace.path", "/tmp/a,b=c\\d"),
        ]);
        let attrs = parse_resource_attributes(&encoded);
        assert_eq!(attrs.get("service.name"), Some(&"iter".to_string()));
        assert_eq!(
            attrs.get("iter.workspace.path"),
            Some(&"/tmp/a,b=c\\d".to_string())
        );
    }

    #[test]
    fn otel_log_layer_records_active_trace_context() {
        let log_exporter = InMemoryLogExporter::default();
        let logger_provider = SdkLoggerProvider::builder()
            .with_log_processor(TraceContextAttributesProcessor)
            .with_simple_exporter(log_exporter.clone())
            .build();

        let span_exporter = InMemorySpanExporterBuilder::new().build();
        let tracer_provider = SdkTracerProvider::builder()
            .with_simple_exporter(span_exporter.clone())
            .build();
        let tracer = tracer_provider.tracer("iter-test");

        let subscriber = tracing_subscriber::registry()
            .with(otel_log_layer(&logger_provider))
            .with(tracing_opentelemetry::layer().with_tracer(tracer));
        let _guard = tracing::subscriber::set_default(subscriber);

        tracing::info_span!("test.span").in_scope(|| {
            tracing::info!(event = "test_event", "test log");
        });

        logger_provider.force_flush().expect("flush logs");

        let logs = log_exporter.get_emitted_logs().expect("emitted logs");
        let log = logs.first().expect("log emitted");
        let trace_context = log.record.trace_context().expect("trace context");
        assert_ne!(trace_context.trace_id, TraceId::INVALID);
        assert_ne!(trace_context.span_id, SpanId::INVALID);

        let attributes = log
            .record
            .attributes_iter()
            .filter_map(|(key, value)| match value {
                AnyValue::String(value) => Some((key.as_str(), value.as_str())),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(attributes.iter().any(|(key, value)| {
            *key == "trace_id" && *value == trace_context.trace_id.to_string()
        }));
        assert!(attributes.iter().any(|(key, value)| {
            *key == "span_id" && *value == trace_context.span_id.to_string()
        }));
    }
}
