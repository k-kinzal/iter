//! `queue kafka { ... }` lowerer (Apache Kafka).
//!
//! Mirrors the [`super::sqs`] shape: bootstrap servers plus optional
//! security/producer/consumer sub-blocks, with mutual-exclusion checks
//! enforced at lowering time.

use std::collections::BTreeMap;

use super::super::Analyzer;
use crate::ast::{KafkaConfig, KafkaConsumer, KafkaProducer, KafkaSecurity, Span};
use crate::parser::CstField;

const KAFKA_FIELDS: &[&str] = &[
    "bootstrap_servers",
    "client_id",
    "client_rack",
    "broker_address_family",
    "broker_address_ttl",
    "metadata_max_age",
    "topic_metadata_refresh_interval",
    "topic_metadata_refresh_fast_interval_ms",
    "socket_timeout",
    "socket_keepalive_enable",
    "socket_nagle_disable",
    "socket_max_fails",
    "reconnect_backoff_ms",
    "reconnect_backoff_max_ms",
    "api_version_request",
    "api_version_request_timeout_ms",
    "security",
    "producer",
    "consumer",
    "exactly_once",
    "extra_config",
    "dlq",
];

const KAFKA_SECURITY_FIELDS: &[&str] = &[
    "security_protocol",
    "sasl_mechanism",
    "sasl_username",
    "sasl_password",
    "sasl_kerberos_service_name",
    "sasl_kerberos_principal",
    "sasl_kerberos_keytab",
    "sasl_kerberos_kinit_cmd",
    "sasl_kerberos_min_time_before_relogin",
    "sasl_oauthbearer_method",
    "sasl_oauthbearer_config",
    "sasl_oauthbearer_client_id",
    "sasl_oauthbearer_client_secret",
    "sasl_oauthbearer_token_endpoint_url",
    "sasl_oauthbearer_scope",
    "sasl_oauthbearer_extensions",
    "enable_sasl_oauthbearer_unsecure_jwt",
    "ssl_ca_location",
    "ssl_certificate_location",
    "ssl_key_location",
    "ssl_key_password",
    "ssl_ca_pem",
    "ssl_certificate_pem",
    "ssl_key_pem",
    "ssl_keystore_location",
    "ssl_keystore_password",
    "ssl_crl_location",
    "ssl_cipher_suites",
    "ssl_curves_list",
    "ssl_sigalgs_list",
    "ssl_endpoint_identification_algorithm",
    "enable_ssl_certificate_verification",
    "ssl_engine_id",
    "ssl_engine_location",
];

const KAFKA_PRODUCER_FIELDS: &[&str] = &[
    "topic",
    "acks",
    "compression_type",
    "compression_level",
    "batch_size",
    "batch_num_messages",
    "linger_ms",
    "queue_buffering_max_messages",
    "queue_buffering_max_kbytes",
    "message_max_bytes",
    "message_copy_max_bytes",
    "max_in_flight_requests_per_connection",
    "request_timeout_ms",
    "message_timeout_ms",
    "delivery_timeout_ms",
    "transactional_id",
    "transaction_timeout_ms",
    "enable_idempotence",
    "enable_gapless_guarantee",
    "partitioner",
    "message_send_max_retries",
    "retry_backoff_ms",
    "retry_backoff_max_ms",
    "key_strategy",
    "headers",
    "timestamp_strategy",
    "partition_strategy",
];

const KAFKA_CONSUMER_FIELDS: &[&str] = &[
    "topics",
    "group_id",
    "group_instance_id",
    "auto_offset_reset",
    "enable_auto_commit",
    "auto_commit_interval_ms",
    "enable_auto_offset_store",
    "fetch_min_bytes",
    "fetch_max_bytes",
    "max_partition_fetch_bytes",
    "fetch_wait_max_ms",
    "fetch_queue_backoff_ms",
    "session_timeout_ms",
    "heartbeat_interval_ms",
    "max_poll_interval_ms",
    "isolation_level",
    "partition_assignment_strategy",
    "check_crcs",
    "queued_min_messages",
    "queued_max_messages_kbytes",
    "poll_timeout",
];

