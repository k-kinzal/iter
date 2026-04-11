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
  cooldown = <duration>   # Optional, e.g. 5s

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
| `per_file` | bool | Optional | `false` | `true` → one Signal per changed file. `false` → one Signal per batch of changes (coalesced by `cooldown`). |
| `cooldown` | duration | Optional | _see notes_ | With `per_file = true`, suppresses repeat events on the same path inside the window (per-path debounce); when omitted, every event fires immediately. With `per_file = false`, the trigger collects events for a fixed window of this length, starting from the first event in the batch, then emits one Signal; when omitted, the library uses an internal default of 250&nbsp;ms. The standalone `iter-watch` CLI defaults `--cooldown` to `2` seconds; passing `--cooldown 0` only disables per-path debounce when `--per-file` is set — in batched mode the library still applies its 250&nbsp;ms internal default. |
| `priority` | `low \| normal \| high \| critical` | Optional | `normal` | Signal priority. |
| `metadata` | block | Optional | `{}` | Metadata applied to every emitted Signal. Per-event placeholders expose `{{path}}`, `{{kind}}`, and `{{timestamp}}`. |
| `max_signals` | integer | Optional | unbounded | Stop after this many Signals. |

## Signal metadata

When `per_file = true` the emitted Signal carries:

| Key | Description |
| --- | --- |
| `path` | Absolute path of the changed file. |
| `kind` | Event kind: `created`, `modified`, `removed`. |
| `timestamp` | RFC3339 event timestamp. |

When `per_file = false`, iter emits a Signal whose `files` metadata key holds a JSON array of changed paths.

## Examples

### Per-file, no coalescing

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

### Batched with a 5-second cooldown

```hcl
trigger on_docs_change watch {
  target   = main
  dir      = "./docs"
  include  = ["**/*.md"]
  per_file = false
  cooldown = 5s
  priority = low
}
```

### Per-file with exclude and a 30-minute per-path debounce

A common shape for monitoring agent session logs while ignoring writes the
runner itself emits underneath the watched root.

```hcl
trigger watch_sessions watch {
  target   = main
  dir      = "/Users/me/Library/Logs/Agents"
  include  = ["**/*.jsonl"]
  exclude  = ["self-loop/**"]
  per_file = true
  cooldown = 1800s
}
```

## Standalone form

`iter-watch` exposes the same fields as CLI flags / environment variables. It is useful for running the watcher in its own container next to a pool of Runner containers. `--include` and `--exclude` are repeatable.

## See Also

- [`compose/trigger.md`](../compose/trigger.md) — shared arguments.
