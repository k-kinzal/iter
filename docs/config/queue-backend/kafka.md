# Queue Backend: `kafka`

Apache Kafka cluster. One producer topic and one consumer group per queue.

AST: `KafkaConfig` in `iter_language/src/ast/queue/kafka.rs`.

## Syntax

```hcl
queue kafka {
  bootstrap_servers = "<broker1>,<broker2>,..."

  client_id                                = "<override>"    # Optional
  client_rack                              = "<rack-id>"     # Optional
  broker_address_family                    = "any|v4|v6"     # Optional
  broker_address_ttl_secs                  = <int>           # Optional
  metadata_max_age_secs                    = <int>           # Optional
  topic_metadata_refresh_interval_secs     = <int>           # Optional
  topic_metadata_refresh_fast_interval_ms  = <int>           # Optional
  socket_timeout_secs                      = <int>           # Optional
  socket_keepalive_enable                  = <bool>          # Optional
  socket_nagle_disable                     = <bool>          # Optional
  socket_max_fails                         = <int>           # Optional
  reconnect_backoff_ms                     = <int>           # Optional
  reconnect_backoff_max_ms                 = <int>           # Optional
  api_version_request                      = <bool>          # Optional
  api_version_request_timeout_ms           = <int>           # Optional
  exactly_once                             = <bool>          # Optional

  security { ... }     # Optional
  producer { ... }     # Optional
  consumer { ... }     # Optional
  extra_config = { ... }   # Optional — untyped escape hatch
  dlq      { ... }     # Optional
}
```

## Top-level Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `bootstrap_servers` | `string` | Required | — | CSV list of broker `host:port`s. |
| `client_id` | `string` | Optional | iter default | Client identifier reported to brokers. |
| `client_rack` | `string` | Optional | — | Client rack for fetch-from-follower. |
| `broker_address_family` | `enum { any \| v4 \| v6 }` | Optional | `any` | Broker DNS preference. |
| `broker_address_ttl_secs` | `integer` | Optional | librdkafka default | Broker DNS re-resolution interval. |
| `metadata_max_age_secs` | `integer` | Optional | librdkafka default | Cluster metadata refresh ceiling. |
| `topic_metadata_refresh_interval_secs` | `integer` | Optional | librdkafka default | Topic metadata refresh cadence. |
| `topic_metadata_refresh_fast_interval_ms` | `integer` | Optional | librdkafka default | Fast-refresh interval on error. |
| `socket_timeout_secs` | `integer` | Optional | librdkafka default | Network socket timeout. |
| `socket_keepalive_enable` | `bool` | Optional | `false` | Enable TCP keepalive. |
| `socket_nagle_disable` | `bool` | Optional | `false` | Disable Nagle's algorithm. |
| `socket_max_fails` | `integer` | Optional | librdkafka default | Connection-failure threshold before broker eviction. |
| `reconnect_backoff_ms` | `integer` | Optional | librdkafka default | Reconnect backoff. |
| `reconnect_backoff_max_ms` | `integer` | Optional | librdkafka default | Reconnect max backoff. |
| `api_version_request` | `bool` | Optional | `true` | Send `ApiVersionRequest` on connect. |
| `api_version_request_timeout_ms` | `integer` | Optional | librdkafka default | Timeout for the version request. |
| `exactly_once` | `bool` | Optional | `false` | Convenience flag — sets idempotence + `acks=all` + a transactional id. |
| `extra_config` | `map(string → string)` | Optional | `{}` | Untyped escape hatch. Applied last; overrides any typed field. Use for librdkafka knobs iter does not expose. |

## `security` block

Security / SASL / TLS surface.

