//! Translate Pub/Sub configuration from the language AST into `iter_core` types.

use iter_core::queue::gcp::{
    PubSubCredentials as CorePubSubCredentials, PubSubInitialSeek as CorePubSubInitialSeek,
    PubSubKeepalive as CorePubSubKeepalive, PubSubPublisherConfig as CorePubSubPublisher,
    PubSubQueueConfig, PubSubSubscriberConfig as CorePubSubSubscriber,
};
use iter_language::{
    PubSubConfig, PubSubCredentialKind, PubSubCredentials, PubSubInitialSeek, PubSubKeepalive,
    PubSubPublisher, PubSubSubscriber, TemplatedString,
};

use super::{
    QueueBuildError, ms_to_duration, opt_u32, opt_u64, secs_to_duration, translate_dlq,
    translate_retry,
};
use crate::secrets::resolve_secret;

pub(super) fn build_pubsub_config(
    cfg: &PubSubConfig,
) -> Result<PubSubQueueConfig, QueueBuildError> {
    let credentials = cfg
        .credentials
        .as_ref()
        .map(translate_pubsub_credentials)
        .transpose()?;

    let publisher = cfg
        .publisher
        .as_ref()
        .map(translate_pubsub_publisher)
        .transpose()?;

    let subscriber = cfg
        .subscriber
        .as_ref()
        .map(translate_pubsub_subscriber)
        .transpose()?;

    let initial_seek = cfg.initial_seek.as_ref().map(translate_pubsub_initial_seek);

    let dlq = cfg.dlq.as_ref().map(translate_dlq).transpose()?;

    Ok(PubSubQueueConfig {
        project: cfg.project.clone(),
        topic: cfg.topic.clone(),
        subscription: cfg.subscription.clone(),
        endpoint: cfg.endpoint.clone(),
        user_agent: cfg.user_agent.clone(),
        connect_timeout: cfg.connect_timeout_secs.map(secs_to_duration),
        request_timeout: cfg.request_timeout_secs.map(secs_to_duration),
        keepalive: cfg.keepalive.as_ref().map(translate_pubsub_keepalive),
        quota_project: cfg.quota_project.clone(),
        scopes: cfg.scopes.clone(),
        credentials,
        publisher,
        subscriber,
        initial_seek,
        dlq,
    })
}

fn translate_pubsub_credentials(
    creds: &PubSubCredentials,
) -> Result<CorePubSubCredentials, QueueBuildError> {
    Ok(match &creds.kind {
        PubSubCredentialKind::Adc => CorePubSubCredentials::Adc,
        PubSubCredentialKind::ServiceAccountFile { path } => {
            CorePubSubCredentials::ServiceAccountFile { path: path.clone() }
        }
        PubSubCredentialKind::ServiceAccountInline { json } => {
            CorePubSubCredentials::ServiceAccountInline {
                json: resolve_secret(json)
                    .map_err(|s| QueueBuildError::secret("credentials.json", s))?,
            }
        }
        PubSubCredentialKind::WorkloadIdentity {
            audience,
            token_file,
            impersonation_target,
        } => CorePubSubCredentials::WorkloadIdentity {
            audience: audience.clone(),
            token_file: token_file.clone(),
            impersonation_target: impersonation_target.clone(),
        },
        PubSubCredentialKind::Impersonate {
            target_principal,
            delegates,
            scopes,
        } => CorePubSubCredentials::Impersonate {
            target_principal: target_principal.clone(),
            delegates: delegates.clone(),
            scopes: scopes.clone(),
        },
        PubSubCredentialKind::AccessToken { token, expiry } => CorePubSubCredentials::AccessToken {
            token: resolve_secret(token)
                .map_err(|s| QueueBuildError::secret("credentials.token", s))?,
            expiry: expiry.clone(),
        },
    })
}

fn translate_pubsub_keepalive(k: &PubSubKeepalive) -> CorePubSubKeepalive {
    CorePubSubKeepalive {
        time: k.time_secs.map(secs_to_duration),
        timeout: k.timeout_secs.map(secs_to_duration),
        permit_without_stream: k.permit_without_stream,
    }
}

fn translate_pubsub_publisher(p: &PubSubPublisher) -> Result<CorePubSubPublisher, QueueBuildError> {
    let retry = p.retry.as_ref().map(translate_retry).transpose()?;
    let ordering_key_metadata = match p.ordering_key_strategy.as_ref() {
        Some(TemplatedString::Literal(_)) => {
            return Err(QueueBuildError::invalid(
                "publisher.ordering_key_strategy must use from_metadata(\"key\"); literal strings are not supported",
            ));
        }
        Some(TemplatedString::FromMetadata(k)) => Some(k.clone()),
        None => None,
    };
    Ok(CorePubSubPublisher {
        delay_threshold: p.delay_threshold_ms.map(ms_to_duration),
        count_threshold: opt_u32(p.count_threshold),
        byte_threshold: opt_u32(p.byte_threshold),
        max_outstanding_messages: opt_u32(p.max_outstanding_messages),
        max_outstanding_bytes: opt_u64(p.max_outstanding_bytes),
        limit_exceeded_behavior: p.limit_exceeded_behavior.clone(),
        workers: opt_u32(p.workers),
        request_timeout: p.request_timeout_secs.map(secs_to_duration),
        retry,
        enable_compression: p.enable_compression,
        compression_bytes_threshold: opt_u32(p.compression_bytes_threshold),
        attributes: p.attributes.clone(),
        ordering_key_metadata,
    })
}

fn translate_pubsub_subscriber(
    s: &PubSubSubscriber,
) -> Result<CorePubSubSubscriber, QueueBuildError> {
    let retry = s.retry.as_ref().map(translate_retry).transpose()?;
    Ok(CorePubSubSubscriber {
        pull_mode: s.pull_mode.clone(),
        stream_ack_deadline_seconds: opt_u32(s.stream_ack_deadline_seconds),
        max_outstanding_messages: opt_u32(s.max_outstanding_messages),
        max_outstanding_bytes: opt_u64(s.max_outstanding_bytes),
        min_duration_per_lease_extension: s
            .min_duration_per_lease_extension_secs
            .map(secs_to_duration),
        max_duration_per_lease_extension: s
            .max_duration_per_lease_extension_secs
            .map(secs_to_duration),
        ping_interval: s.ping_interval_secs.map(secs_to_duration),
        max_messages: opt_u32(s.max_messages),
        return_immediately: s.return_immediately,
        retry,
    })
}

fn translate_pubsub_initial_seek(seek: &PubSubInitialSeek) -> CorePubSubInitialSeek {
    if seek.kind == "snapshot" {
        CorePubSubInitialSeek::Snapshot(seek.snapshot_name.clone().unwrap_or_default())
    } else {
        CorePubSubInitialSeek::Timestamp(seek.timestamp.clone().unwrap_or_default())
    }
}