impl Analyzer {
    pub(super) fn lower_kafka(
        &mut self,
        body: BTreeMap<String, CstField>,
        kind_span: &Span,
    ) -> KafkaConfig {
        let mut fields = body;
        let bootstrap_servers = self
            .take_required_string(&mut fields, "bootstrap_servers", kind_span, "queue kafka")
            .unwrap_or_default();
        let client_id = self.take_optional_string(&mut fields, "client_id");
        let client_rack = self.take_optional_string(&mut fields, "client_rack");
        let broker_address_family = self.take_optional_string(&mut fields, "broker_address_family");
        let broker_address_ttl_secs =
            self.take_optional_duration(&mut fields, "broker_address_ttl");
        let metadata_max_age_secs = self.take_optional_duration(&mut fields, "metadata_max_age");
        let topic_metadata_refresh_interval_secs =
            self.take_optional_duration(&mut fields, "topic_metadata_refresh_interval");
        let topic_metadata_refresh_fast_interval_ms =
            self.take_optional_int(&mut fields, "topic_metadata_refresh_fast_interval_ms");
        let socket_timeout_secs = self.take_optional_duration(&mut fields, "socket_timeout");
        let socket_keepalive_enable =
            self.take_optional_bool(&mut fields, "socket_keepalive_enable");
        let socket_nagle_disable = self.take_optional_bool(&mut fields, "socket_nagle_disable");
        let socket_max_fails = self.take_optional_int(&mut fields, "socket_max_fails");
        let reconnect_backoff_ms = self.take_optional_int(&mut fields, "reconnect_backoff_ms");
        let reconnect_backoff_max_ms =
            self.take_optional_int(&mut fields, "reconnect_backoff_max_ms");
        let api_version_request = self.take_optional_bool(&mut fields, "api_version_request");
        let api_version_request_timeout_ms =
            self.take_optional_int(&mut fields, "api_version_request_timeout_ms");
        let security = self.lower_kafka_security(&mut fields);
        let producer = self.lower_kafka_producer(&mut fields);
        let consumer = self.lower_kafka_consumer(&mut fields);
        let exactly_once = self.take_optional_bool(&mut fields, "exactly_once");
        let extra_config = self.take_optional_string_string_block(&mut fields, "extra_config");
        let dlq = self.lower_dlq_policy(&mut fields, "dlq", kind_span);

        self.reject_unknown_fields(&mut fields, KAFKA_FIELDS, "queue kafka");

        KafkaConfig {
            bootstrap_servers,
            client_id,
            client_rack,
            broker_address_family,
            broker_address_ttl_secs,
            metadata_max_age_secs,
            topic_metadata_refresh_interval_secs,
            topic_metadata_refresh_fast_interval_ms,
            socket_timeout_secs,
            socket_keepalive_enable,
            socket_nagle_disable,
            socket_max_fails,
            reconnect_backoff_ms,
            reconnect_backoff_max_ms,
            api_version_request,
            api_version_request_timeout_ms,
            security,
            producer,
            consumer,
            exactly_once,
            extra_config,
            dlq,
        }
    }