| Name | Type | Description |
| --- | --- | --- |
| `security_protocol` | `enum { plaintext \| ssl \| sasl_plaintext \| sasl_ssl }` | Transport protocol. Defaults to `plaintext`. |
| `sasl_mechanism` | `string` | SASL mechanism: `PLAIN`, `SCRAM-SHA-256`, `SCRAM-SHA-512`, `GSSAPI`, `OAUTHBEARER`. |
| `sasl_username` | secret | SASL username. |
| `sasl_password` | secret | SASL password. |
| `sasl_kerberos_service_name` | `string` | Kerberos service name. |
| `sasl_kerberos_principal` | `string` | Kerberos principal. |
| `sasl_kerberos_keytab` | `string` | Kerberos keytab path. |
| `sasl_kerberos_kinit_cmd` | `string` | Custom `kinit` command line. |
| `sasl_kerberos_min_time_before_relogin_secs` | `integer` | Minimum interval between re-login attempts. |
| `sasl_oauthbearer_method` | `enum { default \| oidc }` | OAUTHBEARER method. |
| `sasl_oauthbearer_config` | `string` | Static OAUTHBEARER config string. |
| `sasl_oauthbearer_client_id` | `string` | OIDC client id. |
| `sasl_oauthbearer_client_secret` | secret | OIDC client secret. |
| `sasl_oauthbearer_token_endpoint_url` | `string` | OIDC token endpoint. |
| `sasl_oauthbearer_scope` | `string` | OIDC scope. |
| `sasl_oauthbearer_extensions` | `string` | OIDC extensions. |
| `enable_sasl_oauthbearer_unsecure_jwt` | `bool` | Allow unsigned JWTs (dev-only). |
| `ssl_ca_location` | `string` | CA bundle path. |
| `ssl_certificate_location` | `string` | Client certificate path. |
| `ssl_key_location` | `string` | Client key path. |
| `ssl_key_password` | secret | Client key password. |
| `ssl_ca_pem` | secret | Inline CA bundle. |
| `ssl_certificate_pem` | secret | Inline client certificate. |
| `ssl_key_pem` | secret | Inline client key. |
| `ssl_keystore_location` | `string` | PKCS12 keystore path. |
| `ssl_keystore_password` | secret | PKCS12 keystore password. |
| `ssl_crl_location` | `string` | CRL path. |
| `ssl_cipher_suites` | `string` | Allowed cipher suites. |
| `ssl_curves_list` | `string` | Allowed elliptic curves. |
| `ssl_sigalgs_list` | `string` | Allowed signature algorithms. |
| `ssl_endpoint_identification_algorithm` | `enum { none \| https }` | Endpoint identification. |
| `enable_ssl_certificate_verification` | `bool` | Verify peer certificate. |
| `ssl_engine_id` | `string` | HSM engine id. |
| `ssl_engine_location` | `string` | HSM engine path. |

## `producer` block

Producer-side knobs. `topic` is required for produce operations.

| Name | Type | Default | Description |
| --- | --- | --- | --- |
| `topic` | `string` | — | Target topic. Required for produce. |
| `acks` | `enum { none \| leader \| all }` | `all` | Ack mode. |
| `compression_type` | `enum { none \| gzip \| snappy \| lz4 \| zstd }` | `none` | Compression codec. |
| `compression_level` | `integer` | codec default | Codec-specific level. |
| `batch_size_bytes` | `integer` | librdkafka default | Batch size in bytes. |
| `batch_num_messages` | `integer` | librdkafka default | Batch size in messages. |
| `linger_ms` | `integer` | librdkafka default | Linger before flush. |
| `queue_buffering_max_messages` | `integer` | librdkafka default | Local queue cap (messages). |
| `queue_buffering_max_kbytes` | `integer` | librdkafka default | Local queue cap (KB). |
| `message_max_bytes` | `integer` | librdkafka default | Max message size. |
| `message_copy_max_bytes` | `integer` | librdkafka default | Zero-copy threshold. |
| `max_in_flight_requests_per_connection` | `integer` | librdkafka default | Max in-flight requests. |
| `request_timeout_ms` | `integer` | librdkafka default | Per-request timeout. |
| `message_timeout_ms` | `integer` | librdkafka default | End-to-end message timeout. |
| `delivery_timeout_ms` | `integer` | librdkafka default | Total delivery timeout. |
| `transactional_id` | `string` | — | Transactional id (forces idempotence). |
| `transaction_timeout_ms` | `integer` | librdkafka default | Transaction timeout. |
| `enable_idempotence` | `bool` | `false` | Idempotent producer. |
| `enable_gapless_guarantee` | `bool` | `false` | Maintain gapless guarantee. |
| `partitioner` | `string` | librdkafka default | Partitioner algorithm. |
| `message_send_max_retries` | `integer` | librdkafka default | Send retry count. |
| `retry_backoff_ms` | `integer` | librdkafka default | Retry backoff. |
| `retry_backoff_max_ms` | `integer` | librdkafka default | Max retry backoff. |
| `key_strategy` | templated string | — | Per-message key source, e.g. `from_metadata("tenant")`. |
| `headers` | `map(string → string)` | `{}` | Static header overlay. |
| `timestamp_strategy` | `enum { signal_created_at \| now }` | `signal_created_at` | Record timestamp source. |
| `partition_strategy` | templated string | `partitioner_default` | Explicit partition source. |

