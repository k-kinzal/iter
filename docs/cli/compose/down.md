# iter compose down

Stop a compose project or selected services.

## Usage

```sh
iter compose down [OPTIONS] [TARGET...]
```

`TARGET` may be a bare service name or `service/NAME`.

The project is derived from `./compose.iter` unless `--file` or
`--project-name` is supplied.

## Options

| Option | Meaning | Use when |
| --- | --- | --- |
| `-f`, `--file <PATH>` | Use a compose file path to derive the project name. Ignored when `--project-name` is supplied. | You started the project from a non-default compose file. |
| `-p`, `--project-name <NAME>` | Select the project by explicit name. Takes precedence over `COMPOSE_PROJECT_NAME` and the compose file path. | You used `compose up -p` or want to avoid path-derived naming. |
| `--source <PATH>` | Stop services whose `build` path matches the given Iterfile. | You want to stop the service or services backed by one Iterfile. |
| `-t`, `--timeout <SECONDS>` | Wait this long after SIGTERM before escalating to SIGKILL. Default is `30`. | The project needs a shorter or longer graceful shutdown window. |
| `-q`, `--quiet` | Suppress status lines. | A script only needs the exit code. |

## Behavior

Without targets, `down` stops the whole project:

1. Discover the active orchestrator from compose labels.
2. Send SIGTERM to the orchestrator when it is live.
3. Send SIGTERM to non-terminal service runners.
4. Wait until runners and orchestrator exit or the timeout expires.
5. Escalate remaining live processes to SIGKILL.

With targets or `--source`, only the selected services are stopped. The
orchestrator and sibling services are left running.

When `--project-name` is absent, the command honors `COMPOSE_PROJECT_NAME`
before deriving the project name from the compose file path.

If no runners are registered for the project, the command succeeds and reports
that no runners were found unless `--quiet` is set.

## Output

Status lines are written to stderr. Stdout is not used.

## Examples

```sh
iter compose down
iter compose down -f dev.compose.iter
iter compose down -p my-project --timeout 5
iter compose down worker-a
iter compose down --source ./worker-a/Iterfile
```

## Related

- [`up.md`](up.md)
- [`ps.md`](ps.md)
- [`../process/stop.md`](../process/stop.md)
