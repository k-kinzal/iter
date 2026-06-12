# Queue Backend: `shell`

Escape-hatch backend: iter delegates enqueue and dequeue to user-provided shell commands. Use this to wrap any queue system iter does not ship first-class for.

AST: `QueueDef::Shell` in `iter_language/src/ast/queue/mod.rs`.

## Syntax

```hcl
queue shell {
  enqueue              = "<enqueue command>"
  dequeue              = "<dequeue command>"
  close                = "<cleanup command>"   # optional
  interpreter          = "<interpreter argv>"  # optional
  enqueue_timeout_secs = <int>                 # optional
}
```

## Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `enqueue` | `string` | Required | — | Command run once per `queue()` call. Receives the full Signal JSON on stdin and `ITER_SIGNAL_ID` / `ITER_SIGNAL_PRIORITY` / `ITER_SIGNAL_PRIORITY_NAME` in the environment. |
| `dequeue` | `string` | Required | — | Long-lived command. Respawned after exit until the queue is closed. Emits NDJSON Signal records on stdout, one per line. |
| `close` | `string` | Optional | — | Cleanup command run once when the queue is closed. |
| `interpreter` | `string` | Optional | `sh -c` | Interpreter invocation. Must accept a single trailing argument containing the script. Named `interpreter` (not `shell`) because `shell` is reserved by the event-handler DSL. |
| `enqueue_timeout_secs` | `integer` | Optional | `30` | Per-enqueue timeout. iter sends `SIGTERM` on timeout, then force-kills after a grace period. |

## NDJSON Protocol

Each line on the dequeue command's stdout is one Signal. Two accepted shapes:

```jsonc
// Full Signal
{"id": "abc-123", "priority": "normal", "metadata": {"task": "lint"}}

// Abbreviated — iter generates a fresh id
{"priority": "high", "metadata": {"source": "external"}}
```

Lines that fail to parse as JSON are logged and skipped; the command is not killed.

## Use Cases

- Wrapping an unsupported queue (e.g., NATS, NSQ, a homegrown system).
- Driving iter from stdin for quick prototyping.
- Bridging an existing message bus without embedding its SDK in iter.

## Caveats

- **No automatic retries.** If the dequeue command exits mid-Signal, that Signal is lost.
- The enqueue command must accept and fully consume stdin within `enqueue_timeout_secs`.
- `dequeue` is a long-lived process. Ensure it flushes stdout line-by-line (`stdbuf -oL` on Linux, unbuffered Python, etc.).

## Examples

### Connect to an external NDJSON line feed

```hcl
queue shell {
  enqueue = "cat >> /var/run/my-queue.ndjson"
  dequeue = "tail -n0 -F /var/run/my-queue.ndjson"
  close   = "rm -f /var/run/my-queue.ndjson"
}
```

### Wrap a custom CLI

```hcl
queue shell {
  enqueue              = "myq push"
  dequeue              = "myq subscribe --ndjson"
  close                = "myq disconnect"
  interpreter          = "/usr/bin/bash -c"
  enqueue_timeout_secs = 10
}
```
