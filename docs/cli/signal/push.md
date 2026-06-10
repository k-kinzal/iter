# iter signal push

Aliases: `iter enqueue`

Create one signal and push it onto a queue.

## Usage

```sh
iter signal push [OPTIONS]
iter enqueue [OPTIONS]
```

## Options

| Option | Meaning | Use when |
| --- | --- | --- |
| `--queue-url <URL>` | Build the queue directly from a URL. Takes precedence over `--file`. | You want to enqueue without reading an Iterfile or compose file. |
| `-f`, `--file <PATH>` | Resolve a queue from an Iterfile or compose file. | You want to use the queue declared by project configuration. |
| `--queue <NAME>` | Select a named queue from a compose file. | The compose file declares more than one queue. |
| `-m`, `--metadata <KEY=VALUE>` | Add string metadata to the signal. Repeatable. | The runner prompt or hooks need signal-specific values. |
| `--priority <low|normal|high|critical>` | Set signal priority. Default is `normal`. | The queue should process this signal before or after normal work. |

## Queue Resolution

The command resolves a queue in this order:

1. `--queue-url <URL>`.
2. `--file <PATH>`.
3. Auto-detected `./compose.iter`.
4. Auto-detected `./Iterfile`.

When a compose file declares multiple queues, `--queue <NAME>` is required. When
an Iterfile is used, `--queue` is not meaningful.

Supported direct queue URLs are:

- `memory://`
- `file:///absolute/or/relative/path`
- `redis://...`
- `rediss://...`

Queue declarations loaded from configuration files use the normal queue syntax
documented under [`../../config/queue-backend/`](../../config/queue-backend/).

## Metadata

Metadata uses `KEY=VALUE` syntax. Values are stored as strings.

```sh
iter enqueue -m source=manual -m ticket=ABC-123
```

## Output

On success, the new signal ID is printed to stdout. Diagnostics are written to
stderr.

## Examples

Push to an explicit memory queue:

```sh
iter enqueue --queue-url memory://
```

Push to a queue declared by an Iterfile:

```sh
iter enqueue -f Iterfile --priority high
```

Push to one queue in a compose file:

```sh
iter enqueue -f compose.iter --queue urgent --metadata source=manual
```

## Related

- [`../../config/queue-backend/memory.md`](../../config/queue-backend/memory.md)
- [`../../config/compose/queue.md`](../../config/compose/queue.md)
- [`../../config/iterfile/queue.md`](../../config/iterfile/queue.md)
