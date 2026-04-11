# `iter` CLI — full command reference

Source of truth: `iter_cli/src/cli.rs` (clap derive structs).

Every `<INSTANCE>` argument accepts a full ULID, the `--name` you gave at
spawn, or a **unique id prefix** (Docker-style). The 12-character ID shown
in `iter ps` is a prefix you can paste back. Ambiguous prefixes return
`AmbiguousPrefix`; supply more characters.

---

## `iter run [PATH]`

Run an Iterfile (or a single named service from a `compose.iter`).

| Flag | Type | Default | Description |
| --- | --- | --- | --- |
| `[PATH]` | path | `./Iterfile` | Iterfile to run, or a `compose.iter` when `--service` is set. |
| `-c, --config <PATH>` | path | `~/.iter/config.toml` | Optional TOML config file. |
| `-d, --detach` | bool | `false` | Spawn as a detached background process and return its ULID. macOS/Linux only. |
| `--name <NAME>` | string | derived from the file path | Human-friendly name. Used by every `iter <verb> <INSTANCE>` command. |
| `--once` | bool | `false` | Exit after exactly one Signal has been processed. |
| `--service <NAME>` | string | — | Run a single named service from a compose file instead of an Iterfile. The positional path is then treated as a `compose.iter`. Used internally by `iter compose up` to spawn each service as its own subprocess; usable manually for one-off service runs. |
| `--debug` | bool | `false` | Enable debug-level tracing. |

Foreground (`iter run`) and `--detach` share the **same** spawn pipeline:
both fork a child, capture `stdout.log` / `stderr.log`, and register the
process in `~/.iter/proc/<id>/`. The flag only changes whether the parent
streams the captured logs and waits, or returns the ULID immediately. Both
are visible to `iter ps` / `iter logs` / `iter inspect` / `iter stop`.

`--process-id` exists but is hidden — set internally by
`iter_core::process::spawn_detached` when forking the detached child; do
not pass it manually.

---

## `iter compose up`

Build and spawn every service and trigger declared in `compose.iter`. Each
service runs as a child `iter run --service <NAME>` process. Each
URL-addressable trigger runs as a standalone `iter-cron` / `iter-files` /
`iter-watch` / `iter-command` / `iter-webhook` process. All children carry
`parent_id = <orchestrator-id>` in their metadata.

| Flag | Type | Default | Description |
| --- | --- | --- | --- |
| `-f, --file <PATH>` | path | `./compose.iter` | Compose file to load. |
| `--on-failure <abort\|continue>` | enum | `abort` | What to do when one task fails. `abort` cancels every other task on the first error; `continue` logs the failure and lets surviving tasks run to completion. |
| `-d, --detach` | bool | `false` | Spawn the orchestrator itself as a detached process and return its ULID. Mirrors `iter run --detach`. |
| `--debug` | bool | `false` | Enable debug-level tracing. |

`--process-id` is hidden — set when the orchestrator forks itself for
`--detach` adoption.

When a service's queue is **not URL-addressable** (e.g. `memory://`), that
service runs in-process inside the orchestrator instead of as a subprocess
— same model as the in-process trigger fallback. URL-addressable queues
(`file://`, `redis://`, `sqs://`, `pubsub://`, `kafka://`, `kinesis://`,
`servicebus://`) take the subprocess path.

---

## `iter compose validate`

Parse and semantic-check `compose.iter`. Exits non-zero on the first error.

| Flag | Type | Default | Description |
| --- | --- | --- | --- |
| `-f, --file <PATH>` | path | `./compose.iter` | Compose file to validate. |
| `--format <text\|json>` | enum | `text` | Output format. `json` emits structured diagnostics. |

---

## `iter compose ls` (alias `iter compose ps`)

List the queues, services, and triggers **declared** in `compose.iter`.
File inspection — does **not** look at the live registry. For live state
use `iter ps`.

| Flag | Type | Default | Description |
| --- | --- | --- | --- |
| `-f, --file <PATH>` | path | `./compose.iter` | Compose file to inspect. |
| `-q, --quiet` | bool | `false` | One `kind/name` pair per line on stdout (e.g. `queue/main`, `service/worker`). |
| `--format <text\|json>` | enum | `text` | `json` emits an array of `{kind, name, detail}` objects. |
| `--no-trunc` | bool | `false` | Disable detail-column truncation in the human table. |

Compose resources have no persistent IDs — `kind/name` is part of the
contract because two kinds may share a name.

---

## `iter validate [PATH]`

Validate an Iterfile **or** `compose.iter`. The file kind is detected from
its basename; default is `./Iterfile`.

| Flag | Type | Default | Description |
| --- | --- | --- | --- |
| `[PATH]` | path | `./Iterfile` | File to validate. |
| `--format <text\|json>` | enum | `text` | Output format. |

---

## `iter ps` (canonical `iter process ls`)

