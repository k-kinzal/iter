# Trigger: `files`

Drains one or more file-path sources (stdin or files on disk) and emits one Signal per path.

AST: `TriggerDecl::Files` and `FilesSource` in `iter_language/src/ast/trigger.rs`.

Standalone binary: `iter-files`.

## Syntax

```hcl
trigger <name> files {
  target = <queue-name>   # optional when there is only one queue

  # Exactly one of the following forms:
  from = stdin
  from = "<path>"                      # bare path or "path:<file>"
  from = ["path:./a", "path:./b", ...] # list form (stdin is NOT allowed in a list)
  path = "<path>"                      # alias for `from = "<path>"`

  no_exit_on_eof = <bool>   # Optional

  priority = low | normal | high | critical   # Optional
  metadata { ... }                            # Optional
  max_signals = <int>                         # Optional
}
```

## Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `target` | queue ref | Conditional | — | Target queue (bare identifier). |
| `from` | ident `stdin` \| string \| `list(string)` | Conditional | — | Scalar source (`stdin` keyword, or a path string), or a list of path strings. In the list form `stdin` is rejected; use the scalar form to read standard input. A path string may be written either as a bare path (`"./list.txt"`) or prefixed with `path:` (`"path:./list.txt"`). Required unless `path` is set. |
| `path` | string | Conditional | — | Shorthand for `from = "<path>"`. Mutually exclusive with `from`. Required unless `from` is set. |
| `no_exit_on_eof` | bool | Optional | `false` | When `true`, park on cancellation after draining every source instead of exiting. Useful when the trigger shares a process with long-lived peers. |
| `priority` | `low \| normal \| high \| critical` | Optional | `normal` | Signal priority. |
| `metadata` | block | Optional | `{}` | Metadata applied to every emitted Signal. `{{path}}` refers to the current path. |
| `max_signals` | integer | Optional | unbounded | Stop after this many Signals. |

## Source types

| Source | Description |
| --- | --- |
| `stdin` (bare ident) | Read paths from standard input, one per line. EOF on stdin ends the source. |
| `"<path>"` or `"path:<path>"` | Read paths from the named file, one per line. |

Blank lines and lines beginning with `#` are ignored.

Multiple sources declared via `from = [...]` are drained left-to-right. Each source starts only after the previous one has been fully drained.

## Signal metadata

Every emitted Signal carries `metadata.path = "<path>"` (absolute when resolvable, otherwise verbatim).

## Examples

### Drain a todo list once and exit

```hcl
trigger backlog files {
  target = main
  path   = "./backlog.txt"
}
```

### Read from stdin (pipe-driven)

```hcl
trigger inbox files {
  target = main
  from   = stdin
}
```

Invoked as:

```sh
git ls-files "*.rs" | iter compose up -f compose.iter
```

### Multiple path sources, processed in order

```hcl
trigger replay files {
  target = main
  from = [
    "path:./critical-paths.txt",
    "path:./remaining-paths.txt",
  ]
  no_exit_on_eof = true
}
```

`stdin` cannot appear inside a `from = [...]` list. If you want stdin plus files, either run a separate `iter-files` peer process or concatenate the lists before piping.

## Standalone form

`iter-files` is the typical way to turn a one-shot file list into Signals for a running Runner. Example:

```sh
find . -name '*.rs' -newer last-run | iter-files --queue redis://... --target work
```

The CLI accepts `--from stdin` and `--from path:<file>` repeatedly — the compose-file `from = [...]` list is the declarative equivalent.

## See Also

- [`compose/trigger.md`](../compose/trigger.md) — shared arguments.
