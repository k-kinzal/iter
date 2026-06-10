# iter compose up

Start services and triggers declared in a compose file.

## Usage

```sh
iter compose up [OPTIONS] [TARGET...]
```

The compose file defaults to `./compose.iter`.

`TARGET` may be a bare service name or `service/NAME`. Targeted startup requires
`--detach`.

## Options

| Option | Meaning | Use when |
| --- | --- | --- |
| `-f`, `--file <PATH>` | Read a compose file other than `./compose.iter`. | The compose file is not in the current directory or has a different name. |
| `--on-failure <abort|continue>` | Choose what happens when one compose task fails. Default is `abort`. | Use `continue` when independent tasks may keep running after one failure. |
| `-d`, `--detach` | Run the compose orchestrator in the background. | The project should outlive the terminal. |
| `-p`, `--project-name <NAME>` | Override the project slug. Takes precedence over `COMPOSE_PROJECT_NAME` and the compose file path. | Multiple compose files would otherwise derive the same project name, or you want a stable explicit name. |
| `--source <PATH>` | Start services whose `build` path matches the given Iterfile. Requires `--detach`. | You want to start the service or services backed by one Iterfile. |
| `--debug` | Enable debug-level tracing output. | You are diagnosing compose startup or runner behavior. |

## Behavior

Without targets, `up` starts the whole project.

Foreground mode runs the orchestrator in the current terminal. Service runners
still register their own process records.

Detached mode starts the orchestrator in the background. The orchestrator is not
registered as an iter process; project discovery is reconstructed from labels on
service runner records.

The project name comes from `--project-name`, `COMPOSE_PROJECT_NAME`, or the
compose file's parent directory basename, in that order.

Targeted mode starts only selected services and requires `--detach`. It does not
start a new foreground project-wide orchestrator. If an active orchestrator
exists for the project, targeted services reuse that project identity.

`--source` resolves to every service whose `build` path matches the given
Iterfile path.

## Output

Detached targeted startup writes service-start status to stderr. The command
does not print a project ID because compose projects are discovered by project
name and runner labels, not by a registered orchestrator record.

## Examples

Start the default compose file:

```sh
iter compose up
```

Start a specific compose file:

```sh
iter compose up -f dev.compose.iter
```

Start in the background:

```sh
iter compose up -f compose.iter --detach
```

Start one service:

```sh
iter compose up worker-a --detach
iter compose up service/worker-a --detach
```

Start services built from one Iterfile:

```sh
iter compose up --source ./worker-a/Iterfile --detach
```

## Related

- [`ps.md`](ps.md)
- [`down.md`](down.md)
- [`config.md`](config.md)
- [`../../config/compose.md`](../../config/compose.md)
