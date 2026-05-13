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

A file is zero or more sections. Order is preserved and is significant for sections that allow multiple occurrences (for example, `prompt` blocks are applied in source order).

Top-level keywords:

| Keyword | Iterfile | compose.iter | Page |
| --- | :---: | :---: | --- |
| `queue` | ✔ | ✔ | [`iterfile/queue.md`](iterfile/queue.md), [`compose/queue.md`](compose/queue.md) |
| `workspace` | ✔ | ✔ (inside inline service) | [`iterfile/workspace.md`](iterfile/workspace.md) |
| `agent` | ✔ | ✔ (inside inline service) | [`iterfile/agent.md`](iterfile/agent.md) |
| `runner` | ✔ | ✔ (inside inline service) | [`iterfile/runner.md`](iterfile/runner.md) |
| `prompt` | ✔ | ✔ (inside inline service) | [`iterfile/prompt.md`](iterfile/prompt.md) |
| `on` | ✔ | ✔ (inside inline service) | [`iterfile/on.md`](iterfile/on.md) |
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
- Reserved keywords (`queue`, `workspace`, `agent`, `trigger`, `runner`, `service`, `prompt`, `on`, `when`, `shell`, `metadata`) take priority in their normal positions; they may still appear as identifiers in ident positions (for example, a field named `prompt` is parseable as a field, though avoided by convention).

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

prompt """
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
field_name = ${ !(kw_on | kw_shell) ~ (ident | string) }
field_rhs  = { block | ("=" ~ value) }
```

A block body contains any mix of:

1. **Fields**: `<name> = <value>`. The name is an identifier or a string literal (strings allow keys that contain characters illegal in identifiers — e.g. Kafka header names like `"x-source"` or librdkafka keys like `"client.dns.lookup"`).
2. **Short-form nested blocks**: `<name> { ... }`, equivalent to `<name> = { ... }`.
3. **Nested routes**: `on "<pattern>" [when "<expr>"] { ... }` — only inside `trigger webhook` blocks.
4. **Actions**: `shell "<command>"` — only inside `on` event handler blocks.

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
prompt_section = { kw_prompt ~ prompt_guard? ~ string_literal }
```

### Top-level `on` section

```pest
on_section = { kw_on ~ ident ~ block }
```

The event name is an identifier (not a string). Valid names are listed on [`iterfile/on.md`](iterfile/on.md).

---

## Guard Expressions

Used by `prompt when ...`:

```pest
guard              = { guard_or }
guard_or           = { guard_and  ~ ("||" ~ guard_and)* }
guard_and          = { guard_atom ~ ("&&" ~ guard_atom)* }
guard_atom         = { guard_paren | guard_iter | guard_meta }
guard_paren        = { "(" ~ guard_or ~ ")" }
guard_meta         = { kw_metadata ~ "." ~ ident ~ guard_eq_op ~ string }
guard_iter         = { kw_iteration ~ "." ~ ident ~ guard_iter_modulus? ~ guard_iter_op ~ guard_iter_rhs }
guard_iter_modulus = { "%" ~ integer }
guard_eq_op        = { "==" | "!=" }
guard_iter_op      = { "==" | "!=" | "<=" | ">=" | "<" | ">" }
guard_iter_rhs     = { integer | string }
```

Supported forms:

| Syntax | Meaning |
| --- | --- |
| `metadata.<key> == "value"` | Metadata equality. |
| `metadata.<key> != "value"` | Metadata inequality. |
| `iteration.<field> <cmp> <int>` | Numeric comparison against a runner iteration field. |
| `iteration.<field> % <int> <cmp> <int>` | Same, but reduce the LHS modulo `<int>` first. |
| `iteration.previous_outcome == "<outcome>"` | String equality against `"none"`, `"success"`, or `"errored"`. |
| `iteration.previous_outcome != "<outcome>"` | Inequality form of the above. |
| `<expr> && <expr>` | Logical AND. |
| `<expr> \|\| <expr>` | Logical OR. |
| `( <expr> )` | Grouping. |

Where `<cmp>` is one of `==`, `!=`, `<`, `<=`, `>`, `>=`. Numeric
`iteration.*` fields are `count`, `previous_exit_code`,
`consecutive_failures`, and `consecutive_successes`; see
[`iterfile/prompt.md`](iterfile/prompt.md#iterationfield-reference)
for the full table.

Constraints:

- Metadata predicates only support `==` / `!=` against a string literal.
- Numeric `iteration.*` fields require an integer RHS. `previous_outcome`
  is the only field that takes a string RHS, and only with `==` / `!=`.
- `% 0` is rejected at parse time. Modulus is only valid on numeric
  `iteration.*` fields, applied at most once on the LHS.
- A missing `previous_exit_code` (no prior turn) makes every comparison
  evaluate to `false`, including `!=`.
- `&&` binds tighter than `||` (standard precedence).

```hcl
prompt when metadata.task == "bug-fix" "Fix bugs."

prompt when metadata.type == "feature" && metadata.priority == "high"
  "Implement high-priority feature."

prompt when (metadata.env == "dev" || metadata.env == "staging") && metadata.task != "ignore"
  "Work on non-production tasks."

# Periodic direction change
prompt when iteration.count % 50 == 0 "The current codebase has problems. Identify the issues and fix them."
```

Webhook route `when "..."` guards are stored as **raw strings** and evaluated by the runner; this grammar does not parse them.

---

## `{{...}}` Placeholders

`{{...}}` placeholders inside string literals are **not** resolved at parse time. The runner substitutes them at execution time using the current Signal, event context, or webhook payload.

Common placeholders (exact availability depends on the context):

| Placeholder | Available in |
| --- | --- |
| `{{signal.id}}` | Shell actions, DLQ templates. |
| `{{metadata.<key>}}` | `prompt` bodies, shell actions, webhook route metadata. |
| `{{iteration.<field>}}` | `prompt` bodies, shell actions in `on agent_*` and `on runner_*` hooks. See [`iterfile/prompt.md`](iterfile/prompt.md#iterationfield-reference) for the field set. |
| `{{today}}` | `prompt` bodies, shell actions. Current local date as `YYYY-MM-DD`. |
| `{{error.kind}}`, `{{error.message}}` | `on runner_error`, DLQ templates. |
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
