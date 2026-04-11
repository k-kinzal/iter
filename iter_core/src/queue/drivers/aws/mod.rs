//! Shared AWS utilities (credentials, HTTP client) used by the SQS and
//! Kinesis drivers.

pub mod credentials;
pub mod http;

pub use credentials::{AwsCredentials, CredentialsBuildError, build_credentials};
pub use http::{AwsHttpClientConfig, build_http_client};
