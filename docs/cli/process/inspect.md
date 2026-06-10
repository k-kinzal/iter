# iter process inspect

Aliases: `iter inspect`

Print the metadata document for one process.

## Usage

```sh
iter process inspect <INSTANCE>
iter inspect <INSTANCE>
```

`INSTANCE` may be a full ID, unique ID prefix, or process name.

## Behavior

`inspect` is JSON-only. It prints the process metadata document as pretty JSON
and does not accept `--format`.

Use `iter process ls` for tabular process views.

## Output

The output is one JSON object on stdout.

## Examples

```sh
iter inspect explorer
iter inspect explorer | jq '.status'
iter inspect explorer | jq '.labels'
```

## Related

- [`ls.md`](ls.md)
- [`logs.md`](logs.md)