    fn lower_kafka_security(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
    ) -> Option<KafkaSecurity> {
        let mut block = self.take_optional_block(fields, "security")?;
        let security_protocol = self.take_optional_string(&mut block, "security_protocol");
        let sasl_mechanism = self.take_optional_string(&mut block, "sasl_mechanism");
        let sasl_username = self.take_optional_secret(&mut block, "sasl_username");
        let sasl_password = self.take_optional_secret(&mut block, "sasl_password");
        let sasl_kerberos_service_name =
            self.take_optional_string(&mut block, "sasl_kerberos_service_name");
        let sasl_kerberos_principal =
            self.take_optional_string(&mut block, "sasl_kerberos_principal");
        let sasl_kerberos_keytab = self.take_optional_string(&mut block, "sasl_kerberos_keytab");
        let sasl_kerberos_kinit_cmd =
            self.take_optional_string(&mut block, "sasl_kerberos_kinit_cmd");
        let sasl_kerberos_min_time_before_relogin_secs =
            self.take_optional_duration(&mut block, "sasl_kerberos_min_time_before_relogin");
        let sasl_oauthbearer_method =
            self.take_optional_string(&mut block, "sasl_oauthbearer_method");
        let sasl_oauthbearer_config =
            self.take_optional_string(&mut block, "sasl_oauthbearer_config");
        let sasl_oauthbearer_client_id =
            self.take_optional_string(&mut block, "sasl_oauthbearer_client_id");
        let sasl_oauthbearer_client_secret =
            self.take_optional_secret(&mut block, "sasl_oauthbearer_client_secret");
        let sasl_oauthbearer_token_endpoint_url =
            self.take_optional_string(&mut block, "sasl_oauthbearer_token_endpoint_url");
        let sasl_oauthbearer_scope =
            self.take_optional_string(&mut block, "sasl_oauthbearer_scope");
        let sasl_oauthbearer_extensions =
            self.take_optional_string(&mut block, "sasl_oauthbearer_extensions");
        let enable_sasl_oauthbearer_unsecure_jwt =
            self.take_optional_bool(&mut block, "enable_sasl_oauthbearer_unsecure_jwt");
        let ssl_ca_location = self.take_optional_string(&mut block, "ssl_ca_location");
        let ssl_certificate_location =
            self.take_optional_string(&mut block, "ssl_certificate_location");
        let ssl_key_location = self.take_optional_string(&mut block, "ssl_key_location");
        let ssl_key_password = self.take_optional_secret(&mut block, "ssl_key_password");
        let ssl_ca_pem = self.take_optional_secret(&mut block, "ssl_ca_pem");
        let ssl_certificate_pem = self.take_optional_secret(&mut block, "ssl_certificate_pem");
        let ssl_key_pem = self.take_optional_secret(&mut block, "ssl_key_pem");
        let ssl_keystore_location = self.take_optional_string(&mut block, "ssl_keystore_location");
        let ssl_keystore_password = self.take_optional_secret(&mut block, "ssl_keystore_password");
        let ssl_crl_location = self.take_optional_string(&mut block, "ssl_crl_location");
        let ssl_cipher_suites = self.take_optional_string(&mut block, "ssl_cipher_suites");
        let ssl_curves_list = self.take_optional_string(&mut block, "ssl_curves_list");
        let ssl_sigalgs_list = self.take_optional_string(&mut block, "ssl_sigalgs_list");
        let ssl_endpoint_identification_algorithm =
            self.take_optional_string(&mut block, "ssl_endpoint_identification_algorithm");
        let enable_ssl_certificate_verification =
            self.take_optional_bool(&mut block, "enable_ssl_certificate_verification");
        let ssl_engine_id = self.take_optional_string(&mut block, "ssl_engine_id");
        let ssl_engine_location = self.take_optional_string(&mut block, "ssl_engine_location");
        self.reject_unknown_fields(&mut block, KAFKA_SECURITY_FIELDS, "kafka security");
        Some(KafkaSecurity {
            security_protocol,
            sasl_mechanism,
            sasl_username,
            sasl_password,
            sasl_kerberos_service_name,
            sasl_kerberos_principal,
            sasl_kerberos_keytab,
            sasl_kerberos_kinit_cmd,
            sasl_kerberos_min_time_before_relogin_secs,
            sasl_oauthbearer_method,
            sasl_oauthbearer_config,
            sasl_oauthbearer_client_id,
            sasl_oauthbearer_client_secret,
            sasl_oauthbearer_token_endpoint_url,
            sasl_oauthbearer_scope,
            sasl_oauthbearer_extensions,
            enable_sasl_oauthbearer_unsecure_jwt,
            ssl_ca_location,
            ssl_certificate_location,
            ssl_key_location,
            ssl_key_password,
            ssl_ca_pem,
            ssl_certificate_pem,
            ssl_key_pem,
            ssl_keystore_location,
            ssl_keystore_password,
            ssl_crl_location,
            ssl_cipher_suites,
            ssl_curves_list,
            ssl_sigalgs_list,
            ssl_endpoint_identification_algorithm,
            enable_ssl_certificate_verification,
            ssl_engine_id,
            ssl_engine_location,
        })
    }

    fn lower_kafka_producer(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
    ) -> Option<KafkaProducer> {
        let mut block = self.take_optional_block(fields, "producer")?;
        let topic = self.take_optional_string(&mut block, "topic");
        let acks = self.take_optional_string(&mut block, "acks");
        let compression_type = self.take_optional_string(&mut block, "compression_type");
        let compression_level = self.take_optional_int(&mut block, "compression_level");
        let batch_size_bytes = self.take_optional_int(&mut block, "batch_size");
        let batch_num_messages = self.take_optional_int(&mut block, "batch_num_messages");
        let linger_ms = self.take_optional_int(&mut block, "linger_ms");
        let queue_buffering_max_messages =
            self.take_optional_int(&mut block, "queue_buffering_max_messages");
        let queue_buffering_max_kbytes =
            self.take_optional_int(&mut block, "queue_buffering_max_kbytes");
        let message_max_bytes = self.take_optional_int(&mut block, "message_max_bytes");
        let message_copy_max_bytes = self.take_optional_int(&mut block, "message_copy_max_bytes");
        let max_in_flight_requests_per_connection =
            self.take_optional_int(&mut block, "max_in_flight_requests_per_connection");
        let request_timeout_ms = self.take_optional_int(&mut block, "request_timeout_ms");
        let message_timeout_ms = self.take_optional_int(&mut block, "message_timeout_ms");
        let delivery_timeout_ms = self.take_optional_int(&mut block, "delivery_timeout_ms");
        let transactional_id = self.take_optional_string(&mut block, "transactional_id");
        let transaction_timeout_ms = self.take_optional_int(&mut block, "transaction_timeout_ms");
        let enable_idempotence = self.take_optional_bool(&mut block, "enable_idempotence");
        let enable_gapless_guarantee =
            self.take_optional_bool(&mut block, "enable_gapless_guarantee");
        let partitioner = self.take_optional_string(&mut block, "partitioner");
        let message_send_max_retries =
            self.take_optional_int(&mut block, "message_send_max_retries");
        let retry_backoff_ms = self.take_optional_int(&mut block, "retry_backoff_ms");
        let retry_backoff_max_ms = self.take_optional_int(&mut block, "retry_backoff_max_ms");
        let key_strategy = self.take_optional_templated_string(&mut block, "key_strategy");
        let headers = self.take_optional_string_string_block(&mut block, "headers");
        let timestamp_strategy = self.take_optional_string(&mut block, "timestamp_strategy");
        let partition_strategy =
            self.take_optional_templated_string(&mut block, "partition_strategy");
        self.reject_unknown_fields(&mut block, KAFKA_PRODUCER_FIELDS, "kafka producer");
        Some(KafkaProducer {
            topic,
            acks,
            compression_type,
            compression_level,
            batch_size_bytes,
            batch_num_messages,
            linger_ms,
            queue_buffering_max_messages,
            queue_buffering_max_kbytes,
            message_max_bytes,
            message_copy_max_bytes,
            max_in_flight_requests_per_connection,
            request_timeout_ms,
            message_timeout_ms,
            delivery_timeout_ms,
            transactional_id,
            transaction_timeout_ms,
            enable_idempotence,
            enable_gapless_guarantee,
            partitioner,
            message_send_max_retries,
            retry_backoff_ms,
            retry_backoff_max_ms,
            key_strategy,
            headers,
            timestamp_strategy,
            partition_strategy,
        })
    }

