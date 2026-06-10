# CLI Conventions

This page describes conventions shared by CLI commands. Command pages document
only the behavior that is specific to that command.

## Resource Forms And Aliases

The canonical form is `iter <resource> <verb>`, for example
`iter process logs` or `iter signal push`.

Top-level aliases exist for common process operations:

```sh
iter ps
iter logs <ID>
iter stop <ID>
iter enqueue
```

Aliases use the same arguments and behavior as their canonical forms.

## Process Targets

Process commands that take an instance target accept:

- a full process ULID,
- a unique process ULID prefix,
- a process name.

Name lookup prefers the single live record when multiple historical records
share a name. If a name or ID prefix is ambiguous, the command fails and the
operator must provide a more specific target.

## Listing Output

Listing-style commands accept a shared vocabulary:

| Option | Meaning |
| --- | --- |
| `-q`, `--quiet` | Print one compact value per line. The value is command-specific. |
| `--format table` | Print the human table view. This is the default. |
| `--format json` | Print machine-readable JSON. |
| `--no-trunc` | Do not shorten process IDs in human or quiet output where truncation applies. |

`--format json` is command-specific:

- `iter process ls`, `iter compose ls`, and `iter compose ps` print NDJSON: one JSON object per line.
- `iter compose config` prints one JSON array.
- `iter inspect` always prints one pretty JSON object and does not accept `--format`.
- `iter validate --format json` and `iter compose validate --format json` print one compact JSON object.

Use `-q` when the next command expects plain values:

```sh
iter ps -q | xargs iter rm
```

Use `--format json` when the next command expects structured records:

```sh
iter ps --format json | jq -r '.id'
```

## stdout And stderr

Commands reserve stdout for data that scripts may capture. Human status,
confirmations, and diagnostics go to stderr.

Examples:

- `iter run --detach` prints only the new process ID to stdout.
- `iter enqueue` prints only the new signal ID to stdout.
- `iter stop`, `iter kill`, `iter rm`, and `iter compose down` print confirmations to stderr unless `--quiet` is set.
- `iter logs` replays captured stdout records to stdout and captured stderr records to stderr.

## Exit Codes

The CLI uses stable error categories:

| Code | Meaning |
| ---: | --- |
| `0` | Success. |
| `1` | User input error, such as an unknown target or missing file. |
| `2` | Runtime failure, such as queue, registry, process, or I/O failure during operation. |
| `64` | Configuration error, such as parse or semantic validation failure. |
| `125` | Internal failure. |
| `130` | Interrupted by SIGINT when surfaced by the process. |

Errors are printed to stderr as a single headline with any causal chain below it.

## Paths And Defaults

`iter run` defaults to `./Iterfile`.

`iter compose` commands that read a compose file default to `./compose.iter`.

`iter validate` defaults to `./Iterfile` and detects whether the named path is an
Iterfile or a compose file from its basename.

Process records live in the local iter process registry under `~/.iter/proc`.
Operators should use CLI commands rather than editing the registry directly.
