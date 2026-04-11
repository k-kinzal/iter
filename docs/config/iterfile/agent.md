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
| [`copilot`](#agent-copilot) | GitHub Copilot (`gh copilot`) | ✔ | ✔ |
| [`cursor`](#agent-cursor) | Cursor | ✘ | ✘ |
| [`cline`](#agent-cline) | Cline | ✘ | ✘ |
| [`opencode`](#agent-opencode) | opencode | ✘ | ✘ |
| [`generic`](#agent-generic) | Arbitrary argv | ✘ | ✘ |

Every named kind (all but `generic`) carries a required `command` field plus a pass-through `args` list. iter prepends mode-specific defaults (`--print`, `exec`, etc.) and appends `args` after them.

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

---

## `agent gemini`

Google Gemini.

### Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `mode` | `enum { interactive \| print }` | Required | — | CLI invocation mode. |
| `command` | `string` | Required | — | Binary name or absolute path. |
| `args` | `list(string)` | Optional | `[]` | Extra arguments. |

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

---

## `agent cursor`

Cursor.

### Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `command` | `string` | Required | — | Binary name or absolute path. |
| `args` | `list(string)` | Optional | `[]` | Extra arguments. |

No `mode` field.

---

## `agent cline`

Cline.

### Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `command` | `string` | Required | — | Binary name or absolute path. |
| `args` | `list(string)` | Optional | `[]` | Extra arguments. |

---

## `agent opencode`

opencode.

### Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `command` | `string` | Required | — | Binary name or absolute path. |
| `args` | `list(string)` | Optional | `[]` | Extra arguments. |

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

---

## See Also

- [`iterfile/prompt.md`](prompt.md) — the prompt(s) the agent receives.
- [`iterfile/runner.md`](runner.md) — the loop that runs the agent.
- [`iterfile/on.md`](on.md) — `agent_starting` and `agent_finished` events.
