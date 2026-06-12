# Trigger: `command`

Runs an external command on a polling interval and emits Signals from its output.

AST: `TriggerDef::Command`, `ExtractExpr`, and `OnErrorKeyword` in `iter_language/src/ast/trigger.rs`.

Standalone binary: `iter-command`.

## Syntax

```hcl
trigger <name> command {
  target = <queue-name>   # optional when there is only one queue

  run     = "<command>"
  shell   = "<interpreter>"                      # Optional, defaults to `sh -c`
  extract = regex("<pattern>")                   # Optional
  poll    = <duration>                           # Optional, e.g. 30s
  dedupe  = <bool>                               # Optional
  on_error = continue | abort | skip             # Optional

  priority = low | normal | high | critical      # Optional
  metadata { ... }                               # Optional
  max_signals = <int>                            # Optional
}
```

## Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `target` | queue ref | Conditional | — | Target queue (bare identifier). |
| `run` | string | Required | — | Command to execute. Passed to the interpreter as a single argument. |
| `shell` | string | Optional | `sh -c` | Interpreter invocation. Must accept a single trailing argument containing the script. Example: `"bash -c"`. |
| `extract` | `regex("...")` | Optional | one Signal per stdout line | Extraction applied to the command's stdout. `regex` runs the pattern line-by-line and uses capture group 1 (or the full match when no group) as the Signal value. If the command emits a JSON array on stdout, iter flattens it into one Signal per element automatically (no `extract` needed). |
| `poll` | duration | Optional | `60s` | Polling interval. The command is re-run after this duration. |
| `dedupe` | bool | Optional | `false` | Skip records already seen in an earlier poll. The dedupe key is the extracted value. |
| `on_error` | `continue \| abort \| skip` | Optional | `continue` | Behaviour when the command exits non-zero. `continue` logs a warning and retries on the next tick; `abort` stops the trigger with an error; `skip` silently swallows the error and continues without emitting. |
| `priority` | `low \| normal \| high \| critical` | Optional | `normal` | Signal priority. |
| `metadata` | block | Optional | `{}` | Metadata applied to every emitted Signal. `{{value}}` refers to the extracted value. |
| `max_signals` | integer | Optional | unbounded | Stop after this many Signals. |

## Extraction modes

### `regex("<pattern>")`

Applies the pattern line-by-line using Rust's `regex` crate syntax.

- Capture group 1 (if present) is the Signal value.
- Named captures (`(?P<name>...)`) become metadata keys on the Signal.
- Otherwise the full match is used.
- Lines that do not match are skipped silently.

### No `extract` — line / JSON array auto-detect

When `extract` is omitted, iter inspects stdout:

- If the whole output parses as a JSON array, iter flattens it — one Signal
  per element. Scalars go into `metadata.value`; objects are merged into
  metadata verbatim (keys must be strings).
- Otherwise each non-blank stdout line is one Signal, stored in
  `metadata.value`.

### Migration from `extract = jq(...)`

Earlier versions accepted `extract = jq("<query>")` and shelled out to the
external `jq` binary. iter no longer ships that path — core has no
hard dependency on `jq`.

Replace it by moving the JSON shaping into `run` (which is already passed
to a shell) and emitting a JSON array, then leaving `extract` unset:

```hcl
# Before
trigger issues command {
  run     = "gh issue list --state=open --json number,title"
  extract = jq(".[] | {number, title}")
}

# After — pipe through jq inside the command itself
trigger issues command {
  run = "gh issue list --state=open --json number,title | jq -c '[.[] | {number, title}]'"
  # extract omitted: the JSON array is flattened automatically
}
```

If you only need a substring of each line, prefer `extract = regex("...")`
with a named capture and skip `jq` entirely.

## Examples

### Poll a build status every 30s

```hcl
trigger build command {
  target   = main
  run      = "kubectl rollout status deployment/app --timeout=0s"
  poll     = 30s
  extract  = regex("status: (.+)")
  dedupe   = true
  on_error = continue
}
```

### Emit one Signal per open GitHub issue

```hcl
trigger issues command {
  target = main
  # Reshape inside the command and emit a JSON array; no `extract` needed.
  run    = "gh issue list --state=open --json number,title,labels \
            | jq -c '[.[] | {number, title, label: (.labels[0].name // \"none\")}]'"
  poll   = 5m
  dedupe = true

  metadata {
    source = "github"
  }
}
```

### Abort-on-error smoke test

```hcl
trigger smoke command {
  run      = "scripts/smoke.sh"
  poll     = 1h
  on_error = abort
}
```

## Standalone form

`iter-command` is the sidecar form. Run it in its own container and point `--queue-url` at a shared queue to decouple polling from worker scaling. The `--on-error` flag accepts the same `continue|abort|skip` values as the compose field.

## See Also

- [`compose/trigger.md`](../compose/trigger.md) — shared arguments.
