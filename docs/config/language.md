# Language Reference

`Iterfile` and `compose.iter` share the same HCL-flavoured DSL. This page covers only the **syntactic layer**: tokens, literals, expressions, block structure. Semantic constraints (which fields a given block accepts, which values are valid) live on each block's own page.

Authoritative sources:

- `iter_language/grammar/iter.pest` — pest grammar
- `iter_language/src/parser.rs` — hand-written parser (differentially tested against the grammar)

---

## File Structure

```pest
file = { SOI ~ section* ~ EOI }

section = { prompt_section | on_section | block_section }
```

A file is zero or more sections. Order is preserved and is significant where evaluation depends on it (for example, the arms of a `prompt { ... }` match block are evaluated top to bottom, first true guard winning).

Top-level keywords:

| Keyword | Iterfile | compose.iter | Page |
| --- | :---: | :---: | --- |
| `queue` | ✔ | ✔ | [`iterfile/queue.md`](iterfile/queue.md), [`compose/queue.md`](compose/queue.md) |
| `workspace` | ✔ | ✔ (inside inline service) | [`iterfile/workspace.md`](iterfile/workspace.md) |
| `agent` | ✔ | ✔ (inside inline service) | [`iterfile/agent.md`](iterfile/agent.md) |
| `runner` | ✔ | ✔ (inside inline service) | [`iterfile/runner.md`](iterfile/runner.md) |
| `prompt` | ✔ (top level only as `prompt as <name>`; the runner's prompt lives inside `runner`) | ✔ (inside inline service) | [`iterfile/prompt.md`](iterfile/prompt.md) |
| `on` | ✘ at top level (lives inside `runner`) | ✔ (inside inline service) | [`iterfile/on.md`](iterfile/on.md) |
| `service` | ✘ | ✔ | [`compose/service.md`](compose/service.md) |
| `trigger` | ✘ | ✔ | [`compose/trigger.md`](compose/trigger.md) |

---

## Whitespace and Comments

```pest
WHITESPACE = _{ " " | "\t" | "\r" | "\n" }
COMMENT    = _{ "#" ~ (!"\n" ~ ANY)* }
```

- Whitespace is permitted between tokens (except inside atomic rules).
- `#` starts a line comment; it runs to the end of the line.
- There are no block comments (`/* ... */`).

---

## Identifiers

```pest
ident          = @{ ident_start ~ ident_continue* }
ident_start    = _{ ASCII_ALPHA | "_" }
ident_continue = _{ ASCII_ALPHANUMERIC | "_" }
```

- ASCII letters, digits, underscores; must not start with a digit.
- Unicode identifiers are not permitted.
- Reserved keywords (`queue`, `workspace`, `agent`, `trigger`, `runner`, `service`, `prompt`, `on`, `when`, `shell`, `capture`, `metadata`) take priority in their normal positions; they may still appear as identifiers in ident positions (for example, a field named `prompt` is parseable as a field, though avoided by convention).

---

## Literals

### Strings

```pest
string         = @{ "\"" ~ string_char* ~ "\"" }
triple_string  = @{ "\"\"\"" ~ triple_body ~ "\"\"\"" }
```

**Regular strings** `"..."`:

- Single line only.
- Supported escapes: `\"`, `\\`, `\n`, `\t`, `\r`, `\0`, `\u{HEX+}`. Any other `\X` is a lexical error.

**Triple-quoted strings** `"""..."""`:

- May span multiple lines.
- Whitespace and newlines are preserved verbatim; the lowering pass handles dedenting.
- Cannot be nested.

```hcl
name = "hello"
greeting = "line1\nline2"

prompt as instructions """
Multi-line
content.
"""
```

### Integers

```pest
integer = @{ ASCII_DIGIT+ }
```

- Base-10 only.
- No negative-number literal.
- `12abc` tokenises as `Integer(12)` followed by `Ident("abc")`.

### Durations

```pest
duration        = @{ ASCII_DIGIT+ ~ duration_suffix }
duration_suffix = _{ "s" | "m" | "h" | "d" }
```

- Positive integer followed by a one-character unit suffix.
- Supported units: `s` (seconds), `m` (minutes), `h` (hours), `d` (days).
- No sub-second units. `10ms` tokenises as `Duration(10m)` plus `Ident("s")`, which is a parse error in most positions.

```hcl
interval = 30s
delay    = 5m
poll     = 1h
```

Each field that accepts a duration stores it in a specific unit (e.g. `delay_secs`); consult the field's page.

### Booleans

```pest
boolean = @{ ("true" | "false") ~ !ident_continue }
```

- `true` or `false`.
- Word boundary guarded: `trueish` is an ident, not `true` + `ish`.
- In value positions the grammar prefers `boolean` over `ident`, so `mode = true` parses as `Bool(true)`.

### Null

```pest
null = @{ "null" ~ !ident_continue }
```

- The literal `null` denotes the explicit absence of a value.
- Word boundary guarded: `nullish` is an ident, not `null` + `ish`.
- In value positions the grammar prefers `null` over `ident`, so `trigger_name = null` parses as `Null`.
- Used in `compose.iter` trigger overrides to remove an inherited trigger: `triggers = { noisy = null }` disables `noisy`.

### Lists

```pest
list = { "[" ~ (value ~ ("," ~ value)*)? ~ "]" }
```

- Comma-separated values in square brackets.
- Trailing comma is **not** permitted.
- Syntactically heterogeneous; the semantic layer often requires homogeneous elements.

```hcl
args     = ["--flag", "--other"]
excludes = ["node_modules", ".git", "target"]
```

---

## Function-Call Expressions

```pest
call      = { ident ~ "(" ~ call_args? ~ ")" }
call_args = { value ~ ("," ~ value)* }
```

Syntactically any identifier can appear as a function. Semantically only the following are accepted:

| Function | Valid contexts | Meaning |
| --- | --- | --- |
| `env("VAR")` | `secret` fields | Read environment variable `VAR` at runtime. The value is redacted from logs. |
| `from_metadata("key")` | `templated` fields | At Signal dispatch time, read metadata key `key` from the Signal and substitute. Used for dynamic values such as Kafka keys or SQS `MessageGroupId`. |
| `regex("pattern")` | `extract` of `trigger command` | Apply a regular expression to the command's stdout. |

Any other identifier in call position produces a semantic error.

---

## Blocks

```pest
block       = { "{" ~ block_entry* ~ "}" }
block_entry = { nested_route | action | field }

field      = { field_name ~ field_rhs }
field_name = ${ !(kw_on | kw_shell | kw_condition | kw_capture) ~ (ident | string) }
field_rhs  = { block | ("=" ~ value) }
```

A block body contains any mix of:

1. **Fields**: `<name> = <value>`. The name is an identifier or a string literal (strings allow keys that contain characters illegal in identifiers — e.g. Kafka header names like `"x-source"` or librdkafka keys like `"client.dns.lookup"`).
2. **Short-form nested blocks**: `<name> { ... }`, equivalent to `<name> = { ... }`.
3. **Nested routes**: `on "<pattern>" [when "<expr>"] { ... }` — only inside `trigger webhook` blocks.
4. **Actions**: `shell "<command>"`,
   `shell { script = "<command>" ... }`, or `enqueue { ... }` — only inside
   `on` event handler blocks. Block-form Runner shell actions may contain
   `capture <name> { ... }`. `enqueue` is valid only in top-level Compose
   Hooks.

Entries are separated by whitespace or newlines; no commas or semicolons are required between entries.

```hcl
agent claude {
  mode    = interactive
  command = "claude"
  args    = ["--timeout", "600"]
}
```

---

## Section Shapes

### Kinded sections

Most top-level sections have the shape "keyword kind body":

```pest
kinded_section = {
      block_keyword
    ~ ident              # kind (e.g. claude, sqs, local)
    ~ ( kind2_with_block | block )?
}

kind2_with_block = { !reserved_section_keyword ~ ident ~ block }
```

- `block_keyword` is a top-level keyword (`queue`, `workspace`, `agent`, `trigger`, `service`).
- `ident` is the kind (for example, the `claude` in `agent claude { ... }`).
- `kind2_with_block` handles the compose.iter shape `queue <name> <kind> { ... }` — the second identifier is consumed only when immediately followed by a `{`.

### Runner section

`runner` is special-cased: it takes no kind.

```pest
runner_section = { kw_runner ~ block? }
```

### Prompt section

```pest
prompt_section  = { kw_prompt ~ ( prompt_as_alias | prompt_guard? ~ string_literal ) }
prompt_as_alias = { kw_as ~ ident ~ string_literal }
```

At top level only the `prompt as <name> "..."` named-definition form is accepted by the semantic layer. The bare `prompt "..."` and guarded `prompt when <guard> "..."` forms still parse syntactically but are rejected during analysis — a runner's prompt is selected inside the `runner` block (`prompt = "..."` or a `prompt { <guard> => ..., _ => ... }` match), not declared as a top-level section.

### `on` event-handler section

```pest
on_section = { kw_on ~ ident ~ block }
```

The event name is an identifier (not a string). In an Iterfile, Runner Hooks
live inside the `runner` block; a top-level `on <event>` is rejected. In a
`compose.iter`, top-level `on <compose-event>` declares a Compose Hook, while
Runner Hooks in an inline service remain inside that service's `runner`.
The two event sets are distinct; see [`iterfile/on.md`](iterfile/on.md) and
[`compose/on.md`](compose/on.md).

Both kinds of Hook share the `on <event> { ... }` outer form but have distinct
contextual bodies. Runner Hooks accept Runner actions (`shell`); Compose Hooks
accept Compose actions (`shell` and `enqueue`).

---

## Expressions

The same expression language is used by conditional Prompt arms and Runner
Hook `when` clauses. Each surface decides which context roots are available.

```pest
expression = { expression_or }
expression_or = { expression_and ~ ("||" ~ expression_and)* }
expression_and = { expression_compare ~ ("&&" ~ expression_compare)* }
expression_compare = { expression_modulus ~ (compare_op ~ expression_modulus)? }
expression_modulus = { expression_primary ~ ("%" ~ expression_primary)* }
expression_primary = { path | string | integer | boolean | null | "(" ~ expression ~ ")" }
path = { ident ~ (("." ~ ident) | ("[" ~ integer ~ "]"))* }
compare_op = { "==" | "!=" | "<=" | ">=" | "<" | ">" }
```

Operators:

| Syntax | Meaning |
| --- | --- |
| `<value> == <value>`, `<value> != <value>` | Equality and inequality of compatible values. |
| `<value> < <value>`, `<=`, `>`, `>=` | Ordering of two JSON numbers or two strings. |
| `<integer> % <integer>` | Integer remainder. |
| `<expr> && <expr>` | Logical AND. |
| `<expr> \|\| <expr>` | Logical OR. |
| `( <expr> )` | Grouping. |

Prompt expressions can use `signal.*`, `metadata.*`, `iteration.*`, and
`var.*`. Runner Hooks expose roots appropriate to their event;
`agent_finished` additionally exposes `agent.session_id` and `agent.output`,
while completion events expose `completion.*` and `runner.*`.

Constraints:

- `% 0` and statically known type mismatches are rejected during declaration
  analysis.
- A condition must evaluate to a boolean. Statically known non-boolean
  conditions are rejected during declaration analysis.
- `metadata.*` values follow the existing template contract and are strings.
- Dynamic path values are type-checked when evaluated.
- A missing path makes a comparison evaluate to `false`, including `!=`.
- `null` compared with a non-null value also evaluates to `false`, including
  `!=`, ordering, and expressions containing `%`. Use `== null` to test for
  an explicit null value.
- `%` binds tighter than comparisons, comparisons bind tighter than `&&`,
  and `&&` binds tighter than `||`.

Expressions appear on the left-hand side of a `prompt { ... }` match arm:

```hcl
runner {
  agent     = claude
  workspace = local
  continue_on_error = true
  behavior  = loop
  prompt {
    metadata.task == "bug-fix" => "Fix bugs."

    metadata.type == "feature" && metadata.priority == "high" => "Implement high-priority feature."

    (metadata.env == "dev" || metadata.env == "staging") && metadata.task != "ignore" => "Work on non-production tasks."

    # Periodic direction change
    iteration.count % 50 == 0 => "The current codebase has problems. Identify the issues and fix them."

    _ => "Please continue."
  }
}
```

Webhook route `when "..."` guards are stored as **raw strings** and evaluated by the runner; this grammar does not parse them.

---

## `{{...}}` Placeholders

`{{...}}` placeholders inside string literals are **not** resolved at parse time. The runner substitutes them at execution time using the current Signal, event context, or webhook payload.

Common placeholders (exact availability depends on the context):

| Placeholder | Available in |
| --- | --- |
| `{{signal.id}}` | Prompt bodies, per-Signal Runner shell actions, and Signal-aware webhook/trigger templates. |
| `{{metadata.<key>}}` | `prompt` bodies, shell actions, webhook route metadata. |
| `{{iteration.<field>}}` | Prompt bodies and Runner lifecycle shell actions. See [`iterfile/prompt.md`](iterfile/prompt.md#iterationfield-reference) for the field set. |
| `{{var.<name>.<field>}}` | Prompt bodies and Runner shell actions after a block-form shell capture has published the named value. |
| `{{agent.session_id}}`, `{{agent.output}}` | Successful `agent_finished` Runner Hooks only. Structured response fields use paths such as `{{agent.output.decision}}`. |
| `{{today}}` | `prompt` bodies, shell actions, and Compose Hook enqueue metadata. Current local date as `YYYY-MM-DD`. |
| `{{error.kind}}`, `{{error.message}}` | DLQ templates. |
| `{{.<payload-path>}}` | Webhook route metadata values. |

Per-block pages document which placeholders apply.

---

## Secret Expressions (`secret`)

A `secret` field accepts any of:

- **A string literal**: `"value"` — used verbatim. Do not commit real secrets this way.
- **`env("VAR")`**: read environment variable `VAR` at runtime; the value is treated as sensitive.
- **`file("./path")`**: read the secret from a file at runtime. The file contents are trimmed and treated as sensitive. The path is resolved relative to the compose file.

```hcl
ssl_key_password = env("KEY_PASSWORD")        # recommended for CI / container secrets
ssl_key_password = file("./secrets/key.txt")  # recommended for on-disk secrets
ssl_key_password = "literal-password"         # discouraged
```

---

## Templated Strings (`templated`)

A `templated` field accepts either:

- **A string literal**: `"static-value"` — the same value for every Signal.
- **`from_metadata("key")`**: per-Signal value read from the named metadata key.

```hcl
message_group_id = from_metadata("customer_id")   # dynamic
message_group_id = "static-group"                 # fixed
```

---

## Priority Keywords

Used in trigger blocks and webhook routes. AST: `PriorityKeyword` in `iter_language/src/ast/prompt.rs`.

| Keyword | Meaning |
| --- | --- |
| `low` | Lowest priority. |
| `normal` | Default priority. |
| `high` | Higher than normal. |
| `critical` | Reserved for incidents that must preempt other work. |

```hcl
on "incident" {
  priority = critical
}
```

---

## On-Error Keywords

Used in `trigger command` to control the behaviour when the polled command exits non-zero. AST: `OnErrorKeyword` in `iter_language/src/ast/trigger.rs`.

| Keyword | Meaning |
| --- | --- |
| `continue` | Log a warning and retry on the next tick (default). |
| `abort` | Stop the trigger with an error. |
| `skip` | Silently swallow the error and continue without emitting. |

```hcl
trigger smoke command {
  run      = "scripts/smoke.sh"
  on_error = abort
}
```
