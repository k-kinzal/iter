# Iterfile blocks — full field reference

Field shapes are taken from `iter_language/src/ast/`; the per-block doc
pages under `docs/config/iterfile/` carry the prose. This file is a
condensed lookup.

---

## `queue <kind>`

At most one per Iterfile. Required when `runner.behavior = wait`.

| Kind | Body required | Doc |
| --- | --- | --- |
| `memory` | optional (`queue memory` is valid) | `docs/config/queue-backend/memory.md` |
| `file` | yes — `path = "<dir>"` | `docs/config/queue-backend/file.md` |
| `redis` | yes — `url`, `key` | `docs/config/queue-backend/redis.md` |
| `shell` | yes — escape hatch | `docs/config/queue-backend/shell.md` |
| `sqs` | yes — `queue_url`, `region` | `docs/config/queue-backend/sqs.md` |
| `pubsub` | yes — GCP | `docs/config/queue-backend/pubsub.md` |
| `kafka` | yes | `docs/config/queue-backend/kafka.md` |
| `kinesis` | yes — AWS | `docs/config/queue-backend/kinesis.md` |
| `servicebus` | yes — Azure | `docs/config/queue-backend/servicebus.md` |

In an Iterfile the queue is **unnamed**. The compose form adds a `<name>`
identifier — see the **iter-compose** skill.

---

## `workspace <kind>`

### `workspace local`

| Field | Type | Required |
| --- | --- | :---: |
| `base` | string | ✔ |

### `workspace clone`

| Field | Type | Required |
| --- | --- | :---: |
| `base` | string | ✔ |
| `remote` | string | optional |
| `excludes` | `list(string)` | ✔ (`[]` allowed; absence is not) — clone-time filter |
| `includes` | `list(string)` | optional (default: `[]`) — clone-time filter |
| `preserve_mtime` | bool | ✔ |
| `apply_back { ... }` | block | ✔ |

`apply_back` block:

| Field | Type | Required | Default |
| --- | --- | :---: | --- |
| `mode` | enum `sync \| discard \| merge` | ✔ | — |
| `excludes` | `list(string)` | optional | `[]` (must be `[]` when `mode = discard`) |
| `includes` | `list(string)` | optional | `[]` (must be `[]` when `mode = discard`) |

`mode` semantics:

- `sync` — full two-way sync (deletions propagate).
- `discard` — temp thrown away on teardown.
- `merge` — copy new/modified back; deletions **not** propagated.

The clone-time filter (top-level `excludes`/`includes`) decides what enters
the workspace; the apply-back filter (inside `apply_back`) decides what
propagates back to base on teardown. The two phases are independent — list
a path in both filters to skip it both ways. Patterns are globs matched
against paths relative to the workspace root; bare patterns match the
basename at any depth and directory matches auto-cover descendants.
`includes` semantics differ per phase: clone-time `includes` rescue
otherwise-excluded paths (everything not excluded still enters), while a
non-empty apply-back `includes` is a whitelist (only matching paths sync
back). See `docs/config/iterfile/workspace.md` for the full glob
reference and the asymmetric-filtering use case.

### `workspace sandbox`

All fields of `workspace clone` plus:

| Field | Type | Required |
| --- | --- | :---: |
| `policy { ... }` | block | ✔ |

`policy` block:

| Field | Type | Required | Default |
| --- | --- | :---: | --- |
| `network` | enum (`off`, `all`) **or** `list(string)` | ✔ | — |
| `allow_read_outside` | `list(string)` | optional | `[]` |
| `allow_write_outside` | `list(string)` | optional | `[]` |
| `extra_deny_paths` | `list(string)` | optional | `[]` |
| `allow_exec` | `list(string)` | optional | `[]` (inherit backend default) |

`network` values: `off` (deny all), `all` (allow all),
`["host1", "host2"]` (allowlist; unioned with the agent's own
`network_hosts`).

The sandbox declaration is the **upper bound**. The agent's
`sandbox_requirements` (lower bound) are unioned to produce the effective
policy.

---

## `agent <kind>`

| Kind | `mode` field | `subcommand` field | Other required |
| --- | :---: | :---: | --- |
| `claude` | ✔ | — | `command` |
| `codex` | ✔ | — | `command` |
| `gemini` | ✔ | — | `command` |
| `copilot` | ✔ | ✔ (overrides default) | `command` |
| `cursor` | — | — | `command` |
| `cline` | — | — | `command` |
| `opencode` | — | — | `command` |
| `grok` | — | — | `command` (optional `session_id_file`) |
| `generic` | — | — | `command` (as `list(string)` — argv vector) |

Common optional field: `args` (`list(string)`, default `[]`).

Per-kind extras:

- `claude` — optional `system_prompt = "<text>"` replaces the CLI's default
  system prompt; optional `session_id_file = "<path>"` persists a UUID and
  passes `--session-id <uuid>` on subsequent iterations; optional inline
  `output_schema` requires `mode = print`.
