//! Translate Kafka configuration from the language AST into `iter_core` types.

use iter_core::queue::kafka::{
    KafkaConsumerConfig as CoreKafkaConsumer, KafkaProducerConfig as CoreKafkaProducer,
    KafkaQueueConfig, KafkaSecurityConfig as CoreKafkaSecurity,
};
use iter_language::{KafkaConfig, KafkaConsumer, KafkaProducer, KafkaSecurity, MetadataSource};

use super::{QueueBuildError, opt_u32, opt_u64, translate_dlq};
use crate::secrets::resolve_secret;

pub(super) fn build_kafka_config(cfg: &KafkaConfig) -> Result<KafkaQueueConfig, QueueBuildError> {
    let security = cfg
        .security
        .as_ref()
        .map(translate_kafka_security)
        .transpose()?;

    let mut producer = cfg
        .producer
        .as_ref()
        .map(translate_kafka_producer)
        .transpose()?;

    let mut consumer = cfg.consumer.as_ref().map(translate_kafka_consumer);

    let exactly_once = cfg.exactly_once.unwrap_or(false);
    if exactly_once {
        // Materialise the shorthand: ensure idempotence + acks=all +
        // capped in-flight + transactional id are present on the
        // resolved producer config so the iter_core layer never has to
        // consult `exactly_once` again.
        let p = producer.get_or_insert_with(CoreKafkaProducer::default);
        p.enable_idempotence = Some(true);
        if p.acks.is_none() {
            p.acks = Some("all".into());
        }
        if p.max_in_flight_requests_per_connection.is_none() {
            p.max_in_flight_requests_per_connection = Some(5);
        }
        if p.transactional_id.is_none() {
            // exactly_once requires a stable transactional id per producer
            // instance; users who care about cross-restart fencing should
            // set it explicitly. The auto-generated value scopes to this
            // process so two iter runners on the same broker don't fence
            // each other out.
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            p.transactional_id = Some(format!("iter-tx-{}-{nanos}", std::process::id()));
        }
        if let Some(c) = consumer.as_mut() {
            if c.isolation_level.is_none() {
                c.isolation_level = Some("read_committed".into());
            }
        }
    }

    let dlq = cfg.dlq.as_ref().map(translate_dlq).transpose()?;

    Ok(KafkaQueueConfig {
        bootstrap_servers: cfg.bootstrap_servers.clone(),
        client_id: cfg.client_id.clone(),
        client_rack: cfg.client_rack.clone(),
        broker_address_family: cfg.broker_address_family.clone(),
        broker_address_ttl_secs: opt_u64(cfg.broker_address_ttl_secs),
        metadata_max_age_secs: opt_u64(cfg.metadata_max_age_secs),
        topic_metadata_refresh_interval_secs: opt_u64(cfg.topic_metadata_refresh_interval_secs),
        topic_metadata_refresh_fast_interval_ms: opt_u64(
            cfg.topic_metadata_refresh_fast_interval_ms,
        ),
        socket_timeout_secs: opt_u64(cfg.socket_timeout_secs),
        socket_keepalive_enable: cfg.socket_keepalive_enable,
        socket_nagle_disable: cfg.socket_nagle_disable,
        socket_max_fails: opt_u32(cfg.socket_max_fails),
        reconnect_backoff_ms: opt_u32(cfg.reconnect_backoff_ms),
        reconnect_backoff_max_ms: opt_u32(cfg.reconnect_backoff_max_ms),
        api_version_request: cfg.api_version_request,
        api_version_request_timeout_ms: opt_u32(cfg.api_version_request_timeout_ms),
        security,
        producer,
        consumer,
        exactly_once,
        extra_config: cfg.extra_config.clone(),
        dlq,
    })
}

