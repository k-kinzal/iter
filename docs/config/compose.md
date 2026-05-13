# compose.iter

A `compose.iter` file orchestrates **multiple iter services and triggers** around one or more shared queues. It is the docker-compose-equivalent in the iter world.

- Run with `iter compose up [-f compose.iter]`.
- Each `service` either references an external `Iterfile` (`build = "./Iterfile"`) or is declared inline with the same sections an Iterfile carries.
- Each `trigger` publishes Signals to a named queue that one or more services consume.

## Top-Level Sections

| Section | Count | Required | Page |
| --- | :---: | :---: | --- |
| `queue <name> <kind>` | 1–N | Required | [`compose/queue.md`](compose/queue.md) |
| `service <name>` | 1–N | Required | [`compose/service.md`](compose/service.md) |
| `trigger <name> <kind>` | 0–N | Optional | [`compose/trigger.md`](compose/trigger.md) |
| `telemetry` | 0–1 | Optional | This page |

Unlike `Iterfile`, `compose.iter` has **no top-level** `workspace`, `agent`, `runner`, `prompt`, or `on` blocks. Those always live inside an inline `service` body (or inside the Iterfile a service references).

## Telemetry

`telemetry` configures OpenTelemetry export for the composed project. iter currently exports traces and correlated logs from the runner lifecycle: iteration, workspace setup, agent run, and workspace teardown spans. In `compose up`, the orchestrator receives the base service name and service subprocesses receive `<service_name>.<service>`.

```hcl
telemetry {
  service_name      = "iter-dev"
  service_namespace = "agents"
  endpoint          = "http://localhost:4318"
  protocol          = "http/protobuf"

  resource_attributes {
    "deployment.environment" = "dev"
    "team.name"              = "agent-tools"
  }
}
```

Fields:

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | `bool` | `true` | Disable export without deleting the block. |
| `service_name` | `string` | Compose project slug | Base OTel `service.name`. |
| `service_namespace` | `string` | unset | OTel `service.namespace` resource attribute. |
| `endpoint` | `string` | `http://localhost:4318` | OTLP HTTP collector endpoint. `/v1/traces` and `/v1/logs` are appended automatically when omitted. |
| `protocol` | `string` | `"http/protobuf"` | OTLP protocol. Only `"http/protobuf"` is supported. |
| `resource_attributes` | block of string values | `{}` | Additional resource attributes. String field names allow dotted OTel keys. |

Agent trace context propagation is driver-specific. iter injects W3C
`traceparent` / `tracestate` into Claude Code `--print` and Codex `exec`,
because those entry points are known to read environment-carried context. For
GitHub Copilot CLI, iter injects the driver-specific `COPILOT_TRACE_PARENT`
environment variable, which Copilot CLI 1.0.43 reads into its SDK config and
forwards as `X-Copilot-Traceparent` on Copilot API calls. All agent subprocesses
that have verified OTel behavior also receive correlation resource attributes
such as `iter.signal.id`, `iter.signal.kind`, `iter.agent.driver`, and
`iter.workspace.path`, so separate agent traces remain joinable by attributes
when an agent starts an independent trace. Gemini, Cursor, Cline, OpenCode, and
generic command drivers are not automatically injected unless their CLI support
is verified.

## Queue Binding

Every service and trigger must resolve to exactly one queue. Bindings are written with `queue = <name>` on services and `target = <name>` on triggers. The queue name is a bare identifier, not a quoted string.

When the compose file declares exactly one queue, the binding may be omitted — the semantic layer auto-resolves to that queue. With two or more queues declared, omitting the binding is a semantic error.

```hcl
queue main memory

# `queue = main` / `target = main` can be omitted because there is only one queue.
service worker { build = "./Iterfile" }
trigger tick cron { schedule = "*/5 * * * *" }
```

## Minimal Example

The smallest valid `compose.iter`: one queue + one service. `trigger` is optional (the runner can drive itself via `runner.behavior = loop` inside the Iterfile), and because the file declares exactly one queue, the `queue = main` binding on the service can be omitted.

```hcl
queue main memory

service worker {
  build = "./Iterfile"
}
```

