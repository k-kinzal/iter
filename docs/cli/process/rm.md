# iter process rm

Aliases: `iter rm`

Remove a terminal process record.

## Usage

```sh
iter process rm [OPTIONS] <INSTANCE>
iter rm [OPTIONS] <INSTANCE>
```

`INSTANCE` may be a full ID, unique ID prefix, or process name.

## Options

| Option | Meaning | Use when |
| --- | --- | --- |
| `-q`, `--quiet` | Suppress confirmation output. | A script only needs the exit code. |

## Behavior

The command removes the process directory only after the process is terminal and
the recorded PID is no longer alive. It refuses to remove a record for a still
running process.

Use `iter process ls --all` to find terminal records.

## Output

Confirmation is written to stderr. Stdout is not used.

## Examples

```sh
iter ps --all
iter rm explorer
iter ps -q --all | xargs iter rm
```

## Related

- [`ls.md`](ls.md)
- [`stop.md`](stop.md)
- [`kill.md`](kill.md)
