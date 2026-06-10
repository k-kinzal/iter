# iter compose ps

List runners and trigger state for one compose project.

## Usage

```sh
iter compose ps [OPTIONS]
```

The project is derived from `./compose.iter` unless `--file` or
`--project-name` is supplied.

## Options

| Option | Meaning | Use when |
| --- | --- | --- |
| `-f`, `--file <PATH>` | Use a compose file path to derive the project name. Ignored when `--project-name` is supplied. | You started the project from a non-default compose file. |
| `-p`, `--project-name <NAME>` | Select the project by explicit name. Takes precedence over `COMPOSE_PROJECT_NAME` and the compose file path. | You used `compose up -p` or want to avoid path-derived naming. |
| `-a`, `--all` | Include terminal runners. | You need to inspect stopped, failed, or killed service records. |
| `-q`, `--quiet` | Print one runner ID per line. | You are piping runner IDs to process commands. |
| `--format <table|json>` | Select the table or NDJSON view. | Use `json` for structured automation. |
| `--no-trunc` | Print full runner IDs where truncation would otherwise apply. | You need complete process IDs. |

## Behavior

`ps` reconstructs one project from compose labels on process records. Service
runners are listed as process records. Trigger status is read from compose
trigger state when available.

When `--project-name` is absent, the command honors `COMPOSE_PROJECT_NAME`
before deriving the project name from the compose file path.

By default, terminal runner records are hidden. Trigger rows are not affected by
`-q`; quiet mode prints runner IDs only.

## Output

The table view lists service runner ID, service name, status, PID, and created
time. If trigger status exists, trigger rows are appended below the runner rows.

`--format json` prints one JSON object per runner or trigger row, one per line.

`-q` prints one runner process ID per line.

## Examples

```sh
iter compose ps
iter compose ps -f dev.compose.iter
iter compose ps -p my-project --all
iter compose ps -q | xargs iter logs
```

## Related

- [`ls.md`](ls.md)
- [`down.md`](down.md)
- [`../process/logs.md`](../process/logs.md)
