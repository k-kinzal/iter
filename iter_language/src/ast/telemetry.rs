//! Compose-level telemetry configuration.
//!
//! Telemetry is intentionally a `compose.iter` concern: it describes how a
//! project-shaped topology exports observations across services and triggers,
//! not how a single runner performs its work.

use std::collections::BTreeMap;

/// OpenTelemetry configuration declared at `compose.iter` top level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryDecl {
    /// Whether telemetry export is enabled. Defaults to `true` when the block
    /// is present.
    pub enabled: bool,
    /// Stable base service name. Runtime layers may append the concrete
    /// component name (for example `.worker`) when a service runs in its own
    /// process.
    pub service_name: Option<String>,
    /// Optional OpenTelemetry service namespace resource attribute.
    pub service_namespace: Option<String>,
    /// OTLP HTTP endpoint, usually the collector's `:4318` base URL.
    pub endpoint: Option<String>,
    /// Export protocol. Currently only `http/protobuf` is supported.
    pub protocol: TelemetryProtocol,
    /// Additional resource attributes attached to every emitted signal.
    pub resource_attributes: BTreeMap<String, String>,
}

/// Supported OTLP export protocols.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryProtocol {
    /// OTLP over HTTP/protobuf.
    HttpProtobuf,
}
