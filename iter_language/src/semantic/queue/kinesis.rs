//! `queue kinesis { ... }` lowerer (AWS Kinesis Data Streams).
//!
//! Reuses the AWS credential and HTTP-client lowerers from
//! [`super::sqs`]. Stream identity is ARN-preferred with a plain-name
//! fallback.

use std::collections::BTreeMap;

use super::super::Analyzer;
use crate::ast::{
    KinesisCheckpoint, KinesisConfig, KinesisConsumer, KinesisIdentity, KinesisProducer,
    KinesisShardListFilter, Span,
};
use crate::diagnostic::Diagnostic;
use crate::parser::RawField;

const KINESIS_FIELDS: &[&str] = &[
    "stream_arn",
    "stream_name",
    "region",
    "endpoint_url",
    "credentials",
    "http_client",
    "producer",
    "consumer",
    "checkpoint",
    "retry",
    "dlq",
];

const KINESIS_PRODUCER_FIELDS: &[&str] = &[
    "partition_key_strategy",
    "explicit_hash_key",
    "ordering",
    "batch_size",
    "batch_max_bytes",
    "batch_linger",
    "aggregation",
];

const KINESIS_CONSUMER_FIELDS: &[&str] = &[
    "consumer_mode",
    "iterator_type",
    "starting_sequence_number",
    "starting_timestamp",
    "fetch_max_records",
    "poll_interval",
    "consumer_arn",
    "consumer_name",
    "shard_discovery_interval",
    "shard_id_filter",
    "shard_list_filter",
];

const KINESIS_SHARD_FILTER_FIELDS: &[&str] = &["type", "shard_id", "timestamp"];

const KINESIS_CHECKPOINT_FIELDS: &[&str] = &[
    "store",
    "table_name",
    "region",
    "endpoint_url",
    "path",
    "interval",
    "lease_duration",
];

impl Analyzer {
    pub(super) fn lower_kinesis(
        &mut self,
        body: BTreeMap<String, RawField>,
        kind_span: &Span,
    ) -> KinesisConfig {
        let mut fields = body;
        let identity = self.lower_kinesis_identity(&mut fields, kind_span);
        let region = self.take_optional_string(&mut fields, "region");
        let endpoint_url = self.take_optional_string(&mut fields, "endpoint_url");
        let credentials = self.lower_sqs_credentials(&mut fields, kind_span);
        let http_client = self.lower_sqs_http_client(&mut fields);
        let producer = self.lower_kinesis_producer(&mut fields);
        let consumer = self.lower_kinesis_consumer(&mut fields, kind_span);
        let checkpoint = self.lower_kinesis_checkpoint(&mut fields, kind_span);
        let retry = self.lower_retry_policy(&mut fields, "retry");
        let dlq = self.lower_dlq_policy(&mut fields, "dlq", kind_span);

        self.reject_unknown_fields(&mut fields, KINESIS_FIELDS, "queue kinesis");

        KinesisConfig {
            identity,
            region,
            endpoint_url,
            credentials,
            http_client,
            producer,
            consumer,
            checkpoint,
            retry,
            dlq,
        }
    }

    fn lower_kinesis_identity(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
        kind_span: &Span,
    ) -> KinesisIdentity {
        let arn = self.take_optional_string(fields, "stream_arn");
        let name = self.take_optional_string(fields, "stream_name");
        match (arn, name) {
            (Some(arn), None) => KinesisIdentity::Arn(arn),
            (None, Some(name)) => KinesisIdentity::Name(name),
            (Some(_), Some(_)) => {
                self.errors.push(
                    Diagnostic::error(
                        kind_span.clone(),
                        "queue kinesis: `stream_arn` and `stream_name` are mutually exclusive",
                    )
                    .with_hint("supply only one (ARN preferred)"),
                );
                KinesisIdentity::Unset
            }
            (None, None) => {
                self.errors.push(
                    Diagnostic::error(
                        kind_span.clone(),
                        "queue kinesis requires `stream_arn` or `stream_name`",
                    )
                    .with_hint("add `stream_arn = \"arn:aws:kinesis:...\"`"),
                );
                KinesisIdentity::Unset
            }
        }
    }