fn translate_kafka_security(s: &KafkaSecurity) -> Result<CoreKafkaSecurity, QueueBuildError> {
    let resolve_opt = |opt: &Option<iter_language::SecretExpr>,
                       label: &'static str|
     -> Result<Option<String>, QueueBuildError> {
        opt.as_ref()
            .map(resolve_secret)
            .transpose()
            .map_err(|s| QueueBuildError::secret(format!("security.{label}"), s))
    };
    Ok(CoreKafkaSecurity {
        security_protocol: s.security_protocol.clone(),
        sasl_mechanism: s.sasl_mechanism.clone(),
        sasl_username: resolve_opt(&s.sasl_username, "sasl_username")?,
        sasl_password: resolve_opt(&s.sasl_password, "sasl_password")?,
        sasl_kerberos_service_name: s.sasl_kerberos_service_name.clone(),
        sasl_kerberos_principal: s.sasl_kerberos_principal.clone(),
        sasl_kerberos_keytab: s.sasl_kerberos_keytab.clone(),
        sasl_kerberos_kinit_cmd: s.sasl_kerberos_kinit_cmd.clone(),
        sasl_kerberos_min_time_before_relogin_secs: opt_u64(
            s.sasl_kerberos_min_time_before_relogin_secs,
        ),
        sasl_oauthbearer_method: s.sasl_oauthbearer_method.clone(),
        sasl_oauthbearer_config: s.sasl_oauthbearer_config.clone(),
        sasl_oauthbearer_client_id: s.sasl_oauthbearer_client_id.clone(),
        sasl_oauthbearer_client_secret: resolve_opt(
            &s.sasl_oauthbearer_client_secret,
            "sasl_oauthbearer_client_secret",
        )?,
        sasl_oauthbearer_token_endpoint_url: s.sasl_oauthbearer_token_endpoint_url.clone(),
        sasl_oauthbearer_scope: s.sasl_oauthbearer_scope.clone(),
        sasl_oauthbearer_extensions: s.sasl_oauthbearer_extensions.clone(),
        enable_sasl_oauthbearer_unsecure_jwt: s.enable_sasl_oauthbearer_unsecure_jwt,
        ssl_ca_location: s.ssl_ca_location.clone(),
        ssl_certificate_location: s.ssl_certificate_location.clone(),
        ssl_key_location: s.ssl_key_location.clone(),
        ssl_key_password: resolve_opt(&s.ssl_key_password, "ssl_key_password")?,
        ssl_ca_pem: resolve_opt(&s.ssl_ca_pem, "ssl_ca_pem")?,
        ssl_certificate_pem: resolve_opt(&s.ssl_certificate_pem, "ssl_certificate_pem")?,
        ssl_key_pem: resolve_opt(&s.ssl_key_pem, "ssl_key_pem")?,
        ssl_keystore_location: s.ssl_keystore_location.clone(),
        ssl_keystore_password: resolve_opt(&s.ssl_keystore_password, "ssl_keystore_password")?,
        ssl_crl_location: s.ssl_crl_location.clone(),
        ssl_cipher_suites: s.ssl_cipher_suites.clone(),
        ssl_curves_list: s.ssl_curves_list.clone(),
        ssl_sigalgs_list: s.ssl_sigalgs_list.clone(),
        ssl_endpoint_identification_algorithm: s.ssl_endpoint_identification_algorithm.clone(),
        enable_ssl_certificate_verification: s.enable_ssl_certificate_verification,
        ssl_engine_id: s.ssl_engine_id.clone(),
        ssl_engine_location: s.ssl_engine_location.clone(),
    })
}

