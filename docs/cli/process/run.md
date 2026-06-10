# iter process run

Aliases: `iter run`

Run an Iterfile as one runner.

## Usage

```sh
iter process run [OPTIONS] [ITERFILE]
iter run [OPTIONS] [ITERFILE]
```

`ITERFILE` defaults to `./Iterfile`.

## Options

| Option | Meaning | Use when |
| --- | --- | --- |
| `-c`, `--config <PATH>` | Load an optional TOML config file. Defaults to `~/.iter/config.toml`. | You need a non-default CLI config for this invocation. |
| `-d`, `--detach` | Spawn a background process and return immediately. | The runner should outlive the terminal or be operated later with `iter ps`, `iter logs`, and `iter stop`. |
| `--name <NAME>` | Assign a human-friendly process name. | Operators will target the process by name instead of ID. |
| `--once` | Exit after exactly one signal is processed. | You want a bounded run, usually for tests or one-shot queue consumption. |
| `--debug` | Enable debug-level tracing output. | You are diagnosing CLI, runner, or compose behavior. |
| `--service <NAME>` | Run one service from a compose file instead of a plain Iterfile. | Advanced use. This is primarily the compose orchestrator's service-spawn entry point. |
| `--arg <KEY=VALUE>` | Override an Iterfile `arg` value. Repeatable. | The Iterfile declares parameters that must vary per run. |

## Behavior

Foreground mode runs in the current terminal and exits when the runner exits.

Detached mode creates a managed process record and prints the new process ID to
stdout. Use the returned ID, a unique prefix, or the assigned name with later
process commands.

```sh
ID=$(iter run Iterfile --detach --name explorer)
iter logs "$ID" --follow
```

`--once` applies to the runner loop. The runner exits after one signal has been
processed.

`--arg` values use `KEY=VALUE` syntax and override `arg` declarations in the
Iterfile before the runner starts.

## Output

Foreground runs stream through the runner's configured stdio behavior.

Detached runs print only the new process ID to stdout. Diagnostics are written to
stderr.

## Examples

Run the default Iterfile:

```sh
iter run
```

Run a named Iterfile in the foreground:

```sh
iter run ./workers/Iterfile
```

Start a long-running runner:

```sh
iter run Iterfile --detach --name api-poller
```

Run once with an argument override:

```sh
iter run Iterfile --once --arg model=claude-sonnet
```

## Related

- [`ls.md`](ls.md)
- [`logs.md`](logs.md)
- [`stop.md`](stop.md)
- [`../../config/iterfile.md`](../../config/iterfile.md)
