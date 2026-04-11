# Queue Backend: `pubsub`

Google Cloud Pub/Sub topic + subscription pair.

AST: `PubSubConfig` in `iter_language/src/ast/queue/pubsub.rs`.

## Syntax

```hcl
queue pubsub {
  project      = "<gcp-project>"
  topic        = "<topic-id>"
  subscription = "<subscription-id>"

  endpoint             = "<override>"     # Optional (emulator, regional endpoint)
  user_agent           = "<override>"     # Optional
  connect_timeout_secs = <int>            # Optional
  request_timeout_secs = <int>            # Optional
  quota_project        = "<billing>"      # Optional
  scopes               = ["..."]          # Optional

  keepalive    { ... }   # Optional
  credentials  { ... }   # Optional
  publisher    { ... }   # Optional
  subscriber   { ... }   # Optional
  initial_seek { ... }   # Optional
  dlq          { ... }   # Optional
}
```

## Top-level Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `project` | `string` | Required | — | GCP project hosting the topic and subscription. |
| `topic` | `string` | Required | — | Topic id used by the publisher. |
| `subscription` | `string` | Required | — | Subscription id used by the subscriber. |
| `endpoint` | `string` | Optional | GCP default | Regional endpoint or `PUBSUB_EMULATOR_HOST`. |
| `user_agent` | `string` | Optional | iter default | Override the User-Agent. |
| `connect_timeout_secs` | `integer` | Optional | SDK default | Connection timeout. |
| `request_timeout_secs` | `integer` | Optional | SDK default | Per-request timeout. |
| `quota_project` | `string` | Optional | — | GCP project billed for API calls. |
| `scopes` | `list(string)` | Optional | `["https://www.googleapis.com/auth/pubsub"]` | OAuth scopes. |

## `keepalive` block

gRPC channel keepalive parameters.

| Name | Type | Description |
| --- | --- | --- |
| `time_secs` | `integer` | Idle interval before sending a keepalive ping. |
| `timeout_secs` | `integer` | Ack deadline for the ping. |
| `permit_without_stream` | `bool` | Allow keepalives on idle channels. |

## `credentials` block

```hcl
credentials {
  kind = "adc" | "service_account_file" | "service_account_inline"
       | "workload_identity" | "impersonate" | "access_token"
  # ...per-kind fields
}
```

### `kind = "adc"`

No fields. Explicit form of "use Application Default Credentials".

### `kind = "service_account_file"`

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `path` | `string` | Required | Path to the service-account JSON. |

### `kind = "service_account_inline"`

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `json` | secret | Required | Service-account JSON. Typically `env("GCP_SERVICE_ACCOUNT_JSON")`. |

### `kind = "workload_identity"`

External-account / Workload Identity Federation.

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `audience` | `string` | Required | Audience the IdP token is minted for. |
| `token_file` | `string` | Required | IdP token path. |
| `impersonation_target` | `string` | Optional | Service account to impersonate after federation. |

### `kind = "impersonate"`

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `target_principal` | `string` | Required | Final principal to impersonate. |
| `delegates` | `list(string)` | Optional | Intermediate principals. |
| `scopes` | `list(string)` | Optional | OAuth scopes. |

### `kind = "access_token"`

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `token` | secret | Required | Bearer token. |
| `expiry` | `string` (RFC3339) | Optional | Token expiry. |

## `publisher` block

| Name | Type | Default | Description |
| --- | --- | --- | --- |
| `delay_threshold_ms` | `integer` | SDK default | Flush delay in milliseconds. |
| `count_threshold` | `integer` | SDK default | Flush after N messages. |
| `byte_threshold` | `integer` | SDK default | Flush after N bytes. |
| `max_outstanding_messages` | `integer` | SDK default | Backpressure cap (message count). |
| `max_outstanding_bytes` | `integer` | SDK default | Backpressure cap (bytes). |
| `limit_exceeded_behavior` | `enum { block \| error }` | `block` | Behaviour when the cap is hit. |
| `workers` | `integer` | SDK default | Worker thread count. |
| `request_timeout_secs` | `integer` | SDK default | Per-publish RPC timeout. |
| `retry` | block | — | Retry policy for publish RPCs. See below. |
| `enable_compression` | `bool` | `false` | Enable gRPC compression. |
| `compression_bytes_threshold` | `integer` | SDK default | Minimum payload size to compress. |
| `attributes` | `map(string → string)` | `{}` | Static attribute overlay. |
| `ordering_key_strategy` | templated string | — | Ordering key source, e.g. `from_metadata("tenant")`. |

## `subscriber` block

| Name | Type | Default | Description |
| --- | --- | --- | --- |
| `pull_mode` | `enum { streaming \| sync }` | `streaming` | Pull strategy. |
| `stream_ack_deadline_seconds` | `integer` (10–600) | `60` | Streaming-only: extended ack deadline. |
| `max_outstanding_messages` | `integer` | SDK default | Streaming-only: backpressure cap (messages). |
| `max_outstanding_bytes` | `integer` | SDK default | Streaming-only: backpressure cap (bytes). |
| `min_duration_per_lease_extension_secs` | `integer` | SDK default | Streaming-only: min lease-extension interval. |
| `max_duration_per_lease_extension_secs` | `integer` | SDK default | Streaming-only: max lease-extension interval. |
| `ping_interval_secs` | `integer` | SDK default | Streaming-only: keepalive ping interval. |
| `max_messages` | `integer` (≤ 1000) | — | Sync-only: max messages per pull. |
| `return_immediately` | `bool` | `false` | Sync-only: return immediately on empty pull. |
| `retry` | block | — | Retry policy for receive RPCs. |

## `initial_seek` block

Idempotent seek operation applied on startup.

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `kind` | `enum { timestamp \| snapshot }` | Required | Seek target. |
| `timestamp` | `string` (RFC3339) | Conditional | Required when `kind = "timestamp"`. |
| `snapshot_name` | `string` | Conditional | Required when `kind = "snapshot"`. |

## `retry` block

Shared shape. See [`sqs.md` § Retry and DLQ](sqs.md#retry-and-dlq-common-shape).

## `dlq` block

Typically `kind = "native"` (configured outside iter on the subscription itself). Shared shape documented in [`sqs.md`](sqs.md#retry-and-dlq-common-shape).

## Examples

### Minimal — ADC

```hcl
queue pubsub {
  project      = "my-project"
  topic        = "iter-signals"
  subscription = "iter-signals-sub"
}
```

### Emulator

```hcl
queue pubsub {
  project      = "demo"
  topic        = "signals"
  subscription = "signals-sub"
  endpoint     = "localhost:8085"
}
```

### Ordered publish via metadata

```hcl
queue pubsub {
  project      = "my-project"
  topic        = "orders"
  subscription = "iter-orders"

  publisher {
    ordering_key_strategy = from_metadata("tenant")
  }

  subscriber {
    pull_mode                   = "streaming"
    stream_ack_deadline_seconds = 120
    max_outstanding_messages    = 1000
  }
}
```
