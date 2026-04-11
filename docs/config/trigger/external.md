# Trigger: external / user-defined

Arbitrary user-defined trigger kind. iter does not interpret the fields; they are preserved verbatim and handed to the runner, which is expected to recognise the kind name and consume the configuration.

AST: `TriggerDecl::External` in `iter_language/src/ast/trigger.rs`.

## Syntax

```hcl
trigger <name> <user-kind> {
  target = <queue-name>   # optional when there is only one queue

  # Any fields, any nesting.
  <key> = <value>
  ...
}
```

Any `<kind>` identifier that iter does not recognise as `loop`, `cron`, `watch`, `files`, `command`, or `webhook` is treated as an external trigger. The entire body is captured as an untyped field map and passed through.

## Arguments

Unlike the built-in kinds, external triggers do **not** have any reserved fields at the language layer — everything in the body, including `target`, is preserved verbatim for the runner that owns the kind. If you want the shared trigger fields (`priority`, `metadata`, `max_signals`) to apply automatically, use a built-in kind instead.

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| any key | any `Value` | Optional | — | Captured verbatim. Interpreted by the runner implementation that claims this kind. |

Values accept the full language [`Value`](../language.md) grammar — strings, integers, durations, booleans, lists, maps, nested blocks.

## Semantics

- The parser preserves every field in source order.
- No validation beyond "the body parses as valid DSL" is performed at the language layer.
- The runner is responsible for rejecting unknown or malformed configuration at load time.

## Use Cases

- Prototyping a new trigger before promoting it to a first-class kind.
- Wiring up a proprietary event source without patching iter itself.
- Plugins that ship their own runner-side trigger implementation.

## Examples

### A custom Sentry trigger

```hcl
trigger sentry sentry_events {
  target = main

  org     = "my-org"
  project = "backend"
  token   = env("SENTRY_TOKEN")

  query = "is:unresolved level:error"
  poll  = 60s

  metadata {
    source = "sentry"
  }
}
```

### A custom graph-update trigger with nested blocks

```hcl
trigger refresh graph_events {
  target = main

  source {
    endpoint = "https://graph.example.com/stream"
    auth     = env("GRAPH_TOKEN")
  }

  filter {
    types = ["commit", "review_requested"]
  }

  max_signals = 10000
}
```

## See Also

- [`compose/trigger.md`](../compose/trigger.md) — shared arguments.
- [`language.md`](../language.md) — value grammar used inside external bodies.
