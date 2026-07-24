---
name: iter-iterfile
description: "Author or modify an Iterfile (single-service HCL config for iter run). Blocks: workspace, agent, runner, prompt, queue, on-event. Workspace kinds, agent CLI kinds, wait/loop behaviors, conditional expressions."
---

# iter-iterfile

An `Iterfile` defines a **single self-contained iter service** — the
Dockerfile-equivalent in iter. It is run directly with `iter run [PATH]` (default
`./Iterfile`) or referenced from compose with
`service <name> { build = "./Iterfile" }`.

If you have not yet, load the **iter** skill first for the conceptual model
and CLI surface.

## Minimal Example

The smallest Iterfile that runs under `iter run`: `workspace` + `agent` +
`runner`, with the prompt bound inside `runner`. No queue (so
`runner.behavior` must be `loop`), no `on` handlers. Every field shown is
required.

```hcl
workspace local {
  base = "."
}

agent claude {
  mode    = print
  command = "claude"
}

runner {
  agent             = claude
  workspace         = local
  continue_on_error = false
  behavior          = loop
  prompt            = "Improve the codebase."
}
```

## Top-Level Sections

| Section | Count | Required | Notes |
| --- | :---: | :---: | --- |
| `queue <kind>` | 0–1 | Optional | Required when `runner.behavior = wait`. |
| `workspace <kind>` | 0–1 | For `iter run` | Kinds: `local`, `clone`, `sandbox`. |
| `agent <kind>` | 0–1 | For `iter run` | 8 kinds; see [`reference/blocks.md`](reference/blocks.md). |
| `prompt as <name> "<body>"` | 0–N | Optional | Reusable named prompt; referenced by bareword in a runner. |
| `runner` | 0–1 | For `iter run` | The only top-level block that takes no kind. Binds `agent`/`workspace`/`queue` by name and carries the `prompt` plus `on <event>` handlers. |

Definitions (`workspace`, `agent`, `queue`) are **named** — the name is the
kind, or an explicit `as <name>` alias. The `runner` block references them
by name and is where the prompt and lifecycle handlers live; there are no
top-level `prompt` or `on` sections.

A partial Iterfile is allowed (a webhook handler may omit
`workspace`/`agent`/`runner`); to be runnable standalone via `iter run` it
needs `workspace` + `agent` + `runner`, and the runner must carry a
`prompt`.

Per-block field tables, including every kind variant, live in
[`reference/blocks.md`](reference/blocks.md).

## "No project-shaped defaults"

iter does not pick semantic policy on the project's behalf. Required fields
must be written even when the natural value is empty. For example,
`workspace clone` requires `excludes` to be stated even when the value is
`[]` ("skip nothing"). `includes` is optional and defaults to `[]` when
omitted.

Wrong — fails validation with `` workspace clone requires `excludes` ``:

```text
workspace clone {
  base           = "."
  preserve_mtime = true

  apply_back {
    mode = sync
  }
}
```

Right — every required field stated explicitly:

```hcl
workspace clone {
  base           = "."
  excludes       = []
  includes       = []
  preserve_mtime = true

  apply_back {
    mode = sync
  }
}

agent claude {
  mode    = print
  command = "claude"
}

runner {
  agent             = claude
  workspace         = clone
  continue_on_error = false
  behavior          = loop
  prompt            = "x"
}
```

## Prompt expressions

The runner's prompt is a `prompt { ... }` match block whose arms carry a
boolean expression. The first arm whose expression is true wins; `_` is
the required default.

```hcl
workspace local { base = "." }
agent claude { mode = print  command = "claude" }

runner {
  agent     = claude
  workspace = local
  continue_on_error = true
  behavior  = loop
  prompt {
    metadata.task == "security" => "Run a security audit."
    iteration.count % 50 == 0 => "The current codebase has problems. Identify the issues and fix them."
    metadata.env == "prod" && metadata.task != "skip" => "Run production-safe checks only."
    _ => "Always-on instructions."
  }
}
```

Prompt expressions can use `signal.*`, `metadata.*`, `iteration.*`, and
`var.*`. Available `iteration.*` fields include `count` (1-indexed),
`previous_exit_code`, `previous_result`
(`"none" | "success" | "errored"`), `consecutive_failures`, and
`consecutive_successes`. Operators: `==`, `!=`, `<`, `<=`, `>`, `>=`, `%`,
`&&`, and `||`, with parentheses for grouping.

