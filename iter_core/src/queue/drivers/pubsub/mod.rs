//! GCP-backed queue implementations.
//!
//! * [`credentials`] — Application Default Credentials chain composition,
//!   service-account file/inline variants, workload identity, impersonation,
//!   raw access tokens.
//! * [`pubsub`] — Cloud Pub/Sub publisher + subscriber.
//!
//! The current build wires the DSL surface end-to-end ([`PubSubQueue::new`]
//! validates and constructs successfully), but the gRPC publish / pull
//! runtime returns [`pubsub::PubSubQueueError::NotYetImplemented`] until
//! the `google-cloud-pubsub` integration lands. See
//! [`credentials::PubSubCredentialsError::NotYetImplemented`] for the
//! same pattern at the credential layer.

pub mod credentials;
#[allow(clippy::module_inception)]
pub mod pubsub;

pub use credentials::{PubSubCredentials, PubSubCredentialsError};
pub use pubsub::{
    PubSubInitialSeek, PubSubKeepalive, PubSubPublisherConfig, PubSubQueue, PubSubQueueConfig,
    PubSubQueueError, PubSubSubscriberConfig,
};
