# Iterfile: `on <event>`

Declares a lifecycle event handler. Zero or more per `Iterfile`. Also usable inside a `compose.iter` inline service.

AST: `EventHandlerDef`, `EventName`, and `RunnerAction` in
`iter_language/src/ast/event.rs`.

## Syntax

```hcl
on <event-name> {
  shell "<command>"

  shell {
    script = "<command>"
    capture <variable-name> {
      stream = stdout
      mode   = replace
      parse  = auto
    }
  }
  ...
}
```

Each handler attaches one or more **actions** (`shell`) to a named lifecycle
event. `enqueue` is a Compose Hook action and is not valid in a Runner Hook.

## Events

The runner emits events in this order:

| Event | When it fires |
| --- | --- |
| `runner_starting` | Once, before the runner enters its per-signal loop. |
| `signal_received` | A Signal was pulled from the queue (or synthesised by `behavior = loop`). |
| `workspace_setup_starting` | Just before the workspace is prepared. |
| `workspace_setup_finished` | Just after the workspace is ready. |
| `agent_starting` | Immediately before the agent process is spawned. |
| `agent_finished` | After the agent process exits (regardless of success). |
| `workspace_teardown_starting` | Before workspace teardown (apply-back, cleanup). |
| `workspace_teardown_finished` | After workspace teardown completes. |
| `runner_error` | A preceding stage failed. Fires instead of any later lifecycle events for that iteration. |
| `runner_completing` | A declared completion condition requested exit and the active iteration, if any, has settled. Fires before source disposition. |
| `runner_finished` | Once when the core Runner loop has ended, regardless of why it stopped. |
| `runner_completed` | Source disposition succeeded and the semantic completed outcome is durable. Only condition-driven completion emits this event. |

`runner_starting` / `runner_finished` fire **per-runner** (exactly once each).
`runner_completing` / `runner_completed` each fire at most once and only on
condition-driven completion; the rest fire per iteration. The condition path
orders its final events as:

```text
runner_completing
runner_finished
source disposition
outcome.json publication
runner_completed
```

Use `runner_completing` for finalization that must happen before the source is
disposed. Use `runner_completed` for integration work that must only happen
after completion is durable.

Misspellings fail at parse time. Some older spellings (`workspace_setting_up`, `workspace_set_up`, `workspace_tearing_down`, `workspace_torndown`) are still accepted as deprecated aliases for the canonical `workspace_setup_starting` / `workspace_setup_finished` / `workspace_teardown_starting` / `workspace_teardown_finished`. Using them produces a deprecation warning; new Iterfiles should use the canonical names.

## Actions

### `shell`

Runs the command string through the user's shell (`/bin/sh -c <command>` on
POSIX). The command line accepts the same `{{...}}` placeholders as `prompt`;
they are resolved immediately before invocation. Substitutions are not
shell-escaped automatically, so quote them according to the script's needs.

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| shorthand command | `string` (positional) | Required in shorthand form | — | `shell "<command>"`; unchanged from earlier Iterfiles. |
| `script` | `string` | Required in block form | — | Command used by `shell { ... }`. |

Use the block form when the command's output should become Runner-scoped
template data:

```hcl
runner {
  # ...
  prompt = """
  Review {{var.context.value.repository}}.
  First output line: {{var.context.lines.[0]}}
  """

  on runner_starting {
    shell {
      script = "./scripts/calculate-context"
      capture context {
        stream = stdout
        mode   = replace
        parse  = auto
      }
    }
  }
}
```

#### `capture <variable-name>`

`capture context { ... }` captures one command stream and publishes it as
`var.context`. A shell action may have multiple captures, provided their names
are unique. Capture is available in Runner lifecycle hooks, including hooks on
an inline Compose service; it is not available in top-level Compose hooks.

| Field | Type | Default | Description |
| --- | --- | --- | --- |
| `stream` | `stdout \| stderr` | `stdout` | Stream to capture. A captured stream is consumed by iter; an uncaptured stream remains inherited. |
| `mode` | `replace \| append` | `replace` | Replace the variable's raw text, or append this execution's bytes to its previous `text` and parse the complete result again. |
| `parse` | `auto`, format, or format list | `auto` | Parser selection. Examples: `auto`, `csv`, `[json, yaml, text]`. Lists are attempted from left to right. |

Every published variable has the same envelope:

| Path | Value |
| --- | --- |
| `var.<name>.text` | Complete UTF-8 stream as text. |
| `var.<name>.lines` | Text split into lines; available regardless of parser. Index with `.[N]`, for example `{{var.context.lines.[0]}}`. |
| `var.<name>.format` | Parser that produced `value`: `text`, `lines`, `json`, `ndjson`, `yaml`, `toml`, `csv`, or `tsv`. |
| `var.<name>.value` | Parsed JSON-shaped value. `csv` and `tsv` produce arrays of row arrays and do not infer headers. |

All supported explicit formats are `text`, `lines`, `json`, `ndjson`, `yaml`,
`toml`, `csv`, and `tsv`. `auto` deliberately uses the conservative order
JSON → NDJSON → TOML → YAML → text. JSON and YAML scalars are not auto-selected,
and CSV/TSV are never guessed; select them explicitly or include them in an
ordered list. `text` and `lines` always parse successfully, so a later list
entry is unreachable after either one.

Captured bytes must be valid UTF-8. For an explicit format or ordered list,
failure to find a matching parser leaves all variables from that shell action
unchanged and records a handler error. Successful captures from the same action
are published together.

