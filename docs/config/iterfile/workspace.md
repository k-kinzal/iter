# Iterfile: `workspace`

Declares the filesystem environment the agent operates in. Optional — zero or one block per `Iterfile`. Also usable inside a `compose.iter` inline service.

A workspace can name a [`source`](source.md) instead of a direct `base`. The
source derives a durable base once for the whole runner; the workspace still
performs its normal per-iteration setup and `apply_back` against that base.

AST: `WorkspaceDef` in `iter_language/src/ast/workspace.rs`.

## Syntax

```hcl
workspace <kind> {
  <fields>
}
```

`<kind>` is one of:

| Kind | Purpose |
| --- | --- |
| [`local`](#workspace-local) | Run directly inside the existing directory. Lightest. |
| [`clone`](#workspace-clone) | Copy the directory into a scratch location for each iteration. |
| [`sandbox`](#workspace-sandbox) | Same as `clone` plus kernel-level sandboxing (`sandbox-exec` on macOS, `bwrap` on Linux). |

---

## `workspace local`

Uses the existing directory at `base` as-is. No copy, no sandbox. Agent side effects land directly in that directory.

### Example

```hcl
workspace local {
  base = "."
}
```

### Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `base` | `string` | Required | — | Workspace root path. Relative paths resolve against the directory containing the `Iterfile`. |
| `source` | `ident` / `string` | Optional | — | Named `source` block, or path sugar equivalent to a directory passthrough source. Mutually exclusive with `base`. |

---

## `workspace clone`

Copies the directory at `base` (or pulls from `remote`) into a scratch directory and runs the agent there. On teardown, reconciles back according to `apply_back.mode`.

### Example

```hcl
workspace clone {
  base           = "."
  excludes       = ["node_modules", ".git", "build", "!.important"]
  preserve_mtime = true

  apply_back {
    mode     = sync
    excludes = ["*.md"]
  }
}
```

### Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `base` | `string` | Required | — | Source directory used as the clone seed. |
| `source` | `ident` / `string` | Optional | — | Named `source` block, or path sugar equivalent to a directory passthrough source. Mutually exclusive with `base`. |
| `remote` | `string` | Optional | — | Remote URL passed verbatim to the clone backend. iter does not interpret it. |
| `excludes` | `list(string)` | Required | — | Clone-time exclude patterns. `[]` explicitly means "skip nothing"; omitting the field is not allowed. Supports `!pattern` negation to rescue specific paths. See [Glob patterns](#glob-patterns). |
| `includes` | `list(string)` | Optional | `[]` | Clone-time rescue patterns. Entries here win over matching entries in `excludes`; paths matching neither list always enter the workspace. `[]` means "no overrides". |
| `preserve_mtime` | `bool` | Required | — | Whether to preserve source mtimes during the copy. |
| `apply_back` | `block` | Required | — | Teardown-time reconciliation block. See [`apply_back` block](#apply_back-block). |

### `apply_back` block

The two filter phases (clone-time on the parent block, apply-back-time inside `apply_back`) are **independent**: the clone-time `excludes`/`includes` decide what enters the workspace; the `apply_back` block decides what propagates back to base on teardown. To skip a path in both phases, list it in both filters.

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `mode` | `enum { sync \| discard \| merge }` | Required | — | Reconciliation strategy. See below. |
| `excludes` | `list(string)` | Optional | `[]` | Apply-back-time exclude patterns. Must be `[]` when `mode = discard`. Supports `!pattern` negation to rescue specific paths. See [Glob patterns](#glob-patterns). |
| `includes` | `list(string)` | Optional | `[]` | Apply-back-time whitelist. When non-empty, only matching files sync back; all others are excluded. Must be `[]` when `mode = discard`. |

#### `mode` values

| Value | Meaning |
| --- | --- |
| `sync` | Copy temp → base; delete files in base that disappeared from temp. Full two-way sync. |
| `discard` | Do not reconcile. Temp is thrown away on teardown. Useful for read-only investigation. |
| `merge` | Copy new/modified files temp → base. Deletions are **not** propagated. |

#### Asymmetric filtering

Use the two filter phases independently when the agent should *see* a path but its writes should *not* leak back. The motivating case: an agent reads existing `.md` documentation but should not be able to scribble new `.md` files into the worktree — except for `docs/config/` which it owns.

```hcl
workspace clone {
  base           = "./worktree"
  excludes       = [".git"]              # clone-time: hide .git from the agent
  preserve_mtime = false

  apply_back {
    mode     = sync
    excludes = ["*.md", "!docs/config/**"]  # block .md except docs/config/
  }
}
```

### Glob patterns

Both filter phases accept glob patterns matched against the **path relative to the workspace root**.

| Token | Meaning |
| --- | --- |
| `*` | Match any sequence of characters within a single path segment (does not cross `/`). |
| `?` | Match exactly one character. |
| `**` | Match any number of path segments (including zero). |
| `dir/**` | Match every descendant of `dir/`. |

**Bare patterns match basenames anywhere.** A pattern with no `/` matches the basename at any depth. `excludes = ["node_modules"]` matches both `./node_modules/foo` and `./vendor/bar/node_modules/baz`. Use `**/` to anchor differently if needed (paths are matched relative to the workspace root, so a leading `/` does not anchor — there is no leading slash on the path being matched).

**Directory patterns auto-cover descendants.** A pattern that matches `dir` also implicitly matches `dir/**`, so descendants are excluded too — no "empty `target/` left behind" footgun.

**`includes` semantics differ per phase.** Clone-time `includes` only *rescue*: an include wins over a matching exclude, and a path matching neither list still enters the workspace. Apply-back `includes` are a *whitelist*: when non-empty, only matching paths propagate back — everything else is blocked regardless of the apply-back `excludes`. Use apply-back `includes` to allow a specific set of paths back and reject everything else; use clone-time `includes` to carve exceptions out of clone-time `excludes`.

**`excludes` supports `!pattern` negation.** A pattern prefixed with `!` rescues paths that would otherwise be excluded. `excludes = ["*.md", "!docs/config/**"]` excludes all `.md` files except those under `docs/config/`. This is the canonical way to carve a hole in an exclusion.

---

## `workspace sandbox`

Same clone steps as `workspace clone`, then runs the agent inside a kernel-level sandbox. Networking, filesystem access, and executable whitelisting are all configurable.

A sandbox declaration defines the **upper bound** (what the project permits). The agent's own `sandbox_requirements` (lower bound) are unioned in to produce the effective policy.

### Example

```hcl
workspace sandbox {
  base           = "."
  excludes       = ["build", "cache", "node_modules"]
  includes       = []
  preserve_mtime = true

  apply_back {
    mode = merge
  }

  policy {
    network             = all
    allow_read_outside  = ["/etc/hosts", "/etc/resolv.conf"]
    allow_write_outside = ["/tmp"]
    extra_deny_paths    = ["/Users/me/.ssh"]
    allow_exec          = ["/usr/bin/git", "/usr/bin/cargo"]
  }
}
```

### Arguments

All arguments of `workspace clone` plus:

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `policy` | `block` | Required | — | Sandbox policy. See below. |

### `policy` nested block

AST: `SandboxPolicyDef`.

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `network` | `enum` / `list(string)` | Required | — | Outbound network access rule. No default — the project must state it explicitly. |
| `allow_read_outside` | `list(string)` | Optional | `[]` | Absolute paths outside the workspace tmpdir the agent may read. |
| `allow_write_outside` | `list(string)` | Optional | `[]` | Absolute paths outside the workspace tmpdir the agent may write. |
| `extra_deny_paths` | `list(string)` | Optional | `[]` | Absolute paths explicitly denied. Deny beats allow. |
| `allow_exec` | `list(string)` | Optional | `[]` | Absolute paths of binaries the sandbox may `execve`. Empty means "inherit backend default" (allow all). On macOS, a non-empty list restricts `process-exec` to the listed paths. **Linux (bwrap):** not yet implemented — the field is accepted but has no effect. See [Platform notes](#allow_exec-platform-notes). |

#### `network` values

AST: `SandboxNetworkDef`.

| Value | Meaning |
| --- | --- |
| `off` | Deny all outbound networking. |
| `all` | Allow all outbound networking. |
| `["host1", "host2", ...]` | Allow only the listed hostnames. The list is unioned with the agent's own `network_hosts`. |

### `allow_exec` platform notes

| Platform | Behaviour |
| --- | --- |
| **macOS** (`sandbox-exec`) | Fully implemented. An empty list emits a blanket `(allow process-exec)`. A non-empty list emits a single `(allow process-exec ...)` block with one `(literal "...")` per entry, restricting execve to those paths only. Listed paths are also added to the `file-read-data` block so the kernel can read the binary image. |
| **Linux** (`bwrap`) | **Not yet implemented.** `bwrap` has no built-in execve filter. The field is accepted in the Iterfile but has no runtime effect on Linux. A future release may add support via selective bind-mounts or a seccomp filter. |

---

## See Also

- [`iterfile/agent.md`](agent.md) — agents declare their own `sandbox_requirements` that combine with the workspace policy.
- [`iterfile/on.md`](on.md) — `workspace_setup_starting`, `workspace_setup_finished`, `workspace_teardown_starting`, `workspace_teardown_finished` lifecycle events (plus the per-runner `runner_starting` / `runner_finished` pair).
