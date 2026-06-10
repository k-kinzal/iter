# iter validate

Validate an Iterfile or compose file.

## Usage

```sh
iter validate [OPTIONS] [PATH]
```

`PATH` defaults to `./Iterfile`.

## Options

| Option | Meaning | Use when |
| --- | --- | --- |
| `--format <text|json>` | Select text or JSON success output. | Use `json` in scripts or CI. |

## Behavior

The command detects the file kind from the path basename:

- compose filenames are validated as compose files,
- all other paths are validated as Iterfiles.

For compose files, validation loads and builds the compose plan. For Iterfiles,
validation parses the Iterfile and reports success when the file is valid.

Use [`compose/validate.md`](compose/validate.md) when the command should default
to `./compose.iter`.

## Output

Iterfile text success output:

```text
OK
```

Compose text success output:

```text
OK (<queues> queue, <services> service, <triggers> trigger)
```

JSON success output is one compact object:

```json
{"ok":true,"summary":{"queues":1,"services":1,"triggers":0}}
```

Diagnostics are written to stderr on failure.

## Examples

```sh
iter validate
iter validate Iterfile
iter validate compose.iter
iter validate --format json compose.iter
```

## Related

- [`compose/validate.md`](compose/validate.md)
- [`../config/iterfile.md`](../config/iterfile.md)
- [`../config/compose.md`](../config/compose.md)
