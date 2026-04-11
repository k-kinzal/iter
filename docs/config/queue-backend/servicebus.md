# Queue Backend: `servicebus`

Azure Service Bus queue or topic + subscription.

AST: `ServiceBusConfig` in `iter_language/src/ast/queue/servicebus.rs`.

## Syntax

```hcl
queue servicebus {
  fully_qualified_namespace = "<namespace>.servicebus.windows.net"

  entity_kind       = "queue" | "subscription"   # Required
  queue_name        = "<name>"    # Required when entity_kind = queue
  topic_name        = "<name>"    # Required when entity_kind = subscription
  subscription_name = "<name>"    # Required when entity_kind = subscription

  transport                   = "amqp_tcp" | "amqp_websockets"   # Optional
  custom_endpoint_address     = "<private endpoint>"             # Optional
  connection_idle_timeout_secs = <int>                           # Optional
  identifier                  = "<client-id>"                    # Optional
  authority_host              = "<sovereign cloud host>"         # Optional

  web_proxy { ... }   # Optional — only with amqp_websockets
  auth      { ... }   # Optional when using default AAD chain
  sender    { ... }
  receiver  { ... }
  session   { ... }   # Required when entity has RequiresSession = true
  retry     { ... }
  dlq       { ... }
}
```

## Top-level Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `fully_qualified_namespace` | `string` | Conditional | — | `<ns>.servicebus.windows.net`. Required unless `auth.kind = "connection_string"`, which embeds the namespace. |
| `entity_kind` | `enum { queue \| subscription }` | Required | — | Kind of entity addressed. |
| `queue_name` | `string` | Conditional | — | Required when `entity_kind = "queue"`. |
| `topic_name` | `string` | Conditional | — | Required when `entity_kind = "subscription"`. |
| `subscription_name` | `string` | Conditional | — | Required when `entity_kind = "subscription"`. |
| `transport` | `enum { amqp_tcp \| amqp_websockets }` | Optional | `amqp_tcp` | AMQP transport. |
| `custom_endpoint_address` | `string` | Optional | — | Private-endpoint host. |
| `connection_idle_timeout_secs` | `integer` | Optional | SDK default | Connection idle timeout. |
| `identifier` | `string` | Optional | SDK default | Client identifier reported to the broker. |
| `authority_host` | `string` | Optional | Azure Public | Sovereign-cloud authority host (US Gov, China, etc.). |

## `web_proxy` block

Only valid with `transport = "amqp_websockets"`.

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `url` | `string` | Required | Proxy URL. |
| `username` | `string` | Optional | Proxy username. |
| `password` | secret | Optional | Proxy password. |

## `auth` block

```hcl
auth {
  kind = "aad_default" | "connection_string" | "shared_access_signature"
       | "aad_client_secret" | "aad_client_certificate"
       | "aad_managed_identity" | "aad_workload_identity"
  # ...per-kind fields
}
```

### `kind = "aad_default"`

No fields. Uses the native chain (Managed Identity → Workload Identity → Az CLI).

### `kind = "connection_string"`

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `value` | secret | Required | Full Service Bus connection string. |

### `kind = "shared_access_signature"`

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `sas_token` | secret | Required | Pre-signed SAS token. |

### `kind = "aad_client_secret"`

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `tenant_id` | `string` (UUID) | Required | Tenant id. |
| `client_id` | `string` (UUID) | Required | Application (client) id. |
| `client_secret` | secret | Required | Client secret. |

### `kind = "aad_client_certificate"`

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `tenant_id` | `string` (UUID) | Required | Tenant id. |
| `client_id` | `string` (UUID) | Required | Application (client) id. |
| `cert_path` | `string` | Required | PEM or PFX certificate path. |
| `cert_password` | secret | Optional | Certificate password. |

### `kind = "aad_managed_identity"`

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `client_id` | `string` | Optional | User-assigned identity id. Omit for system-assigned. |

### `kind = "aad_workload_identity"` (AKS)

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `tenant_id` | `string` (UUID) | Required | Tenant id. |
| `client_id` | `string` (UUID) | Required | Federated application id. |
| `token_file` | `string` | Required | Federated token file path. |

## `sender` block

