# Iterfile: `runner`

Declares the runtime policy for the iter loop. Optional — zero or one block per `Iterfile`. Also usable inside a `compose.iter` inline service.

AST: `RunnerDef` and `RunnerBehavior` in `iter_language/src/ast/runner.rs`.

## Syntax

```hcl
runner {
  continue_on_error      = <bool>
  behavior               = <wait | loop [{ delay_secs = <int> }]>
  iteration_timeout_secs = <int | duration>   # optional
}
```

`runner` is the only top-level block that takes **no kind**.

## Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `continue_on_error` | `bool` | Required | — | Whether to continue the loop after a stage failure. No default — iter does not pick an error policy on the project's behalf. |
| `behavior` | `enum { wait \| loop { ... } }` | Required | — | What to do when the queue yields no Signal (or when there is no queue at all). No default. |
| `iteration_timeout_secs` | `integer` or `duration` | Optional | unbounded | Hard upper bound on a single iteration. When the agent (and its descendants) exceed this, iter cancels the iteration, kills the agent process tree, and feeds an `IterationTimeout` error into the normal `continue_on_error` path. Must be positive. |

### `continue_on_error`

- `true` — log the failure and proceed to the next Signal. Useful for long-running `loop` services.
- `false` — one bad Signal aborts the whole runner. Appropriate for single-shot runs and debugging.

### `behavior = wait`

Block on `Queue::dequeue` until a Signal arrives or the runner is cancelled.

**Constraint**: a `wait` runner with no `queue` is a semantic error — there is no Signal source.

```hcl
runner {
  continue_on_error = false
  behavior          = wait
}
```

### `behavior = loop { ... }`

Synthesise an empty Signal on each iteration when the queue is empty. Iterations can optionally sleep in between (no sleep before the first one).

If a queue exists, real Signals on the queue are always preferred; synthesis only fires on an empty queue.

#### Nested fields

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `delay_secs` | `integer` or `duration` | Optional | no delay | Sleep between iterations. `duration` values (`30s`, `5m`) are normalised to seconds internally. |

```hcl
# 30 seconds between iterations
runner {
  continue_on_error = true
  behavior          = loop { delay_secs = 30 }
}

# No delay (tight loop)
runner {
  continue_on_error = false
  behavior          = loop
}

# duration literal
runner {
  continue_on_error = true
  behavior          = loop { delay_secs = 5m }
}
```

### `iteration_timeout_secs`

A guard against runaway iterations. Set it when an agent can plausibly hang
(missing tool, infinite tool loop, network stall) and you would rather lose
the turn than block the whole runner.

```hcl
runner {
  continue_on_error      = true
  behavior               = loop
  iteration_timeout_secs = 15m
}
```

Semantics:

- The clock starts when the agent step begins (after dequeue, render, and
  workspace setup) and stops when the agent returns.
- On expiry iter signals the agent's per-iteration cancel token. The agent
  observes the cancel through its own `select!`, sends `SIGTERM` to the
  whole process group (agent + sandbox + grandchildren), waits a short
  grace period, then escalates to `SIGKILL`. The runner keeps awaiting the
  agent through this graceful shutdown rather than dropping it on the
  floor; an unresponsive agent is force-dropped after a bounded drain
  window and `ProcessGroup`'s `Drop` issues a last-resort `SIGKILL`.
- The timeout is surfaced as `AgentError::IterationTimeout` and flows into
  the normal `RunnerError { stage = AgentRun }` path. With
  `continue_on_error = true` the loop moves on; with `false` the runner
  aborts.
- This is a runaway guard, **not an SLA**. Set it generously (minutes, not
  seconds) — the cost of a false trip is a wasted turn.

## Behaviour Matrix

| `queue` present? | `behavior` | Result |
| :---: | --- | --- |
| Yes | `wait` | Wait for real Signals; one iteration per Signal. |
| Yes | `loop { delay_secs = N }` | Prefer real Signals; synthesise if empty, sleep N seconds between synthesised iterations. |
| No | `wait` | **Semantic error** — no Signal source. |
| No | `loop { delay_secs = N }` | Tight polling loop with synthetic Signals only (ralph-loop pattern). |

## Usage Patterns

### Single-shot

```hcl
queue memory
runner {
  continue_on_error = false
  behavior          = wait
}
```

Drive from CLI or a single enqueue, then exit on failure.

### Continuous background iteration

```hcl
# No queue needed
runner {
  continue_on_error = true
  behavior          = loop { delay_secs = 60 }
}
```

### Event-driven with periodic checks

```hcl
queue file { path = ".iter/queue" }

runner {
  continue_on_error = true
  behavior          = loop { delay_secs = 300 }
}
```

Real Signals (cron, webhook, etc.) are handled immediately. When the queue is empty, iter still runs a synthetic "check-in" iteration every 5 minutes.

## Iteration State

The runner maintains per-iteration state that prompt bodies and shell
hooks can read through the `iteration.*` placeholder root, and that
`prompt when` guards can branch on. Key semantics:

- **`iteration.count` is 1-indexed at render time.** The first
  iteration sees `count == 1`, so `iteration.count % 10 == 0` fires on
  iterations 10, 20, 30, … as a human would expect. Lifecycle events
  with no signal in scope (`runner_starting` / `runner_finished` /
  `runner_error` before any iteration began) still receive a snapshot,
  but `count` reflects the latest turn — `0` at `runner_starting`.
- **The counter advances even when a stage fails.** Whether iter moves
  on to the next turn is governed by `continue_on_error`, but the
  counter itself keeps climbing — a failed turn is still a turn that
  happened.
- **`iteration.previous_result`** carries `"none"`, `"success"`, or
  `"errored"` from the prior turn. `"success"` when the full iteration
  pipeline (setup → agent → teardown) completed without a stage error.
  `"errored"` when a runner stage failed — workspace setup error,
  prompt render error, agent process spawn / I/O error, iteration
  timeout, or workspace teardown error. `"none"` only on the first
  iteration. The streak counters
  (`iteration.consecutive_failures` /
  `iteration.consecutive_successes`) track the same conditions.

## See Also

- [`iterfile/on.md`](on.md) — lifecycle events fired by the runner, and the full `iteration.*` field set.
- [`iterfile/prompt.md`](prompt.md) — `prompt when` guards over `iteration.*` (e.g. every-N iterations).
- [`iterfile/queue.md`](queue.md) — the queue referenced by `wait`.