fn translate_kafka_producer(p: &KafkaProducer) -> Result<CoreKafkaProducer, QueueBuildError> {
    let (key_strategy_metadata, key_from_signal_id) = match p.key_strategy.as_ref() {
        Some(MetadataSource::FromMetadata(k)) => (Some(k.clone()), false),
        Some(MetadataSource::Literal(s)) if s == "signal_id" => (None, true),
        Some(MetadataSource::Literal(s)) if s == "none" => (None, false),
        Some(MetadataSource::Literal(other)) => {
            return Err(QueueBuildError::invalid(format!(
                "producer.key_strategy literal must be `none` or `signal_id`; got `{other}`"
            )));
        }
        None => (None, false),
    };
    let partition_strategy_metadata = match p.partition_strategy.as_ref() {
        Some(MetadataSource::FromMetadata(k)) => Some(k.clone()),
        Some(MetadataSource::Literal(s)) if s == "partitioner_default" => None,
        Some(MetadataSource::Literal(other)) => {
            return Err(QueueBuildError::invalid(format!(
                "producer.partition_strategy literal must be `partitioner_default`; got `{other}`"
            )));
        }
        None => None,
    };
    Ok(CoreKafkaProducer {
        topic: p.topic.clone(),
        acks: p.acks.clone(),
        compression_type: p.compression_type.clone(),
        compression_level: p.compression_level.map(|n| i32::try_from(n).unwrap_or(0)),
        batch_size_bytes: opt_u32(p.batch_size_bytes),
        batch_num_messages: opt_u32(p.batch_num_messages),
        linger_ms: opt_u32(p.linger_ms),
        queue_buffering_max_messages: opt_u32(p.queue_buffering_max_messages),
        queue_buffering_max_kbytes: opt_u32(p.queue_buffering_max_kbytes),
        message_max_bytes: opt_u32(p.message_max_bytes),
        message_copy_max_bytes: opt_u32(p.message_copy_max_bytes),
        max_in_flight_requests_per_connection: opt_u32(p.max_in_flight_requests_per_connection),
        request_timeout_ms: opt_u32(p.request_timeout_ms),
        message_timeout_ms: opt_u32(p.message_timeout_ms),
        delivery_timeout_ms: opt_u32(p.delivery_timeout_ms),
        transactional_id: p.transactional_id.clone(),
        transaction_timeout_ms: opt_u32(p.transaction_timeout_ms),
        enable_idempotence: p.enable_idempotence,
        enable_gapless_guarantee: p.enable_gapless_guarantee,
        partitioner: p.partitioner.clone(),
        message_send_max_retries: opt_u32(p.message_send_max_retries),
        retry_backoff_ms: opt_u32(p.retry_backoff_ms),
        retry_backoff_max_ms: opt_u32(p.retry_backoff_max_ms),
        key_strategy_metadata,
        key_from_signal_id,
        headers: p.headers.clone(),
        timestamp_strategy: p.timestamp_strategy.clone(),
        partition_strategy_metadata,
    })
}

fn translate_kafka_consumer(c: &KafkaConsumer) -> CoreKafkaConsumer {
    CoreKafkaConsumer {
        topics: c.topics.clone(),
        group_id: c.group_id.clone(),
        group_instance_id: c.group_instance_id.clone(),
        auto_offset_reset: c.auto_offset_reset.clone(),
        enable_auto_commit: c.enable_auto_commit,
        auto_commit_interval_ms: opt_u32(c.auto_commit_interval_ms),
        enable_auto_offset_store: c.enable_auto_offset_store,
        fetch_min_bytes: opt_u32(c.fetch_min_bytes),
        fetch_max_bytes: opt_u32(c.fetch_max_bytes),
        max_partition_fetch_bytes: opt_u32(c.max_partition_fetch_bytes),
        fetch_wait_max_ms: opt_u32(c.fetch_wait_max_ms),
        fetch_queue_backoff_ms: opt_u32(c.fetch_queue_backoff_ms),
        session_timeout_ms: opt_u32(c.session_timeout_ms),
        heartbeat_interval_ms: opt_u32(c.heartbeat_interval_ms),
        max_poll_interval_ms: opt_u32(c.max_poll_interval_ms),
        isolation_level: c.isolation_level.clone(),
        partition_assignment_strategy: c.partition_assignment_strategy.clone(),
        check_crcs: c.check_crcs,
        queued_min_messages: opt_u32(c.queued_min_messages),
        queued_max_messages_kbytes: opt_u32(c.queued_max_messages_kbytes),
        poll_timeout_ms: opt_u32(c.poll_timeout_ms),
    }
}