    fn lower_kafka_consumer(
        &mut self,
        fields: &mut BTreeMap<String, CstField>,
    ) -> Option<KafkaConsumer> {
        let mut block = self.take_optional_block(fields, "consumer")?;
        let topics = self.take_optional_string_list(&mut block, "topics");
        let group_id = self.take_optional_string(&mut block, "group_id");
        let group_instance_id = self.take_optional_string(&mut block, "group_instance_id");
        let auto_offset_reset = self.take_optional_string(&mut block, "auto_offset_reset");
        let enable_auto_commit = self.take_optional_bool(&mut block, "enable_auto_commit");
        let auto_commit_interval_ms = self.take_optional_int(&mut block, "auto_commit_interval_ms");
        let enable_auto_offset_store =
            self.take_optional_bool(&mut block, "enable_auto_offset_store");
        let fetch_min_bytes = self.take_optional_int(&mut block, "fetch_min_bytes");
        let fetch_max_bytes = self.take_optional_int(&mut block, "fetch_max_bytes");
        let max_partition_fetch_bytes =
            self.take_optional_int(&mut block, "max_partition_fetch_bytes");
        let fetch_wait_max_ms = self.take_optional_int(&mut block, "fetch_wait_max_ms");
        let fetch_queue_backoff_ms = self.take_optional_int(&mut block, "fetch_queue_backoff_ms");
        let session_timeout_ms = self.take_optional_int(&mut block, "session_timeout_ms");
        let heartbeat_interval_ms = self.take_optional_int(&mut block, "heartbeat_interval_ms");
        let max_poll_interval_ms = self.take_optional_int(&mut block, "max_poll_interval_ms");
        let isolation_level = self.take_optional_string(&mut block, "isolation_level");
        let partition_assignment_strategy =
            self.take_optional_string(&mut block, "partition_assignment_strategy");
        let check_crcs = self.take_optional_bool(&mut block, "check_crcs");
        let queued_min_messages = self.take_optional_int(&mut block, "queued_min_messages");
        let queued_max_messages_kbytes =
            self.take_optional_int(&mut block, "queued_max_messages_kbytes");
        let poll_timeout_ms = self.take_optional_int(&mut block, "poll_timeout");
        self.reject_unknown_fields(&mut block, KAFKA_CONSUMER_FIELDS, "kafka consumer");
        Some(KafkaConsumer {
            topics,
            group_id,
            group_instance_id,
            auto_offset_reset,
            enable_auto_commit,
            auto_commit_interval_ms,
            enable_auto_offset_store,
            fetch_min_bytes,
            fetch_max_bytes,
            max_partition_fetch_bytes,
            fetch_wait_max_ms,
            fetch_queue_backoff_ms,
            session_timeout_ms,
            heartbeat_interval_ms,
            max_poll_interval_ms,
            isolation_level,
            partition_assignment_strategy,
            check_crcs,
            queued_min_messages,
            queued_max_messages_kbytes,
            poll_timeout_ms,
        })
    }
}
