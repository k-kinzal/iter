# compose.iter `trigger` — full reference

AST: `NamedTrigger` in `iter_language/src/ast/compose.rs`; bodies reuse
`TriggerDecl` in `iter_language/src/ast/trigger.rs`.

## Shared Arguments

Apply to every kind:

| Field | Type | Required | Default |
| --- | --- | :---: | --- |
| `target` | bare ident | conditional | auto-resolved with single queue |
| `metadata { … }` | block | optional | `{}` |
| `priority` (`low \| normal \| high \| critical`) | enum | optional | `normal` |
| `max_signals` | int | optional | unbounded |

`metadata` values may carry `{{...}}` placeholders (`payload.*`,
`headers.*`, `query.*`, `signal.*`, plus per-trigger fields like
`{{path}}`, `{{value}}`).

There is no `loop` trigger — continuous iteration belongs on the runner.

---

## `cron`

```hcl
trigger <name> cron {
  target = <queue>            # optional with single queue

  schedule = "<cron expr>"

  timezone   = "<IANA zone>"  # optional, default UTC
  at_startup = <bool>         # optional, default false
  catch_up   = <duration>     # optional, e.g. 300s
  jitter     = <duration>     # optional, default 0s

  priority    = <enum>
  metadata    { ... }
  max_signals = <int>
}
```

| Field | Required | Default | Notes |
| --- | :---: | --- | --- |
| `schedule` | ✔ | — | Standard 5-field or 6-field (with seconds). iter does not transform it. |
| `timezone` | optional | `UTC` | IANA zone name. |
| `at_startup` | optional | `false` | Emits one Signal with `metadata.startup = "true"` on start. |
| `catch_up` | optional | disabled | Window for catching up one missed tick on startup; older missed ticks dropped. Adds `metadata.catch_up = "true"`. |
| `jitter` | optional | `0s` | Random delay added before each tick — useful for fleets. |

Standalone `iter-cron`: `--schedule`, `--timezone`, `--at-startup`,
`--catch-up-window <secs>`, `--jitter <secs>`.

---

## `watch`

```hcl
trigger <name> watch {
  target = <queue>

  dir      = "<path>"
  include  = ["<glob>", ...]
  exclude  = ["<glob>", ...]
  per_file = <bool>
  cooldown = <duration>

  priority    = <enum>
  metadata    { ... }
  max_signals = <int>
}
```

| Field | Required | Default | Notes |
| --- | :---: | --- | --- |
| `dir` | ✔ | — | Watched recursively. Resolved relative to the compose file. |
| `include` | optional | `[]` | gitignore-style globs vs the path relative to `dir`. Empty = watch everything. |
| `exclude` | optional | `[]` | Same syntax. Wins over `include`. |
| `per_file` | optional | `false` | `true` → one Signal per file; `false` → batch coalesced by `cooldown`. |
| `cooldown` | optional | per-mode default | With `per_file=true`: per-path debounce. With `per_file=false`: batch window (library default 250&nbsp;ms; the standalone `iter-watch` CLI defaults `--cooldown 2`). |

Per-file Signal metadata: `path`, `kind` (`created`/`modified`/`removed`),
`timestamp` (RFC3339).
Batched Signal metadata: `files` (JSON array of changed paths).

---

## `files`

Drains one or more file-path sources and emits one Signal per path.

```hcl
trigger <name> files {
  target = <queue>

  # Exactly one of:
  from = stdin
  from = "<path>"                       # bare or "path:<file>"
  from = ["path:./a", "path:./b", ...]  # list — stdin NOT allowed inside
  path = "<path>"                       # alias for `from = "<path>"`

  no_exit_on_eof = <bool>     # optional, default false

  priority    = <enum>
  metadata    { ... }
  max_signals = <int>
}
```

- `from` and `path` are mutually exclusive; one of them is required.
- A path string may be bare (`"./list.txt"`) or `path:`-prefixed
  (`"path:./list.txt"`).
- Lines that are blank or start with `#` are skipped.
- Multiple sources in `from = [...]` drain left-to-right; each source
  starts only after the previous one is fully drained.