- `codex` — optional inline `output_schema` requires `mode = print`.
- `cline` — optional `system_prompt = "<text>"` replaces the CLI's default
  system prompt.
- `grok` — optional `system_prompt = "<text>"` replaces the CLI's default
  system prompt; optional `session_id_file = "<path>"` persists its session
  identifier across iterations; optional inline `output_schema` constrains
  its headless response.
- `copilot` — `subcommand` (list of strings). Unset → iter picks a sane
  default (`["copilot", "suggest"]`); `[]` strips the subcommand;
  `[...]` overrides it.
- `generic` — `command` is a `list(string)` argv. iter prepends nothing
  and `execve`s the command as-is; the program reads the prompt itself.

`mode` values (kinds that take it): `interactive`, `print`.

`output_schema` accepts a JSON document string or a direct JSON-shaped value.
It never treats a string as a file path.
For Codex, iter writes the CLI-required Schema file under the Runner's private
temporary directory. It never uses the Workspace for that file; a sandboxed
Workspace permits the private directory explicitly.

---

## `runner`

The only top-level block that takes no kind.

| Field | Type | Required | Description |
| --- | --- | :---: | --- |
| `continue_on_error` | bool | ✔ | After a stage failure: continue (`true`) or abort (`false`). |
| `behavior` | `wait` \| `loop` \| `loop { delay_secs = N }` | ✔ | What to do when the queue is empty. |
| `iteration_timeout_secs` | int or duration | optional | Hard upper bound per iteration. Cancels the agent process tree on expiry. |

Behaviour matrix:

| Queue present? | `behavior` | Result |
| :---: | --- | --- |
| Yes | `wait` | Block on real Signals. |
| Yes | `loop { delay_secs = N }` | Prefer real Signals; synthesise empty Signal + sleep N seconds when empty. |
| No | `wait` | **Semantic error** (no Signal source). |
| No | `loop { delay_secs = N }` | Tight polling loop with synthesised Signals only. |

`delay_secs` accepts integers or duration literals (`30s`, `5m`).

---

## `prompt`

Each Runner selects one prompt using `prompt = <value>` or a match block:

```hcl
prompt {
  <expression> => "<body>"
  _            => "<default-body>"
}
```

Body forms (from `language.md`): single-line `"..."`, triple-quoted
`""" ... """`, or a triple-quoted heredoc. A bare identifier references a
top-level `prompt as <name>` definition.

Expressions use `signal.*`, `metadata.*`, `iteration.*`, and `var.*`, with
`==`, `!=`, `<`, `<=`, `>`, `>=`, `%`, `&&`, `||`, array indexes, and
parentheses. Conditional arms are evaluated in source order; the first true
arm wins, otherwise `_` is selected. `% 0` is rejected during analysis.

Placeholders such as `{{metadata.task}}` are resolved at dispatch time, not
parse time.

---

## `on <event> [when <expression>]`

Zero or more per Iterfile. Events (in order):

`runner_starting` · `signal_received` · `workspace_setup_starting` ·
`workspace_setup_finished` · `agent_starting` · `agent_finished` ·
`workspace_teardown_starting` · `workspace_teardown_finished` ·
`runner_error` · `runner_finished`.

Deprecated aliases (still parse, emit warning): `workspace_setting_up`,
`workspace_set_up`, `workspace_tearing_down`, `workspace_torndown`.

Actions:

```hcl
shell "<command>"

shell {
  script = "<command>"
  capture context {
    stream = stdout
    mode   = replace
    parse  = auto
  }
}
```

Both forms run through `/bin/sh -c` (POSIX) and accept `{{...}}`
placeholders. Block-form capture is Runner-only; top-level Compose hooks do
not support it. Capture defaults are `stdout`, `replace`, and `auto`.
Supported formats: `text`, `lines`, `json`, `ndjson`, `yaml`, `toml`, `csv`,
and `tsv`; `parse` may also be an ordered list. Auto tries JSON → NDJSON →
TOML → YAML → text and never guesses CSV/TSV.

`capture context` publishes `var.context.text`, `.lines`, `.format`, and
`.value`. Values are visible only after the action completes:
`runner_starting` and `signal_received` can affect the next Prompt render;
`agent_starting` and later cannot change the current Prompt.

Placeholder roots:

| Root | Available where |
| --- | --- |
| `signal.*`, `metadata.*` | per-iteration events only |
| `iteration.*` | every event (`count == 0` at `runner_starting`) |
| `var.*` | after an earlier capture in the same Runner |
| `agent.session_id`, `agent.output` | successful `agent_finished` only |

Multiple `on <same-event>` blocks are allowed; each is a separate handler.

`agent.output` is structured JSON after a supported Agent's inline
`output_schema` has been validated, and text for other capturable responses.
Use `agent.output.<field>` in both Hook conditions and templates. The value is
scoped to the current `AgentRun`, not stored under `var.*`.
