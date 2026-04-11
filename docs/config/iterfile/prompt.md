# Iterfile: `prompt`

Declares the prompt text sent to the agent each iteration. Zero or more per `Iterfile`. Also usable inside a `compose.iter` inline service.

AST: `PromptDecl` and `PromptGuard` in `iter_language/src/ast/prompt.rs`.

## Syntax

```hcl
prompt [when <guard>] "<body>"
```

The body accepts any string literal form documented in [`language.md`](../language.md): single-line, triple-quoted multi-line, or triple-quoted indented heredoc.

## Arguments

| Name | Type | Required | Default | Description |
| --- | --- | :---: | --- | --- |
| `body` | `string` (positional) | Required | — | Prompt text. May contain `{{...}}` placeholders that are resolved at dispatch time. |
| `when` | guard expression | Optional | — | Boolean predicate; the prompt fires only when it evaluates true against the current Signal's metadata. |

## Placeholder resolution

Placeholders such as `{{metadata.task}}` or `{{signal.id}}` are **not** resolved at parse time. They are left as literal tokens and expanded by the runner when the Signal is known.

See [`language.md` § Placeholders](../language.md) for the full set of placeholder roots.

## `when` guards

A guard is a boolean expression over the Signal's metadata and the
runner's iteration state. Grammar:

```
guard      ::= term ( ( "&&" | "||" ) term )*
term       ::= "metadata"  "." <key>   ( "==" | "!=" ) <string>
             | "iteration" "." <field> ( "%" <int> )? <cmp> <int>
             | "iteration" "." "previous_outcome"     ( "==" | "!=" ) <outcome>
             | "(" guard ")"
cmp        ::= "==" | "!=" | "<" | "<=" | ">" | ">="
outcome    ::= "\"none\"" | "\"success\"" | "\"errored\""
```

`&&` and `||` associate left-to-right; use parentheses to group.
Metadata predicates only support equality/inequality against a string
literal. Iteration predicates compare numeric fields against an integer
(optionally reduced `% N` first), with `previous_outcome` as a special
string-valued field that only takes `==` / `!=`.

### `iteration.<field>` reference

| Field | Type | Meaning |
| --- | --- | --- |
| `count` | integer (1-indexed) | Iteration number for the turn currently being rendered. The first iteration sees `count == 1`. |
| `previous_exit_code` | integer or absent | Process exit code from the prior turn. Any comparison evaluates to `false` on the first iteration since there is no prior turn to compare against. |
| `consecutive_failures` | integer | Runner stage failure streak. Increments when a runner stage fails: workspace setup error, prompt render error, agent process spawn / I/O error, iteration timeout, or workspace teardown error. Resets to 0 on the next successful iteration. Agent-internal behaviour (e.g. the agent producing unhelpful output) does not affect this counter. |
| `consecutive_successes` | integer | Runner stage success streak. Increments when the full iteration pipeline (setup → agent → teardown) completes without a stage error. Resets to 0 on the next stage failure. |
| `previous_outcome` | `"none" \| "success" \| "errored"` | Runner-level result of the prior turn. `"success"` when the full iteration pipeline completed without a stage error. `"errored"` when a runner stage failed (same conditions as `consecutive_failures`). `"none"` only on the first iteration. |

`% 0` is rejected at parse time. `previous_outcome` is the only field
that accepts a string RHS, and it does not support `%`.

### Examples

```hcl
# Unconditional
prompt "Please continue."

# Fires only when the signal carries metadata.task == "security"
prompt when metadata.task == "security" """
Perform a security audit. Focus on authentication, input validation,
and secret handling.
"""

# Compound guard
prompt when metadata.env == "prod" && metadata.task != "skip" "Run production-safe checks only."

# Periodic direction change: fires on count == 5, 10, 15, ...
prompt when iteration.count % 50 == 0 "The current codebase has problems. Identify the issues and fix them."
```

## Evaluation order and multiplicity

- Each `prompt` block is evaluated independently. A Signal may match zero, one, or many prompts.
- When multiple prompts match, their bodies are concatenated **in source order**, separated by a blank line, and sent as a single prompt.
- A file with zero matching prompts produces an **empty prompt**; the agent is still invoked, but with nothing to read.

```hcl
prompt "Start from the failing tests."
prompt when metadata.strict == "true" "Do not modify public API signatures."
prompt when metadata.strict == "true" "Require green CI before committing."
```

For `metadata.strict == "true"`, the agent receives all three prompt bodies joined in order.

## See Also

- [`language.md`](../language.md) — string literal forms and placeholder syntax.
- [`iterfile/agent.md`](agent.md) — how the resolved prompt is delivered to each agent kind.
- [`iterfile/runner.md`](runner.md) — when the prompt is evaluated in the iteration lifecycle.
