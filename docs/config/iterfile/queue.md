# Iterfile: `queue`

Declares the Signal queue backend for the service. Optional — zero or one block per `Iterfile`.

AST: `QueueDecl` in `iter_language/src/ast/queue/mod.rs`.

## Syntax

```hcl
queue <kind> {
  <fields>
}
```

For the simplest kinds (`memory`) the body may be omitted:

```hcl
queue memory
```

## Supported Kinds

| Kind | Category | Details |
| --- | --- | --- |
| `memory` | Local | [`queue-backend/memory.md`](../queue-backend/memory.md) |
| `file` | Local (persistent) | [`queue-backend/file.md`](../queue-backend/file.md) |
| `redis` | Central | [`queue-backend/redis.md`](../queue-backend/redis.md) |
| `shell` | Escape hatch | [`queue-backend/shell.md`](../queue-backend/shell.md) |
| `sqs` | SaaS (AWS) | [`queue-backend/sqs.md`](../queue-backend/sqs.md) |
| `pubsub` | SaaS (GCP) | [`queue-backend/pubsub.md`](../queue-backend/pubsub.md) |
| `kafka` | Distributed | [`queue-backend/kafka.md`](../queue-backend/kafka.md) |
| `kinesis` | SaaS (AWS) | [`queue-backend/kinesis.md`](../queue-backend/kinesis.md) |
| `servicebus` | SaaS (Azure) | [`queue-backend/servicebus.md`](../queue-backend/servicebus.md) |

All backend fields are documented on the per-backend pages. This page covers only the block's role within an `Iterfile`.

## Usage Rules (Iterfile context)

- At most one `queue` block per Iterfile.
- If `runner.behavior = wait`, a queue is **required** — otherwise there is no Signal source. This is enforced by the semantic layer.
- If `runner.behavior = loop { ... }`, a queue is **optional**. When present, real Signals on the queue take precedence over synthesised empty Signals.

## Difference from `compose.iter`

In compose.iter, the same kinds and fields apply, but the block has an additional **name** identifier (`queue <name> <kind> { ... }`) so that multiple services and triggers can refer to it. In an Iterfile, the queue is unnamed — it is the single queue this service consumes from.

See [`compose/queue.md`](../compose/queue.md) for the compose form.

## Examples

### In-process queue (testing)

```hcl
queue memory

runner {
  continue_on_error = false
  behavior          = wait
}
```

### Persistent local queue

```hcl
queue file {
  path = "./.iter/queue"
}
```

### Redis

```hcl
queue redis {
  url = "redis://localhost:6379"
  key = "iter:signals"
}
```

### AWS SQS

```hcl
queue sqs {
  queue_url = "https://sqs.us-east-1.amazonaws.com/123456789012/iter-signals"
  region    = "us-east-1"
}
```

For the full set of fields per backend, see the pages under [`queue-backend/`](../queue-backend/).

## See Also

- [`iterfile/runner.md`](runner.md) — the `wait` vs. `loop` decision that determines whether a queue is required.
- [`queue-backend/`](../queue-backend/) — per-backend details.
