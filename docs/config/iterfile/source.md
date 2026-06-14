# Iterfile: `source`

Declares exploration-scoped provenance and disposition for a workspace base.

`workspace.apply_back` still controls each iteration's temp tree. `source`
controls the durable base around the whole runner: derive it once at runner
start, then discard, merge, sync, or defer it once at runner finish.

## Syntax

```hcl
source <kind> as <name> {
  <fields>
}

workspace clone {
  source = <name>
}
```

`workspace ... { source = "/path" }` is path sugar for a directory passthrough
source. Existing `base = "/path"` remains accepted and behaves the same as
before.

## Kinds

| Kind | Locator | Derive modes |
| --- | --- | --- |
| `directory` | `path = "..."` | `passthrough`, `copy` |
| `git` | exactly one of `path = "..."` or `url = "..."` | `worktree`, `clone` |

## `derive`

Scalar form uses defaults:

```hcl
derive = passthrough
derive = worktree
```

Block form overrides fields:

```hcl
derive = copy {
  excludes = ["target"]
  preserve_mtime = true
}

derive = worktree {
  ref = "HEAD"
  branch = "iter/exp-1"
}
```

| Mode | Kind | Fields |
| --- | --- | --- |
| `passthrough` | `directory` | none |
| `copy` | `directory` | `excludes`, `preserve_mtime` |
| `worktree` | `git path` | `ref`, `branch` |
| `clone` | `git path` or `git url` | `ref`, `branch`, `depth` |

## `disposition`

Required when `derive` creates a separate base (`copy`, `worktree`, `clone`).
Forbidden for `passthrough`.

| Mode | Meaning |
| --- | --- |
| `discard` | Drop the durable base; leave canonical untouched. |
| `merge` | Non-destructive fold back. Directories copy newer files without deleting; git merges or pushes the branch. |
| `sync` | Directory sync back, including deletions. |
| `defer` | Leave the base parked and record a pending operator decision. |

Examples:

```hcl
disposition = merge {
  excludes = ["*.tmp"]
  includes = ["src/**"]
}

disposition = defer {
  promote = merge { into = "main" ff = only }
}
```

For deferred dispositions, finish-time leaves canonical untouched and writes a
pending decision into the process record. Later:

```sh
iter promote <proc>
iter discard <proc>
```

`promote` executes the recorded inner disposition and clears the pending
record. `discard` drops the parked base and clears it.

## Examples

Directory snapshot:

```hcl
source directory as experiment {
  path = "/repo/main"
  derive = copy { excludes = [".git", "target"] preserve_mtime = true }
  disposition = defer {
    promote = sync
  }
}

workspace sandbox {
  source = experiment
  excludes = []
  preserve_mtime = true
  apply_back { mode = sync }
  policy { network = off }
}
```

Git worktree:

```hcl
source git as detached {
  path = "/repo/main"
  derive = worktree { ref = "HEAD" }
  disposition = merge { into = "main" ff = only }
}

workspace clone {
  source = detached
  excludes = [".git"]
  preserve_mtime = true
  apply_back { mode = merge }
}
```

## Validation

`iter validate` rejects:

- `derive = passthrough` with any `disposition`
- `derive = copy | worktree | clone` with no `disposition`
- `worktree` or `clone` on `source directory`
- `source git` with both `url` and `path`, or neither
- `disposition = defer { promote = defer { ... } }`
- an unknown `workspace.source` name
- both `base` and `source` on one workspace

## See Also

- [`workspace.md`](workspace.md) — per-iteration workspace setup and `apply_back`.
- [`../../cli/process/index.md`](../../cli/process/index.md) — process operation commands.
