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
  cooldown = 5s
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

## See Also

- [`trigger/`](../trigger/) — per-kind field reference.
- [`compose/queue.md`](queue.md) — the queues triggers publish into.
- [`compose/service.md`](service.md) — the services that consume from those queues.
