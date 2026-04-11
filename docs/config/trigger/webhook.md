# Trigger: `webhook`

Exposes an HTTP listener. Each incoming request is matched against per-event routes; matching routes emit a Signal.

AST: `TriggerDecl::Webhook`, `WebhookRoute`, and `SecretExpr` in `iter_language/src/ast/trigger.rs`.

Standalone binary: `iter-webhook`.

## Syntax

```hcl
trigger <name> webhook {
  target = <queue-name>   # optional when there is only one queue

  # Bind address — pick ONE form:
  host = "<bind host>"          # Optional, default 0.0.0.0 (pairs with `port`)
  port = <int>                  # Required unless `bind` is set
  bind = "<ADDR:PORT>"          # Mutually exclusive with `host` + `port`

  path   = "<http path>"
  secret = env("<VAR>") | file("./<path>") | "<literal>"   # Optional

  priority = low | normal | high | critical   # Optional, inherited by routes
  metadata { ... }                            # Optional, inherited by routes
  max_signals = <int>                         # Optional

  on "<event-pattern>" {
    when     = "<expression>"                   # Optional
    priority = low | normal | high | critical   # Optional, overrides trigger default
    metadata { ... }                            # Optional, merged over trigger metadata
  }
  # ...more `on` blocks
}
```

## Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `target` | queue ref | Conditional | — | Target queue (bare identifier). |
| `host` | string | Optional | `0.0.0.0` | Bind host. Mutually exclusive with `bind`. |
| `port` | integer | Conditional | — | Bind port. Required unless `bind` is set. |
| `bind` | string | Optional | — | Full `ADDR:PORT` string (equivalent to `iter-webhook --bind`). Mutually exclusive with `host` + `port`. |
| `path` | string | Required | — | HTTP path the listener serves. Requests to other paths return 404. |
| `secret` | secret | Optional | disabled | Shared secret used to verify incoming payloads. Resolves as `env("VAR")`, `file("./path")`, or a string literal. Each source decides its own verification scheme (HMAC header, body signature, etc.). |
| `priority` | `low \| normal \| high \| critical` | Optional | `normal` | Default priority for routes that do not set their own. |
| `metadata` | block | Optional | `{}` | Metadata base. Every route merges its own `metadata` block on top; on key collision the route wins. |
| `max_signals` | integer | Optional | unbounded | Stop after this many Signals. |

## `on` blocks (routes)

```hcl
on "<event-pattern>" {
  when     = "<expression>"
  priority = low | normal | high | critical
  metadata {
    key = "value-or-template"
  }
}
```

Each `on` block declares a route. The route key is a **quoted event name pattern**. The listener extracts the event from the incoming payload (the extraction convention is backend-dependent — e.g. GitHub's `X-GitHub-Event` header) and matches it against each route pattern in declaration order. The first match wins.

### Route fields

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `when` | string | Optional | always match | Additional guard expression evaluated against the parsed payload. Preserved verbatim; evaluated by the runner. Use to filter on body fields beyond the event name. |
| `priority` | `low \| normal \| high \| critical` | Optional | trigger `priority` or `normal` | Overrides the trigger-level priority for this route. |
| `metadata` | block | Optional | `{}` | Metadata template fields. Values may contain `{{...}}` placeholders referring to `payload.*`, `headers.*`, `query.*`, and `signal.*`. Merged over the trigger-level `metadata`; route keys win on collision. |

### Inheritance

If a route omits `priority`, the trigger-level `priority` applies; if neither is set, priority defaults to `normal`. Trigger-level `metadata` entries flow into every route automatically; a route's own `metadata` block overrides individual keys but does not have to restate the rest.

## Examples

### GitHub issues + security advisories

```hcl
trigger github webhook {
  target = main
  port   = 8080
  path   = "/webhook/github"
  secret = env("GITHUB_WEBHOOK_SECRET")

  priority = normal
  metadata {
    trigger = "github"
  }

  on "issues.opened" {
    metadata {
      source = "github"
      repo   = "{{payload.repository.full_name}}"
      issue  = "{{payload.issue.number}}"
    }
  }

  on "security_advisory" {
    priority = critical
    metadata {
      task = "security"
    }
  }
}
```

### Slack slash-command with `bind`

```hcl
trigger slack webhook {
  target = main
  bind   = "0.0.0.0:8081"
  path   = "/slack/iter"
  secret = env("SLACK_SIGNING_SECRET")

  on "/iter-run" {
    when     = "payload.text != ''"
    priority = high
    metadata {
      source  = "slack"
      user    = "{{payload.user_id}}"
      command = "{{payload.text}}"
    }
  }
}
```

### Minimal — single wildcard route, no secret

```hcl
trigger ping webhook {
  port = 8080
  path = "/ping"

  on "*" {}
}
```

### Secret from file

```hcl
trigger gh webhook {
  host   = "127.0.0.1"
  port   = 9000
  path   = "/hooks"
  secret = file("./secrets/github.txt")

  on "push" {
    metadata {
      repo = "demo"
    }
  }
}
```

## Standalone form

`iter-webhook` runs the listener in its own process (typical in Kubernetes behind an Ingress, or on a public LB). It publishes into a shared queue that Runner pods elsewhere consume from.

The CLI exposes the same fields as flags: `--bind ADDR:PORT`, `--path`, `--secret-env VAR`, `--secret-file FILE`, and `--route PATTERN[:PRIORITY]` (for simple route declarations). `--priority` and `--metadata` flags apply to routes that do not set their own — the compose equivalent is trigger-level `priority` / `metadata` inheritance.

## See Also

- [`compose/trigger.md`](../compose/trigger.md) — shared arguments.
- [`language.md`](../language.md) — secret expressions and placeholder syntax.
