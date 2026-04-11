//! Shared HTTP-client configuration for AWS-backed queues.
//!
//! AWS SDK clients separate two layers of tuning:
//!
//! * **Operation lifecycle** — overall operation timeout and per-attempt
//!   timeout, plus connect/read timeouts. These bind to
//!   [`aws_smithy_types::timeout::TimeoutConfig`] and apply across
//!   retries. The SDK propagates connect/read into the underlying
//!   connector via `HttpConnectorSettings` automatically.
//! * **HTTP transport** — connection-pool tuning. We expose
//!   `connection_pool_idle_timeout` via [`aws_smithy_http_client::Builder`].
//!
//! [`build_http_client`] takes the resolved Iterfile `http_client { ... }`
//! block and returns both artifacts so the caller can install them on
//! the `SdkConfig` builder for the specific service.
//!
//! Some Iterfile fields (`tcp_keepalive`, `max_idle_connections_per_host`,
//! `proxy_url`, `no_proxy`) are accepted but not currently propagated:
//! the smithy HTTP `Builder` does not expose them at the top level in
//! v1.1, and users wanting proxy support in the meantime can fall back
//! to the standard `HTTPS_PROXY` / `NO_PROXY` env vars which the
//! underlying hyper-util connector honours. Setting the field emits a
//! `tracing::warn` so the limitation is visible.

use std::time::Duration;

use aws_smithy_http_client::{
    Builder as HttpBuilder,
    tls::{self, rustls_provider::CryptoMode},
};
use aws_smithy_runtime_api::client::http::SharedHttpClient;
use aws_smithy_types::timeout::TimeoutConfig;

/// Resolved (literal) form of the Iterfile `http_client { ... }` block.
/// All fields are optional; an entirely-default `AwsHttpClientConfig`
/// produces no HTTP-client override and no timeout config — the SDK uses
/// its own defaults.
#[derive(Debug, Clone, Default)]
pub struct AwsHttpClientConfig {
    /// TCP connect timeout.
    pub connect_timeout: Option<Duration>,
    /// Time to first response byte.
    pub read_timeout: Option<Duration>,
    /// End-to-end operation timeout (across retries).
    pub operation_timeout: Option<Duration>,
    /// Per-attempt timeout (each retry resets it).
    pub operation_attempt_timeout: Option<Duration>,
    /// TCP keepalive interval. Currently informational; the smithy HTTP
    /// builder has no public hook in v1.1.
    pub tcp_keepalive: Option<Duration>,
    /// Idle-pool ceiling per host. Currently informational; no public
    /// hook in v1.1.
    pub max_idle_connections_per_host: Option<u64>,
    /// How long an idle pooled connection survives.
    pub connection_pool_idle_timeout: Option<Duration>,
    /// Fully-qualified proxy URL applied to all requests. Currently
    /// informational; users should set `HTTPS_PROXY` / `HTTP_PROXY` in
    /// the environment instead.
    pub proxy_url: Option<String>,
    /// Hostnames / suffixes that bypass the proxy. Pairs with
    /// `proxy_url`; informational only — see `NO_PROXY` env var.
    pub no_proxy: Option<Vec<String>>,
}

impl AwsHttpClientConfig {
    /// True when no field is set; lets callers skip the build entirely.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.connect_timeout.is_none()
            && self.read_timeout.is_none()
            && self.operation_timeout.is_none()
            && self.operation_attempt_timeout.is_none()
            && self.tcp_keepalive.is_none()
            && self.max_idle_connections_per_host.is_none()
            && self.connection_pool_idle_timeout.is_none()
            && self.proxy_url.is_none()
            && self.no_proxy.is_none()
    }
}

/// Built HTTP-layer artifacts. Either field may be `None` when the
/// corresponding Iterfile knobs were unset, in which case the caller
/// leaves the SDK default in place.
#[derive(Debug, Clone, Default)]
pub struct AwsHttpClientArtifacts {
    /// Custom HTTP client; install via `SdkConfig::builder().http_client(...)`.
    pub http_client: Option<SharedHttpClient>,
    /// Operation-lifecycle timeout config; install via
    /// `SdkConfig::builder().timeout_config(...)`.
    pub timeout_config: Option<TimeoutConfig>,
}

/// Translate the Iterfile `http_client { ... }` block into SDK
/// artifacts.
#[must_use]
pub fn build_http_client(config: &AwsHttpClientConfig) -> AwsHttpClientArtifacts {
    let mut artifacts = AwsHttpClientArtifacts::default();

    let mut tb = TimeoutConfig::builder();
    let mut any_timeout = false;
    if let Some(d) = config.connect_timeout {
        tb = tb.connect_timeout(d);
        any_timeout = true;
    }
    if let Some(d) = config.read_timeout {
        tb = tb.read_timeout(d);
        any_timeout = true;
    }
    if let Some(d) = config.operation_timeout {
        tb = tb.operation_timeout(d);
        any_timeout = true;
    }
    if let Some(d) = config.operation_attempt_timeout {
        tb = tb.operation_attempt_timeout(d);
        any_timeout = true;
    }
    if any_timeout {
        artifacts.timeout_config = Some(tb.build());
    }

    if let Some(d) = config.connection_pool_idle_timeout {
        let client = HttpBuilder::new()
            .pool_idle_timeout(d)
            .tls_provider(tls::Provider::Rustls(CryptoMode::AwsLc))
            .build_https();
        artifacts.http_client = Some(client);
    }

    if config.tcp_keepalive.is_some() {
        tracing::warn!(
            "http_client.tcp_keepalive is set but the AWS SDK builder does not expose a hook for it; the value is ignored"
        );
    }
    if config.max_idle_connections_per_host.is_some() {
        tracing::warn!(
            "http_client.max_idle_connections_per_host is set but the AWS SDK builder does not expose a hook for it; the value is ignored"
        );
    }
    if config.proxy_url.is_some() {
        tracing::warn!(
            "http_client.proxy_url is set but is not yet wired into the SDK; set HTTPS_PROXY / HTTP_PROXY in the environment as a workaround"
        );
    }

    artifacts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_returns_empty_artifacts() {
        let cfg = AwsHttpClientConfig::default();
        assert!(cfg.is_empty());
        let out = build_http_client(&cfg);
        assert!(out.http_client.is_none());
        assert!(out.timeout_config.is_none());
    }

    #[test]
    fn timeouts_only_skips_http_client() {
        let cfg = AwsHttpClientConfig {
            operation_timeout: Some(Duration::from_secs(60)),
            operation_attempt_timeout: Some(Duration::from_secs(20)),
            ..Default::default()
        };
        let out = build_http_client(&cfg);
        assert!(out.http_client.is_none());
        let tc = out.timeout_config.expect("timeout_config");
        assert_eq!(tc.operation_timeout(), Some(Duration::from_secs(60)));
        assert_eq!(
            tc.operation_attempt_timeout(),
            Some(Duration::from_secs(20))
        );
    }

    #[test]
    fn pool_idle_timeout_builds_http_client() {
        let cfg = AwsHttpClientConfig {
            connection_pool_idle_timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        };
        let out = build_http_client(&cfg);
        assert!(out.http_client.is_some());
    }
}