#### Visibility and timing

`var.*` belongs to one running Runner and survives across its iterations. A
capture becomes visible after its shell action completes, so actions and
template renders later in lifecycle order can use it:

- `runner_starting` capture is visible to the first Prompt.
- `signal_received` capture is visible to that Signal's Prompt because this
  event fires before Prompt rendering.
- `agent_starting` and later captures cannot change the Prompt already rendered
  for the current iteration; they are available to later hooks and subsequent
  iterations.

Variables are not persisted beyond the Runner process and do not cross between
Compose services.

**Available placeholder roots**

| Root | Example | Notes |
| --- | --- | --- |
| `signal.*` | `{{signal.id}}` | Properties of the Signal being processed. Not available in runner-level completion events; a time condition can fire while idle. |
| `metadata.*` | `{{metadata.task}}` | User-attached key/value pairs on the Signal. Same scope as `signal.*`. |
| `iteration.*` | `{{iteration.count}}` | Runner iteration state — available in **every** event including `runner_starting` (initial state, `count == 0`, `previous_result == "none"`) and `runner_finished` (terminal state). See [`iterfile/prompt.md`](prompt.md#iterationfield-reference) for the field set. |
| `var.*` | `{{var.context.value.foo}}` | Captures published earlier by the same Runner. Available in prompts and Runner shell actions, including completion hooks. |
| `completion.*` | `{{completion.condition.name}}` | Only in `runner_completing` / `runner_completed`. Includes `condition.name`, `condition.kind`, condition-specific redacted fields, `requested_at`, and (only after durability) `completed_at`. |
| `runner.*` | `{{runner.elapsed_seconds}}` | Only in completion events. Includes `started_at`, monotonic `elapsed_seconds`, and `last_signal_id`. |

Runtime templates are strict. Referencing a missing capture or nested value is
a render error; publish captures in an earlier hook and avoid references that
may be absent on a given lifecycle path.

From `workspace_setup_finished` through `workspace_teardown_finished`, the
shell process already runs with the active workspace as its current directory;
there is no separate `workspace.*` placeholder root.

`iteration.previous_result` reflects the prior iteration's
runner-level classification: `"none"` on the first turn (and at
`runner_starting`), `"success"` when the full iteration pipeline
(setup → agent → teardown) completed without a stage error, and
`"errored"` when a runner stage failed — workspace setup error, prompt
render error, agent process spawn / I/O error, iteration timeout, or
workspace teardown error. The streak counters
(`iteration.consecutive_failures` /
`iteration.consecutive_successes`) update together — stage failures
bump one and reset the other, stage successes do the mirror.

`iteration.count` reflects the most recent turn at every lifecycle
event: `0` at `runner_starting` (no turns yet), `N` at
`runner_finished` after N turns completed, and inside `runner_error`
the count of the turn that errored. Per-iteration events
(`signal_received` through `workspace_teardown_finished`) see the same
1-indexed value the prompt template sees for that turn.

Completion hooks intentionally have no current `signal.*` root. Use
`{{runner.last_signal_id}}` when a last attempted Signal is useful, and allow
for it to be absent when a deadline or elapsed budget completes before the
first iteration.

```hcl
runner {
  # ...
  on runner_completing {
    shell "scripts/finalize-exploration.sh {{completion.condition.name}}"
  }

  on runner_completed {
    shell "scripts/publish-evaluation-signal.sh {{completion.condition.kind}}"
  }
}
```

### Multiple actions

Actions are executed **in source order**. A non-zero shell exit is logged but
does not stop later actions. Action infrastructure errors (for example an
invalid UTF-8 capture) are counted and logged by the event dispatcher; the
remaining actions still run.

```hcl
on agent_finished {
  shell "git add -A"
  shell "git commit -m 'iter: {{signal.id}}' || true"
  shell "git push origin HEAD"
}
```

## Multiplicity

You may declare **multiple `on` blocks for the same event**. Each block is a separate handler; all handlers for a given event run in source order, and each handler's actions run sequentially within it.

```hcl
on agent_finished {
  shell "scripts/lint.sh"
}

on agent_finished {
  shell "scripts/post-run-metrics.sh"
}
```

Equivalent in effect to a single `on agent_finished` with both `shell` actions, but lets you keep related handlers close to the config they depend on.

## Examples

### One-shot worktree setup

```hcl
on runner_starting {
  shell "test -d .iter/wt || git worktree add .iter/wt HEAD"
}

on runner_finished {
  shell "echo 'runner done; worktree retained at .iter/wt'"
}
```

### Install dependencies after clone

```hcl
workspace clone { ... }

on workspace_setup_finished {
  shell "npm install --no-audit --no-fund"
}
```

### Commit and surface errors

```hcl
on agent_finished {
  # This hook runs with the active workspace as cwd.
  shell "git status --short"
}

on runner_error {
  shell "notify-team 'iter failed on iteration {{iteration.count}}'"
}
```

### Long-running loop with periodic health check

```hcl
runner {
  continue_on_error = true
  behavior          = loop { delay_secs = 300 }
}

on workspace_teardown_finished {
  shell "curl -fsS https://example.com/healthz"
}
```

## See Also

- [`iterfile/runner.md`](runner.md) — iteration lifecycle that drives these events.
- [`iterfile/prompt.md`](prompt.md) — placeholder syntax shared with `shell`.
- [`language.md`](../language.md) — string literal forms.
