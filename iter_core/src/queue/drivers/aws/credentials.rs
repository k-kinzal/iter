//! Composed credential provider for AWS-backed queues.
//!
//! The [`build_credentials`] function takes the resolved Iterfile
//! credential block and returns an [`SharedCredentialsProvider`]
//! suitable for plugging into any [`aws_config::SdkConfig`]. Every
//! variant either layers explicit material over the SDK's default
//! chain or replaces the chain outright (`Imds`, `Process`).
//!
//! The provider is per-process; the chain itself caches credentials
//! according to the SDK's default expiry behaviour, so repeated calls
//! to [`build_credentials`] should be amortised by holding onto the
//! returned provider.

use std::path::PathBuf;
use std::time::Duration;

use aws_config::{
    BehaviorVersion,
    default_provider::credentials::DefaultCredentialsChain,
    imds::credentials::ImdsCredentialsProvider,
    profile::ProfileFileCredentialsProvider,
    sts::AssumeRoleProvider,
    web_identity_token::{StaticConfiguration, WebIdentityTokenCredentialsProvider},
};
use aws_credential_types::{Credentials, provider::SharedCredentialsProvider};
use thiserror::Error;

/// Resolved (literal-string) form of the Iterfile `credentials { ... }`
/// block. The operator (CLI) is responsible for resolving every
/// `SecretExpr` into a plain `String` before constructing this enum;
/// `iter_core` stays free of the `SecretExpr` type so it can be reused
/// outside the CLI.
#[derive(Debug, Clone)]
pub enum AwsCredentials {
    /// Use the SDK default chain unmodified.
    Default,
    /// Inline static keys (dev / `LocalStack`).
    Static {
        /// `AWS_ACCESS_KEY_ID` equivalent.
        access_key_id: String,
        /// `AWS_SECRET_ACCESS_KEY` equivalent.
        secret_access_key: String,
        /// Optional STS session token.
        session_token: Option<String>,
    },
    /// `AssumeRole` over a source provider.
    AssumeRole {
        /// Role ARN to assume. Required.
        role_arn: String,
        /// Optional session name; the SDK generates one when absent.
        session_name: Option<String>,
        /// External-id challenge for cross-account roles.
        external_id: Option<String>,
        /// Session duration in seconds.
        duration_seconds: Option<u32>,
        /// Source profile name; when set, the `AssumeRole` call uses
        /// that profile's credentials, otherwise it uses the SDK
        /// default chain.
        source_profile: Option<String>,
    },
    /// Named profile in `~/.aws/credentials` / `~/.aws/config`.
    Profile {
        /// Profile name.
        name: String,
    },
    /// Web identity token file (EKS IRSA / GitHub OIDC).
    WebIdentityTokenFile {
        /// Role ARN to assume.
        role_arn: String,
        /// JWT token file path.
        token_file: String,
        /// Optional session name.
        session_name: Option<String>,
    },
    /// EC2 / ECS instance metadata service.
    Imds,
    /// `credential_process`-style external command. The SDK does not
    /// ship a generic process provider; we surface a clear error
    /// instead of silently shelling out for the user (the future
    /// implementation is tracked in the queue plan).
    Process {
        /// Command line invoked to mint credentials.
        command: String,
    },
}

/// Errors building the credential provider.
#[derive(Debug, Error)]
pub enum CredentialsBuildError {
    /// Per-attempt timeout could not be parsed, or a duration overflowed.
    #[error("invalid duration: {0}")]
    InvalidDuration(String),

    /// `credentials.kind = "process"` is not yet supported by iter; the
    /// SDK has no first-party generic provider for it.
    #[error(
        "credentials.kind = \"process\" is not yet supported. Use a wrapping shell that exports AWS_* env vars and `kind = \"default\"` instead. (command: `{command}`)"
    )]
    ProcessUnsupported {
        /// The command the user supplied — echoed back so they know
        /// which Iterfile entry to fix.
        command: String,
    },

    /// A non-SDK build step failed (e.g. building the web-identity
    /// provider when its required fields are missing).
    #[error("sdk error: {0}")]
    Sdk(String),
}