| Name | Type | Default | Description |
| --- | --- | --- | --- |
| `message_id` | templated string | — | Per-message id. `from_metadata("...")` pulls from Signal metadata. |
| `correlation_id` | templated string | — | Correlation id. |
| `content_type` | `string` | `application/json` | Static content type. |
| `subject` | `string` | — | Static subject. |
| `reply_to` | `string` | — | Reply-to entity name. |
| `reply_to_session_id` | `string` | — | Reply-to session id. |
| `time_to_live_secs` | `integer` | entity default | Per-message TTL. |
| `scheduled_enqueue_time` | `string` (RFC3339) | — | Scheduled enqueue time. |
| `partition_key_strategy` | templated string | `none` | Partition key source. |
| `session_id_strategy` | templated string | `none` | Session id source (required for session-enabled entities). |
| `application_properties` | `map(string → string)` | `{}` | Static overlay. |
| `batch_size` | `integer` | `1` | Batch size cap. |
| `batch_max_bytes` | `integer` | Standard 256 KB / Premium 1 MB | Batch byte cap. |
| `batch_linger_secs` | `integer` | `0` | Max wait before flushing a partial batch. |
| `retry` | block | — | Per-sender retry policy. Shared shape, see [`sqs.md`](sqs.md#retry-and-dlq-common-shape). |

## `receiver` block

| Name | Type | Default | Description |
| --- | --- | --- | --- |
| `receive_mode` | `enum { peek_lock \| receive_and_delete }` | `peek_lock` | Receive mode. |
| `prefetch_count` | `integer` | `0` | Prefetch count. |
| `sub_queue` | `enum { none \| dead_letter \| transfer_dead_letter }` | `none` | Sub-queue selection. Read DLQ contents by setting `dead_letter`. |
| `identifier` | `string` | — | Client identifier. |
| `max_wait_time_secs` | `integer` | SDK default | Max wait per receive batch. |
| `max_messages` | `integer` | `1` | Max messages per receive batch. |
| `max_auto_lock_renewal_duration_secs` | `integer` | SDK default | Auto lock-renewal cap. |
| `on_handler_error` | `enum { abandon \| dead_letter \| defer }` | `abandon` | Handler-error disposition. |
| `dead_letter_reason_template` | `string` | — | DLQ reason template (`{{error.kind}}`, etc.). |
| `dead_letter_description_template` | `string` | — | DLQ description template. |
| `retry` | block | — | Per-receiver retry policy. |

## `session` block

Required when the entity has `RequiresSession = true`.

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `mode` | `enum { accept_specific \| accept_next }` | Required | Session acceptance strategy. |
| `session_id` | `string` | Conditional | Required when `mode = "accept_specific"`. |
| `session_idle_timeout_secs` | `integer` | Optional | Idle timeout before releasing the session. |

## `retry` block

Shared shape. See [`sqs.md`](sqs.md#retry-and-dlq-common-shape).

## `dlq` block

Service Bus has a native DLQ; the typical setting is `kind = "native"`. Read DLQ contents by pointing a separate `queue servicebus { ... }` at the same entity with `receiver.sub_queue = "dead_letter"`. Shared shape in [`sqs.md`](sqs.md#retry-and-dlq-common-shape).

## Examples

### Queue with AAD default

```hcl
queue servicebus {
  fully_qualified_namespace = "my-ns.servicebus.windows.net"
  entity_kind               = "queue"
  queue_name                = "iter-signals"

  auth { kind = "aad_default" }
}
```

### Subscription with session affinity

```hcl
queue servicebus {
  fully_qualified_namespace = "my-ns.servicebus.windows.net"
  entity_kind               = "subscription"
  topic_name                = "orders"
  subscription_name         = "iter-orders"

  auth {
    kind           = "aad_client_secret"
    tenant_id      = "11111111-2222-3333-4444-555555555555"
    client_id      = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
    client_secret  = env("AZURE_CLIENT_SECRET")
  }

  sender {
    session_id_strategy = from_metadata("tenant")
    batch_size          = 50
  }

  session {
    mode                       = "accept_next"
    session_idle_timeout_secs  = 30
  }
}
```

### Reading the DLQ

```hcl
queue servicebus {
  fully_qualified_namespace = "my-ns.servicebus.windows.net"
  entity_kind               = "queue"
  queue_name                = "iter-signals"
  auth { kind = "aad_default" }

  receiver {
    sub_queue = "dead_letter"
  }
}
```
