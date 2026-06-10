# iter process stop

Aliases: `iter stop`

Request graceful termination of one process.

## Usage

```sh
iter process stop [OPTIONS] <INSTANCE>
iter stop [OPTIONS] <INSTANCE>
```

`INSTANCE` may be a full ID, unique ID prefix, or process name.

## Options

| Option | Meaning | Use when |
| --- | --- | --- |
| `-q`, `--quiet` | Suppress confirmation output. | A script only needs the exit code. |

## Behavior

The command sends SIGTERM to the process and updates its process record. If the
record is already terminal, the command succeeds and reports that state unless
`--quiet` is set.

`stop` is the normal shutdown command. Use [`kill.md`](kill.md) only when the
process does not exit or must be forcefully terminated.

## Output

Confirmation is written to stderr. Stdout is not used.

## Examples

```sh
iter stop explorer
iter stop -q explorer
```

## Related

- [`kill.md`](kill.md)
- [`rm.md`](rm.md)
