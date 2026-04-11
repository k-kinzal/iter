//! GCP credential composition for Pub/Sub.
//!
//! Layered over the Application Default Credentials chain. The variants
//! mirror the [`PubSubCredentialKind`](iter_language::PubSubCredentialKind)
//! AST surface verbatim so the lowered config can be passed through
//! unchanged.
//!
//! # Stub implementation
//!
//! The Phase-2 milestone of the queue-backend expansion ships the AST,
//! semantic lowerer, and `AnyQueue` dispatch end-to-end so projects can write
//! `queue pubsub { ... }` and have iter parse and compose the config. The
//! actual `google-cloud-auth` wiring is being filled in iteratively; the
//! current build returns
//! [`PubSubCredentialsError::NotYetImplemented`] for every non-`adc`
//! variant. The `Adc` variant resolves to "do nothing — let the client
//! library use ADC", which is the production deploy path on GCE / GKE /
//! Cloud Run / Cloud Functions and works out-of-the-box there.

use thiserror::Error;

/// Resolved GCP credential surface.
///
/// Construction does no I/O — it is the validated, owned shape of what
/// the user wrote in the Iterfile. Resolution to a token source happens
/// when the Pub/Sub client connects.
#[derive(Debug, Clone)]
pub enum PubSubCredentials {
    /// Application Default Credentials chain (env → gcloud → metadata).
    Adc,
    /// Service-account JSON loaded from a path on disk.
    ServiceAccountFile {
        /// Filesystem path to the JSON key file.
        path: String,
    },
    /// Service-account JSON supplied inline.
    ServiceAccountInline {
        /// JSON document — typically resolved from `env(...)` by the
        /// compose layer before reaching `iter_core`.
        json: String,
    },
    /// External-account / Workload Identity Federation flow.
    WorkloadIdentity {
        /// Audience the `IdP` token is minted for.
        audience: String,
        /// Filesystem path to the `IdP` token file.
        token_file: String,
        /// Optional service-account to impersonate after federation.
        impersonation_target: Option<String>,
    },
    /// Service-account impersonation chain.
    Impersonate {
        /// Final principal to impersonate.
        target_principal: String,
        /// Optional intermediate principals.
        delegates: Option<Vec<String>>,
        /// OAuth scopes.
        scopes: Option<Vec<String>>,
    },
    /// Pre-minted access token.
    AccessToken {
        /// Bearer token.
        token: String,
        /// RFC3339 expiry timestamp (optional).
        expiry: Option<String>,
    },
}

/// Errors building a Pub/Sub credential provider.
#[derive(Debug, Error)]
pub enum PubSubCredentialsError {
    /// The requested variant is not yet wired in. Documented contractually
    /// so projects can target the variant in Iterfile today and rely on a
    /// follow-up release filling it in without DSL churn.
    #[error("Pub/Sub credentials variant `{variant}` is not yet implemented")]
    NotYetImplemented {
        /// Name of the AST variant the user requested.
        variant: &'static str,
    },
}

impl PubSubCredentials {
    /// Resolve the credential into a token source the Pub/Sub client can
    /// use. The current implementation only honours
    /// [`PubSubCredentials::Adc`] — every other variant returns
    /// [`PubSubCredentialsError::NotYetImplemented`] so callers fail
    /// fast with a clear, actionable diagnostic rather than silently
    /// falling back to ADC.
    ///
    /// # Errors
    ///
    /// Returns [`PubSubCredentialsError::NotYetImplemented`] for every
    /// non-`Adc` variant in the current build.
    pub fn validate(&self) -> Result<(), PubSubCredentialsError> {
        match self {
            Self::Adc => Ok(()),
            Self::ServiceAccountFile { .. } => Err(PubSubCredentialsError::NotYetImplemented {
                variant: "service_account_file",
            }),
            Self::ServiceAccountInline { .. } => Err(PubSubCredentialsError::NotYetImplemented {
                variant: "service_account_inline",
            }),
            Self::WorkloadIdentity { .. } => Err(PubSubCredentialsError::NotYetImplemented {
                variant: "workload_identity",
            }),
            Self::Impersonate { .. } => Err(PubSubCredentialsError::NotYetImplemented {
                variant: "impersonate",
            }),
            Self::AccessToken { .. } => Err(PubSubCredentialsError::NotYetImplemented {
                variant: "access_token",
            }),
        }
    }
}
