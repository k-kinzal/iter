# Trigger: `cron`

Emits a Signal on a cron schedule.

AST: `TriggerDef::Cron` in `iter_language/src/ast/trigger.rs`.

Standalone binary: `iter-cron`.

## Syntax

```hcl
trigger <name> cron {
  target = <queue-name>   # optional when there is only one queue

  schedule = "<cron-expression>"

  timezone   = "<IANA zone>"   # Optional
  at_startup = <bool>          # Optional
  catch_up   = <duration>      # Optional, e.g. 300s
  jitter     = <duration>      # Optional, e.g. 5s

  priority = low | normal | high | critical   # Optional
  metadata { ... }                            # Optional
  max_signals = <int>                         # Optional
}
```

## Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `target` | queue ref | Conditional | — | Target queue name (bare identifier). Auto-resolved when the compose file has exactly one queue. |
| `schedule` | string | Required | — | Cron expression. Standard 5-field or 6-field (with seconds) form, depending on the backend; iter does not transform the expression. |
| `timezone` | string | Optional | `UTC` | IANA zone name (e.g. `America/New_York`, `Asia/Tokyo`). |
| `at_startup` | bool | Optional | `false` | Emit one Signal with `metadata.startup = "true"` on start, before entering the schedule. |
| `catch_up` | duration | Optional | disabled | Window for catching up one missed tick on startup. If the most recent missed tick is older than this window, it is dropped. Signals emitted this way carry `metadata.catch_up = "true"`. |
| `jitter` | duration | Optional | `0s` | Maximum random delay added before each tick. Useful for spreading load across fleets. |
| `priority` | `low \| normal \| high \| critical` | Optional | `normal` | Priority assigned to every emitted Signal. |
| `metadata` | block | Optional | `{}` | Metadata copied into every emitted Signal. |
| `max_signals` | integer | Optional | unbounded | Stop after this many Signals. |

## Examples

### Nightly audit

```hcl
trigger nightly cron {
  target   = main
  schedule = "0 3 * * *"
  timezone = "UTC"

  metadata {
    task = "audit"
  }
}
```

### Per-minute health check with catch-up and startup burst

```hcl
trigger healthcheck cron {
  target     = main
  schedule   = "* * * * *"
  at_startup = true
  catch_up   = 300s
  jitter     = 10s
  priority   = low
}
```

### One-shot smoke test

```hcl
trigger smoke cron {
  schedule    = "0 0 1 1 *"
  at_startup  = true
  max_signals = 1
}
```

## Standalone form

`iter-cron` takes the same field set as CLI flags / environment variables and publishes into a queue that another process consumes. Use this for Kubernetes CronJobs, systemd timers, or Docker sidecar patterns where the trigger lives in its own container.

Relevant flags:

- `--schedule "<expr>"`
- `--timezone "<IANA zone>"`
- `--at-startup`
- `--catch-up-window <secs>` (integer seconds; 0 disables)
- `--jitter <secs>` (integer seconds)

## See Also

- [`compose/trigger.md`](../compose/trigger.md) — shared arguments (`priority`, `metadata`, `max_signals`, `target`).
