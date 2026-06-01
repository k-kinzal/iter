# Prompt guide — breadth as the primary axis

A prompt controls exploration breadth independently of the Iterfile's
structural settings. A workspace configured for wide exploration can be
narrowed back down by the prompt; a narrow configuration cannot be widened
by one. This guide orders prompt patterns from widest to narrowest.

---

## Level 0: Maximum breadth

The agent receives no direction. Each iteration starts from the workspace
state alone — no mission, no method, no evaluation criteria.

```hcl
prompt "Please continue."
```

The agent decides what to look at, what to try, and when to stop. This is
the widest prompt possible. It pairs naturally with `apply_back { mode =
discard }` (no carry-over between iterations) for absolute breadth, or
`apply_back { mode = sync }` when workspace-file handoff is the only
permitted continuity channel.

---

## Level 1: Goal without method

A destination is named but the path is unspecified. The agent can approach
the goal from any angle.

```hcl
prompt "Please continue toward better error handling."
```

The goal anchors exploration to a region of the problem space but does not
prescribe how to get there. Avoid listing sub-goals or lenses — each one
narrows.

---

## Level 2: Goal with lenses

Lenses enumerate the angles from which the agent should look. This focuses
exploration but also caps it — the agent is unlikely to discover angles
outside the listed set.

```hcl
prompt """
Explore error handling.

Lenses:
- Panic vs Result boundaries.
- Error context propagation.
- User-facing error messages.
"""
```

**Width trap:** naming existing abstractions in lenses (e.g. "the Runner's
error path", "Queue::dequeue failures") anchors the agent to the current
design. Prefer describing the *concern* ("error context propagation") over
the *location* ("iter_core/src/runner/mod.rs").

---

## Level 3: Goal with method

The prompt prescribes *how* to work, not just *what* to look at. This is
significantly narrower than Levels 0–2 because the agent can no longer
choose its own approach.

```hcl
prompt """
Improve test coverage.

How to proceed:
- Find an untested code path.
- Write a failing test.
- Make it pass.
"""
```

Each methodological instruction ("form a hypothesis", "verify with code",
"leave a handoff note") removes a degree of freedom.

---

## Level 4: Specific task

The prompt names a concrete deliverable. Exploration is minimal — the
agent executes rather than explores.

```hcl
prompt "Add a unit test for Queue::dequeue timeout behaviour."
```

---

## What narrows the prompt (often unintentionally)

### Handoff instructions

> "Leave a handoff note for the next turn."

This recreates session continuity through the workspace. If the structural
settings removed the session-log factor (`session_id_file` omitted), a
handoff instruction adds it back via a different channel.

### Iteration state in the prompt body

> `Iteration {{iteration.count}} / previous_result={{iteration.previous_result}}`

Embedding iteration count or result in the unconditional prompt gives the
agent a sense of sequential history. Use `iteration.*` in `when` guards
for conditional behaviour (direction changes, failure recovery) rather than
in the prompt body.

### Naming existing abstractions

> "Whether the Core abstractions (Runner / Signal-Queue / Trigger) draw
> their boundaries in the right places."

Listing existing names anchors the agent to the current decomposition. The
question "are these the right abstractions?" becomes difficult to ask when
the abstractions are named as givens.

### "Improvement" framing

> "Explore design **improvements**."

"Improvement" presupposes a known evaluation axis. The agent optimises
along that axis instead of discovering that the axis itself may be wrong.
Prefer neutral verbs: "explore", "examine", "probe".

### Prescriptive document references

> "Read the same project guidance every turn."

Referencing an opinionated document each iteration re-anchors the agent to
the document's worldview, suppressing independent observations.

---

## Prompt guards and breadth

### Direction-change cues (widen)

`iteration.count % N` is a tool for periodic disruption. The agent has
no memory of prior iterations (in Wide & Shallow), so the cue must
reference what the agent *can* see — the current state of the
workspace. Negating the current state forces the agent to look for
problems rather than continuing in the direction the workspace
suggests.

```hcl
prompt when iteration.count % 50 == 0 """
The current codebase has problems. Identify the issues and fix them.
"""
```

Note: `consecutive_failures` and `consecutive_successes` track
**runner stage** results (workspace setup errors, process spawn
failures, iteration timeouts), not agent-level results. They are
useful in `on runner_error` shell hooks for operational alerting, but
not in prompts — the agent cannot fix infrastructure failures by
changing its approach.

### Course-correction signals (replace, don't append)

When an external agent sends a course-correction signal via `iter enqueue
-m prompt="..."`, the signal prompt should **replace** the base prompt,
not append to it. Use mutually exclusive guards:

```hcl
prompt when metadata.prompt == "" "Please continue."

prompt when metadata.prompt != "" """
{{metadata.prompt}}
"""
```

If both prompts fire (the default when guards are not exclusive), the base
prompt's framing persists alongside the correction, limiting its
effectiveness.

---

## See Also

- [`reference/breadth-examples.md`](breadth-examples.md) — full Iterfile
  examples at each breadth level.
- [`reference/blocks.md`](blocks.md) — field-level reference for all
  Iterfile blocks.
- `docs/config/iterfile/prompt.md` — prompt syntax and `when` guard
  grammar.