/// Build a [`SharedCredentialsProvider`] from the resolved Iterfile
/// credential block.
///
/// # Errors
///
/// Returns [`CredentialsBuildError::ProcessUnsupported`] when
/// [`AwsCredentials::Process`] is selected, and propagates any inner
/// SDK construction error.
pub async fn build_credentials(
    creds: &AwsCredentials,
) -> Result<SharedCredentialsProvider, CredentialsBuildError> {
    match creds {
        AwsCredentials::Default => {
            let chain = DefaultCredentialsChain::builder().build().await;
            Ok(SharedCredentialsProvider::new(chain))
        }
        AwsCredentials::Static {
            access_key_id,
            secret_access_key,
            session_token,
        } => {
            let creds = Credentials::from_keys(
                access_key_id.clone(),
                secret_access_key.clone(),
                session_token.clone(),
            );
            Ok(SharedCredentialsProvider::new(creds))
        }
        AwsCredentials::AssumeRole {
            role_arn,
            session_name,
            external_id,
            duration_seconds,
            source_profile,
        } => {
            // Source provider: a named profile when given, otherwise
            // the SDK default chain. AssumeRoleProvider then layers
            // STS on top.
            let source: SharedCredentialsProvider = if let Some(profile) = source_profile {
                let p = ProfileFileCredentialsProvider::builder()
                    .profile_name(profile)
                    .build();
                SharedCredentialsProvider::new(p)
            } else {
                let chain = DefaultCredentialsChain::builder().build().await;
                SharedCredentialsProvider::new(chain)
            };
            let sdk_config = aws_config::SdkConfig::builder()
                .behavior_version(BehaviorVersion::latest())
                .build();
            let mut builder = AssumeRoleProvider::builder(role_arn).configure(&sdk_config);
            if let Some(name) = session_name {
                builder = builder.session_name(name);
            }
            if let Some(eid) = external_id {
                builder = builder.external_id(eid);
            }
            if let Some(secs) = duration_seconds {
                builder = builder.session_length(Duration::from_secs(u64::from(*secs)));
            }
            let provider = builder.build_from_provider(source).await;
            Ok(SharedCredentialsProvider::new(provider))
        }
        AwsCredentials::Profile { name } => {
            let p = ProfileFileCredentialsProvider::builder()
                .profile_name(name)
                .build();
            Ok(SharedCredentialsProvider::new(p))
        }
        AwsCredentials::WebIdentityTokenFile {
            role_arn,
            token_file,
            session_name,
        } => {
            let session = session_name
                .clone()
                .unwrap_or_else(|| "iter-web-identity".to_string());
            let provider = WebIdentityTokenCredentialsProvider::builder()
                .static_configuration(StaticConfiguration {
                    web_identity_token_file: PathBuf::from(token_file),
                    role_arn: role_arn.clone(),
                    session_name: session,
                })
                .build();
            Ok(SharedCredentialsProvider::new(provider))
        }
        AwsCredentials::Imds => {
            let p = ImdsCredentialsProvider::builder().build();
            Ok(SharedCredentialsProvider::new(p))
        }
        AwsCredentials::Process { command } => Err(CredentialsBuildError::ProcessUnsupported {
            command: command.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_keys_round_trip() {
        let creds = AwsCredentials::Static {
            access_key_id: "AKIA".into(),
            secret_access_key: "secret".into(),
            session_token: Some("tok".into()),
        };
        let provider = build_credentials(&creds).await.expect("build");
        let resolved =
            aws_credential_types::provider::ProvideCredentials::provide_credentials(&provider)
                .await
                .expect("resolve");
        assert_eq!(resolved.access_key_id(), "AKIA");
        assert_eq!(resolved.secret_access_key(), "secret");
        assert_eq!(resolved.session_token(), Some("tok"));
    }

    #[tokio::test]
    async fn process_kind_returns_unsupported_error() {
        let creds = AwsCredentials::Process {
            command: "echo".into(),
        };
        let err = build_credentials(&creds).await.expect_err("unsupported");
        assert!(matches!(
            err,
            CredentialsBuildError::ProcessUnsupported { .. }
        ));
    }
}
