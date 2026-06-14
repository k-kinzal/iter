# Process Commands

Process commands operate one managed iter process.

A process is the operator-facing record for a runner. It is what `iter ps`
lists, what `iter logs` reads, and what `iter stop`, `iter kill`, and `iter rm`
target.

| Command | Alias | Purpose |
| --- | --- | --- |
| [`iter process run`](run.md) | `iter run` | Run an Iterfile as one runner. |
| [`iter process ls`](ls.md) | `iter ps` | List process records. |
| [`iter process logs`](logs.md) | `iter logs` | Replay or follow one process log. |
| [`iter process inspect`](inspect.md) | `iter inspect` | Print one process metadata document. |
| [`iter process stop`](stop.md) | `iter stop` | Request graceful termination with SIGTERM. |
| [`iter process kill`](kill.md) | `iter kill` | Force termination with SIGKILL. |
| [`iter process rm`](rm.md) | `iter rm` | Remove a stopped process record. |
| [`iter process promote`](promote.md) | `iter promote` | Execute a deferred source disposition. |
| [`iter process discard`](discard.md) | `iter discard` | Drop a deferred source base. |

Use process commands for runner-level operation. Use
[`compose`](../compose/index.md) commands when the target is a compose project or
a named service inside a compose project.
