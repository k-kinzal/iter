# Queue Backend: `sqs`

AWS Simple Queue Service (Standard or FIFO). Suitable for multi-region, multi-account, and serverless-producer topologies.

AST: `SqsConfig` in `iter_language/src/ast/queue/sqs.rs`.

## Syntax

```hcl
queue sqs {
  # Identity — one of queue_url OR (queue_name + account_id) is required.
  queue_url  = "<full URL>"
  # -- or --
  queue_name = "<name>"
  account_id = "<12-digit>"

  region                 = "<aws-region>"      # Required
  endpoint_url           = "<override URL>"    # Optional (LocalStack, VPC endpoints)
  fifo                   = <bool>              # Optional, auto-detected from .fifo suffix
  use_fips               = <bool>              # Optional
  use_dual_stack         = <bool>              # Optional
  sts_regional_endpoints = "regional" | "legacy"   # Optional
  app_name               = "<user-agent tag>"  # Optional

  credentials  { ... }   # Optional
  http_client  { ... }   # Optional
  producer     { ... }   # Optional
  consumer     { ... }   # Optional
  retry        { ... }   # Optional
  dlq          { ... }   # Optional
}
```

## Top-level Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `queue_url` | `string` | Conditional | — | Full queue URL. Mutually exclusive with `queue_name` + `account_id`. |
| `queue_name` | `string` | Conditional | — | Queue name. Combined with `account_id` and the resolved region at build time. |
| `account_id` | `string` | Conditional | — | 12-digit AWS account id. Required when using `queue_name`. |
| `region` | `string` | Required | — | AWS region. No default — the SDK chain cannot derive it from the queue URL. |
| `endpoint_url` | `string` | Optional | — | Override endpoint (LocalStack, VPC endpoints, FIPS/dual-stack hosts). |
| `fifo` | `bool` | Optional | auto-detected | Force FIFO mode. Auto-detected from a `.fifo` URL suffix when absent. |
| `use_fips` | `bool` | Optional | `false` | Prefer FIPS-compliant endpoints. |
| `use_dual_stack` | `bool` | Optional | `false` | Prefer dual-stack IPv4+IPv6 endpoints. |
| `sts_regional_endpoints` | `enum { regional \| legacy }` | Optional | `regional` | STS endpoint policy. |
| `app_name` | `string` | Optional | — | Propagated into the AWS SDK User-Agent string. |

## `credentials` block

Layers over the AWS default credential chain. Exactly one `kind` must be chosen.

```hcl
credentials {
  kind = "static" | "assume_role" | "profile" | "web_identity_token_file" | "imds" | "process" | "default"
  # ...per-kind fields
}
```

### `kind = "static"`

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `access_key_id` | secret | Required | AWS_ACCESS_KEY_ID. Accepts `env("VAR")` or a string literal. |
| `secret_access_key` | secret | Required | AWS_SECRET_ACCESS_KEY. |
| `session_token` | secret | Optional | STS session token. |

### `kind = "assume_role"`

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `role_arn` | `string` | Required | Role ARN to assume. |
| `session_name` | `string` | Optional | Session name (SDK generates one when absent). |
| `external_id` | secret | Optional | External-id challenge for cross-account roles. |
| `duration_seconds` | `integer` | Optional | Session duration. |
| `source_profile` | `string` | Optional | Profile whose credentials mint the AssumeRole call. |

### `kind = "profile"`

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `name` | `string` | Required | Profile in `~/.aws/credentials` or `~/.aws/config`. |

### `kind = "web_identity_token_file"` (EKS IRSA, Pod Identity)

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `role_arn` | `string` | Required | Role ARN to assume. |
| `token_file` | `string` | Required | Path to the JWT. |
| `session_name` | `string` | Optional | Session name. |

### `kind = "imds"`

No fields. Uses EC2/ECS instance metadata.

### `kind = "process"`

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `command` | `string` | Required | `credential_process`-style external command. |

### `kind = "default"`

No fields. Explicit form of "use the SDK default chain".

## `http_client` block

Connection-level HTTP knobs. All optional; each maps to hyper / smithy-http-client.

| Name | Type | Default | Description |
| --- | --- | --- | --- |
| `connect_timeout_secs` | `integer` | SDK default | TCP connect timeout. |
| `read_timeout_secs` | `integer` | SDK default | Idle-read timeout. |
| `operation_timeout_secs` | `integer` | SDK default | Total time including retries. |
| `operation_attempt_timeout_secs` | `integer` | SDK default | Per-attempt timeout. |
| `tcp_keepalive_secs` | `integer` | SDK default | TCP keepalive timer. |
| `max_idle_connections_per_host` | `integer` | SDK default | Pool cap. |
| `connection_pool_idle_timeout_secs` | `integer` | SDK default | Pool eviction timer. |
| `proxy_url` | `string` | — | HTTP proxy URL. |
| `no_proxy` | `list(string)` | — | `NO_PROXY`-style suffix list. |

## `producer` block

Producer-side (`queue.queue`) knobs.

