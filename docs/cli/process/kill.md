# iter process kill

Aliases: `iter kill`

Force termination of one process.

## Usage

```sh
iter process kill [OPTIONS] <INSTANCE>
iter kill [OPTIONS] <INSTANCE>
```

`INSTANCE` may be a full ID, unique ID prefix, or process name.

## Options

| Option | Meaning | Use when |
| --- | --- | --- |
| `-q`, `--quiet` | Suppress confirmation output. | A script only needs the exit code. |

## Behavior

The command sends SIGKILL when a process is still live. If the record is already
terminal but the recorded PID is still alive, `kill` still attempts forceful
termination. If the process is already gone, the command succeeds.

Use `kill` after `stop` fails to end the underlying process or when graceful
shutdown is not appropriate.

## Output

Confirmation is written to stderr. Stdout is not used.

## Examples

```sh
iter kill explorer
iter kill -q explorer
```

## Related

- [`stop.md`](stop.md)
- [`rm.md`](rm.md)
