# iter compose config

List the static resources declared by a compose file.

## Usage

```sh
iter compose config [OPTIONS]
```

The compose file defaults to `./compose.iter`.

## Options

| Option | Meaning | Use when |
| --- | --- | --- |
| `-f`, `--file <PATH>` | Read a compose file other than `./compose.iter`. | The compose file is not in the current directory or has a different name. |
| `-q`, `--quiet` | Print one `kind/name` per line. | You want to grep or pipe declared resources. |
| `--format <table|json>` | Select the table or JSON-array view. | Use `json` for structured automation. |
| `--no-trunc` | Accepted as a shared listing option. | Kept for command-line consistency; compose resource names are not process IDs. |

## Behavior

`config` loads and builds the compose plan, then lists declared resources. It
does not inspect runtime state and does not connect to process records.

Rows may include telemetry, queues, services, and triggers.

Use [`ps.md`](ps.md) for runtime runners and trigger state.

## Output

The table view uses `KIND`, `NAME`, and `DETAIL` columns.

`--format json` prints one JSON array.

`-q` prints one `kind/name` value per line, such as `queue/main` or
`service/worker`.

## Examples

```sh
iter compose config
iter compose config -q | grep '^service/'
iter compose config --format json
```

## Related

- [`validate.md`](validate.md)
- [`ps.md`](ps.md)
- [`../../config/compose.md`](../../config/compose.md)
