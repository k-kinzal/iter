//! Azure credential composition for Service Bus.
//!
//! Layered over `azure_identity::DefaultAzureCredential`. The variants
//! mirror the [`ServiceBusAuthKind`](iter_language::ServiceBusAuthKind)
//! AST surface verbatim so the lowered config can be passed through
//! unchanged.
//!
//! # Stub implementation
//!
//! Phase 3 of the queue-backend expansion ships the AST, semantic
//! lowerer, and `AnyQueue` dispatch end-to-end so projects can write
//! `queue servicebus { ... }` and have iter parse and compose the
//! config. The actual `azure_identity` / `azservicebus` wiring is being
//! filled in iteratively; the current build returns
//! [`ServiceBusCredentialsError::NotYetImplemented`] for every variant
//! except [`ServiceBusCredentials::AadDefault`] (which resolves to "do
//! nothing — let the SDK use `DefaultAzureCredential`", matching the
//! production deploy path on AKS / App Service / Functions).

use thiserror::Error;

/// Resolved Azure credential surface.
///
/// Construction does no I/O — it is the validated, owned shape of what
/// the user wrote in the Iterfile. Resolution to a token source happens
/// when the Service Bus client connects.
#[derive(Debug, Clone)]
pub enum ServiceBusCredentials {
    /// Native chain (Managed Identity → Workload Identity → Az CLI).
    AadDefault,
    /// SAS connection string.
    ConnectionString {
        /// Full Service Bus connection string.
        value: String,
    },
    /// Pre-signed SAS token.
    SharedAccessSignature {
        /// SAS token string.
        sas_token: String,
    },
    /// AAD client-secret credential.
    AadClientSecret {
        /// Tenant id (UUID).
        tenant_id: String,
        /// Client (application) id (UUID).
        client_id: String,
        /// Client secret.
        client_secret: String,
    },
    /// AAD client-certificate credential.
    AadClientCertificate {
        /// Tenant id (UUID).
        tenant_id: String,
        /// Client (application) id (UUID).
        client_id: String,
        /// Path to a PEM/PFX certificate.
        cert_path: String,
        /// Optional certificate password.
        cert_password: Option<String>,
    },
    /// Managed Identity (system-assigned when `client_id` omitted).
    AadManagedIdentity {
        /// Optional user-assigned identity id.
        client_id: Option<String>,
    },
    /// Workload Identity (AKS).
    AadWorkloadIdentity {
        /// Tenant id (UUID).
        tenant_id: String,
        /// Client id (UUID).
        client_id: String,
        /// Federated token file path.
        token_file: String,
    },
}

/// Errors building a Service Bus credential provider.
#[derive(Debug, Error)]
pub enum ServiceBusCredentialsError {
    /// The requested variant is not yet wired in. Documented contractually
    /// so projects can target the variant in Iterfile today and rely on a
    /// follow-up release filling it in without DSL churn.
    #[error("Service Bus credentials variant `{variant}` is not yet implemented")]
    NotYetImplemented {
        /// Name of the AST variant the user requested.
        variant: &'static str,
    },
}

impl ServiceBusCredentials {
    /// Resolve the credential into a token source the Service Bus client
    /// can use. The current implementation only honours
    /// [`ServiceBusCredentials::AadDefault`] — every other variant returns
    /// [`ServiceBusCredentialsError::NotYetImplemented`] so callers fail
    /// fast with a clear, actionable diagnostic rather than silently
    /// falling back to `DefaultAzureCredential`.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceBusCredentialsError::NotYetImplemented`] for
    /// every non-`AadDefault` variant in the current build.
    pub fn validate(&self) -> Result<(), ServiceBusCredentialsError> {
        match self {
            Self::AadDefault => Ok(()),
            Self::ConnectionString { .. } => Err(ServiceBusCredentialsError::NotYetImplemented {
                variant: "connection_string",
            }),
            Self::SharedAccessSignature { .. } => {
                Err(ServiceBusCredentialsError::NotYetImplemented {
                    variant: "shared_access_signature",
                })
            }
            Self::AadClientSecret { .. } => Err(ServiceBusCredentialsError::NotYetImplemented {
                variant: "aad_client_secret",
            }),
            Self::AadClientCertificate { .. } => {
                Err(ServiceBusCredentialsError::NotYetImplemented {
                    variant: "aad_client_certificate",
                })
            }
            Self::AadManagedIdentity { .. } => Err(ServiceBusCredentialsError::NotYetImplemented {
                variant: "aad_managed_identity",
            }),
            Self::AadWorkloadIdentity { .. } => {
                Err(ServiceBusCredentialsError::NotYetImplemented {
                    variant: "aad_workload_identity",
                })
            }
        }
    }
}
