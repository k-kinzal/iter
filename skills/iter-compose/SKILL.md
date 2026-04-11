---
name: iter-compose
description: "Author or modify compose.iter — iter's multi-service orchestration. Queues, services (build/inline), triggers (cron/watch/files/command/webhook). Queue binding, subprocess model, standalone trigger binaries."
---

# iter-compose

A `compose.iter` orchestrates **multiple iter services and triggers** around
one or more shared queues — the docker-compose-equivalent in iter. It is
loaded by `iter compose up [-f compose.iter]`.

`compose.iter` is **not** a superset of an Iterfile. It has no top-level
`workspace` / `agent` / `runner` / `prompt` / `on` blocks. Those always live
inside an inline `service` body, or inside the Iterfile a service references.

For the inline-service body syntax, load the **iter-iterfile** skill — the
block grammar is identical.

## Required structure

| Section | Count | Required |
| --- | :---: | :---: |
| `queue <name> <kind>` | 1–N | ✔ |
| `service <name>` | 1–N | ✔ |
| `trigger <name> <kind>` | 0–N | optional |

A compose file with zero queues or zero services fails validation.

## Minimal example

```hcl
queue main memory {}

service worker {
  build = "./Iterfile"
}
```

One queue, one service. `queue = main` is omitted because exactly one
queue is declared (auto-resolution kicks in). `trigger` is optional — the
runner can drive itself via `runner.behavior = loop` inside the Iterfile.

The docs note that simple kinds may write `queue main memory` without the
`{}` body; the current parser still requires the empty braces. Add `{}`
even when the body has no fields.

## How `iter compose up` orchestrates

Each declared resource runs as its **own** OS process so that every one
appears as an independent row in `iter ps` with its own captured logs:

| Resource | Spawned binary | Argv shape |
| --- | --- | --- |
| `service <name>` (URL-addressable queue) | `iter` | `iter run --service <name> <compose.iter>` |
| `trigger <name> cron` | `iter-cron` | flags built from the trigger body |
| `trigger <name> files` | `iter-files` | flags built from the trigger body |
| `trigger <name> watch` | `iter-watch` | flags built from the trigger body |
| `trigger <name> command` | `iter-command` | flags built from the trigger body |
| `trigger <name> webhook` | `iter-webhook` | flags built from the trigger body |

The orchestrator itself is a parent process visible in `iter ps`; every
child it spawns carries `parent_id = <orchestrator-id>` in its
metadata. `iter logs <child-id>` works on each one independently.

### In-process fallback for non-URL queues

When a queue has no cross-process URL form (today: `memory://` and any
declared kind that resolves to a memory backend), the resource attached to
it falls back to running **in-process inside the orchestrator** instead
of as a subprocess. This applies to both services (whose `queue =`
points at the memory queue) and triggers (whose `target =` does). They
still appear in the orchestrator's process record but do not get their
own `iter ps` row.

URL-addressable queues — `file://`, `redis://`, `sqs://`, `pubsub://`,
`kafka://`, `kinesis://`, `servicebus://` — take the subprocess path.

### `iter compose up --detach`

Mirrors `iter run --detach`: the orchestrator forks itself into the
background and the parent CLI returns the orchestrator's ULID. All
service / trigger children spawn under the detached orchestrator. Use
`iter logs <orchestrator-id>` for orchestrator-level events and
`iter logs <child-id>` for any individual service or trigger.

### Shutdown

The orchestrator propagates SIGTERM / SIGINT to every child it tracks.
`iter stop <orchestrator-id>` is the supported way to stop a whole
compose deployment; child processes terminate, write their terminal
status, and become removable via `iter rm`.

## Queue binding rules

- `queue = <name>` on a `service` — which queue it consumes from.
- `target = <name>` on a `trigger` — which queue it publishes into.
- The value is a **bare identifier** (`queue = main`), never a quoted
  string.
- When the file declares **exactly one queue**, both `queue =` and
  `target =` may be omitted — the semantic layer auto-resolves.
- With two or more queues, omitting the binding is a semantic error.
- A reference to an undeclared name is a semantic error.
- A queue nobody binds to is allowed but produces a load-time warning.

## Service forms

A `service` body is either **external** (`build = "<path-to-Iterfile>"`)
or **inline**. The two forms are mutually exclusive — `build` cannot
coexist with any inline section. The current parser only ships the
external form; inline service bodies are documented but not yet wired
into compose validation. For multi-file deployments, prefer the `build`
form today and keep service-specific logic inside the referenced
Iterfile.

External form:

