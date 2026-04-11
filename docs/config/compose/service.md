# compose.iter: `service`

Declares a named iter service. One or more per `compose.iter`.

AST: `NamedService`, `ServiceSource`, and `InlineService` in `iter_language/src/ast/compose.rs`.

## Syntax

A service body takes one of two shapes — **external** (points at an Iterfile) or **inline** (declares runner-side sections in place).

```hcl
# External
service <name> {
  build = "<path-to-Iterfile>"
  queue = "<queue-name>"   # optional when there is only one queue
}

# Inline
service <name> {
  queue = "<queue-name>"   # optional when there is only one queue

  workspace <kind> { ... }
  agent <kind>     { ... }
  runner           { ... }
  prompt ...
  on <event> { ... }
}
```

A single service body must be entirely external or entirely inline — `build` and any of the inline sections cannot coexist.

## Shared Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `queue` | `string` | Conditional | — | Name of the queue this service consumes from. Optional when the file declares exactly one queue. Required otherwise. |

## External services (`build`)

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `build` | `string` | Required | — | Path to the Iterfile that defines the service, resolved relative to the `compose.iter` file. |

### Queue override

If the referenced Iterfile contains its own `queue` block, the compose-level `queue = <name>` **overrides** it. This makes it easy to repurpose an Iterfile across environments without editing the Iterfile itself.

```hcl
queue main redis {
  url = "redis://localhost:6379"
  key = "iter:main"
}

service worker {
  build = "./Iterfile"
  queue = main
}
```

## Inline services

The inline body accepts the same section kinds an Iterfile can carry. Field-level schemas live on the section pages:

| Section | Count | Page |
| --- | :---: | --- |
| `workspace <kind>` | 0–1 | [`iterfile/workspace.md`](../iterfile/workspace.md) |
| `agent <kind>` | 0–1 | [`iterfile/agent.md`](../iterfile/agent.md) |
| `runner` | 0–1 | [`iterfile/runner.md`](../iterfile/runner.md) |
| `prompt` | 0–N | [`iterfile/prompt.md`](../iterfile/prompt.md) |
| `on <event>` | 0–N | [`iterfile/on.md`](../iterfile/on.md) |

Inline bodies are permitted to be partial the same way Iterfiles are: a webhook-style service may omit `workspace`/`agent`/`runner` if it only needs event handlers.

### Example

```hcl
queue main memory

service housekeeping {
  queue = main

  workspace clone {
    base           = "."
    excludes       = ["node_modules", ".git"]
    includes       = []
    preserve_mtime = true

    apply_back {
      mode = merge
    }
  }

  agent claude {
    mode    = print
    command = "claude"
  }

  runner {
    continue_on_error = true
    behavior          = loop { delay_secs = 30 }
  }

  prompt "Apply pending formatting and commit in small batches."

  on agent_finished {
    shell "git -C {{workspace.path}} status --short"
  }
}
```

## Choosing `build` vs. inline

- **Use `build`** when the service is reusable (CI also runs `iter run ./Iterfile`, teammates run it locally, etc.).
- **Use inline** when the service is only meaningful inside this compose topology, or when you want the full deployment visible in a single file.

## See Also

- [`compose/queue.md`](queue.md) — queue bindings.
- [`compose/trigger.md`](trigger.md) — signal sources.
- [`iterfile.md`](../iterfile.md) — the Iterfile referenced by `build`.
