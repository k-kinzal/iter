# iter compose validate

Parse and semantic-check a compose file.

## Usage

```sh
iter compose validate [OPTIONS]
```

The compose file defaults to `./compose.iter`.

## Options

| Option | Meaning | Use when |
| --- | --- | --- |
| `-f`, `--file <PATH>` | Validate a compose file other than `./compose.iter`. | The compose file is not in the current directory or has a different name. |
| `--format <text|json>` | Select text or JSON success output. | Use `json` in scripts or CI. |

## Behavior

The command loads the compose file and builds the compose plan. It exits before
starting services, triggers, or queues.

Validation failures include parse errors and semantic errors such as missing
sections, unknown references, unsupported trigger kinds, and invalid composition.

## Output

Text success output:

```text
OK (<queues> queue, <services> service, <triggers> trigger)
```

JSON success output:

```json
{"ok":true,"summary":{"queues":1,"services":1,"triggers":0}}
```

Diagnostics are written to stderr on failure.

## Examples

```sh
iter compose validate
iter compose validate -f dev.compose.iter
iter compose validate --format json
```

## Related

- [`config.md`](config.md)
- [`../validate.md`](../validate.md)
- [`../../config/compose.md`](../../config/compose.md)