| Name | Type | Default | Description |
| --- | --- | --- | --- |
| `delay_seconds` | `integer` | `0` | Default per-message `DelaySeconds`. |
| `message_attributes` | `map(string → string)` | `{}` | Static attributes applied to every message. |
| `trace_header` | `bool` | `false` | Toggle X-Ray `AWSTraceHeader`. |
| `message_group_id` | templated string | — | FIFO `MessageGroupId` source. Use `from_metadata("key")` to populate from Signal metadata. |
| `message_deduplication_id` | templated string | — | FIFO `MessageDeduplicationId` source. |
| `batch_size` | `integer` (1–10) | `1` | `SendMessageBatch` group size. |
| `batch_max_bytes` | `integer` (≤ 262144) | AWS limit | Per-batch byte cap. |
| `batch_linger_secs` | `integer` | `0` | Max wait before flushing a partial batch. |

## `consumer` block

Consumer-side (`queue.dequeue`) knobs.

| Name | Type | Default | Description |
| --- | --- | --- | --- |
| `visibility_timeout_secs` | `integer` | queue default | Message hide-time on receive. |
| `wait_time_seconds` | `integer` (0–20) | `0` | Long-poll wait. |
| `max_number_of_messages` | `integer` (1–10) | `1` | Messages per `ReceiveMessage`. |
| `message_attribute_names` | `list(string)` | `[]` | Attributes to fetch. |
| `message_system_attribute_names` | `list(string)` | `[]` | System attributes to fetch. |
| `concurrent_receivers` | `integer` | `1` | Concurrent receive loops. |

## `retry` block

SDK-call retry policy. See [`queue-backend/retry-and-dlq.md`](#retry-and-dlq-common-shape) below for the shape shared with other backends.

## `dlq` block

Dead-letter handling. See below.

---

## Retry and DLQ (common shape)

These sub-blocks are identical across SQS, Kafka, Kinesis, Pub/Sub, and Service Bus.

### `retry`

| Name | Type | Default | Description |
| --- | --- | --- | --- |
| `mode` | `enum { standard \| adaptive \| fixed \| exponential }` | `standard` | Retry algorithm. |
| `max_attempts` | `integer` | backend default | Max total attempts. |
| `initial_backoff_secs` | `integer` | backend default | Initial backoff. |
| `max_backoff_secs` | `integer` | backend default | Ceiling for exponential backoff. |
| `try_timeout_secs` | `integer` | — | Per-attempt timeout. |
| `retryable_codes` | `list(string)` | — | Whitelist of retryable error codes (primarily Pub/Sub). |

### `dlq`

| Name | Type | Default | Description |
| --- | --- | --- | --- |
| `kind` | `enum { none \| native \| iter_republish }` | `none` | Dead-letter strategy. `native` uses the backend's own DLQ mechanism; `iter_republish` has iter push poison records to `target` after `max_receive_count` attempts. |
| `max_receive_count` | `integer` | — | Threshold for `iter_republish`. |
| `reason_template` | `string` | — | Template attached to republished records. |
| `include_headers` | `bool` | `true` | Carry source headers/attributes across. |
| `target` | DLQ target block | required when `kind = iter_republish` | Destination. |

#### DLQ `target` kinds

```hcl
target sqs        { queue_url = "...", region = "..." }
target kinesis    { stream_arn = "...", region = "..." }
target kafka      { brokers = "...", topic = "..." }
target s3         { bucket = "...", prefix = "...", region = "..." }
target file       { path = "..." }
target pubsub     { project = "...", topic = "..." }
target servicebus { namespace = "...", entity = "..." }
```

Each target kind exposes the minimum identity fields its backend needs. Fuller per-target configs (auth, encryption keys, custom endpoints) land alongside the matching backend impl.

---

## Examples

### Minimal — default credential chain

```hcl
queue sqs {
  queue_url = "https://sqs.us-east-1.amazonaws.com/123456789012/iter-signals"
  region    = "us-east-1"
}
```

### FIFO with per-signal MessageGroupId

```hcl
queue sqs {
  queue_name = "iter-signals.fifo"
  account_id = "123456789012"
  region     = "us-east-1"
  fifo       = true

  producer {
    message_group_id         = from_metadata("tenant")
    message_deduplication_id = from_metadata("signal_hash")
  }
}
```

### LocalStack with static credentials

```hcl
queue sqs {
  queue_url    = "http://localhost:4566/000000000000/iter"
  region       = "us-east-1"
  endpoint_url = "http://localhost:4566"

  credentials {
    kind              = "static"
    access_key_id     = "test"
    secret_access_key = "test"
  }
}
```

### EKS IRSA with iter-republish DLQ

```hcl
queue sqs {
  queue_url = "https://sqs.us-west-2.amazonaws.com/1234/work"
  region    = "us-west-2"

  credentials {
    kind       = "web_identity_token_file"
    role_arn   = "arn:aws:iam::1234:role/iter-worker"
    token_file = "/var/run/secrets/eks.amazonaws.com/serviceaccount/token"
  }

  consumer {
    visibility_timeout_secs = 300
    wait_time_seconds       = 20
    max_number_of_messages  = 10
    concurrent_receivers    = 4
  }

  retry {
    mode         = "exponential"
    max_attempts = 5
  }

  dlq {
    kind              = "iter_republish"
    max_receive_count = 5
    target s3 {
      bucket = "iter-poison"
      prefix = "work/"
      region = "us-west-2"
    }
  }
}
```
