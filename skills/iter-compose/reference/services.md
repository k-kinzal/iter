# compose.iter `service` — full reference

AST: `NamedService`, `ServiceSource`, `InlineService` in
`iter_language/src/ast/compose.rs`.

## Two Forms

A service body is either **external** (`build = "<path>"`) or **inline**
(declares Iterfile-style sections in place). The two forms are mutually
exclusive — `build` cannot coexist with any inline section.

```hcl
# External
service <name> {
  build = "<path-to-Iterfile>"
  queue = <queue-name>     # bare ident; optional when exactly one queue
}

# Inline
service <name> {
  queue = <queue-name>     # bare ident; optional when exactly one queue

  workspace <kind> { ... }
  agent <kind>     { ... }
  runner           { ... }
  prompt ...
  on <event> { ... }
}
```

## Shared Field

| Field | Type | Required | Description |
| --- | --- | :---: | --- |
| `queue` | bare identifier | conditional | Name of the queue this service consumes from. Optional iff the file declares exactly one queue. |

## External Services (`build`)

| Field | Type | Required | Description |
| --- | --- | :---: | --- |
| `build` | string | ✔ | Path to the Iterfile that defines the service, resolved relative to the `compose.iter` file. |

### Queue Override

If the referenced Iterfile contains its own `queue` block, the compose-level
`queue = <name>` **overrides** it. Use this to repurpose one Iterfile across
topologies without editing it.

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

## Inline Services

The body accepts the same kinds an Iterfile carries; field-level schemas
live on the Iterfile per-block pages and in the **iter-iterfile** skill's
`reference/blocks.md`.

| Section | Count | Reference |
| --- | :---: | --- |
| `workspace <kind>` | 0–1 | `docs/config/iterfile/workspace.md` |
| `agent <kind>` | 0–1 | `docs/config/iterfile/agent.md` |
| `runner` | 0–1 | `docs/config/iterfile/runner.md` |
| `prompt [when …] "<body>"` | 0–N | `docs/config/iterfile/prompt.md` |
| `on <event> { … }` | 0–N | `docs/config/iterfile/on.md` |

Inline bodies are permitted to be partial (a webhook-style service may omit
`workspace` / `agent` / `runner` if it only carries event handlers).

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

## Choosing `build` vs Inline

- **`build`** when the service is reusable — CI also runs
  `iter run ./Iterfile`, teammates use it locally, etc.
- **Inline** when the service is only meaningful inside this compose
  topology, or when you want the full deployment legible in one file.
