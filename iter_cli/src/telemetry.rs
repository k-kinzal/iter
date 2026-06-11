//! `tracing-subscriber` initialisation driven by `--debug` and the user
//! [`Config`](iter_core::Config).
//!
//! The CLI installs exactly one global subscriber per process. The level
//! is selected as follows, in order:
//!
//! 1. `--debug` on the command line ⇒ `DEBUG`.
//! 2. The `RUST_LOG` environment variable, if present ⇒ honoured by
//!    [`EnvFilter`](tracing_subscriber::EnvFilter).
//! 3. `log_level` from the user config ⇒ that level.
//! 4. Otherwise the default ⇒ `INFO`.
//!
//! `init` is idempotent: a second call after the global subscriber has
//! already been registered is a no-op so reconfiguring the CLI from inside a
//! test does not panic.
//!
//! The subscriber installs two formatter layers:
//!
//! * a stderr layer for terminal/console visibility, and
//! * a `log.ndjson` layer that funnels every formatted record into the
//!   per-process [`crate::process::ProcessRuntime`]'s `log.ndjson` via
//!   [`iter_core::process::install_global_log_sender`]. The runtime
//!   publishes its [`LogSender`](iter_core::process::LogSender)
//!   as soon as it is constructed; lines emitted before that — typically
//!   CLI startup — only land on stderr.

use std::io::{self, Write};

use iter_core::log::LogStream;
use iter_core::process::{LIFECYCLE_TARGET, global_log_sender};
use iter_core::{Config, LogLevel};
use iter_language::{TelemetryDef, TelemetryProtocol};
use tracing::Level;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// Install the global tracing subscriber.
///
/// `debug` reflects the `--debug` flag on the chosen subcommand. `config`
/// is the loaded user [`Config`].
///
/// Returns a guard that keeps any configured OpenTelemetry provider alive and
/// shuts it down when the caller finishes.
pub fn init(debug: bool, config: &Config) -> TelemetryGuard {
    init_inner(debug, config, iter_tracing::OtelRuntimeConfig::from_env())
}

/// Install tracing for a compose-managed process using the parsed
/// `compose.iter` telemetry block. Environment variables still win when the
/// compose file omits a field, so detached service subprocesses and manually
/// launched services share the same path.
pub fn init_for_compose(
    debug: bool,
    config: &Config,
    telemetry: Option<&TelemetryDef>,
    project: &str,
    component: Option<&str>,
) -> TelemetryGuard {
    init_inner(
        debug,
        config,
        compose_otel_config(telemetry, project, component),
    )
}

fn init_inner(
    debug: bool,
    config: &Config,
    otel: Option<iter_tracing::OtelRuntimeConfig>,
) -> TelemetryGuard {
    iter_tracing::install_trace_context_propagator();

    let level = resolve_level(debug, config);
    let env_filter = match EnvFilter::try_from_default_env() {
        Ok(filter) => filter,
        Err(_) => EnvFilter::new(level.to_string()),
    };

    let stderr_layer = fmt::layer().with_target(false).with_writer(io::stderr);
    // The `iter::lifecycle` target is delivered to `log.ndjson`
    // directly by the lifecycle writer task via the back-pressured
    // [`LogSender::send_line`](iter_core::process::LogSender::send_line)
    // path. Filtering it out here keeps the on-disk record
    // duplicate-free; the stderr layer above is unfiltered so
    // foreground attach still shows the lifecycle stream.
    let log_layer = fmt::layer()
        .with_target(false)
        .with_ansi(false)
        .with_writer(LogJsonMakeWriter)
        .with_filter(filter_fn(|metadata| metadata.target() != LIFECYCLE_TARGET));

    let Some(otel_config) = otel else {
        let _installed = tracing_subscriber::registry()
            .with(env_filter)
            .with(stderr_layer)
            .with(log_layer)
            .try_init()
            .is_ok();
        return TelemetryGuard {
            _inner: iter_tracing::TelemetryGuard::noop(),
        };
    };

    let tracer_provider = match iter_tracing::build_tracer_provider(&otel_config) {
        Ok(provider) => provider,
        Err(err) => {
            eprintln!("warning: failed to initialize OpenTelemetry trace exporter: {err}");
            let _installed = tracing_subscriber::registry()
                .with(env_filter)
                .with(stderr_layer)
                .with(log_layer)
                .try_init()
                .is_ok();
            return TelemetryGuard {
                _inner: iter_tracing::TelemetryGuard::noop(),
            };
        }
    };
    let logger_provider = match iter_tracing::build_logger_provider(&otel_config) {
        Ok(provider) => provider,
        Err(err) => {
            eprintln!("warning: failed to initialize OpenTelemetry log exporter: {err}");
            let _installed = tracing_subscriber::registry()
                .with(env_filter)
                .with(stderr_layer)
                .with(log_layer)
                .try_init()
                .is_ok();
            return TelemetryGuard {
                _inner: iter_tracing::TelemetryGuard::noop(),
            };
        }
    };
    let otel_layer = iter_tracing::otel_layer(&tracer_provider, "iter");
    let otel_log_layer = iter_tracing::otel_log_layer(&logger_provider)
        .with_filter(filter_fn(otel_log_target_enabled));

    let _installed = tracing_subscriber::registry()
        .with(env_filter)
        .with(stderr_layer)
        .with(log_layer)
        .with(otel_log_layer)
        .with(otel_layer)
        .try_init()
        .is_ok();

    TelemetryGuard {
        _inner: iter_tracing::TelemetryGuard::new(tracer_provider, logger_provider),
    }
}

