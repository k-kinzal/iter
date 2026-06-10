# iter process logs

Aliases: `iter logs`

Replay or follow the log for one managed process.

## Usage

```sh
iter process logs [OPTIONS] <INSTANCE>
iter logs [OPTIONS] <INSTANCE>
```

`INSTANCE` may be a full ID, unique ID prefix, or process name.

## Options

| Option | Meaning | Use when |
| --- | --- | --- |
| `-f`, `--follow` | Continue reading new log entries. | You want `tail -f` behavior for a running process. |
| `--tail <N>` | Print only the last `N` lines before following or exiting. | You only need recent output. |
| `-t`, `--timestamps` | Prefix each line with an RFC3339 microsecond timestamp. | You need timing context or logs from multiple processes. |

## Behavior

The command reads the per-process `log.ndjson` file. Captured stdout entries are
written to stdout. Captured stderr entries are written to stderr.

Interactive TTY agents may not produce a replayable log; in that case the
command can exit successfully with no lines.

## Examples

```sh
iter logs explorer
iter logs explorer --tail 100
iter logs explorer --follow --timestamps
```

## Related

- [`ls.md`](ls.md)
- [`inspect.md`](inspect.md)
- [`../conventions.md`](../conventions.md)