List process records in the local registry. Foreground and detached runs
appear identically — both went through `spawn_detached` and recorded
`stdout.log` / `stderr.log`.

| Flag | Type | Default | Description |
| --- | --- | --- | --- |
| `-a, --all` | bool | `false` | Include stopped / failed / killed records in addition to running ones. |
| `-q, --quiet` | bool | `false` | One full ULID per line on stdout. Composes with `xargs iter rm`. |
| `--format <text\|json>` | enum | `text` | `json` emits one NDJSON record per process with full ID and ISO-8601 UTC timestamps. |
| `--no-trunc` | bool | `false` | Disable the 12-character ULID truncation in the human table. |

The truncated `ID` column in the table is a **valid prefix** — paste it
back into any `iter <verb> <INSTANCE>` form.

---

## `iter logs <INSTANCE>` (canonical `iter process logs`)

Tail the captured stdout / stderr of a process.

| Flag | Type | Default | Description |
| --- | --- | --- | --- |
| `<INSTANCE>` | string | required | ULID, `--name`, or unique id prefix. |
| `-f, --follow` | bool | `false` | Follow new lines as they arrive (`tail -f` semantics). |
| `--tail <N>` | int | unbounded | Print only the last N lines before following. |

Logs are available for both foreground and detached processes. The only
case where `iter logs` yields nothing is an interactive TTY agent
(`StdioPolicy::Passthrough`), which writes nothing to the on-disk log
files.

---

## `iter inspect <INSTANCE>` (canonical `iter process inspect`)

Print the JSON metadata document for a process (argv, subcommand, debug
flag, status, lifecycle timestamps, `parent_id`).

| Flag | Type | Description |
| --- | --- | --- |
| `<INSTANCE>` | string | ULID, `--name`, or unique id prefix. |

Always JSON — `inspect` is the source of truth for a resource. Tabular
views belong on `iter ps`.

---

## `iter stop <INSTANCE>` / `iter kill <INSTANCE>`

Send `SIGTERM` (`stop`) or `SIGKILL` (`kill`) to a process.

| Flag | Type | Default | Description |
| --- | --- | --- | --- |
| `<INSTANCE>` | string | required | ULID, `--name`, or unique id prefix. |
| `-q, --quiet` | bool | `false` | Suppress the `<id>: <from> -> <to>` confirmation on stderr. |

`stop` flips the record to `Killed` synchronously after sending SIGTERM,
but the underlying child may still be alive. `kill` escalates: if the
record is already terminal but the recorded pid is still live, `kill`
force-signals it without re-transitioning the status.

---

## `iter rm <INSTANCE>` (canonical `iter process rm`)

Remove a terminal process directory. Refuses to remove a still-running
record. Use `iter stop` then `iter rm`, or `iter kill && iter rm`.

| Flag | Type | Default | Description |
| --- | --- | --- | --- |
| `<INSTANCE>` | string | required | ULID, `--name`, or unique id prefix. |
| `-q, --quiet` | bool | `false` | Suppress the `removed <id>` confirmation. |

Probe errors bias toward refusing removal — silently treating them as
"dead" would let `iter rm` race with a live runner and delete the proc
directory out from under its log writes.

---

## `iter enqueue` (canonical `iter signal push`)

Push a single Signal onto a queue.

| Flag | Type | Default | Description |
| --- | --- | --- | --- |
| `--queue-url <URL>` | string | — | Connect-style queue URL (`file:///abs/path`, `memory://`, `redis://...`). Takes precedence over `-f`. |
| `-f, --file <PATH>` | path | auto-detect (`./compose.iter` → `./Iterfile`) | Path to a `compose.iter` or `Iterfile` whose queue declaration is reused for the connection. |
| `--queue <NAME>` | string | — | Queue name when the resolved file declares more than one queue. Compose-only. |
| `-m, --metadata <KEY=VALUE>` | repeatable | `[]` | Metadata pair (string-typed). Repeat for multiple keys. |
| `--priority <low\|normal\|high\|critical>` | enum | `normal` | Signal priority. |

Resolution order: `--queue-url` ▶ `-f` (with `--queue` if many queues) ▶
auto-detection in cwd.

---

## `iter completions <SHELL>`

Emit a shell completion script to stdout.

| Argument | Choices | Description |
| --- | --- | --- |
| `<SHELL>` | `bash` / `zsh` / `fish` / `powershell` / `elvish` | Shell flavour. |

Examples:

```sh
source <(iter completions bash)
iter completions zsh  > ~/.zfunc/_iter
iter completions fish > ~/.config/fish/completions/iter.fish
```

---

## Exit codes

- `0` — success.
- non-zero — a CLI-level error or any error bubbled from the dispatched
  subcommand. iter does not currently allocate stable, documented exit
  codes per failure class; rely on stderr output rather than the numeric
  code for scripted decisions.
