# antigravity_cli

Type-safe Rust command builders and executor for Google's Antigravity CLI
(`agy`) `1.0.16`.

The crate models Antigravity as its own CLI surface, not as adapter code for a
particular caller. Command families live in separate modules so future
Antigravity version updates can be reviewed by the affected command area.

## Scope

- Models the root `agy` run (print, prompt-interactive, and bare-positional TUI
  modes) and its full root-flag surface.
- Models the `plugin`, `install`, `models`, `update`, `changelog`, and `help`
  subcommands.
- Executes commands through `Antigravity::execute`, returning the decoded
  stdout/stderr plus a typed [`Exit`] classification.

## No JSON

Unlike the JSON-emitting agent CLIs, `agy` has **no machine-readable output
mode**. `agy --print` writes free-form text to stdout and human-readable
markers to stderr. This crate therefore exposes the raw process output and a
typed exit classification only — there is no event stream or result-object
parser to model. Callers that need to distinguish auth prompts, token-limit
notices, or TTY failures scan the returned text themselves.

## Example

```rust
use antigravity_cli::{Antigravity, RunCommand};

# async fn example() -> Result<(), antigravity_cli::Error> {
let agy = Antigravity::default().with_current_dir(".");
let output = agy.execute(&RunCommand::print("Summarize this repository.")).await?;
if output.is_success() {
    println!("{}", output.stdout());
}
# Ok(())
# }
```

## Exit codes

`agy` overloads its exit code: `0` is reported for a clean run but also for an
auth-required prompt and a trapped `SIGTERM`; `2` is argument rejection; `126`
/ `127` are launch failures. [`Exit`] classifies the raw disposition (success,
failure code, signal, or indeterminate); higher-level meaning is left to the
caller because the code alone is not authoritative.

`-h` / `--help` is accepted by every command (Go's flag convention) and is not
modeled as a distinct field. `plugins` is an alias for `plugin`; the canonical
`plugin` form is modeled.

## Versioning

`SUPPORTED_ANTIGRAVITY_VERSION` records the Antigravity version the surface was
authored against. Updating the supported CLI version should update the command
types and tests together.
