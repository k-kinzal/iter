# compose.iter: `trigger`

Declares a named Signal producer that publishes into a queue. Zero or more per `compose.iter`.

AST: `NamedTrigger` in `iter_language/src/ast/compose.rs`; the body reuses `TriggerDecl` from `iter_language/src/ast/trigger.rs`.

## Syntax

```hcl
trigger <name> <kind> {
  target = <queue-name>   # bare identifier; optional when there is only one queue
  <kind-specific fields>
}
```

`<name>` is a free-form identifier. `<kind>` selects the signal-producing strategy.

## Shared Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `target` | queue ref | Conditional | — | Name of the queue that receives emitted Signals. Optional when the file declares exactly one queue. Required otherwise. The value is a bare identifier (`target = main`), not a quoted string. |
| `metadata` | block `{ key = "value" ... }` | Optional | `{}` | Metadata template fields copied into every emitted Signal. Values are strings (with `{{...}}` placeholders resolved at emit time). Written as a nested block, not an assignment. |
| `priority` | `low \| normal \| high \| critical` | Optional | `normal` | Priority assigned to every emitted Signal. Individual webhook routes may override this; a webhook trigger's `metadata` and `priority` are merged into routes that do not set their own. |
| `max_signals` | integer | Optional | unbounded | Stop the trigger after this many Signals have been emitted. Useful for smoke tests. |

Kind-specific fields live on the per-kind page.

## Supported Kinds

| Kind | Purpose | Standalone binary | Page |
| --- | --- | --- | --- |
| `cron` | Emit on a cron schedule. | `iter-cron` | [`trigger/cron.md`](../trigger/cron.md) |
| `watch` | Emit on filesystem changes. | `iter-watch` | [`trigger/watch.md`](../trigger/watch.md) |
| `files` | Drain file-path lists (stdin or files). | `iter-files` | [`trigger/files.md`](../trigger/files.md) |
| `command` | Poll an external command's output. | `iter-command` | [`trigger/command.md`](../trigger/command.md) |
| `webhook` | Serve an HTTP listener with per-event routes. | `iter-webhook` | [`trigger/webhook.md`](../trigger/webhook.md) |
| `<user-defined>` | Arbitrary external kind (fields preserved verbatim). | — | [`trigger/external.md`](../trigger/external.md) |

There is no `loop` trigger kind. Continuous iteration lives on the runner (`runner.behavior = loop { ... }`) instead.

## Examples

### Schedule

```hcl
queue main memory

service worker { build = "./Iterfile" }

trigger nightly cron {
  target   = main
  schedule = "0 3 * * *"
  timezone = "UTC"
}
```

### Filesystem watch

```hcl
trigger on_source_change watch {
  target   = main
  dir      = "./src"
  include  = ["**/*.rs"]
  per_file = false
  interval = 5s
}
```

### Command polling with extraction

```hcl
trigger build_status command {
  target   = main
  run      = "kubectl rollout status deployment/app --timeout=0s"
  poll     = 30s
  extract  = regex("status: (.+)")
  dedupe   = true
  on_error = continue
}
```

### Webhook with per-event routing

```hcl
trigger github webhook {
  target = main
  port   = 8080
  path   = "/webhook/github"
  secret = env("GITHUB_WEBHOOK_SECRET")

  priority = normal
  metadata {
    trigger = "github"
  }

  on "issues.opened" {
    metadata {
      source = "github"
      repo   = "{{payload.repository.full_name}}"
      issue  = "{{payload.issue.number}}"
    }
  }

  on "security_advisory" {
    priority = critical
    metadata {
      task = "security"
    }
  }
}
```

## Supervision

Compose-managed triggers run under a supervisor that automatically restarts
them when they exit unexpectedly or return a runtime error. This keeps
long-running triggers (`cron`, `watch`, `command`, `webhook`) alive for the
lifetime of the orchestrator without manual intervention.

### Lifecycle States

| State | Meaning |
| --- | --- |
| `Starting` | Initial state before the trigger's first run. |
| `Running` | The trigger is actively executing. |
| `Restarting` | The trigger exited and the supervisor is waiting (backoff) before relaunching. |
| `Completed` | A finite trigger finished normally. No restart. |
| `Failed` | A build-time error prevented the trigger from starting. No retry. |
| `Stopped` | The orchestrator was shut down (e.g. Ctrl-C). |

### Restart Policy

- **Runtime errors** and **unexpected exits** are retried with exponential
  backoff (1 s base, 60 s cap).
- **Build errors** (`TriggerRunError::Build`) are *not* retried because they
  indicate a configuration problem that will not resolve on its own.
- **Finite triggers** — currently only `files` without `no_exit_on_eof` —
  may complete normally (`Completed`) without triggering a restart. If the
  trigger has `terminate_on_completion` set, a terminate signal is enqueued
  on the target queue after completion.
- Cancellation (orchestrator shutdown) moves the trigger to `Stopped`
  regardless of its current state.

### State Persistence

The supervisor writes a `status.json` file on every lifecycle transition to:

```
~/.iter/trigger-state/<project>/<trigger_name>/status.json
```

This file is a JSON object with fields: `name`, `state`, `kind`,
`restart_count`, `last_error`, `last_state_change`, and `is_finite`.
`iter compose ps` reads these files to report trigger health.

Individual trigger kinds may persist their own restart-sensitive state in the
same directory:

- **`files`** — persists a byte-offset cursor (`cursor.json`) so a
  restarted trigger resumes from the last successfully emitted line instead
  of re-reading from the beginning. Delivery is at-least-once: a crash
  between signal enqueue and cursor save may cause one line to be re-emitted.
- **`watch`** — persists the pending change batch (`pending_batch.json`)
  before flushing, so in-flight batches are recovered and re-flushed on
  restart rather than lost.

## See Also

- [`trigger/`](../trigger/) — per-kind field reference.
- [`compose/queue.md`](queue.md) — the queues triggers publish into.
- [`compose/service.md`](service.md) — the services that consume from those queues.
