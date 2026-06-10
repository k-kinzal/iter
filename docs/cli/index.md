# iter CLI

The `iter` CLI starts, observes, and operates iter runners.

CLI commands are organized around resources:

| Resource | Purpose |
| --- | --- |
| [`process`](process/index.md) | Run and operate one managed iter process. |
| [`compose`](compose/index.md) | Start and operate a compose project made of services and triggers. |
| [`signal`](signal/index.md) | Push outside information into a queue. |

Most commands have a canonical resource form and a shorter top-level alias.
Prefer the canonical form in documentation and scripts when it makes the target
resource clearer.

| Canonical | Alias |
| --- | --- |
| `iter process run` | `iter run` |
| `iter process ls` | `iter ps` |
| `iter process logs` | `iter logs` |
| `iter process inspect` | `iter inspect` |
| `iter process stop` | `iter stop` |
| `iter process kill` | `iter kill` |
| `iter process rm` | `iter rm` |
| `iter signal push` | `iter enqueue` |

## Common Paths

Run a single Iterfile:

```sh
iter run Iterfile
```

Run a single Iterfile in the background:

```sh
iter run Iterfile --detach --name explorer
iter logs explorer --follow
iter stop explorer
```

Start a compose project:

```sh
iter compose up -f compose.iter
```

Start a compose project in the background and inspect its runners:

```sh
iter compose up -f compose.iter --detach
iter compose ps -f compose.iter
```

Push a manual signal:

```sh
iter enqueue -f compose.iter --queue main --metadata source=manual
```

## Reference

- [`conventions.md`](conventions.md) covers output, IDs, exit codes, and common flags.
- [`process/`](process/index.md) covers `iter process ...` and top-level process aliases.
- [`compose/`](compose/index.md) covers `iter compose ...`.
- [`signal/`](signal/index.md) covers `iter signal push` and `iter enqueue`.
- [`validate.md`](validate.md) covers top-level validation.
- [`completions.md`](completions.md) covers shell completion generation.

Configuration-file syntax is documented under [`../config/`](../config/).
