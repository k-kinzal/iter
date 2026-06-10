# Iterfile: `prompt`

Declares the prompt text sent to the agent each iteration. The prompt lives **inside the `runner` block** as a `prompt` field (or `prompt { ... }` match block); it is no longer a top-level section. Reusable prompt bodies are declared at top level with `prompt as <name>` and referenced by bareword. Also usable inside a `compose.iter` inline service.

AST: `PromptExpr`, `PromptValue`, `PromptArm`, `NamedPrompt`, and `PromptGuard` in `iter_language/src/ast/prompt.rs`.

## Syntax

A runner selects its prompt with one of two forms:

```hcl
runner {
  # ...bindings...

  # Single prompt — inline string or a bareword reference to a `prompt as <name>` definition.
  prompt = "<body>"

  # OR a match block — first true guard wins; `_` is the required default arm.
  prompt {
    <guard> => "<body>"
    <guard> => <named-prompt>
    _       => "<body>"
  }
}
```

A reusable prompt is declared at top level and referenced by name:

```hcl
prompt as recovery """
The previous turn failed. Re-read the workspace and recover.
"""

runner {
  # ...bindings...
  prompt {
    iteration.previous_result == "errored" => recovery
    _                                       => "Please continue."
  }
}
```

Bodies accept any string literal form documented in [`language.md`](../language.md): single-line, triple-quoted multi-line, or triple-quoted indented heredoc.

## Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `prompt` value | `string` or named-prompt reference | Required (for `iter run`) | — | Prompt text, or a bareword referencing a top-level `prompt as <name>`. May contain `{{...}}` placeholders that are resolved at dispatch time. |
| match arm guard | guard expression | — | — | Boolean predicate; the arm's value is selected for the iteration when it is the first guard (top to bottom) to evaluate true against the current Signal's metadata and iteration state. |
| `_` default arm | `string` or reference | Required in a match block | — | Selected when no guarded arm matches. |

## Placeholder resolution

Placeholders such as `{{metadata.task}}` or `{{signal.id}}` are **not** resolved at parse time. They are left as literal tokens and expanded by the runner when the Signal is known.

See [`language.md` § Placeholders](../language.md) for the full set of placeholder roots.

## Match-arm guards

A guard is a boolean expression over the Signal's metadata and the
runner's iteration state. It forms the left-hand side of a
`prompt { <guard> => ... }` match arm. Grammar:

```
guard      ::= term ( ( "&&" | "||" ) term )*
term       ::= "metadata"  "." <key>   ( "==" | "!=" ) <string>
             | "iteration" "." <field> ( "%" <int> )? <cmp> <int>
             | "iteration" "." "previous_result"     ( "==" | "!=" ) <result>
             | "(" guard ")"
cmp        ::= "==" | "!=" | "<" | "<=" | ">" | ">="
result    ::= "\"none\"" | "\"success\"" | "\"errored\""
```

`&&` and `||` associate left-to-right; use parentheses to group.
Metadata predicates only support equality/inequality against a string
literal. Iteration predicates compare numeric fields against an integer
(optionally reduced `% N` first), with `previous_result` as a special
string-valued field that only takes `==` / `!=`.

### `iteration.<field>` reference

| Field | Type | Meaning |
| --- | --- | --- |
| `count` | integer (1-indexed) | Iteration number for the turn currently being rendered. The first iteration sees `count == 1`. |
| `previous_exit_code` | integer or absent | Process exit code from the prior turn. Any comparison evaluates to `false` on the first iteration since there is no prior turn to compare against. |
| `consecutive_failures` | integer | Runner stage failure streak. Increments when a runner stage fails: workspace setup error, prompt render error, agent process spawn / I/O error, iteration timeout, or workspace teardown error. Resets to 0 on the next successful iteration. Agent-internal behaviour (e.g. the agent producing unhelpful output) does not affect this counter. |
| `consecutive_successes` | integer | Runner stage success streak. Increments when the full iteration pipeline (setup → agent → teardown) completes without a stage error. Resets to 0 on the next stage failure. |
| `previous_result` | `"none" \| "success" \| "errored"` | Runner-level result of the prior turn. `"success"` when the full iteration pipeline completed without a stage error. `"errored"` when a runner stage failed (same conditions as `consecutive_failures`). `"none"` only on the first iteration. |

`% 0` is rejected at parse time. `previous_result` is the only field
that accepts a string RHS, and it does not support `%`.

### Examples

```hcl
# Unconditional
runner {
  agent     = claude
  workspace = local
  continue_on_error = true
  behavior  = loop
  prompt    = "Please continue."
}
```

```hcl
# Guards as match arms — first true arm wins, `_` is the default.
runner {
  agent     = claude
  workspace = local
  continue_on_error = true
  behavior  = loop
  prompt {
    # Fires only when the signal carries metadata.task == "security"
    metadata.task == "security" => """
    Perform a security audit. Focus on authentication, input validation,
    and secret handling.
    """
    # Compound guard
    metadata.env == "prod" && metadata.task != "skip" => "Run production-safe checks only."
    # Periodic direction change: fires on count == 50, 100, 150, ...
    iteration.count % 50 == 0 => "The current codebase has problems. Identify the issues and fix them."
    _ => "Please continue."
  }
}
```

## Evaluation order and selection

- A `prompt = ...` field selects exactly one body unconditionally.
- A `prompt { ... }` match block selects exactly one arm: guarded arms are evaluated **top to bottom** and the **first** one whose guard is true wins; if none match, the `_` default arm is used. Unlike the removed top-level form, arms are not concatenated — a single body is sent per iteration.
- The `_` default is required in a match block, so a match always selects a body. A runner with no `prompt` at all produces an **empty prompt**; the agent is still invoked, but with nothing to read.

```hcl
runner {
  agent     = claude
  workspace = local
  continue_on_error = true
  behavior  = loop
  prompt {
    metadata.strict == "true" => """
    Do not modify public API signatures.
    Require green CI before committing.
    """
    _ => "Start from the failing tests."
  }
}
```

For `metadata.strict == "true"`, the agent receives the strict arm's body; otherwise it receives the default. To combine instructions, write them into a single arm body (as above) rather than relying on multiple prompts firing.

## See Also

- [`language.md`](../language.md) — string literal forms and placeholder syntax.
- [`iterfile/agent.md`](agent.md) — how the resolved prompt is delivered to each agent kind.
- [`iterfile/runner.md`](runner.md) — when the prompt is evaluated in the iteration lifecycle.
