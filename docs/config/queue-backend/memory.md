# Queue Backend: `memory`

In-process priority queue. Signals live only in the owning process; nothing is persisted.

AST: `QueueDef::Memory` in `iter_language/src/ast/queue/mod.rs`.

## Syntax

```hcl
queue memory
```

The body may be omitted entirely. An empty block is also accepted:

```hcl
queue memory {}
```

## Arguments

None. Memory has no configurable fields.

## Use Cases

- Unit tests and examples.
- Single-process `iter run` with a one-off enqueue.
- Inline compose where every producer and consumer is in the same process.

## Caveats

- **No persistence.** Process crash loses every in-flight Signal.
- **No cross-process sharing.** Triggers in standalone binaries (`iter-cron`, etc.) cannot publish into a memory queue of a separate Runner process.
- Unbounded in size. Producers that outrun consumers grow the heap.

For anything beyond a single short-lived process, use [`file`](file.md), [`redis`](redis.md), or one of the SaaS backends.

## Examples

### Iterfile (unnamed)

```hcl
queue memory

runner {
  continue_on_error = false
  behavior          = wait
}
```

### compose.iter (named)

```hcl
queue main memory

service worker { build = "./Iterfile" }

trigger once cron {
  schedule    = "0 0 1 1 *"
  at_startup  = true
  max_signals = 1
}
```