/// Compute the effective log level. Public for tests.
#[must_use]
pub fn resolve_level(debug: bool, config: &Config) -> Level {
    if debug {
        return Level::DEBUG;
    }
    config
        .log_level
        .map_or(Level::INFO, LogLevel::as_tracing_level)
}

/// Keeps process-global telemetry resources alive until shutdown.
pub struct TelemetryGuard {
    _inner: iter_tracing::TelemetryGuard,
}

fn compose_otel_config(
    telemetry: Option<&TelemetryDef>,
    project: &str,
    component: Option<&str>,
) -> Option<iter_tracing::OtelRuntimeConfig> {
    match telemetry {
        Some(decl) => otel_config_from_compose(decl, project, component),
        None => iter_tracing::OtelRuntimeConfig::from_env(),
    }
}

fn otel_config_from_compose(
    decl: &TelemetryDef,
    project: &str,
    component: Option<&str>,
) -> Option<iter_tracing::OtelRuntimeConfig> {
    if !decl.enabled {
        return None;
    }
    let mut resource_attributes = decl.resource_attributes.clone();
    resource_attributes
        .entry("iter.compose.project".to_string())
        .or_insert_with(|| project.to_string());
    if let Some(component) = component {
        resource_attributes
            .entry("iter.compose.service".to_string())
            .or_insert_with(|| component.to_string());
    }
    if let Some(namespace) = &decl.service_namespace {
        resource_attributes
            .entry("service.namespace".to_string())
            .or_insert_with(|| namespace.clone());
    }
    let service_name = match component {
        Some(component) => {
            iter_compose::telemetry::component_service_name(decl, project, component)
        }
        None => decl
            .service_name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| project.to_string()),
    };
    match decl.protocol {
        TelemetryProtocol::HttpProtobuf => {}
    }
    Some(iter_tracing::OtelRuntimeConfig {
        service_name,
        endpoint: decl
            .endpoint
            .clone()
            .or_else(|| std::env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").ok())
            .or_else(|| std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok())
            .unwrap_or_else(|| iter_tracing::DEFAULT_OTLP_HTTP_ENDPOINT.to_string()),
        logs_endpoint: decl
            .endpoint
            .clone()
            .or_else(|| std::env::var("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT").ok())
            .or_else(|| std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok())
            .unwrap_or_else(|| iter_tracing::DEFAULT_OTLP_HTTP_ENDPOINT.to_string()),
        resource_attributes,
        protocol: iter_tracing::OtelProtocol::HttpProtobuf,
    })
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

/// `MakeWriter` implementation that pushes each formatted tracing record
/// into the per-process `log.ndjson` via the global
/// [`LogSender`](iter_core::process::LogSender).
///
/// Returns a writer that no-ops until
/// [`install_global_log_sender`](iter_core::process::install_global_log_sender)
/// publishes a sender — typically when a
/// [`ProcessRuntime`](iter_core::process::ProcessRuntime) is constructed.
struct LogJsonMakeWriter;

impl<'a> MakeWriter<'a> for LogJsonMakeWriter {
    type Writer = LogJsonRecordWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogJsonRecordWriter::default()
    }
}

