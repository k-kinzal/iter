# compose.iter: `queue`

Declares a **named** queue that services and triggers can refer to. One or more per `compose.iter`.

AST: `NamedQueue` in `iter_language/src/ast/compose.rs`; the backend body reuses `QueueDecl` from `iter_language/src/ast/queue/mod.rs`.

## Syntax

```hcl
queue <name> <kind> {
  <fields>
}
```

`<name>` is a free-form identifier used by `queue = <name>` on services and `target = <name>` on triggers (both are bare identifiers, not quoted strings). `<kind>` is one of the backend kinds documented under [`queue-backend/`](../queue-backend/).

For simple kinds the body may be omitted:

```hcl
queue main memory
```

## Difference from `Iterfile`

In an Iterfile the queue block is unnamed — there is only one queue per Iterfile. In compose, every queue is named because multiple services and triggers bind to it.

| Context | Form | Count |
| --- | --- | --- |
| `Iterfile` | `queue <kind> { ... }` | 0–1 |
| `compose.iter` | `queue <name> <kind> { ... }` | 1–N |

## Supported Kinds

Identical to the Iterfile queue block. Backend-specific fields are documented on the per-backend pages:

| Kind | Page |
| --- | --- |
| `memory` | [`queue-backend/memory.md`](../queue-backend/memory.md) |
| `file` | [`queue-backend/file.md`](../queue-backend/file.md) |
| `redis` | [`queue-backend/redis.md`](../queue-backend/redis.md) |
| `shell` | [`queue-backend/shell.md`](../queue-backend/shell.md) |
| `sqs` | [`queue-backend/sqs.md`](../queue-backend/sqs.md) |
| `pubsub` | [`queue-backend/pubsub.md`](../queue-backend/pubsub.md) |
| `kafka` | [`queue-backend/kafka.md`](../queue-backend/kafka.md) |
| `kinesis` | [`queue-backend/kinesis.md`](../queue-backend/kinesis.md) |
| `servicebus` | [`queue-backend/servicebus.md`](../queue-backend/servicebus.md) |

## Binding Rules

- Services: `queue = <name>`. When there is exactly one queue in the file, the binding may be omitted and the semantic layer auto-resolves it.
- Triggers: `target = <name>`. Same omission rule.
- A reference to an undeclared name is a semantic error.
- A queue that nobody binds to is allowed but produces a warning at load time.

## Examples

### Single queue, auto-bound

```hcl
queue main memory

service worker { build = "./Iterfile" }

trigger nightly cron {
  schedule = "0 3 * * *"
}
```

### Priority lanes

```hcl
queue urgent redis {
  url = "redis://localhost:6379"
  key = "iter:urgent"
}

queue bulk file {
  path = "./.iter/queue-bulk"
}

service responder {
  build = "./Iterfile"
  queue = urgent
}

service housekeeping {
  build = "./Iterfile"
  queue = bulk
}

trigger alerts webhook {
  target = urgent
  port   = 8080
  path   = "/alerts"

  on "*" {}
}

trigger sweep cron {
  target   = bulk
  schedule = "*/15 * * * *"
}
```

## See Also

- [`compose/service.md`](service.md) — how services bind a queue.
- [`compose/trigger.md`](trigger.md) — how triggers publish to a queue.
- [`queue-backend/`](../queue-backend/) — per-backend field reference.
