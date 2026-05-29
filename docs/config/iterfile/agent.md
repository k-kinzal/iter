# Iterfile: `agent`

Declares the AI agent that runs inside the workspace. Optional — zero or one block per `Iterfile`. Also usable inside a `compose.iter` inline service.

AST: `AgentDecl` and `AgentMode` in `iter_language/src/ast/agent.rs`.

## Syntax

```hcl
agent <kind> {
  <fields>
}
```

## Supported Kinds

| Kind | Backing CLI | Has `mode` | Has `subcommand` |
| --- | --- | :---: | :---: |
| [`claude`](#agent-claude) | Claude Code (`claude`) | ✔ | ✘ |
| [`codex`](#agent-codex) | OpenAI Codex | ✔ | ✘ |
| [`gemini`](#agent-gemini) | Google Gemini | ✔ | ✘ |
| [`hermes`](#agent-hermes) | Nous Hermes (`hermes`) | ✔ | ✘ |
| [`antigravity`](#agent-antigravity) | Google Antigravity (`agy`) | ✔ | ✘ |
| [`copilot`](#agent-copilot) | GitHub Copilot (`gh copilot`) | ✔ | ✔ |
| [`cursor`](#agent-cursor) | Cursor | ✘ | ✘ |
| [`cline`](#agent-cline) | Cline | ✘ | ✘ |
| [`opencode`](#agent-opencode) | opencode | ✘ | ✘ |
| [`grok`](#agent-grok) | xAI Grok Build (`grok`) | ✘ | ✘ |
| [`generic`](#agent-generic) | Arbitrary argv | ✘ | ✘ |
| [`noop`](#agent-noop) | Built-in (no binary) | ✘ | ✘ |
| [`fake`](#agent-fake) | Built-in (no binary) | ✘ | ✘ |

Every named kind (all but `generic`, `noop`, and `fake`) carries a required `command` field plus a pass-through `args` list. iter prepends mode-specific defaults (`--print`, `exec`, etc.) and appends `args` after them.

`noop` and `fake` do not require any external binary — they run entirely in-process.

---

## `env` block

All agent kinds accept an optional `env { ... }` block that declares environment variables injected into the agent's child process. Keys must match `[A-Z][A-Z0-9_]*` (POSIX uppercase convention). Avoid using the `ITER_` prefix in env keys — that prefix is reserved for runtime overrides and internal iter variables.

### Syntax

```hcl
agent claude {
  mode    = print
  command = "claude"

  env {
    API_TOKEN   = "sk-default-token"
    DEBUG_LEVEL = "info"
  }
}
```

### Runtime overrides with `ITER_` prefix

Each declared key can be overridden at runtime by setting an environment variable with the `ITER_` prefix. For example, if the Iterfile declares `API_TOKEN = "default"`, setting `ITER_API_TOKEN=production-secret` in the shell overrides the value. Only keys declared in the `env` block can be overridden — `ITER_` variables without a matching declaration are ignored.

### Template expansion

Values support `{{arg.*}}` template syntax, the same as other string fields:

```hcl
agent claude {
  mode    = print
  command = "claude"

  env {
    WORKTREE_NAME = "{{arg.worktree}}"
  }
}
```

Runtime template references such as `{{signal.*}}` and `{{metadata.*}}` are not expanded in env values — they are passed to the child process as literal strings.

### Precedence

User-declared env vars are applied **before** iter-managed environment variables (OpenTelemetry attributes, hook context, sandbox prefixes). This means iter's internal variables always take precedence if there is a name collision.

---

## `AgentMode` values

Used by kinds that support the `mode` field.

| Value | Description |
| --- | --- |
| `interactive` | TTY-attached interactive mode. For human-in-the-loop use. |
| `print` | Non-interactive batch mode. The agent writes output and exits. For automation. |

---

## `agent claude`

Anthropic Claude Code.

### Examples

```hcl
agent claude {
  mode    = interactive
  command = "claude"
}

agent claude {
  mode            = print
  command         = "/usr/local/bin/claude"
  args            = ["--timeout", "600", "--dangerously-skip-permissions"]
  session_id_file = ".iter/session.txt"
}
```

### Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `mode` | `enum { interactive \| print }` | Required | — | CLI invocation mode. |
| `command` | `string` | Required | — | Binary name or absolute path. Resolved via `PATH`. |
| `args` | `list(string)` | Optional | `[]` | Extra arguments appended after iter-managed defaults. |
| `session_id_file` | `string` | Optional | — | File path (relative to workspace cwd) where iter persists a stable session id. On first invocation iter writes a fresh UUID v4; subsequent iterations read the same file and pass `--session-id <uuid>`. Omit to run each iteration as a fresh session. |
| `env` | `block { KEY = "value" }` | Optional | — | Environment variables injected into the child process. See [`env` block](#env-block). |

---

## `agent codex`

OpenAI Codex.

### Example

```hcl
agent codex {
  mode    = print
  command = "codex"
  args    = ["--model", "o1-preview"]
}
```

### Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `mode` | `enum { interactive \| print }` | Required | — | CLI invocation mode. |
| `command` | `string` | Required | — | Binary name or absolute path. |
| `args` | `list(string)` | Optional | `[]` | Extra arguments. |
| `env` | `block { KEY = "value" }` | Optional | — | Environment variables. See [`env` block](#env-block). |

---

## `agent gemini`

Google Gemini.

### Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `mode` | `enum { interactive \| print }` | Required | — | CLI invocation mode. |
| `command` | `string` | Required | — | Binary name or absolute path. |
| `args` | `list(string)` | Optional | `[]` | Extra arguments. |
| `env` | `block { KEY = "value" }` | Optional | — | Environment variables. See [`env` block](#env-block). |

---

## `agent hermes`

Nous Research Hermes Agent — an open-source, self-hosted AI coding agent.

### Examples

```hcl
agent hermes {
  mode    = print
  command = "hermes"
  args    = ["--yolo", "--max-turns", "30"]
}

agent hermes {
  mode    = interactive
  command = "hermes"
}
```

### Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `mode` | `enum { interactive \| print }` | Required | — | CLI invocation mode. |
| `command` | `string` | Required | — | Binary name or absolute path. |
| `args` | `list(string)` | Optional | `[]` | Extra arguments. |
| `env` | `block { KEY = "value" }` | Optional | — | Environment variables. See [`env` block](#env-block). |

Print mode uses `-z` (scripted mode, suppresses banners/spinners). Interactive mode uses `--tui`. In non-TTY environments, include `--yolo` in `args` to bypass tool-approval prompts. Session persistence is available via `--resume <id>` in `args`.

---

## `agent antigravity`

Google Antigravity CLI (`agy`), successor to Gemini CLI.

### Example

```hcl
agent antigravity {
  mode    = print
  command = "agy"
  args    = ["--print-timeout", "600"]
}

agent antigravity {
  mode            = print
  command         = "agy"
  conversation_id = "my-session"
}
```

### Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `mode` | `enum { interactive \| print }` | Required | — | CLI invocation mode. |
| `command` | `string` | Required | — | Binary name or absolute path. |
| `args` | `list(string)` | Optional | `[]` | Extra arguments. |
| `conversation_id` | `string` | Optional | — | Conversation identifier for session persistence. When set, iter passes `--conversation <id>` on every invocation so the agent resumes the same session. Omit to start a fresh conversation each iteration. |
| `env` | `block { KEY = "value" }` | Optional | — | Environment variables. See [`env` block](#env-block). |

---

## `agent copilot`

GitHub Copilot. Unusually, the CLI takes a subcommand between the binary and the positional prompt (`gh copilot suggest "..."`). The `subcommand` field overrides that insertion.

### Examples

```hcl
# Use iter's default subcommand
agent copilot {
  mode    = print
  command = "gh"
}

# Override the subcommand explicitly
agent copilot {
  mode       = print
  command    = "gh"
  subcommand = ["copilot", "suggest"]
  args       = ["--target", "shell"]
}

# Strip the subcommand entirely
agent copilot {
  mode       = print
  command    = "my-copilot-wrapper"
  subcommand = []
}
```

### Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `mode` | `enum { interactive \| print }` | Required | — | CLI invocation mode. |
| `command` | `string` | Required | — | Binary name or absolute path. |
| `subcommand` | `list(string)` | Optional | iter default | Tokens inserted between `command` and the positional prompt. Unset means iter picks a sane default. `[]` means "no subcommand". `[...]` overrides entirely. |
| `args` | `list(string)` | Optional | `[]` | Arguments appended between `subcommand` and the positional prompt. |
| `env` | `block { KEY = "value" }` | Optional | — | Environment variables. See [`env` block](#env-block). |

---

## `agent cursor`

Cursor.

### Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `command` | `string` | Required | — | Binary name or absolute path. |
| `args` | `list(string)` | Optional | `[]` | Extra arguments. |
| `env` | `block { KEY = "value" }` | Optional | — | Environment variables. See [`env` block](#env-block). |

No `mode` field.

---

## `agent cline`

Cline.

### Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `command` | `string` | Required | — | Binary name or absolute path. |
| `args` | `list(string)` | Optional | `[]` | Extra arguments. |
| `env` | `block { KEY = "value" }` | Optional | — | Environment variables. See [`env` block](#env-block). |

---

## `agent opencode`

opencode.

### Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `command` | `string` | Required | — | Binary name or absolute path. |
| `args` | `list(string)` | Optional | `[]` | Extra arguments. |
| `env` | `block { KEY = "value" }` | Optional | — | Environment variables. See [`env` block](#env-block). |

---

## `agent grok`

xAI Grok Build (`grok`). Headless-first: iter drives the official `grok -p` headless mode (the prompt is the value of `-p`) and auto-approves tool executions with `--always-approve`. There is no `mode` field — Grok's TUI and ACP (`grok agent stdio`) integrations are out of scope for this driver.

iter builds the command as:

```text
grok -p "<prompt>" --always-approve [-s <session-id>] [args...]
```

Authentication uses `XAI_API_KEY` (or a prior local login). Set it through the agent `env` block or rely on it being passed through by the sandbox (the `grok` sandbox profile passes `XAI_*` / `GROK_*`).

### Examples

```hcl
agent grok {
  command = "grok"
}

agent grok {
  command         = "grok"
  args            = ["--output-format", "json"]
  session_id_file = ".iter/session.txt"
}
```

### Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `command` | `string` | Required | — | Binary name or absolute path. Resolved via `PATH`. |
| `args` | `list(string)` | Optional | `[]` | Extra arguments appended after iter-managed headless flags (`-p`, `--always-approve`, `-s`). |
| `session_id_file` | `string` | Optional | — | File path (relative to workspace cwd) where iter persists a stable session id. On first invocation iter writes a fresh UUID v4; subsequent iterations read the same file and pass `-s <uuid>` so Grok resumes the same headless session. Omit to run each iteration as a fresh session. |
| `env` | `block { KEY = "value" }` | Optional | — | Environment variables injected into the child process. See [`env` block](#env-block). |

---

## `agent generic`

Run any program as an agent. iter prepends nothing and `execve`s the command as-is. The program is responsible for reading the prompt (from stdin or elsewhere).

### Examples

```hcl
agent generic {
  command = ["python", "./scripts/my-agent.py", "--verbose"]
}

agent generic {
  command = ["/usr/local/bin/my-runner"]
}
```

### Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `command` | `list(string)` | Required | — | argv vector. First element is the program; the rest are arguments. |
| `env` | `block { KEY = "value" }` | Optional | — | Environment variables. See [`env` block](#env-block). |

---

## `agent noop`

Does nothing. Exits immediately with success. No external binary required.

Use cases: verifying workspace setup/teardown in isolation, testing event handler wiring, benchmarking runner overhead, dry-running Iterfile configuration.

### Example

```hcl
agent noop {}
```

### Arguments

None. The body must be empty.

---

## `agent fake`

Configurable fake agent for verification testing. Produces deterministic file changes, STDIO output, and exit status through the real pipeline without a real AI agent binary.

The implementation exercises real infrastructure: `StdioSink` log capture, workspace filesystem writes (including `apply_back`), cancellation token handling, and exit status classification.

### Examples

```hcl
agent fake {
  exit_code   = 0
  delay_secs  = 2
  stdout      = ["processing file A", "processing file B", "done"]
  stderr      = ["warning: deprecated API"]
  files {
    "output/result.txt"  = "refactored content"
    "output/report.json" = """{"status": "ok", "changes": 3}"""
  }
  last_output = "Completed refactoring with 3 changes."
  turn_count  = 5
}

# Minimal — all defaults, behaves like noop
agent fake {}
```

### Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `exit_code` | `integer` | Optional | `0` | Process exit code. 0 = success, non-zero = failure. |
| `delay_secs` | `integer` | Optional | `0` | Simulated execution delay in seconds. Respects cancellation. |
| `stdout` | `list(string)` | Optional | `[]` | Lines written to stdout via the StdioSink. |
| `stderr` | `list(string)` | Optional | `[]` | Lines written to stderr via the StdioSink. |
| `files` | `block { "path" = "content" }` | Optional | `{}` | Files to create/overwrite in the workspace. Keys are relative paths. |
| `last_output` | `string` | Optional | — | Value for `AgentReport.last_output`. |
| `turn_count` | `integer` | Optional | — | Value for `AgentReport.turn_count`. |

---

## See Also

- [`iterfile/prompt.md`](prompt.md) — the prompt(s) the agent receives.
- [`iterfile/runner.md`](runner.md) — the loop that runs the agent.
- [`iterfile/on.md`](on.md) — `agent_starting` and `agent_finished` events.
