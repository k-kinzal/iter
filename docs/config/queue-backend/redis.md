# Queue Backend: `redis`

Central queue backed by a Redis list. Suitable for multi-host deployments where all workers can reach the same Redis endpoint.

AST: `QueueDef::Redis` in `iter_language/src/ast/queue/mod.rs`.

## Syntax

```hcl
queue redis {
  url = "<redis-url>"
  key = "<namespace-key>"
}
```

## Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `url` | `string` | Required | — | Redis connection URL. Accepts the full Redis URL grammar (`redis://`, `rediss://`, auth, port, db). |
| `key` | `string` | Required | — | Redis list key that backs this queue. Acts as the project-shaped namespace. No default — multiple projects sharing a Redis must use distinct keys, and iter will not guess a safe one. |

Authentication and TLS flow through the URL:

```hcl
queue redis {
  url = "rediss://user:pass@redis.example.com:6380/0"
  key = "iter:prod:signals"
}
```

## Semantics

- Priority ordering is honoured via Redis sorted-set coordination behind the list key.
- A single Redis instance can host multiple iter queues by distinguishing `key`.
- Cluster / sentinel topologies are supported via the URL.

## Use Cases

- Multi-host fleets of Runners sharing one work pool.
- Deployments that already run Redis for other reasons.
- Fan-in from several standalone trigger binaries into shared workers.

## Caveats

- **Shared infrastructure.** A slow consumer on one key can starve others only to the extent Redis itself is saturated — size your Redis accordingly.
- No native dead-letter routing. Use the `retry` / `dlq` policy features (on the backends that support them) when poison-message handling matters.

## Examples

### Iterfile (unnamed)

```hcl
queue redis {
  url = "redis://localhost:6379"
  key = "iter:signals"
}
```

### compose.iter (named, TLS)

```hcl
queue prod redis {
  url = "rediss://:password@redis.internal:6380/0"
  key = "iter:prod:signals"
}

service worker {
  build = "./Iterfile"
  queue = "prod"
}
```