## Full Example

Every top-level section present, every optional field populated. Shows both service forms (`build` and inline), plus `cron` and `webhook` triggers with their full compose-level field set.

```hcl
queue urgent redis {
  url = "redis://localhost:6379"
  key = "iter:urgent"
}

queue bulk file {
  path = "./.iter/queue-bulk"
}

telemetry {
  service_name      = "iter-dev"
  service_namespace = "agents"
  endpoint          = "http://localhost:4318"
  protocol          = "http/protobuf"

  resource_attributes {
    "deployment.environment" = "dev"
  }
}

# External service — defers to an Iterfile on disk.
service api_handler {
  build = "./services/api.Iterfile"
  queue = urgent
}

# Inline service — spells out every section in place.
service chores {
  queue = bulk

  workspace sandbox {
    base           = "."
    remote         = "https://github.com/example/repo.git"
    excludes       = ["node_modules", ".git", "build"]
    includes       = [".important"]
    preserve_mtime = true

    apply_back {
      mode = merge
    }

    policy {
      network             = ["api.anthropic.com"]
      allow_read_outside  = ["/etc/hosts"]
      allow_write_outside = ["/tmp"]
      extra_deny_paths    = ["/Users/me/.ssh"]
      allow_exec          = ["/usr/bin/git"]
    }
  }

  agent claude {
    mode            = print
    command         = "claude"
    args            = ["--dangerously-skip-permissions"]
    session_id_file = ".iter/session.txt"
  }

  runner {
    continue_on_error = true
    behavior          = loop { delay_secs = 60 }
  }

  prompt "Clean up stale TODOs and apply any trivial fixes."

  prompt when metadata.strict == "true" """
  Do not modify public API signatures.
  Require green CI before committing.
  """

  on agent_finished {
    shell "git -C {{workspace.path}} status --short"
  }

  on runner_error {
    shell "notify-team 'iter failed: {{error.message}}'"
  }
}

trigger nightly cron {
  target      = bulk
  schedule    = "0 3 * * *"
  timezone    = "UTC"
  at_startup  = false
  catch_up    = 300s
  jitter      = 10s
  priority    = normal
  max_signals = 100

  metadata {
    task = "audit"
  }
}

trigger github_alerts webhook {
  target = urgent
  host   = "0.0.0.0"
  port   = 8080
  path   = "/webhook/github"
  secret = env("GITHUB_WEBHOOK_SECRET")

  priority = high
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
      task   = "security"
      source = "github"
    }
  }
}
```

## Execution Model

`iter compose up` runs each service's Runner loop in a separate task and each trigger in its own producer task. All of them share the process; the queues coordinate work.

Triggers run **in-process** — they are not separate child processes and do not appear in `iter ps` or `iter compose ps`. Only services (the runners) are registered in `~/.iter/proc/`.

For one-process-per-Trigger deployments (Docker, systemd, stdin-driven pipelines), use the standalone binaries instead:

| Trigger kind | Standalone binary |
| --- | --- |
| `cron` | `iter-cron` |
| `watch` | `iter-watch` |
| `files` | `iter-files` |
| `command` | `iter-command` |
| `webhook` | `iter-webhook` |

The standalone binaries publish Signals into the same queue types and coexist with a Runner started elsewhere via `iter run` or `iter compose up`.

## Stateless Project Model

`iter compose` is **stateless** — it mirrors `docker compose` rather than `docker run --name`. The compose orchestrator is *not* registered in `~/.iter/proc/`, and there is no per-project state file. The only durable record of a project is the `iter.compose.*` labels stamped onto each child runner's `meta.json`:

| Label | Meaning |
| --- | --- |
| `iter.compose.project` | Project slug (see below). |
| `iter.compose.service` | Service name from the compose plan. |
| `iter.compose.orchestrator_pid` | pid of the orchestrator that spawned the runner. |
| `iter.compose.orchestrator_start_time` | Round-trippable start-time fingerprint, paired with `pid` for `kill -0` + start-time cross-check (handles pid reuse). |

`compose ls`, `compose ps`, and `compose down` reconstruct project state by reading these labels off the local registry — the same way `docker compose ps` reads container labels.

