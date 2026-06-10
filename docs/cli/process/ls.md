# iter process ls

Aliases: `iter ps`

List managed iter process records.

## Usage

```sh
iter process ls [OPTIONS]
iter ps [OPTIONS]
```

## Options

| Option | Meaning | Use when |
| --- | --- | --- |
| `-a`, `--all` | Include terminal records such as stopped, failed, and killed processes. | You need to inspect history or remove old records. |
| `-q`, `--quiet` | Print one process ID per line. | You are piping IDs to another command. |
| `--format <table|json>` | Select the table or NDJSON view. | Use `json` for structured automation. |
| `--no-trunc` | Print full process IDs where truncation would otherwise apply. | You need complete IDs in the table or quiet view. |

## Behavior

The command reads the local process registry and refreshes process status before
listing records. By default it hides terminal records.

The table view includes ID, name, status, PID, created time, and Iterfile.

## Output

`--format table` prints a human table.

`--format json` prints one JSON object per process, one per line.

`-q` prints one process ID per line and ignores the table layout.

## Examples

```sh
iter ps
iter ps --all
iter ps -q | xargs iter rm
iter ps --format json | jq -r '.id'
```

## Related

- [`inspect.md`](inspect.md)
- [`rm.md`](rm.md)
- [`../conventions.md`](../conventions.md)
