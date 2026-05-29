---
name: iter
description: "iter CLI: run, compose up, ps, logs, stop, kill, rm, inspect, enqueue, validate. Foreground/detached runners, ULID/name resolution. Entry point before authoring Iterfile or compose.iter."
---

# iter

`iter` is an AI-agent control framework. The CLI turns an `Iterfile` (single
service) or `compose.iter` (multi-service) into a running composition of
queue, workspace, agent, and trigger pieces, and gives you commands to run,
inspect, and manage those compositions.

## Core concepts

- **Runner** — the unit of exploration. Binds a **Workspace** (where and how
  the agent runs) with an **Agent** (which AI agent runs and how it is
  invoked) and iterates over them.
- **Workspace** — filesystem environment per iteration. Kinds: `local`
  (run in place), `clone` (copy to scratch), `sandbox` (clone + kernel-level
  sandbox).
- **Agent** — the AI process iter spawns each iteration. Kinds: `claude`,
  `codex`, `gemini`, `copilot`, `cursor`, `cline`, `opencode`, `grok`, `generic`.
- **Signal / Queue** — a Signal is one unit of work; a Queue carries Signals
  into a Runner. Each iteration consumes exactly one Signal (real, or
  synthesised by `runner.behavior = loop`).
- **Trigger** — a Signal source. Kinds: `cron`, `watch`, `files`, `command`,
  `webhook`. Triggers only exist in `compose.iter`.

A Runner alone holds exploration inside its own radius. Triggers feed
external information into the Queue to widen that radius.

## Command cheatsheet

The canonical form is `iter <resource> <verb>`; the top-level aliases below
are kept for ergonomics.

| Command | Canonical | Purpose |
| --- | --- | --- |
| `iter run [PATH]` | `iter process run` | Run an Iterfile in runner-only mode (no triggers). |
| `iter run --detach` | — | Spawn the runner as a background process; print its ULID and return. |
| `iter compose up` | — | Spawn every service and trigger declared in `compose.iter`, each as its own subprocess. |
| `iter compose up --detach` | — | Spawn the orchestrator itself as a background process. |
| `iter compose validate` | — | Parse and semantic-check `compose.iter`. |
| `iter compose ls` | `iter compose ps` | List queues / services / triggers declared in `compose.iter` (file inspection — no live state). |
| `iter validate [PATH]` | — | Validate an Iterfile **or** `compose.iter` (auto-detected by basename). |
| `iter ps` | `iter process ls` | List process records in the local registry (running + recent). |
| `iter logs <ID\|NAME>` | `iter process logs` | Tail the captured stdout / stderr of a process. |
| `iter inspect <ID\|NAME>` | `iter process inspect` | Print the JSON metadata document for a process. |
| `iter stop <ID\|NAME>` | `iter process stop` | Send `SIGTERM` and mark the record `Killed`. |
| `iter kill <ID\|NAME>` | `iter process kill` | Send `SIGKILL` and mark the record `Killed`. |
| `iter rm <ID\|NAME>` | `iter process rm` | Remove a terminal process directory. |
| `iter enqueue` | `iter signal push` | Push one Signal onto a queue. |
| `iter completions <SHELL>` | — | Emit a shell completion script (`bash`/`zsh`/`fish`/`powershell`/`elvish`). |

Per-flag detail lives in [`reference/commands.md`](reference/commands.md).

## Foreground vs `--detach` (same observability, different blocking)

`iter run` and `iter run --detach` use the **same spawn pipeline** — both
fork a child process, capture `stdout.log` and `stderr.log` on disk, and
register the process in `~/.iter/proc/<id>/`. The flag only changes whether
the parent CLI streams the captured logs and waits, or returns the ULID
immediately.

| You want… | Use |
| --- | --- |
| Logs streamed to your terminal; lifecycle tied to your shell. | `iter run` (foreground) |
| To leave the runner up after you log out / a long-running service. | `iter run --detach --name <NAME>` |
| To later inspect with `iter logs` / `iter inspect`. | Either — both register in the local store. |
| One Signal then exit. | `iter run --once` (with or without `--detach`). |

Same logic applies to `iter compose up [--detach]`. `--detach` is
macOS / Linux only; on Windows `iter` returns a clean "not supported" error.

## Targeting processes (ULID / name / id prefix)

`iter logs / inspect / stop / kill / rm` accept any of:

1. The full lower-case ULID (`01k8tswzmgxdpejjbq8z9mra19`).
2. The `--name` you assigned at spawn (when unique among live records).
3. **Any unique ID prefix** (Docker-style). The 12-character ID shown in
   `iter ps` is a prefix you can paste back: `iter logs 01k8tswzmgxd`. Two
   or more matches → `AmbiguousPrefix`; specify more characters.

ULIDs display as lower-case (proc-dir names too); existing upper-case
directories from older versions still resolve.

## Common workflows

**One-shot probe** — tight loop that exits on the first Signal:

```sh
iter run --once --debug ./Iterfile
```

**Long-running detached service** — spawn, follow logs, stop:

```sh
ID=$(iter run --detach --name worker ./Iterfile)
iter logs -f "$ID"
iter stop worker
```

**Multi-service deployment** — every service / trigger gets its own
`iter ps` row:

```sh
iter compose up -f ./compose.iter --detach   # orchestrator backgrounds itself
iter ps                                       # lists orchestrator + services + triggers
iter logs <service-prefix>                    # any 12-char id from `iter ps`
iter stop <orchestrator-id>                   # propagates SIGTERM to all children
```

**Push a Signal manually** — reuse a declared queue without spinning up a
producer:

```sh
iter enqueue -f ./compose.iter --queue main \
    -m task=audit --priority high
```

When the file declares exactly one queue, `--queue` may be omitted —
auto-resolution kicks in.

## Pitfalls

- **No project-shaped defaults.** iter never picks `behavior`,
  `continue_on_error`, or sandbox `network` for you; every required field
  has to be written, even when the value is empty (`excludes = []`).
- **`runner.behavior = wait` requires a queue.** Without one the runner
  has no Signal source and validation fails.
- **Validate before running.** `iter validate ./Iterfile` (or
  `iter compose validate -f compose.iter`) catches schema errors before any
  agent process is spawned.
- **`iter rm` refuses still-running records.** Use `iter stop` then
  `iter rm` (or `iter kill` for the forceful path). Probe errors bias the
  call toward refusing — `iter rm` will not race a live writer.
- **`iter compose ls` inspects the file, not the live store.** It lists
  what `compose.iter` *declares*. To see live processes, use `iter ps`.

## Pointers

- Conceptual model: runners consume queued signals, agents act inside
  workspaces, and triggers introduce outside information.
- CLI flag tables: [`reference/commands.md`](reference/commands.md).
- Iterfile field reference: `docs/config/iterfile.md` and the per-block
  pages under `docs/config/iterfile/`.
- compose.iter field reference: `docs/config/compose.md` and the per-block
  pages under `docs/config/compose/`.
- Shared DSL syntax: `docs/config/language.md`.
- Authoring an Iterfile: load the **iter-iterfile** skill.
- Authoring a `compose.iter`: load the **iter-compose** skill.
