# Iterfile

An `Iterfile` defines a **single self-contained iter service**. It is the Dockerfile-equivalent in the iter world.

- Run directly: `iter run [PATH]` (defaults to `./Iterfile`).
- Reference from compose: `service <name> { build = "./Iterfile" }`.

## Top-Level Sections

| Section | Count | Required | Page |
| --- | :---: | :---: | --- |
| `arg <name>` | 0–N | Optional | (below) |
| `queue <kind>` | 0–1 | Optional | [`iterfile/queue.md`](iterfile/queue.md) |
| `workspace <kind>` | 0–1 | Optional (see below) | [`iterfile/workspace.md`](iterfile/workspace.md) |
| `agent <kind>` | 0–1 | Optional (see below) | [`iterfile/agent.md`](iterfile/agent.md) |
| `runner` | 0–1 | Optional (see below) | [`iterfile/runner.md`](iterfile/runner.md) |
| `prompt` | 0–N | Optional | [`iterfile/prompt.md`](iterfile/prompt.md) |
| `on <event>` | 0–N | Optional | [`iterfile/on.md`](iterfile/on.md) |

An `Iterfile` is permitted to be **partial**. A webhook-handler Iterfile might omit `workspace` / `agent` / `runner`; a worker-only Iterfile might omit `prompt`. To run standalone via `iter run`, the file needs `workspace` + `agent` + `runner` + `prompt`.

## Minimal Example

The smallest Iterfile that runs under `iter run`: `workspace` + `agent` + `runner` + `prompt`, with no `queue` (so `runner.behavior` must be `loop`) and no `on` handlers. Every field shown is required.

```hcl
workspace local {
  base = "."
}

agent claude {
  mode    = print
  command = "claude"
}

runner {
  continue_on_error = false
  behavior          = loop
}

prompt "Improve the codebase."
```

## Arg Declarations

`arg` declares a named parameter whose value is resolved at load time, before the runner starts. Args are referenced in string fields via `{{arg.<name>}}`.

```hcl
arg model = "gpt-4o"
arg worktree_name
```

- With a default: `arg <name> = "<value>"`. The default is used when no override is supplied.
- Without a default (required): `arg <name>`. The caller must supply a value via `--arg <name>=<value>` (CLI) or `args { <name> = "<value>" }` (compose).

Arg names must start with a letter or underscore and contain only ASCII alphanumerics and underscores. Duplicate names are rejected at parse time.

### Overrides

**CLI:** `iter run --arg model=claude-sonnet --arg worktree_name=exp-1`

**Compose:**

```hcl
service explorer {
  build = "./Iterfile"
  args {
    model          = "claude-sonnet"
    worktree_name  = "exp-1"
  }
}
```

Overrides take precedence over Iterfile defaults. An override naming an undeclared arg is an error.

### Template Rendering

`{{arg.*}}` references are expanded in all string fields at load time: workspace paths, agent command/args, queue URLs, prompt bodies, and event shell commands. They are distinct from runtime templates like `{{signal.*}}` and `{{metadata.*}}`, which are rendered per-iteration.

## Execution Model

The runner loop shape is determined by `runner.behavior`:

- `behavior = wait` — block on the queue until a Signal arrives. Requires a `queue` block.
- `behavior = loop { delay_secs = N }` — synthesise an empty Signal each iteration; `queue` is optional.

The runner-level lifecycle (fires once per `iter run`):

- `on runner_starting` fires before the first iteration.
- `on runner_finished` fires just before `iter run` returns, regardless of why the runner stopped.

Each iteration:

1. Pull a Signal (from the queue, or synthesised).
2. `on signal_received` fires.
3. `on workspace_setup_starting` → build workspace → `on workspace_setup_finished`.
4. `on agent_starting` → run agent → `on agent_finished`.
5. `on workspace_teardown_starting` → tear down workspace → `on workspace_teardown_finished`.
6. On failure at any step, `on runner_error` fires.

Each section page documents its own fields and the events it emits.

## Full Example

Every top-level section present, every optional field populated. Uses `workspace sandbox` (the most feature-rich workspace kind) and declares a handler for every lifecycle event.

```hcl
arg environment = "staging"
arg repo_url

queue redis {
  url = "redis://localhost:6379"
  key = "iter:signals"
}

workspace sandbox {
  base           = "."
  remote         = "{{arg.repo_url}}"
  excludes       = ["node_modules", ".git", "build"]
  includes       = [".important"]
  preserve_mtime = true

  apply_back {
    mode = sync
  }

  policy {
    network             = ["api.anthropic.com"]
    allow_read_outside  = ["/etc/hosts", "/etc/resolv.conf"]
    allow_write_outside = ["/tmp"]
    extra_deny_paths    = ["/Users/me/.ssh"]
    allow_exec          = ["/usr/bin/git", "/usr/bin/cargo"]
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

prompt "Please continue."

prompt when metadata.task == "security" """
Security audit: review for vulnerabilities.
Focus on authentication, input validation, and secret handling.
"""

on runner_starting {
  shell "logger 'iter: runner starting'"
}

on signal_received {
  shell "logger 'iter: signal {{signal.id}} received'"
}

on workspace_setup_starting {
  shell "logger 'iter: preparing workspace'"
}

on workspace_setup_finished {
  shell "npm install --no-audit --no-fund"
}

on agent_starting {
  shell "logger 'iter: starting agent'"
}

on agent_finished {
  shell "git -C {{workspace.path}} status --short"
}

on workspace_teardown_starting {
  shell "logger 'iter: tearing down'"
}

on workspace_teardown_finished {
  shell "curl -fsS https://example.com/healthz"
}

on runner_finished {
  shell "logger 'iter: runner stopped'"
}

on runner_error {
  shell "notify-error 'Runner failed: {{error.message}}'"
}
```

## See Also

- [`language.md`](language.md) — shared DSL syntax.
- [`compose.md`](compose.md) — the orchestrator that composes multiple Iterfiles.