## `consumer` block

Consumer-side knobs. `topics` and `group_id` are required.

| Name | Type | Default | Description |
| --- | --- | --- | --- |
| `topics` | `list(string)` | — | Topics to subscribe to. Required. |
| `group_id` | `string` | — | Consumer group id. Required. |
| `group_instance_id` | `string` | — | Static membership id. |
| `auto_offset_reset` | `enum { earliest \| latest \| error }` | `latest` | Initial offset policy. |
| `enable_auto_commit` | `bool` | `false` | Auto-commit. iter commits manually when false. |
| `auto_commit_interval_ms` | `integer` | librdkafka default | Auto-commit interval. |
| `enable_auto_offset_store` | `bool` | librdkafka default | Automatic offset store. |
| `fetch_min_bytes` | `integer` | librdkafka default | Min fetch bytes per request. |
| `fetch_max_bytes` | `integer` | librdkafka default | Max fetch bytes per request. |
| `max_partition_fetch_bytes` | `integer` | librdkafka default | Max fetch bytes per partition. |
| `fetch_wait_max_ms` | `integer` | librdkafka default | Max fetch wait. |
| `fetch_queue_backoff_ms` | `integer` | librdkafka default | Backoff between empty fetches. |
| `session_timeout_ms` | `integer` | librdkafka default | Group session timeout. |
| `heartbeat_interval_ms` | `integer` | librdkafka default | Heartbeat interval. |
| `max_poll_interval_ms` | `integer` | librdkafka default | Max poll interval. |
| `isolation_level` | `enum { read_committed \| read_uncommitted }` | `read_committed` | Read isolation. |
| `partition_assignment_strategy` | `string` | librdkafka default | Group partition-assignment strategy. |
| `check_crcs` | `bool` | `true` | CRC verification. |
| `queued_min_messages` | `integer` | librdkafka default | Min queued messages. |
| `queued_max_messages_kbytes` | `integer` | librdkafka default | Max queued bytes (KB). |
| `poll_timeout_ms` | `integer` | `100` | iter-level poll timeout. |

## `dlq` block

Kafka has no native DLQ; iter implements dead-letter routing with `kind = "iter_republish"`. Shared shape documented in [`sqs.md`](sqs.md#retry-and-dlq-common-shape).

## Examples

### Local Kafka — plaintext

```hcl
queue kafka {
  bootstrap_servers = "localhost:9092"

  producer { topic = "iter-signals" }

  consumer {
    topics   = ["iter-signals"]
    group_id = "iter-worker"
  }
}
```

### SASL_SSL with SCRAM

```hcl
queue kafka {
  bootstrap_servers = "kafka-0:9093,kafka-1:9093,kafka-2:9093"

  security {
    security_protocol = "sasl_ssl"
    sasl_mechanism    = "SCRAM-SHA-512"
    sasl_username     = env("KAFKA_USER")
    sasl_password     = env("KAFKA_PASSWORD")
    ssl_ca_location   = "/etc/ssl/certs/kafka-ca.pem"
  }

  producer {
    topic              = "iter-signals"
    compression_type   = "zstd"
    compression_level  = 3
    enable_idempotence = true
    key_strategy       = from_metadata("tenant")
  }

  consumer {
    topics                = ["iter-signals"]
    group_id              = "iter-prod"
    auto_offset_reset     = "earliest"
    max_poll_interval_ms  = 300000
  }
}
```

### Exactly-once via convenience flag

```hcl
queue kafka {
  bootstrap_servers = "kafka:9092"
  exactly_once      = true

  producer {
    topic             = "orders"
    transactional_id  = "iter-orders-01"
  }

  consumer {
    topics           = ["orders"]
    group_id         = "iter-orders"
    isolation_level  = "read_committed"
  }
}
```