The match selects exactly one body per iteration: conditional arms are
evaluated top to bottom and the first true arm wins; if none match, the
`_` default arm is used. To combine instructions, write them into a single
arm body rather than expecting multiple arms to fire.

## Lifecycle Events

Events fire in this order. `runner_starting` and `runner_finished` fire once
per `iter run`; the rest fire once per iteration.

1. `runner_starting`
2. `signal_received`
3. `workspace_setup_starting` → `workspace_setup_finished`
4. `agent_starting` → `agent_finished`
5. `workspace_teardown_starting` → `workspace_teardown_finished`
6. `runner_error` (fires instead of remaining per-iteration events on
   failure)
7. `runner_finished`

Each `on <event> [when <expression>]` block carries one or more `shell`
actions. The shorthand is `shell "<command>"`. Use
`shell { script = "..." ... }` when command output
must be captured into the Runner-scoped `var.*` template root. `enqueue` is a
Compose Hook action and is not available in Runner Hooks.

```hcl
workspace local { base = "." }
agent claude { mode = print  command = "claude" }

runner {
  agent     = claude
  workspace = local
  continue_on_error = true
  behavior  = loop
  prompt    = "x"

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

  on agent_finished {
    shell "git status --short"
  }

  on agent_finished when agent.output.decision == "fix" {
    shell "./scripts/apply-review-fixes"
  }

  on signal_received {
    shell "logger 'iter: signal {{signal.id}} received'"
  }

  on runner_error {
    shell "logger 'iter: runner errored on iteration {{iteration.count}}'"
  }
}
```

Capture fields default to `stream = stdout`, `mode = replace`, and
`parse = auto`. Explicit formats are `text`, `lines`, `json`, `ndjson`,
`yaml`, `toml`, `csv`, and `tsv`; an ordered list such as
`parse = [json, yaml, text]` is also valid. Auto-detection tries JSON, NDJSON,
TOML, YAML, then text; it never guesses CSV/TSV.

Each capture publishes `var.<name>.text`, `.lines`, `.format`, and `.value`.
The value is forward-only: `runner_starting` is visible to the first Prompt,
and `signal_received` to that Signal's Prompt. `agent_starting` and later
events run after the current Prompt was rendered. Values persist across
iterations of that Runner, but not across Runner processes or Compose
services. Capture is invalid in top-level Compose hooks.

Runner Prompt and shell templates accept `signal.*`, `metadata.*`,
`iteration.*`, `var.*`, and `today` as applicable. Completion shell actions
instead add `completion.*` and `runner.*` and have no current Signal. See
`docs/config/iterfile/on.md` for the full placeholder and timing contract.

Successful `agent_finished` Hooks additionally expose `agent.session_id` and
`agent.output`. Structured fields such as `agent.output.decision` are
available when `claude`, `codex`, or `grok` uses inline `output_schema`;
otherwise capturable Agents expose final-response text at `agent.output`.
`output_schema` accepts a JSON document string or a direct JSON-shaped value,
never a user file path. For Claude/Codex it requires `mode = print`.
Codex's CLI-required Schema file is placed in the Runner's private temporary
directory, never in the Workspace; sandboxed runs permit that directory.

Multiple `on <event>` blocks for the same event are allowed — each is a
separate handler, all run in source order.
One handler evaluates its `when` expression once before running its actions.
Evaluation failures are recorded as handler errors.

## Validate Before Running

```sh
iter validate ./Iterfile
```

Catches required-field omissions, unknown kinds, illegal expressions,
and the `wait`-without-`queue` semantic error before any agent is spawned.

## Pointers

- Prompt breadth guide: [`reference/prompt-guide.md`](reference/prompt-guide.md).
- Full Iterfile examples by breadth: [`reference/breadth-examples.md`](reference/breadth-examples.md).
- Workspace kinds + sandbox policy: `docs/config/iterfile/workspace.md`.
- Agent kinds (per-CLI invocation shape): `docs/config/iterfile/agent.md`.
- Runner semantics (wait/loop/timeout, iteration state):
  `docs/config/iterfile/runner.md`.
- Prompt expressions + placeholders: `docs/config/iterfile/prompt.md`.
- Queue backends: `docs/config/iterfile/queue.md`,
  `docs/config/queue-backend/`.
- Lifecycle events + `shell` actions: `docs/config/iterfile/on.md`.
- Shared DSL syntax (strings, durations, identifiers):
  `docs/config/language.md`.
- Multi-service orchestration: load the **iter-compose** skill.
- Running and inspecting: load the **iter** skill.
