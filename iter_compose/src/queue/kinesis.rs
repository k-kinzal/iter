//! Translate Kinesis configuration from the language AST into `iter_core` types.

use iter_core::queue::kinesis::{
    KinesisCheckpointConfig as CoreKinesisCheckpoint, KinesisConsumerConfig as CoreKinesisConsumer,
    KinesisIdentity as CoreKinesisIdentity, KinesisProducerConfig as CoreKinesisProducer,
    KinesisQueueConfig, KinesisShardListFilter as CoreKinesisShardListFilter,
};
use iter_language::{
    KinesisCheckpoint, KinesisConfig, KinesisConsumer, KinesisIdentity, KinesisProducer,
    KinesisShardListFilter,
};

use super::sqs::{translate_aws_credentials, translate_aws_http_client};
use super::{
    QueueBuildError, ms_to_duration, opt_u32, secs_to_duration, translate_dlq, translate_retry,
    translate_template,
};

pub(super) fn build_kinesis_config(
    cfg: &KinesisConfig,
) -> Result<KinesisQueueConfig, QueueBuildError> {
    let identity = match &cfg.identity {
        KinesisIdentity::Unset => {
            return Err(QueueBuildError::invalid(
                "internal: Kinesis identity is unset; the lowerer should have rejected this",
            ));
        }
        KinesisIdentity::Arn(arn) => CoreKinesisIdentity::Arn(arn.clone()),
        KinesisIdentity::Name(name) => CoreKinesisIdentity::Name(name.clone()),
    };

    let credentials = cfg
        .credentials
        .as_ref()
        .map(translate_aws_credentials)
        .transpose()?;

    let http_client = cfg.http_client.as_ref().map(translate_aws_http_client);
    let producer = cfg.producer.as_ref().map(translate_kinesis_producer);
    let consumer = cfg.consumer.as_ref().map(translate_kinesis_consumer);
    let checkpoint = cfg.checkpoint.as_ref().map(translate_kinesis_checkpoint);
    let retry = cfg.retry.as_ref().map(translate_retry).transpose()?;
    let dlq = cfg.dlq.as_ref().map(translate_dlq).transpose()?;

    Ok(KinesisQueueConfig {
        identity,
        region: cfg.region.clone(),
        endpoint_url: cfg.endpoint_url.clone(),
        credentials,
        http_client,
        producer,
        consumer,
        checkpoint,
        retry,
        dlq,
    })
}

fn translate_kinesis_producer(p: &KinesisProducer) -> CoreKinesisProducer {
    CoreKinesisProducer {
        partition_key_strategy: p.partition_key_strategy.as_ref().map(translate_template),
        explicit_hash_key: p.explicit_hash_key.clone(),
        ordering: p.ordering.clone(),
        batch_size: opt_u32(p.batch_size),
        batch_max_bytes: opt_u32(p.batch_max_bytes),
        batch_linger: p.batch_linger_secs.map(secs_to_duration),
        aggregation: p.aggregation,
    }
}

fn translate_kinesis_consumer(c: &KinesisConsumer) -> CoreKinesisConsumer {
    CoreKinesisConsumer {
        consumer_mode: c.consumer_mode.clone(),
        iterator_type: c.iterator_type.clone(),
        starting_sequence_number: c.starting_sequence_number.clone(),
        starting_timestamp: c.starting_timestamp.clone(),
        fetch_max_records: opt_u32(c.fetch_max_records),
        poll_interval: c.poll_interval_ms.map(ms_to_duration),
        consumer_arn: c.consumer_arn.clone(),
        consumer_name: c.consumer_name.clone(),
        shard_discovery_interval: c.shard_discovery_interval_secs.map(secs_to_duration),
        shard_id_filter: c.shard_id_filter.clone(),
        shard_list_filter: c
            .shard_list_filter
            .as_ref()
            .map(translate_kinesis_shard_filter),
    }
}

fn translate_kinesis_shard_filter(f: &KinesisShardListFilter) -> CoreKinesisShardListFilter {
    CoreKinesisShardListFilter {
        kind: f.kind.clone(),
        shard_id: f.shard_id.clone(),
        timestamp: f.timestamp.clone(),
    }
}

fn translate_kinesis_checkpoint(c: &KinesisCheckpoint) -> CoreKinesisCheckpoint {
    CoreKinesisCheckpoint {
        store: c.store.clone(),
        table_name: c.table_name.clone(),
        region: c.region.clone(),
        endpoint_url: c.endpoint_url.clone(),
        path: c.path.clone(),
        interval: c.interval_secs.map(secs_to_duration),
        lease_duration: c.lease_duration_secs.map(secs_to_duration),
    }
}
