# Iterfile: `on <event>`

Declares a lifecycle event handler. Zero or more per `Iterfile`. Also usable inside a `compose.iter` inline service.

AST: `EventHandlerDef`, `EventName`, and `Action` in `iter_language/src/ast/event.rs`.

## Syntax

```hcl
on <event-name> {
  shell "<command>"
  ...
}
```

Each handler attaches one or more **actions** (currently only `shell`) to a named lifecycle event.

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
| `runner_finished` | Once, just before `iter run` returns — regardless of why the runner stopped. |

`runner_starting` / `runner_finished` fire **per-runner** (exactly once each); the rest fire **per-iteration**. Use the runner-level pair for one-shot setup or teardown that must not repeat — for example, creating a git worktree on first launch.

Misspellings fail at parse time. Some older spellings (`workspace_setting_up`, `workspace_set_up`, `workspace_tearing_down`, `workspace_torndown`) are still accepted as deprecated aliases for the canonical `workspace_setup_starting` / `workspace_setup_finished` / `workspace_teardown_starting` / `workspace_teardown_finished`. Using them produces a deprecation warning; new Iterfiles should use the canonical names.

## Actions

### `shell "<command>"`

Runs the command string through the user's shell (`/bin/sh -c <command>` on POSIX). The command line accepts the same `{{...}}` placeholders as `prompt`; they are resolved immediately before invocation.

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `command` | `string` (positional) | Required | — | Shell command. Resolved placeholders expand into properly quoted substitutions. |

**Available placeholder roots**

| Root | Example | Notes |
| --- | --- | --- |
| `signal.*` | `{{signal.id}}` | Properties of the Signal being processed. Not available in `runner_starting` / `runner_finished` (no signal in scope). |
| `metadata.*` | `{{metadata.task}}` | User-attached key/value pairs on the Signal. Same scope as `signal.*`. |
| `iteration.*` | `{{iteration.count}}` | Runner iteration state — available in **every** event including `runner_starting` (initial state, `count == 0`, `previous_result == "none"`) and `runner_finished` (terminal state). See [`iterfile/prompt.md`](prompt.md#iterationfield-reference) for the field set. |
| `workspace.*` | `{{workspace.path}}` | Workspace paths (available from `workspace_setup_finished` onwards). |
| `agent.*` | `{{agent.exit_code}}` | Agent result info (available from `agent_finished` onwards). |
| `error.*` | `{{error.message}}` | Only defined inside `on runner_error`. |

Placeholders that resolve to unset values expand to the empty string; iter does not throw.

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

### Multiple actions

Actions are executed **in source order**. A non-zero exit aborts the handler and surfaces as a handler-level error; the runner then proceeds to `runner_error` (unless the failure itself came from `runner_error`).

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
  shell "git -C {{workspace.path}} status --short"
}

on runner_error {
  shell "notify-team 'iter failed: {{error.message}}'"
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
