# Breadth examples — full Iterfiles from widest to narrowest

Each example is a complete, runnable Iterfile. The structural settings and
prompt work together to establish the exploration breadth; changing either
one independently shifts the effective breadth.

---

## Widest: no goal, no carry-over, no history

Every iteration is independent. The agent sees only the workspace snapshot
and nothing else — no session log, no file timestamps, no git history, no
handoff notes from prior turns.

```hcl
workspace sandbox {
  base           = "."
  excludes       = [".git"]
  includes       = []
  preserve_mtime = false

  apply_back {
    mode = discard
  }

  policy {
    network = all
  }
}

agent claude {
  mode    = print
  command = "claude"
}

runner {
  continue_on_error = true
  behavior          = loop { delay_secs = 60 }
}

prompt "Please continue."
```

Characteristics:
- `excludes = [".git"]` — no commit history.
- `preserve_mtime = false` — no file-timestamp signal.
- No `session_id_file` — fresh session every iteration.
- `apply_back { mode = discard }` — writes are thrown away; no carry-over.
- Prompt is goalless and method-free.

---

## Wide: goal, workspace carry-over, periodic disruption

The agent has a broad destination but chooses its own path. Workspace files
persist between iterations via `apply_back { mode = sync }`, giving the
agent a light continuity channel. A periodic guard forces direction
changes.

```hcl
workspace sandbox {
  base           = "."
  excludes       = [".git"]
  includes       = []
  preserve_mtime = false

  apply_back {
    mode = sync
  }

  policy {
    network = all
  }
}

agent claude {
  mode    = print
  command = "claude"
}

runner {
  continue_on_error = true
  behavior          = loop { delay_secs = 60 }
}

prompt when metadata.prompt == "" "Please continue toward better error handling."

prompt when metadata.prompt != "" """
{{metadata.prompt}}
"""

prompt when iteration.count % 50 == 0 """
The current codebase has problems. Identify the issues and fix them.
"""

```

Characteristics:
- Same factor removal as Widest (no git, no mtime, no session log).
- `apply_back { mode = sync }` — workspace files carry over (the single
  continuity channel).
- Goal named ("error handling") but no lenses, no method.
- Course-correction signal replaces the base prompt (mutually exclusive
  guards).
- Periodic disruption via `count % 50`.

---

## Medium: goal with lenses, workspace carry-over, history signals

The agent has a mission area and named angles to examine. Iteration state
is used only in guards, not in the prompt body.

```hcl
workspace sandbox {
  base           = "."
  excludes       = [".git"]
  includes       = []
  preserve_mtime = false

  apply_back {
    mode = sync
  }

  policy {
    network = all
  }
}

agent claude {
  mode    = print
  command = "claude"
}

runner {
  continue_on_error = true
  behavior          = loop { delay_secs = 60 }
}

prompt when metadata.prompt == "" """
Explore error handling in the codebase.

Lenses:
- Panic vs Result boundaries.
- Error context propagation.
- User-facing error messages.
"""

prompt when metadata.prompt != "" """
{{metadata.prompt}}
"""

prompt when iteration.count % 7 == 0 """
Change direction: pick a lens you have not tried yet, or invent a new one.
"""

```

Characteristics:
- Same structural width as Wide.
- Lenses constrain the search space to named angles (but the agent can
  still invent new ones via the disruption cue).
- No method prescribed — the agent decides how to examine each lens.

---

## Narrow: goal with method, session continuity

The agent has a mission, a method, and persistent memory across
iterations. The prompt explicitly prescribes how to work.

```hcl
workspace clone {
  base           = "."
  excludes       = [".git"]
  includes       = []
  preserve_mtime = true

  apply_back {
    mode = sync
  }
}

agent claude {
  mode            = print
  command         = "claude"
  session_id_file = ".iter/session.txt"
}

runner {
  continue_on_error = true
  behavior          = loop { delay_secs = 60 }
}

prompt """
Improve test coverage for the queue subsystem.

How to proceed:
- Find an untested code path.
- Write a failing test.
- Make it pass.
- Commit with a descriptive message.

Iteration {{iteration.count}} / previous_result={{iteration.previous_result}}
"""
```

Characteristics:
- `preserve_mtime = true` — file timestamps visible.
- `session_id_file` — agent session persists across iterations.
- Method prescribed step by step.
- `iteration.*` in the prompt body — the agent knows its position in the
  sequence.
- No disruption cues — the agent stays on track.

---

## Narrowest: specific task, full context, single shot

The agent receives a concrete deliverable with full project context. This
is execution, not exploration.

```hcl
workspace local {
  base = "."
}

agent claude {
  mode            = print
  command         = "claude"
  session_id_file = ".iter/session.txt"
}

runner {
  continue_on_error = false
  behavior          = loop
}

prompt "Add a unit test for Queue::dequeue timeout behaviour in iter_core/src/queue/file.rs."
```

Characteristics:
- `workspace local` — no isolation, full project access including `.git`.
- `session_id_file` — session persists.
- `continue_on_error = false` — fail fast.
- Prompt names the exact file and behaviour to test.

---

## Breadth summary

| Level | `apply_back` | `session_id_file` | `preserve_mtime` | `.git` visible | Prompt shape | `iteration.*` |
| --- | --- | --- | --- | --- | --- | --- |
| Widest | `discard` | no | `false` | no | `"Please continue."` | guards only |
| Wide | `sync` | no | `false` | no | goal, no method | guards only |
| Medium | `sync` | no | `false` | no | goal + lenses | guards only |
| Narrow | `sync` | yes | `true` | no | goal + method | in body |
| Narrowest | n/a (local) | yes | n/a | yes | specific task | in body |

Each row adds factors relative to the one above. The prompt and
structural settings move together — widening the prompt while narrowing
the structure (or vice versa) produces inconsistent behaviour.

---

## See Also

- [`reference/prompt-guide.md`](prompt-guide.md) — prompt patterns and
  anti-patterns for each breadth level.
- [`reference/blocks.md`](blocks.md) — field-level reference for all
  Iterfile blocks.