- Every emitted Signal carries `metadata.path = "<path>"`.

Standalone `iter-files`: `--from stdin` and `--from path:<file>` are
repeatable.

---

## `command`

Runs an external command on a poll interval and emits Signals from its
stdout.

```hcl
trigger <name> command {
  target = <queue>

  run     = "<command>"
  shell   = "<interpreter>"        # optional, default "sh -c"
  extract = regex("<pattern>")     # optional
  poll    = <duration>             # optional, default 60s
  dedupe  = <bool>                 # optional, default false
  on_error = continue | abort | skip   # optional, default continue

  priority    = <enum>
  metadata    { ... }
  max_signals = <int>
}
```

Extraction:

- `extract = regex("<pattern>")` — applied line-by-line. Capture group 1
  becomes the Signal value (or the full match if no group). Named
  captures `(?P<name>...)` become metadata keys. Non-matching lines are
  silently skipped.
- No `extract` — auto-detect:
  - JSON array on stdout → flattened, one Signal per element. Scalars
    land in `metadata.value`; objects merge verbatim into metadata.
  - Otherwise each non-blank stdout line becomes a Signal with the line
    in `metadata.value`.

`on_error`: `continue` warns and retries on the next tick; `abort` stops
the trigger; `skip` silently swallows errors and emits nothing.

`metadata` values can reference `{{value}}` for the extracted scalar.

Earlier `extract = jq("<query>")` is no longer supported — fold the
JSON shaping into `run` and emit a JSON array instead.

Standalone `iter-command`: `--on-error <continue|abort|skip>` mirrors the
field.

---

## `webhook`

Serves an HTTP listener; per-event routes turn requests into Signals.

```hcl
trigger <name> webhook {
  target = <queue>

  # Bind: pick ONE form
  host = "<bind host>"          # optional, default 0.0.0.0 (pairs with port)
  port = <int>                  # required unless `bind` is set
  bind = "<ADDR:PORT>"          # mutually exclusive with host+port

  path   = "<http path>"
  secret = env("VAR") | file("./<path>") | "<literal>"   # optional

  priority    = <enum>          # default for routes
  metadata    { ... }           # base; routes merge over
  max_signals = <int>

  on "<event-pattern>" {
    when     = "<expression>"
    priority = <enum>
    metadata { ... }
  }
  # …more `on` blocks
}
```

Trigger-level fields:

| Field | Required | Default | Notes |
| --- | :---: | --- | --- |
| `host` | optional | `0.0.0.0` | Mutually exclusive with `bind`. |
| `port` | conditional | — | Required unless `bind` is set. |
| `bind` | optional | — | Full `ADDR:PORT`. Mutually exclusive with `host` + `port`. |
| `path` | ✔ | — | HTTP path the listener serves; other paths return 404. |
| `secret` | optional | disabled | `env("VAR")`, `file("./path")`, or a string literal. |

Per-route fields (each `on "<event-pattern>" { ... }` block):

| Field | Required | Default |
| --- | :---: | --- |
| `when` | optional | always match |
| `priority` | optional | trigger `priority` or `normal` |
| `metadata` | optional | `{}` (merged over trigger metadata; route wins on collision) |

Pattern matching: the listener extracts the event from the incoming
payload (e.g. GitHub's `X-GitHub-Event` header) and matches it against each
route in declaration order. First match wins. `"*"` is a wildcard.

`metadata` values may reference `{{payload.*}}`, `{{headers.*}}`,
`{{query.*}}`, and `{{signal.*}}`.

Standalone `iter-webhook` flags: `--bind ADDR:PORT`, `--path`,
`--secret-env VAR`, `--secret-file FILE`,
`--route PATTERN[:PRIORITY]` (simple route declarations),
`--priority`, `--metadata KEY=VALUE`.

---

## External / User-Defined Kinds

A `trigger <name> <user-defined-kind>` whose kind is not one of the five
above is preserved verbatim — fields are not interpreted by iter and the
trigger has no built-in implementation. See `docs/config/trigger/external.md`.