### Project Name

The project slug defaults to the canonical basename of the compose file's parent directory, normalised by the same function `docker compose` v2 uses on the basename (`compose-go`'s `NormalizeProjectName`): lowercase, drop everything outside `[a-z0-9_-]` (spaces, dots, and non-ASCII code points are silently stripped — not rejected), then trim leading `_` / `-`. So a directory named `Obsidian Vault` produces the slug `obsidianvault`, and `My.Project.v2` becomes `myprojectv2`. Override with:

- `-p` / `--project-name <name>` on `compose up`, `compose ps`, `compose down`.
- `COMPOSE_PROJECT_NAME` environment variable.

iter applies the same normalisation to overrides and the env var, so `-p "Foo Bar"` and `COMPOSE_PROJECT_NAME="Foo Bar"` both resolve to `foobar`. (This is one place iter is intentionally more lenient than `docker compose` v2, which rejects unnormalised input on those two paths and only normalises the directory basename.) Validation only fires when the normalised string is empty (e.g. a directory named `!!!`), in which case pass `-p` explicitly.

Same slug = same project. Two compose files in different directories whose basenames normalise to the same value collide unless one passes `-p` (matches `docker compose`).

### Subcommands

| Command | Purpose |
| --- | --- |
| `iter compose up [-d]` | Start the orchestrator (foreground or detached). With `-d`, the orchestrator forks, sets a new session, redirects stdio to `/dev/null`, and exits the parent shell. |
| `iter compose up SERVICE [...] -d` | Start only the named service(s) as detached subprocesses. Requires a URL-addressable queue. |
| `iter compose validate` | Parse and semantic-check the compose file. |
| `iter compose config` | List the telemetry, queues, services, and triggers declared in the file (static plan listing — no runtime queries). |
| `iter compose ls` | List every active project, grouped from runner labels across the local registry. |
| `iter compose ps` | List the runners belonging to a single project. |
| `iter compose down` | Send `SIGTERM` to every runner in a project and to its orchestrator (discovered via labels). Escalates to `SIGKILL` after `--timeout` seconds (default 30). |
| `iter compose down SERVICE [...]` | Send `SIGTERM` only to the named service runner(s), leaving the orchestrator and siblings running. |

`iter compose up -d` refuses to start a second orchestrator for a project whose previous orchestrator is still alive — the check uses the same labels-based discovery as `compose ls`.

### Targeted Service Restart

When one Iterfile referenced by a `compose.iter` changes, targeted up/down lets the operator restart only the affected service:

```sh
iter compose down worker-a
iter compose up worker-a --detach
```

Both commands accept bare service names and explicit `service/NAME` references. Multiple services can be targeted in one invocation:

```sh
iter compose down worker-a worker-b
iter compose up worker-a worker-b --detach
```

The `--source` flag selects services by their Iterfile path instead of by name:

```sh
iter compose down --source ./worker-a/Iterfile
iter compose up --source ./worker-a/Iterfile --detach
```

If multiple services share the same Iterfile, all matching services are selected.

Targeted `compose up` requires `--detach` because each service runs as its own subprocess. The service's queue must be URL-addressable (`file://`, `redis://`, etc.); non-addressable queues fail with an actionable diagnostic.

When the project already has a live orchestrator, targeted `up` reuses its identity in the service labels so `compose ps` and `compose down` see the new service as part of the same project.

When no targets are supplied, `compose up` and `compose down` retain their project-wide behaviour.

### Logs

The orchestrator's stdout/stderr is discarded under `-d` (it has no registry record to write into). For service logs, use `iter logs <runner-id>` against any of the runners listed by `iter compose ps`. For trigger debugging, run the orchestrator in the foreground (`iter compose up` without `-d`) so trigger output reaches your terminal directly.

## See Also

- [`iterfile.md`](iterfile.md) — the self-contained single-service counterpart.
- [`compose/queue.md`](compose/queue.md) — named queue declarations.
- [`compose/service.md`](compose/service.md) — `build` vs. inline services.
- [`compose/trigger.md`](compose/trigger.md) — trigger declarations.
