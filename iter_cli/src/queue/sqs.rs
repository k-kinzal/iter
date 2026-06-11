//! Translate SQS configuration from the language AST into `iter_core` types.

use iter_core::queue::aws::{AwsCredentials, AwsHttpClientConfig};
use iter_core::queue::sqs::{
    SqsConsumerConfig as CoreSqsConsumerConfig, SqsIdentity as CoreSqsIdentity,
    SqsProducerConfig as CoreSqsProducerConfig, SqsQueueConfig,
};
use iter_language::{
    SqsConfig, SqsConsumer, SqsCredentialKind, SqsCredentials, SqsHttpClient, SqsIdentity,
    SqsProducer,
};

use super::{
    QueueBuildError, opt_u32, secs_to_duration, translate_dlq, translate_retry, translate_template,
};
use crate::secrets::resolve_secret;

pub(super) fn build_sqs_config(cfg: &SqsConfig) -> Result<SqsQueueConfig, QueueBuildError> {
    let identity = match &cfg.identity {
        SqsIdentity::Unset => {
            return Err(QueueBuildError::invalid(
                "internal: SQS identity is unset; the lowerer should have rejected this",
            ));
        }
        SqsIdentity::Url(url) => CoreSqsIdentity::Url(url.clone()),
        SqsIdentity::NameWithAccount { name, account_id } => CoreSqsIdentity::NameWithAccount {
            name: name.clone(),
            account_id: account_id.clone(),
        },
    };

    let credentials = cfg
        .credentials
        .as_ref()
        .map(translate_aws_credentials)
        .transpose()?;

    let http_client = cfg.http_client.as_ref().map(translate_aws_http_client);
    let producer = cfg.producer.as_ref().map(translate_sqs_producer);
    let consumer = cfg.consumer.as_ref().map(translate_sqs_consumer);
    let retry = cfg.retry.as_ref().map(translate_retry).transpose()?;
    let dlq = cfg.dlq.as_ref().map(translate_dlq).transpose()?;

    Ok(SqsQueueConfig {
        identity,
        region: cfg.region.clone(),
        endpoint_url: cfg.endpoint_url.clone(),
        fifo: cfg.fifo,
        use_fips: cfg.use_fips,
        use_dual_stack: cfg.use_dual_stack,
        sts_regional_endpoints: cfg.sts_regional_endpoints.clone(),
        app_name: cfg.app_name.clone(),
        credentials,
        http_client,
        producer,
        consumer,
        retry,
        dlq,
    })
}

pub(super) fn translate_aws_credentials(
    creds: &SqsCredentials,
) -> Result<AwsCredentials, QueueBuildError> {
    Ok(match &creds.kind {
        SqsCredentialKind::Default => AwsCredentials::Default,
        SqsCredentialKind::Static {
            access_key_id,
            secret_access_key,
            session_token,
        } => AwsCredentials::Static {
            access_key_id: resolve_secret(access_key_id)
                .map_err(|s| QueueBuildError::secret("credentials.access_key_id", s))?,
            secret_access_key: resolve_secret(secret_access_key)
                .map_err(|s| QueueBuildError::secret("credentials.secret_access_key", s))?,
            session_token: session_token
                .as_ref()
                .map(resolve_secret)
                .transpose()
                .map_err(|s| QueueBuildError::secret("credentials.session_token", s))?,
        },
        SqsCredentialKind::AssumeRole {
            role_arn,
            session_name,
            external_id,
            duration_seconds,
            source_profile,
        } => AwsCredentials::AssumeRole {
            role_arn: role_arn.clone(),
            session_name: session_name.clone(),
            external_id: external_id
                .as_ref()
                .map(resolve_secret)
                .transpose()
                .map_err(|s| QueueBuildError::secret("credentials.external_id", s))?,
            duration_seconds: duration_seconds.map(|s| u32::try_from(s.max(0)).unwrap_or(u32::MAX)),
            source_profile: source_profile.clone(),
        },
        SqsCredentialKind::Profile { name } => AwsCredentials::Profile { name: name.clone() },
        SqsCredentialKind::WebIdentityTokenFile {
            role_arn,
            token_file,
            session_name,
        } => AwsCredentials::WebIdentityTokenFile {
            role_arn: role_arn.clone(),
            token_file: token_file.clone(),
            session_name: session_name.clone(),
        },
        SqsCredentialKind::Imds => AwsCredentials::Imds,
        SqsCredentialKind::Process { command } => AwsCredentials::Process {
            command: command.clone(),
        },
    })
}

pub(super) fn translate_aws_http_client(http: &SqsHttpClient) -> AwsHttpClientConfig {
    AwsHttpClientConfig {
        connect_timeout: http.connect_timeout_secs.map(secs_to_duration),
        read_timeout: http.read_timeout_secs.map(secs_to_duration),
        operation_timeout: http.operation_timeout_secs.map(secs_to_duration),
        operation_attempt_timeout: http.operation_attempt_timeout_secs.map(secs_to_duration),
        tcp_keepalive: http.tcp_keepalive_secs.map(secs_to_duration),
        max_idle_connections_per_host: http
            .max_idle_connections_per_host
            .map(|n| u64::try_from(n.max(0)).unwrap_or(0)),
        connection_pool_idle_timeout: http.connection_pool_idle_timeout_secs.map(secs_to_duration),
        proxy_url: http.proxy_url.clone(),
        no_proxy: http.no_proxy.clone(),
    }
}

fn translate_sqs_producer(producer: &SqsProducer) -> CoreSqsProducerConfig {
    CoreSqsProducerConfig {
        delay_seconds: producer
            .delay_seconds
            .map(|s| u32::try_from(s.max(0)).unwrap_or(0)),
        message_attributes: producer.message_attributes.clone(),
        trace_header: producer.trace_header,
        message_group_id: producer.message_group_id.as_ref().map(translate_template),
        message_deduplication_id: producer
            .message_deduplication_id
            .as_ref()
            .map(translate_template),
        batch_size: opt_u32(producer.batch_size),
        batch_max_bytes: opt_u32(producer.batch_max_bytes),
        batch_linger: producer.batch_linger_secs.map(secs_to_duration),
    }
}

fn translate_sqs_consumer(consumer: &SqsConsumer) -> CoreSqsConsumerConfig {
    CoreSqsConsumerConfig {
        visibility_timeout: consumer.visibility_timeout_secs.map(secs_to_duration),
        wait_time_seconds: opt_u32(consumer.wait_time_seconds),
        max_number_of_messages: opt_u32(consumer.max_number_of_messages),
        message_attribute_names: consumer.message_attribute_names.clone(),
        message_system_attribute_names: consumer.message_system_attribute_names.clone(),
        concurrent_receivers: opt_u32(consumer.concurrent_receivers),
    }
}