```hcl
queue main file { path = "./.iter/queue" }

service worker {
  build = "./Iterfile"
  queue = main          # overrides any queue declared inside the Iterfile
}
```

Inline form (per `docs/config/compose/service.md`; see also the
**iter-iterfile** skill for the body grammar):

```hcl
service chores {
  queue = main

  workspace local { base = "." }
  agent claude { mode = print  command = "claude" }
  runner { continue_on_error = true  behavior = loop { delay_secs = 60 } }
  prompt "Apply pending formatting and commit in small batches."
}
```

If a referenced Iterfile already declares its own `queue` block, the
compose-level `queue = <name>` overrides it. This lets you reuse one
Iterfile across topologies. [`reference/services.md`](reference/services.md)
covers service-level specifics.

## Trigger kinds

There is **no `loop` trigger kind**. Continuous iteration belongs on the
runner (`runner.behavior = loop { ... }`), not on a trigger.

| Kind | Purpose | Subprocess binary |
| --- | --- | --- |
| `cron` | Emit on a cron schedule. | `iter-cron` |
| `watch` | Emit on filesystem changes. | `iter-watch` |
| `files` | Drain file-path lists from stdin or files. | `iter-files` |
| `command` | Poll an external command's output. | `iter-command` |
| `webhook` | Serve an HTTP listener with per-event routes. | `iter-webhook` |

Shared trigger arguments (apply to every kind):

| Field | Default |
| --- | --- |
| `target` | auto-resolved when single queue, otherwise required |
| `metadata { ... }` | `{}` |
| `priority` (`low \| normal \| high \| critical`) | `normal` |
| `max_signals` | unbounded |

Per-kind fields, examples, and the regex / JSON-array semantics for
`command` live in [`reference/triggers.md`](reference/triggers.md).

```hcl
queue bulk file { path = "./.iter/queue-bulk" }

service worker {
  build = "./Iterfile"
  queue = bulk
}

trigger nightly cron {
  target   = bulk
  schedule = "0 3 * * *"
  timezone = "UTC"

  metadata {
    task = "audit"
  }
}
```

## When to use the standalone binaries directly

`iter compose up` is convenient when one machine owns the whole
deployment. For one-process-per-trigger topologies — Kubernetes pods,
systemd units, Docker sidecars, stdin pipelines — invoke the standalone
binaries (`iter-cron`, `iter-watch`, `iter-files`, `iter-command`,
`iter-webhook`) yourself. They share the same flag surface compose uses
when it spawns them and publish into the same queue backends, so they
coexist with a Runner started elsewhere via `iter run` or another
`iter compose up`.

Pick standalone when:

- The trigger needs its own container image, scaling profile, or network
  scope (e.g. `iter-webhook` behind an Ingress).
- A pipe-driven invocation is more natural than a long-lived process
  (`git ls-files | iter-files --queue-url redis://... --target work`).
- You want the trigger to fail and restart independently of the worker
  pool.

## Validate before running

```sh
iter compose validate -f ./compose.iter
```

Catches missing `target` bindings, unknown queue kinds, undeclared queue
references, and inline-vs-`build` conflicts before any service spawns.

## Pitfalls

- **Memory queues silently disable the subprocess split.** A
  `queue x memory {}` forces every service / trigger bound to it
  in-process inside the orchestrator. Use a `file` queue (or any
  URL-addressable backend) when you want each service in its own
  `iter ps` row.
- **`iter compose ls` is file-only**, not live state. It lists what
  `compose.iter` *declares*. For live processes, use `iter ps`.
- **`stop` the orchestrator, not the children.** Stopping the
  orchestrator propagates to children. Stopping a child individually
  works but the orchestrator may try to restart-or-fail per its
  `--on-failure` policy.
- **Queue override is silent.** `service x { build = "./f.iter" queue = q }`
  silently replaces any `queue` block declared *inside* `f.iter`. Use
  this deliberately for reuse; check both files when debugging routing.

## Pointers

- compose.iter overview: `docs/config/compose.md`.
- Per-section pages: `docs/config/compose/queue.md`,
  `docs/config/compose/service.md`, `docs/config/compose/trigger.md`.
- Queue backends: `docs/config/queue-backend/`
  (memory / file / redis / shell / sqs / pubsub / kafka / kinesis /
  servicebus).
- Trigger kinds: `docs/config/trigger/`
  (cron / watch / files / command / webhook / external).
- Inline-service body syntax: load the **iter-iterfile** skill.
- Running `iter compose up` and inspecting state: load the **iter** skill.