    fn lower_kinesis_producer(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
    ) -> Option<KinesisProducer> {
        let mut block = self.take_optional_block(fields, "producer")?;
        let partition_key_strategy =
            self.take_optional_templated_string(&mut block, "partition_key_strategy");
        let explicit_hash_key = self.take_optional_string(&mut block, "explicit_hash_key");
        let ordering = self.take_optional_string(&mut block, "ordering");
        let batch_size = self.take_optional_int(&mut block, "batch_size");
        let batch_max_bytes = self.take_optional_int(&mut block, "batch_max_bytes");
        let batch_linger_secs = self.take_optional_duration(&mut block, "batch_linger");
        let aggregation = self.take_optional_bool(&mut block, "aggregation");
        self.reject_unknown_fields(&mut block, KINESIS_PRODUCER_FIELDS, "kinesis producer");
        Some(KinesisProducer {
            partition_key_strategy,
            explicit_hash_key,
            ordering,
            batch_size,
            batch_max_bytes,
            batch_linger_secs,
            aggregation,
        })
    }

    fn lower_kinesis_consumer(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
        kind_span: &Span,
    ) -> Option<KinesisConsumer> {
        let mut block = self.take_optional_block(fields, "consumer")?;
        let consumer_mode = self.take_optional_string(&mut block, "consumer_mode");
        let iterator_type = self.take_optional_string(&mut block, "iterator_type");
        let starting_sequence_number =
            self.take_optional_string(&mut block, "starting_sequence_number");
        let starting_timestamp = self.take_optional_string(&mut block, "starting_timestamp");
        let fetch_max_records = self.take_optional_int(&mut block, "fetch_max_records");
        let poll_interval_ms = self
            .take_optional_duration(&mut block, "poll_interval")
            .map(|s| s.saturating_mul(1000));
        let consumer_arn = self.take_optional_string(&mut block, "consumer_arn");
        let consumer_name = self.take_optional_string(&mut block, "consumer_name");
        let shard_discovery_interval_secs =
            self.take_optional_duration(&mut block, "shard_discovery_interval");
        let shard_id_filter = self.take_optional_string_list(&mut block, "shard_id_filter");
        let shard_list_filter = self.lower_kinesis_shard_filter(&mut block);
        self.reject_unknown_fields(&mut block, KINESIS_CONSUMER_FIELDS, "kinesis consumer");

        if let (Some(mode), Some(_arn)) = (consumer_mode.as_deref(), consumer_arn.as_ref()) {
            if mode == "polling" {
                self.errors.push(Diagnostic::error(
                    kind_span.clone(),
                    "kinesis consumer: `consumer_arn` only valid with `consumer_mode = \"enhanced_fan_out\"`",
                ));
            }
        }

        Some(KinesisConsumer {
            consumer_mode,
            iterator_type,
            starting_sequence_number,
            starting_timestamp,
            fetch_max_records,
            poll_interval_ms,
            consumer_arn,
            consumer_name,
            shard_discovery_interval_secs,
            shard_id_filter,
            shard_list_filter,
        })
    }

    fn lower_kinesis_shard_filter(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
    ) -> Option<KinesisShardListFilter> {
        let mut block = self.take_optional_block(fields, "shard_list_filter")?;
        let kind = self.take_optional_string(&mut block, "type");
        let shard_id = self.take_optional_string(&mut block, "shard_id");
        let timestamp = self.take_optional_string(&mut block, "timestamp");
        self.reject_unknown_fields(
            &mut block,
            KINESIS_SHARD_FILTER_FIELDS,
            "kinesis shard_list_filter",
        );
        Some(KinesisShardListFilter {
            kind,
            shard_id,
            timestamp,
        })
    }

    fn lower_kinesis_checkpoint(
        &mut self,
        fields: &mut BTreeMap<String, RawField>,
        kind_span: &Span,
    ) -> Option<KinesisCheckpoint> {
        let mut block = self.take_optional_block(fields, "checkpoint")?;
        let store = self.take_optional_string(&mut block, "store");
        let table_name = self.take_optional_string(&mut block, "table_name");
        let region = self.take_optional_string(&mut block, "region");
        let endpoint_url = self.take_optional_string(&mut block, "endpoint_url");
        let path = self.take_optional_string(&mut block, "path");
        let interval_secs = self.take_optional_duration(&mut block, "interval");
        let lease_duration_secs = self.take_optional_duration(&mut block, "lease_duration");
        self.reject_unknown_fields(&mut block, KINESIS_CHECKPOINT_FIELDS, "kinesis checkpoint");

        match store.as_deref() {
            Some("dynamodb") if table_name.is_none() => {
                self.errors.push(Diagnostic::error(
                    kind_span.clone(),
                    "kinesis checkpoint store `dynamodb` requires `table_name`",
                ));
            }
            Some("file") if path.is_none() => {
                self.errors.push(Diagnostic::error(
                    kind_span.clone(),
                    "kinesis checkpoint store `file` requires `path`",
                ));
            }
            _ => {}
        }

        Some(KinesisCheckpoint {
            store,
            table_name,
            region,
            endpoint_url,
            path,
            interval_secs,
            lease_duration_secs,
        })
    }
}
