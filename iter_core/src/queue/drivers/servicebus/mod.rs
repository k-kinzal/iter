//! Azure-backed queue implementations.
//!
//! * [`credentials`] — `DefaultAzureCredential`-style composition, plus
//!   connection-string and SAS-token paths.
//! * [`servicebus`] — Azure Service Bus queues, topics+subscriptions, and
//!   sub-queue (DLQ / transfer-DLQ) consumers.
//!
//! The current build wires the DSL surface end-to-end ([`ServiceBusQueue::new`]
//! validates and constructs successfully), but the AMQP runtime returns
//! [`servicebus::ServiceBusQueueError::NotYetImplemented`] until the
//! `azservicebus` integration lands. See
//! [`credentials::ServiceBusCredentialsError::NotYetImplemented`] for the
//! same pattern at the credential layer.

pub mod credentials;
#[allow(clippy::module_inception)]
pub mod servicebus;

pub use credentials::{ServiceBusCredentials, ServiceBusCredentialsError};
pub use servicebus::{
    ServiceBusEntity, ServiceBusProxyConfig, ServiceBusQueue, ServiceBusQueueConfig,
    ServiceBusQueueError, ServiceBusReceiverConfig, ServiceBusSenderConfig,
    ServiceBusSessionConfig,
};
