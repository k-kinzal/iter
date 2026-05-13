# Queue Backend: `file`

Persistent single-host queue backed by a filesystem path. Survives process restarts and can be shared by multiple processes on the same host.

AST: `QueueDecl::File` in `iter_language/src/ast/queue/mod.rs`.

## Syntax

```hcl
queue file {
  path = "<filesystem-path>"
}
```

## Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `path` | `string` | Required | — | Filesystem path to the queue database. Created on first enqueue. Relative paths resolve against the directory containing the `Iterfile` / `compose.iter`. No default — iter does not guess where on a given project this file should live. |

## Semantics

- Priority-ordered FIFO within each priority bucket.
- Durable: a crashed process restarts and resumes from the on-disk state.
- Multi-process-safe on a single host via file locks. Not safe across hosts or network filesystems that do not implement `flock` correctly.

## Use Cases

- Single-host deployments where losing in-flight Signals is unacceptable.
- Running `iter run` alongside standalone `iter-watch` / `iter-cron` / `iter-files` producers on the same machine.
- Local development that needs persistence between sessions.

## Caveats

- **Single-host only.** For multi-host fan-in use [`redis`](redis.md) or a SaaS backend.
- Back up the directory containing `path` if the Signal history matters.

## Examples

### Iterfile (unnamed)

```hcl
queue file {
  path = "./.iter/queue"
}
```

### compose.iter (named)

```hcl
queue work file {
  path = "/var/lib/iter/work.db"
}

service worker {
  build = "./Iterfile"
  queue = work
}

trigger changes watch {
  target   = work
  dir      = "./src"
  include  = ["**/*.rs"]
  per_file = false
  interval = 5s
}
```
