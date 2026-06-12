# iter validate

Validate an Iterfile.

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

Each deliverable has one validation verb:

- Iterfiles are validated by `iter validate`.
- Compose files are validated by
  [`iter compose validate -f <path>`](compose/validate.md), whatever the
  file is named.

`iter validate` always parses `PATH` as an Iterfile, whatever it is named,
with one compatibility exception: the exact basename `compose.iter`
delegates to `iter compose validate` and prints a note on stderr:

```text
note: compose files are validated by 'iter compose validate'; delegating
```

A compose-format file under any other name fails Iterfile validation, and
the diagnostics end with a hint pointing at `iter compose validate -f <path>`.

## Output

Text success output:

```text
OK
```

Delegated `compose.iter` success output matches `iter compose validate`:

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
iter validate path/to/Iterfile
iter validate --format json
```

## Related

- [`compose/validate.md`](compose/validate.md)
- [`../config/iterfile.md`](../config/iterfile.md)
- [`../config/compose.md`](../config/compose.md)
