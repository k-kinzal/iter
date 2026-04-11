# Queue Backend: `kinesis`

AWS Kinesis Data Streams. Reuses the AWS credential and HTTP-client surface from [`sqs`](sqs.md).

AST: `KinesisConfig` in `iter_language/src/ast/queue/kinesis.rs`.

## Syntax

```hcl
queue kinesis {
  # Identity — one of stream_arn OR stream_name is required.
  stream_arn  = "arn:aws:kinesis:..."
  # -- or --
  stream_name = "<name>"

  region       = "<aws-region>"      # Required
  endpoint_url = "<override URL>"    # Optional (LocalStack, Kinesalite)

  credentials { ... }   # Optional — same shape as sqs
  http_client { ... }   # Optional — same shape as sqs
  producer    { ... }
  consumer    { ... }
  checkpoint  { ... }   # Required for stable consumption
  retry       { ... }   # Optional
  dlq         { ... }   # Optional (iter-implemented)
}
```

## Top-level Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `stream_arn` | `string` | Conditional | — | Stream ARN. Preferred; mutually exclusive with `stream_name`. |
| `stream_name` | `string` | Conditional | — | Plain stream name; resolved against the configured region/account. |
| `region` | `string` | Required | — | AWS region. |
| `endpoint_url` | `string` | Optional | — | Override endpoint (LocalStack / Kinesalite). |

## `credentials` and `http_client` blocks

Identical shape to the SQS backend — see [`sqs.md`](sqs.md#credentials-block) and [`sqs.md`](sqs.md#http_client-block).

## `producer` block

| Name | Type | Default | Description |
| --- | --- | --- | --- |
| `partition_key_strategy` | templated string | `random` | `explicit`, `random`, or `from_metadata("key")`. |
| `explicit_hash_key` | `string` | — | Per-message explicit hash key escape hatch. |
| `ordering` | `enum { none \| strict_per_key }` | `none` | `strict_per_key` chains `SequenceNumberForOrdering` across puts. |
| `batch_size` | `integer` (1–500) | `1` | `PutRecords` batch size. |
| `batch_max_bytes` | `integer` (≤ 5 MiB) | AWS limit | Per-batch byte cap. |
| `batch_linger_secs` | `integer` | `0` | Max wait before flushing a partial batch. |
| `aggregation` | `bool` | `false` | Enable iter-implemented KPL-style aggregation. |

## `consumer` block

| Name | Type | Default | Description |
| --- | --- | --- | --- |
| `consumer_mode` | `enum { polling \| enhanced_fan_out }` | `polling` | Receive mode. |
| `iterator_type` | `string` | — | Polling iterator type or EFO starting position (`LATEST`, `TRIM_HORIZON`, `AT_SEQUENCE_NUMBER`, `AFTER_SEQUENCE_NUMBER`, `AT_TIMESTAMP`). |
| `starting_sequence_number` | `string` | — | Required for `AT_SEQUENCE_NUMBER` / `AFTER_SEQUENCE_NUMBER`. |
| `starting_timestamp` | `string` (RFC3339 or epoch) | — | Required for `AT_TIMESTAMP`. |
| `fetch_max_records` | `integer` | AWS default | Polling: max records per `GetRecords`. |
| `poll_interval_ms` | `integer` (≥ 200) | AWS default | Polling interval. |
| `consumer_arn` | `string` | — | EFO: pre-existing consumer ARN. |
| `consumer_name` | `string` | — | EFO: registered consumer name (alternate to `consumer_arn`). |
| `shard_discovery_interval_secs` | `integer` | iter default | `ListShards` cadence. |
| `shard_id_filter` | `list(string)` | — | Client-side filter on shard ids. |
| `shard_list_filter` | block | — | Server-side `ShardFilter`. See below. |

### `shard_list_filter` sub-block

| Name | Type | Description |
| --- | --- | --- |
| `kind` | `string` | Filter type, e.g. `AT_LATEST`, `FROM_TRIM_HORIZON`, `AT_TIMESTAMP`, `AT_SHARD_ID`. |
| `shard_id` | `string` | Shard-id anchor (for shard-relative filters). |
| `timestamp` | `string` | Timestamp anchor (for time-based filters). |

## `checkpoint` block

Required for stable consumption. `memory` is acceptable only for tests.

| Name | Type | Required | Description |
| --- | --- | :---: | --- |
| `store` | `enum { dynamodb \| file \| memory }` | Required | Checkpoint backend. |
| `table_name` | `string` | Conditional | Required when `store = "dynamodb"`. |
| `region` | `string` | Optional | DynamoDB region override. Defaults to the stream's region. |
| `endpoint_url` | `string` | Optional | DynamoDB endpoint override (LocalStack). |
| `path` | `string` | Conditional | Required when `store = "file"`. |
| `interval_secs` | `integer` | Optional | Checkpoint flush interval. |
| `lease_duration_secs` | `integer` | Optional | Lease duration for multi-worker leasing. |

## `retry` block

Shared shape. See [`sqs.md`](sqs.md#retry-and-dlq-common-shape).

## `dlq` block

Kinesis has no native DLQ; use `kind = "iter_republish"`. Shared shape in [`sqs.md`](sqs.md#retry-and-dlq-common-shape).

## Examples

### Minimal — polling with DynamoDB checkpoints

```hcl
queue kinesis {
  stream_arn = "arn:aws:kinesis:us-east-1:1234:stream/iter-signals"
  region     = "us-east-1"

  consumer {
    consumer_mode = "polling"
    iterator_type = "LATEST"
  }

  checkpoint {
    store      = "dynamodb"
    table_name = "iter-kinesis-checkpoints"
  }
}
```

### Enhanced Fan-Out with strict ordering

```hcl
queue kinesis {
  stream_arn = "arn:aws:kinesis:us-east-1:1234:stream/orders"
  region     = "us-east-1"

  producer {
    partition_key_strategy = from_metadata("tenant")
    ordering               = "strict_per_key"
    batch_size             = 250
    batch_linger_secs      = 1
  }

  consumer {
    consumer_mode = "enhanced_fan_out"
    consumer_arn  = "arn:aws:kinesis:us-east-1:1234:stream/orders/consumer/iter:1700000000"
    iterator_type = "LATEST"
  }

  checkpoint {
    store               = "dynamodb"
    table_name          = "iter-orders-checkpoints"
    lease_duration_secs = 30
  }
}
```