/// Per-record buffer. `tracing-subscriber` may make multiple `write` calls
/// for one event (target, fields, message, newline); we accumulate until
/// the trailing newline is observed and then flush a single line into the
/// NDJSON pipeline.
#[derive(Default)]
struct LogJsonRecordWriter {
    buf: Vec<u8>,
}

impl Write for LogJsonRecordWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for LogJsonRecordWriter {
    fn drop(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let Some(sender) = global_log_sender() else {
            return;
        };
        let mut text = String::from_utf8_lossy(&self.buf).into_owned();
        // Strip the formatter's trailing newline so the line stays a
        // single NDJSON record. Stripping CR before LF handles formatters
        // that emit "\r\n" on Windows-style outputs even though our
        // platform is Unix; cheap and robust.
        if text.ends_with('\n') {
            text.pop();
        }
        if text.ends_with('\r') {
            text.pop();
        }
        if !text.is_empty() {
            sender.try_send_line(LogStream::Stderr, text);
        }
    }
}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvSnapshot(Vec<(&'static str, Option<OsString>)>);

    impl EnvSnapshot {
        fn capture(keys: &[&'static str]) -> Self {
            Self(
                keys.iter()
                    .copied()
                    .map(|key| (key, std::env::var_os(key)))
                    .collect(),
            )
        }
    }

    impl Drop for EnvSnapshot {
        fn drop(&mut self) {
            for (key, value) in &self.0 {
                // SAFETY: tests that mutate process environment hold ENV_LOCK.
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }

    fn clear_otel_env() -> EnvSnapshot {
        let keys = [
            iter_tracing::ITER_OTEL_ENABLED,
            "OTEL_SERVICE_NAME",
            "OTEL_TRACES_EXPORTER",
            "OTEL_LOGS_EXPORTER",
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "OTEL_RESOURCE_ATTRIBUTES",
        ];
        let snapshot = EnvSnapshot::capture(&keys);
        for key in keys {
            // SAFETY: tests that mutate process environment hold ENV_LOCK.
            unsafe {
                std::env::remove_var(key);
            }
        }
        snapshot
    }

    fn telemetry_decl(enabled: bool) -> TelemetryDef {
        TelemetryDef {
            enabled,
            service_name: Some("configured".into()),
            service_namespace: None,
            endpoint: None,
            protocol: TelemetryProtocol::HttpProtobuf,
            resource_attributes: BTreeMap::new(),
        }
    }

    #[test]
    fn debug_flag_overrides_config() {
        let config = Config {
            log_level: Some(LogLevel::Error),
        };
        assert_eq!(resolve_level(true, &config), Level::DEBUG);
    }

    #[test]
    fn config_level_used_when_debug_off() {
        let config = Config {
            log_level: Some(LogLevel::Warn),
        };
        assert_eq!(resolve_level(false, &config), Level::WARN);
    }

    #[test]
    fn defaults_to_info_when_nothing_set() {
        let config = Config::default();
        assert_eq!(resolve_level(false, &config), Level::INFO);
    }

    #[test]
    fn compose_without_telemetry_block_can_fall_back_to_env() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _snapshot = clear_otel_env();
        // SAFETY: serialised via ENV_LOCK.
        unsafe {
            std::env::set_var(iter_tracing::ITER_OTEL_ENABLED, "true");
            std::env::set_var("OTEL_SERVICE_NAME", "ambient");
        }

        let config = compose_otel_config(None, "project", None).expect("env config");

        assert_eq!(config.service_name, "ambient");
    }

    #[test]
    fn disabled_compose_telemetry_does_not_fall_back_to_env() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _snapshot = clear_otel_env();
        // SAFETY: serialised via ENV_LOCK.
        unsafe {
            std::env::set_var(iter_tracing::ITER_OTEL_ENABLED, "true");
            std::env::set_var("OTEL_SERVICE_NAME", "ambient");
        }
        let decl = telemetry_decl(false);

        assert!(compose_otel_config(Some(&decl), "project", None).is_none());
    }
}
