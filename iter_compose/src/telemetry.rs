//! Helpers for projecting compose telemetry declarations into process env.

use iter_language::TelemetryDecl;

/// Build the environment variables a compose-managed service subprocess needs
/// to initialise OpenTelemetry before it reparses the compose file.
#[must_use]
pub fn service_env(
    telemetry: Option<&TelemetryDecl>,
    project: &str,
    service_name: &str,
) -> Vec<(String, String)> {
    let Some(decl) = telemetry.filter(|decl| decl.enabled) else {
        return Vec::new();
    };
    let mut env = vec![
        (
            iter_tracing::ITER_OTEL_ENABLED.to_string(),
            "true".to_string(),
        ),
        (
            "OTEL_SERVICE_NAME".to_string(),
            component_service_name(decl, project, service_name),
        ),
        ("OTEL_TRACES_EXPORTER".to_string(), "otlp".to_string()),
        ("OTEL_LOGS_EXPORTER".to_string(), "otlp".to_string()),
        (
            "OTEL_EXPORTER_OTLP_PROTOCOL".to_string(),
            "http/protobuf".to_string(),
        ),
        (
            "OTEL_PROPAGATORS".to_string(),
            "tracecontext,baggage".to_string(),
        ),
    ];
    if let Some(endpoint) = &decl.endpoint {
        env.push(("OTEL_EXPORTER_OTLP_ENDPOINT".to_string(), endpoint.clone()));
    }
    let attrs = resource_attributes(decl, project, Some(service_name));
    if !attrs.is_empty() {
        env.push(("OTEL_RESOURCE_ATTRIBUTES".to_string(), attrs));
    }
    env
}

/// Build the resource attribute string for environment-driven SDK setup.
#[must_use]
pub fn resource_attributes(decl: &TelemetryDecl, project: &str, component: Option<&str>) -> String {
    let mut attrs = decl.resource_attributes.clone();
    attrs
        .entry("iter.compose.project".to_string())
        .or_insert_with(|| project.to_string());
    if let Some(component) = component {
        attrs
            .entry("iter.compose.service".to_string())
            .or_insert_with(|| component.to_string());
    }
    if let Some(namespace) = &decl.service_namespace {
        attrs
            .entry("service.namespace".to_string())
            .or_insert_with(|| namespace.clone());
    }
    iter_tracing::format_resource_attributes(
        attrs
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str())),
    )
}

/// Derive a concrete `OTel` service name from a project-level declaration.
#[must_use]
pub fn component_service_name(decl: &TelemetryDecl, project: &str, component: &str) -> String {
    let base = decl
        .service_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .unwrap_or(project);
    format!("{base}.{component}")
}
