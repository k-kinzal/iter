# iter compose ls

List compose projects discovered from the local process registry.

## Usage

```sh
iter compose ls [OPTIONS]
```

## Options

| Option | Meaning | Use when |
| --- | --- | --- |
| `-a`, `--all` | Include projects whose runners are all terminal. | You need to inspect exited projects that still have process records. |
| `-q`, `--quiet` | Print one project name per line. | You are piping project names. |
| `--format <table|json>` | Select the table or NDJSON view. | Use `json` for structured automation. |
| `--no-trunc` | Accepted as a shared listing option. | Kept for command-line consistency; project names are not process IDs. |

## Behavior

The command scans process records, groups records with compose labels by project,
and reports orchestrator liveness. The orchestrator is not itself a process
record, so a project is discovered through its service runner records.

By default, projects without a live orchestrator are hidden. Use `--all` to show
exited projects that still have records.

## Output

The table view includes project name, service count, runner count, status, and
orchestrator PID when known.

`--format json` prints one project object per line.

`-q` prints one project name per line.

## Examples

```sh
iter compose ls
iter compose ls --all
iter compose ls --format json
```

## Related

- [`ps.md`](ps.md)
- [`down.md`](down.md)
- [`../conventions.md`](../conventions.md)
