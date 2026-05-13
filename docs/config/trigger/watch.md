# Trigger: `watch`

Emits a Signal on filesystem changes inside a directory.

AST: `TriggerDecl::Watch` in `iter_language/src/ast/trigger.rs`.

Standalone binary: `iter-watch`.

## Syntax

```hcl
trigger <name> watch {
  target = <queue-name>   # optional when there is only one queue

  dir      = "<path>"
  include  = ["<glob>", ...]
  exclude  = ["<glob>", ...]
  per_file = <bool>
  interval = <duration>   # Optional, e.g. 5s

  priority = low | normal | high | critical   # Optional
  metadata { ... }                            # Optional
  max_signals = <int>                         # Optional
}
```

## Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `target` | queue ref | Conditional | — | Target queue (bare identifier). |
| `dir` | string | Required | — | Directory to monitor. Resolved relative to the compose file. Watched recursively. |
| `include` | `list(string)` | Optional | `[]` | gitignore-style globs evaluated against the changed file's path *relative* to `dir`. `**` traverses directories. Empty means "watch everything". |
| `exclude` | `list(string)` | Optional | `[]` | gitignore-style globs evaluated against the same relative path. A match here unconditionally rejects the event (wins over `include`). |
| `per_file` | bool | Optional | `false` | `true` → one Signal per changed file (when no `interval` is set). `false` → one Signal per batch of changes (merged by `interval`). |
| `interval` | duration | Optional | _see notes_ | Publish interval. After the first matching event, collect all changes for this duration, then emit one merged Signal. No events are suppressed — all observed changes are preserved in signal metadata. With `per_file = true` and no interval, every event fires its own Signal immediately. With `per_file = false` and no interval, the library uses an internal default of 250&nbsp;ms. The standalone `iter-watch` CLI defaults `--interval` to `2` seconds; passing `--interval 0` disables the interval. `cooldown` is accepted as a deprecated alias for `interval`; using both is a validation error. |
| `priority` | `low \| normal \| high \| critical` | Optional | `normal` | Signal priority. |
| `metadata` | block | Optional | `{}` | Metadata applied to every emitted Signal. Per-event placeholders expose `{{path}}`, `{{kind}}`, and `{{timestamp}}`. |
| `max_signals` | integer | Optional | unbounded | Stop after this many Signals. |

## Signal metadata

When `per_file = true` (no interval) the emitted Signal carries:

| Key | Description |
| --- | --- |
| `path` | Absolute path of the changed file. |
| `kind` | Event kind: `created`, `modified`, `removed`. |
| `timestamp` | RFC3339 event timestamp. |

When events are merged (either `per_file = false`, or any mode with an `interval`), the Signal carries:

| Key | Description |
| --- | --- |
| `files` | JSON array of unique changed paths, preserving first-seen order. |
| `events` | JSON array of objects, each with `path`, `kind`, and `timestamp`, preserving event order. Repeated changes to the same path appear as separate entries. |
| `changed_count` | Number of unique files changed (integer). |
| `event_count` | Total number of events observed in the interval (integer). |

## Examples

### Per-file, no interval

```hcl
trigger on_source_change watch {
  target   = main
  dir      = "./src"
  include  = ["**/*.rs"]
  per_file = true

  metadata {
    source = "watch"
    path   = "{{path}}"
  }
}
```

### Batched with a 5-second interval

```hcl
trigger on_docs_change watch {
  target   = main
  dir      = "./docs"
  include  = ["**/*.md"]
  per_file = false
  interval = 5s
  priority = low
}
```

### Per-file with exclude and a 30-minute interval

A common shape for monitoring agent session logs while ignoring writes the
runner itself emits underneath the watched root. Events within each 30-minute
window are merged into a single Signal with full event detail.

```hcl
trigger watch_sessions watch {
  target   = main
  dir      = "/Users/me/Library/Logs/Agents"
  include  = ["**/*.jsonl"]
  exclude  = ["self-loop/**"]
  per_file = true
  interval = 1800s
}
```

## Standalone form

`iter-watch` exposes the same fields as CLI flags / environment variables. It is useful for running the watcher in its own container next to a pool of Runner containers. `--include` and `--exclude` are repeatable.

## See Also

- [`compose/trigger.md`](../compose/trigger.md) — shared arguments.
